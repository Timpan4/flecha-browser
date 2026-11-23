//! The Linux app: CEF setup, the GPUI window, the report, and the after-exit
//! leak check.

use crate::{config, engine, probe, pump, scenario, timers, view};
use anyhow::{Context as _, Result, bail};
use cef::{CefString, ImplCommandLine as _};
use futures::channel::mpsc;
use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, point, px, size};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Instant;

pub fn main() -> ExitCode {
    // Leak check, run by scripts/run-checks.sh after the app exits. No CEF.
    if let Some(report) =
        std::env::args().find_map(|a| a.strip_prefix("--pump-wait-gone=").map(str::to_string))
    {
        return match wait_gone(&report) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(2),
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::FAILURE
            }
        };
    }
    // The runner's sweep of a run's marked processes. No CEF.
    for (flag, kill) in [("--pump-sweep=", false), ("--pump-sweep-kill=", true)] {
        if let Some(runtime) =
            std::env::args().find_map(|a| a.strip_prefix(flag).map(str::to_string))
        {
            return sweep_main(&runtime, kill);
        }
    }
    env_logger::init();
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let Some(cmd) = args.as_cmd_line() else {
        eprintln!("cannot parse command line");
        return ExitCode::FAILURE;
    };
    let is_browser = cmd.has_switch(Some(&CefString::from("type"))) != 1;
    let mut app = engine::build_app();

    // Child processes (renderer, GPU, utility, zygote) return here.
    let code = cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if !is_browser {
        return ExitCode::from(code.clamp(0, 255) as u8);
    }

    match browser_main(args, app) {
        Ok(pass) if pass => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(2),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn browser_main(args: cef::args::Args, mut app: cef::App) -> Result<bool> {
    let cfg = config::Config::from_args()?;
    let pump_requests = pump::install();
    let context_ready = engine::install_context_ready();

    let settings = cef::Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        multi_threaded_message_loop: 0,
        no_sandbox: cfg.no_sandbox as _,
        root_cache_path: CefString::from(cfg.cache_dir.to_string_lossy().as_ref()),
        ..Default::default()
    };
    if cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        bail!("CefInitialize failed");
    }

    let report: scenario::Report = Default::default();
    let children = Rc::new(RefCell::new(None));
    let gpui_started = Rc::new(Cell::new(false));
    {
        let report = report.clone();
        let children = children.clone();
        let gpui_started = gpui_started.clone();
        let cfg = cfg.clone();
        gpui_platform::application().run(move |cx: &mut App| {
            gpui_started.set(true);
            if let Err(err) = start(
                cx,
                cfg,
                pump_requests,
                context_ready,
                report.clone(),
                children,
            ) {
                report
                    .borrow_mut()
                    .insert("error".into(), json!(format!("{err:#}")));
                cx.quit();
            }
        });
    }

    // CefShutdown must follow every browser closing. The scenario records
    // `browsers_closed` false when it starts and true once every OnBeforeClose
    // arrived; with no record, no scenario ran and no browser exists.
    let browsers_closed = report.borrow().get("browsers_closed") != Some(&json!(false));
    let shutdown_ms = browsers_closed.then(|| {
        let t = Instant::now();
        cef::shutdown();
        t.elapsed().as_secs_f64() * 1e3
    });

    // An empty child list proves nothing unless both snapshots found every
    // process; `--pump-wait-gone` fails the run on any discovery error.
    let (before, mut discovery_errors) = match children.borrow_mut().take() {
        Some(tree) => (tree.procs, tree.errors),
        None => (Vec::new(), vec!["no snapshot before shutdown".to_string()]),
    };
    // Zygotes exit when this process closes their sockets, so some are still
    // alive here; `--pump-wait-gone` checks them after exit.
    let alive_now: Vec<Value> = before
        .iter()
        .filter(|p| probe::alive(p.pid, p.start_time))
        .map(probe::Proc::summary)
        .collect();
    let me = probe::self_pid();
    let after = probe::tree(me);
    discovery_errors.extend(after.errors);
    let remaining: Vec<Value> = after
        .procs
        .iter()
        .filter(|p| p.pid != me)
        .map(probe::Proc::summary)
        .collect();

    let mut report = report.borrow().clone();
    report.insert(
        "step_timeout_ms".into(),
        json!(cfg.step_timeout.as_secs_f64() * 1e3),
    );
    report.insert("warmup_ms".into(), json!(cfg.warmup.as_secs_f64() * 1e3));
    report.insert("measure_ms".into(), json!(cfg.measure.as_secs_f64() * 1e3));
    report.insert("sandbox".into(), json!(!cfg.no_sandbox));
    report.insert("scenario".into(), json!(format!("{:?}", cfg.scenario)));
    report.insert("gpui_ran".into(), json!(gpui_started.get()));
    report.insert("pump_totals".into(), pump::counts_json(pump::counts()));
    report.insert(
        "pump_requests_received_any_thread".into(),
        json!(pump::requests_received_any_thread()),
    );
    report.insert(
        "max_pump_interval_ms".into(),
        json!(pump::MAX_PUMP_INTERVAL.as_millis()),
    );
    report.insert(
        "shutdown".into(),
        json!({
            "cef_shutdown_called": shutdown_ms.is_some(),
            "cef_shutdown_skipped_reason": (!browsers_closed).then_some("browsers still open"),
            "cef_shutdown_ms": shutdown_ms,
            "children_before_shutdown": before.iter().map(probe::Proc::summary).collect::<Vec<_>>(),
            "discovery_errors": discovery_errors,
            "children_alive_right_after_cef_shutdown": alive_now,
            "descendants_right_after_cef_shutdown": remaining,
        }),
    );
    let pass_key = match cfg.scenario {
        config::Scenario::Idle => "idle_pass",
        config::Scenario::Animate => "animate_pass",
        config::Scenario::Cdp => "cdp_pass",
        config::Scenario::DevTools => "devtools_pass",
    };
    let pass = report.get(pass_key) == Some(&json!(true))
        && report.get("pump_pass") == Some(&json!(true))
        && report.get("isolation_clean") == Some(&json!(true))
        && browsers_closed
        && !report.contains_key("error")
        && !report.contains_key("shutdown_error");
    report.insert("scenario_pass".into(), json!(pass));
    let text = serde_json::to_string_pretty(&Value::Object(report))?;
    std::fs::write(&cfg.out, &text).with_context(|| format!("write {}", cfg.out.display()))?;
    println!("{text}");
    Ok(pass)
}

