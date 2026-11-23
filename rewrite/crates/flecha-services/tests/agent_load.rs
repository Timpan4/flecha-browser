//! Thousands of concurrent `wait_for` calls over HTTP against the fake backend. Each one holds a
//! deadline and polls, which is what starved CEF's pump when GPUI timers carried it (Spike 2).
//! The server must serve them on its one thread and go quiet once they end: no timer outlives
//! its call. Run with `--nocapture` for the measurements.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::*;
use flecha_core::TabId;
use serde_json::json;

/// "Thousands of concurrent tool calls", from the task that asked for this test.
const CALLS: usize = 4000;
/// Half the calls time out after the `wait_for` default, which leaves every call open while
/// the rest connect.
const TIMEOUT_MS: u64 = flecha_services::agent::tools::DEFAULT_WAIT_MS;

#[derive(Clone, Copy, Debug, Default)]
struct ThreadStat {
    cpu_ticks: u64,
    voluntary_switches: u64,
    involuntary_switches: u64,
}

fn server_thread() -> String {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(Result::ok)
        .map(|task| task.path())
        .find(|task| {
            std::fs::read_to_string(task.join("comm")).is_ok_and(|comm| comm.trim() == "flecha-mcp")
        })
        .expect("the server runs on its own named thread")
        .to_string_lossy()
        .into_owned()
}

fn thread_stat(task: &str) -> ThreadStat {
    let stat = std::fs::read_to_string(format!("{task}/stat")).unwrap();
    // Fields after the parenthesized name; utime and stime are fields 14 and 15.
    let fields: Vec<&str> = stat.rsplit_once(") ").unwrap().1.split(' ').collect();
    let status = std::fs::read_to_string(format!("{task}/status")).unwrap();
    let count = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(|value| value.trim().parse().unwrap())
            .unwrap()
    };
    ThreadStat {
        cpu_ticks: fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap(),
        voluntary_switches: count("voluntary_ctxt_switches:"),
        involuntary_switches: count("nonvoluntary_ctxt_switches:"),
    }
}

fn process_threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

#[tokio::test(flavor = "multi_thread")]
async fn thousands_of_waits_share_the_server_thread_and_leave_no_timers() {
    let run = start();
    let tab = TabId::new("mine");
    {
        let mut state = run.browser.state();
        let window = state.focused_window().id.clone();
        assert!(state.open_today(
            tab.clone(),
            flecha_core::Tab::new("https://example.com/", 1),
            &window,
            None,
            true
        ));
    }
    let task = server_thread();
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let threads_before = process_threads();
    let before = thread_stat(&task);
    let probes_before = run.browser.stats.probes.load(Ordering::Relaxed);
    let started = Instant::now();

    let port = run.port();
    let bearer = Arc::new(run.bearer());
    let mut calls = Vec::with_capacity(CALLS);
    for index in 0..CALLS {
        // Connect one at a time so the test measures the server, not the kernel's SYN retries.
        let stream = connect(port).await;
        let bearer = bearer.clone();
        // A selector per call, so the fake can tell which calls the server has dispatched.
        let arguments = if index % 2 == 0 {
            json!({ "tabId": "mine", "selector": format!("#ready-{index}"), "timeoutMs": 60_000 })
        } else {
            json!({ "tabId": "mine", "selector": format!("#never-{index}"), "timeoutMs": TIMEOUT_MS })
        };
        calls.push(tokio::spawn(async move {
            let reply = exchange_on(
                stream,
                &post(port, Some(&bearer), &[], &tool_call("wait_for", arguments)),
            )
            .await;
            assert_eq!(reply.status, 200, "{reply:?}");
            text_of(&reply.message()["result"]).to_owned()
        }));
    }
    let all_connected = started.elapsed();
    let dispatched = || run.browser.stats.probed.lock().unwrap().len();
    while dispatched() < CALLS && !calls.iter().any(|call| call.is_finished()) {
        tokio::time::sleep(flecha_services::agent::tools::WAIT_POLL).await;
    }
    let all_dispatched = started.elapsed();
    assert_eq!(dispatched(), CALLS, "every call reached the backend");
    assert!(
        calls.iter().all(|call| !call.is_finished()),
        "a call finished before the last one was dispatched, so they did not all overlap"
    );
    let peak_threads = process_threads();
    assert_eq!(
        peak_threads, threads_before,
        "the server started threads for its calls"
    );
    // Both ends of every open connection live in this process.
    let open_sockets = std::fs::read_dir("/proc/self/fd").unwrap().count();
    run.browser.page(&tab, |page| {
        page.elements
            .extend((0..CALLS).step_by(2).map(|index| format!("#ready-{index}")));
    });
    let mut ready = 0;
    let mut timed_out = 0;
    for call in calls {
        match call.await.unwrap().as_str() {
            "ready" => ready += 1,
            text if text == format!("Timed out after {TIMEOUT_MS}ms") => timed_out += 1,
            other => panic!("unexpected result {other:?}"),
        }
    }
    let elapsed = started.elapsed();
    let loaded = thread_stat(&task);
    let probes = run.browser.stats.probes.load(Ordering::Relaxed) - probes_before;
    assert_eq!((ready, timed_out), (CALLS / 2, CALLS / 2));

    // Idle: no connections and no calls. A timer left behind would wake the thread.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle_start = thread_stat(&task);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle_end = thread_stat(&task);

    eprintln!(
        "{CALLS} concurrent wait_for calls in {:.2}s (all connected after {:.2}s, all open in the backend at once after {:.2}s): {ready} ready, {timed_out} timed out after {TIMEOUT_MS}ms",
        elapsed.as_secs_f64(),
        all_connected.as_secs_f64(),
        all_dispatched.as_secs_f64()
    );
    eprintln!(
        "server thread under load: {:.2}s CPU ({:.0}% of wall time), {} voluntary and {} involuntary context switches; {probes} backend probes ({:.0}/s)",
        (loaded.cpu_ticks - before.cpu_ticks) as f64 / ticks_per_second,
        100.0 * (loaded.cpu_ticks - before.cpu_ticks) as f64
            / ticks_per_second
            / elapsed.as_secs_f64(),
        loaded.voluntary_switches - before.voluntary_switches,
        loaded.involuntary_switches - before.involuntary_switches,
        probes as f64 / elapsed.as_secs_f64()
    );
    eprintln!(
        "server thread idle for 2s afterwards: {} CPU ticks, {} voluntary and {} involuntary context switches",
        idle_end.cpu_ticks - idle_start.cpu_ticks,
        idle_end.voluntary_switches - idle_start.voluntary_switches,
        idle_end.involuntary_switches - idle_start.involuntary_switches
    );
    eprintln!(
        "process threads: {threads_before} before, {peak_threads} with every call open; {open_sockets} open file descriptors then (two per connection)"
    );

    assert_eq!(
        idle_end.voluntary_switches - idle_start.voluntary_switches,
        0,
        "the idle server thread woke up: a timer outlived its call"
    );
    assert_eq!(
        idle_end.cpu_ticks - idle_start.cpu_ticks,
        0,
        "the idle server thread used CPU"
    );
}