/// Adds the after-exit leak check to a scenario report and sets `pass`.
/// Lists (`kill` false) or kills (`kill` true) this user's processes marked
/// with `XDG_RUNTIME_DIR=runtime`; see `cef_gpui_pump::sweep`. Prints one
/// `pid pgid state args` line per marked process, or one `sweep:` line per
/// kill outcome. Exits 0 only if the scan, and each kill, completed; the runner
/// counts any other exit as an incomplete sweep.
fn sweep_main(runtime: &str, kill: bool) -> ExitCode {
    use cef_gpui_pump::sweep::{self, Outcome};
    if !runtime.starts_with('/') {
        eprintln!("error: the sweep needs an absolute XDG_RUNTIME_DIR, got {runtime:?}");
        return ExitCode::from(2);
    }
    let marker = format!("XDG_RUNTIME_DIR={runtime}");
    let found = match sweep::scan(&marker) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("error: process scan failed: {err}");
            return ExitCode::FAILURE;
        }
    };
    if !found.unreadable.is_empty() {
        eprintln!(
            "sweep: environ unreadable, not checked for the marker: {:?}",
            found.unreadable
        );
    }
    if !kill {
        for m in &found.marked {
            println!("{} {} {} {}", m.pid, m.pgid, m.state, m.args);
        }
        return ExitCode::SUCCESS;
    }
    let mut complete = true;
    for (pid, outcome) in sweep::kill(&found.marked, &marker) {
        match outcome {
            Outcome::Killed => println!("sweep: killed {pid}"),
            Outcome::GoneBeforeOpen => println!("sweep: {pid} gone before pidfd_open"),
            Outcome::Skipped(why) => println!("sweep: skipped {pid}: {why:?}"),
            Outcome::Failed(err) => {
                complete = false;
                println!("sweep: {pid} not signaled: {err}");
            }
        }
    }
    if complete {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn wait_gone(path: &str) -> Result<bool> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    let mut report: serde_json::Map<String, Value> = serde_json::from_str(&text)?;
    let timeout = std::time::Duration::try_from_secs_f64(
        report["step_timeout_ms"]
            .as_f64()
            .context("report has no numeric step_timeout_ms")?
            / 1e3,
    )
    .context("step_timeout_ms is out of range")?;
    // Every recorded child must parse, or the check could skip one and call it
    // clean. A process first seen during CefShutdown counts too.
    let mut children = Vec::new();
    for key in [
        "children_before_shutdown",
        "descendants_right_after_cef_shutdown",
    ] {
        for p in report["shutdown"][key]
            .as_array()
            .with_context(|| format!("report has no shutdown.{key}"))?
        {
            let pid = p["pid"]
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .filter(|n| *n > 0)
                .with_context(|| format!("bad pid in {key} record {p}"))?;
            let start = p["start_time"]
                .as_u64()
                .with_context(|| format!("bad start_time in {key} record {p}"))?;
            children.push((pid, start));
        }
    }
    children.sort_unstable();
    children.dedup();
    let discovery_errors = report["shutdown"]["discovery_errors"]
        .as_array()
        .context("report has no shutdown.discovery_errors")?
        .len();
    let result = probe::wait_gone(&children, timeout);
    // With a discovery error, a child may be missing from both lists.
    let clean = result["clean"] == json!(true) && discovery_errors == 0;
    let pass = report.get("scenario_pass") == Some(&json!(true)) && clean;
    report.insert("children_after_exit".into(), result);
    report.insert("shutdown_clean".into(), json!(clean));
    report.insert("pass".into(), json!(pass));
    std::fs::write(path, serde_json::to_string_pretty(&Value::Object(report))?)?;
    Ok(pass)
}

fn start(
    cx: &mut App,
    cfg: config::Config,
    pump_requests: mpsc::UnboundedReceiver<(std::time::Instant, i64)>,
    context_ready: mpsc::UnboundedReceiver<()>,
    report: scenario::Report,
    children: Rc<RefCell<Option<probe::Tree>>>,
) -> Result<()> {
    // The pump starts first: CEF needs it to finish initializing.
    cx.spawn(async move |cx| pump::run(pump_requests, cx).await)
        .detach();

    const PAGE_WIDTH: i32 = 1024;
    const PAGE_HEIGHT: i32 = 640;
    let (frame_tx, frame_rx) = mpsc::unbounded();
    let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded();
    let hub = Rc::new(engine::Hub {
        view_width: PAGE_WIDTH,
        view_height: PAGE_HEIGHT,
        scale_factor: Cell::new(1.0),
        frame: RefCell::new(None),
        frames_overwritten: Cell::new(0),
        frame_tx,
        paints: Default::default(),
        loaded: Default::default(),
        tracked: Default::default(),
        lifecycle_tx,
        render_process_terminations: Default::default(),
        paint_log: Default::default(),
    });
    let stats = Rc::new(view::ViewStats::default());
    let bounds = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(
            px(PAGE_WIDTH as f32),
            px(PAGE_HEIGHT as f32 + view::HEADER_HEIGHT),
        ),
    };
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    };
    if cfg.no_inactive_throttle {
        options.inactive_frame_interval = None;
    }
    report.borrow_mut().insert(
        "gpui_inactive_frame_interval_ms".into(),
        json!(
            options
                .inactive_frame_interval
                .map(|d| d.as_secs_f64() * 1e3)
        ),
    );
    let window = {
        let hub = hub.clone();
        let stats = stats.clone();
        cx.open_window(options, move |window, cx| {
            hub.scale_factor.set(window.scale_factor());
            cx.new(|cx| view::PageView::new(hub, stats, frame_rx, cx))
        })?
    };
    report
        .borrow_mut()
        .insert("window_scale_factor".into(), json!(hub.scale_factor.get()));

    let run = scenario::Run {
        timers: timers::Timers::new(cx.background_executor().clone()),
        cfg,
        hub,
        stats,
        window,
        lifecycle: lifecycle_rx,
        context_ready,
        report,
        children_before_shutdown: children,
        navigation_started: None,
        idle_window: None,
    };
    cx.spawn(async move |cx| run.run(cx).await).detach();
    Ok(())
}
