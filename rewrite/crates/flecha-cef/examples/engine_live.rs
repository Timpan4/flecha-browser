//! Drives the engine's Phase 2 wiring in a real CEF browser and checks it through the in-process
//! CDP bridge: network blocking with its per-site switch, ad hiding, style and script Boosts,
//! `navigator.webdriver`, navigation helpers and the page lifecycle call tab sleep uses.
//!
//! CEF must never reach the desktop session, so this runs only inside the isolated launcher:
//!
//! ```sh
//! cargo build -p flecha-cef --example engine_live
//! tools/isolated-run.sh x11 --binary target/debug/examples/engine_live -- ./target/debug/flecha
//! ```
//!
//! It serves its own pages on 127.0.0.1 and exits nonzero on the first failed check. The
//! launcher's watchdog bounds every wait.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use flecha_boosts::{Boosts, CompiledBoost};
use flecha_cef::{DevToolsEvent, Engine, Event, Owner, Partition, Runtime, Viewport};
use flecha_core::{TabId, WindowId};
use gpui::{Bounds, point, px, size};
use serde_json::Value;

const PROBE: &str = "/flecha-ad-probe.js";

fn main() -> Result<()> {
    ensure!(
        std::env::var("FLECHA_LAUNCHER").as_deref() == Ok("isolated"),
        "run this only through tools/isolated-run.sh"
    );
    let mut runtime = Runtime::entry()?;
    let profile = PathBuf::from(std::env::var_os("FLECHA_PROFILE").context("FLECHA_PROFILE")?);
    let result = run(&mut runtime, &profile);
    drop(runtime);
    match &result {
        Ok(()) => println!("PASS engine_live: every check passed and CEF shut down"),
        Err(_) => eprintln!("FAIL engine_live: CEF shut down after the failure above"),
    }
    result
}

fn pump() {
    cef::do_message_loop_work();
    std::thread::sleep(Duration::from_millis(5));
}

fn run(runtime: &mut Runtime, profile: &Path) -> Result<()> {
    let cef_profile = profile.join("cef");
    std::fs::create_dir_all(&cef_profile)?;
    runtime.initialize(&cef_profile)?;
    while !runtime.is_ready() {
        pump();
    }

    let probe_hits = Arc::new(AtomicUsize::new(0));
    let port = serve(probe_hits.clone())?;
    let base = format!("http://localhost:{port}");

    let mut engine = Engine::new(cef_profile.clone(), || TabId::new("popup"));
    let lists = profile.join("adblock");
    std::fs::create_dir_all(&lists)?;
    write_lists(&lists)?;
    let report = engine.load_adblock_rules(&lists);
    ensure!(
        report.failures.is_empty(),
        "the cached lists failed to load: {report:?}"
    );
    ensure!(
        engine.adblocker().has_engine(),
        "no blocking engine after loading the lists"
    );
    engine.set_boosts(boosts()?);

    let tab = TabId::new("live");
    let result = checks(&mut engine, &tab, &base, &probe_hits);
    // CEF can't shut down while a browser is open, so a failed check closes it too.
    if let Err(error) = &result {
        eprintln!("FAIL engine_live: {error:#}");
    }
    engine.close_all();
    while !engine.is_drained() {
        while engine.next_event().is_some() {}
        pump();
    }
    engine.release_contexts();
    result
}

fn checks(engine: &mut Engine, tab: &TabId, base: &str, probe_hits: &AtomicUsize) -> Result<()> {
    let window = Bounds::new(point(px(0.), px(0.)), size(px(1024.), px(768.)));
    let viewport = Viewport::new(window, window, window, 1.);
    engine.ensure(
        tab.clone(),
        Owner::Tab(WindowId::new("window")),
        Partition::Shared,
        &format!("{base}/page"),
        viewport,
    )?;
    wait_loaded(engine, tab, "/page")?;

    // Generic hiding lands one CDP round trip after the load.
    let page = settle(engine, tab, |page| {
        page["boostRan"] == "yes" && page["cosmetic"] == "none"
    })?;
    ensure!(
        page["webdriver"] == Value::Bool(false),
        "navigator.webdriver is {}",
        page["webdriver"]
    );
    println!("PASS navigator.webdriver is false while CDP evaluates through SendDevToolsMessage");
    ensure!(
        page["ok"] == true,
        "the page's own script didn't load: {page}"
    );
    ensure!(page["adProbe"] == false, "the blocked script ran: {page}");
    ensure!(
        probe_hits.load(Ordering::SeqCst) == 0,
        "the blocked script reached the server"
    );
    let blocked = engine.blocked_count(tab);
    ensure!(blocked >= 1, "blocked_count is {blocked}");
    println!(
        "PASS the IO-thread filter cancelled {PROBE} before it reached the server, blocked_count={blocked}"
    );
    ensure!(
        page["cosmetic"] == "none",
        "the ad element wasn't hidden: {page}"
    );
    println!("PASS cosmetic filtering hid the ad element");
    ensure!(
        page["color"] == "rgb(1, 2, 3)",
        "the style Boost didn't apply: {page}"
    );
    println!("PASS the style Boost applied as an adopted style sheet");
    ensure!(
        page["boostRuns"] == 1,
        "the script Boost ran {} times",
        page["boostRuns"]
    );
    println!("PASS the script Boost ran once in the page's world");

    // Negative control: with the site shield off for localhost the same script loads.
    engine.set_adblock_site_disabled("localhost", true);
    engine.reload(tab);
    wait_loaded(engine, tab, "/page")?;
    let page = settle(engine, tab, |page| page["adProbe"] == true)?;
    ensure!(
        probe_hits.load(Ordering::SeqCst) >= 1,
        "the allowed script never reached the server"
    );
    ensure!(
        engine.blocked_count(tab) == 0,
        "a new page starts a new count"
    );
    ensure!(
        page["cosmetic"] != "none",
        "ad hiding stayed on for an allowed site: {page}"
    );
    println!(
        "PASS with blocking off for the site, the same script and element loaded (negative control)"
    );
    engine.set_adblock_site_disabled("localhost", false);

    // Navigation helpers.
    ensure!(
        !engine.can_go_back(tab),
        "a reloaded first page has no history"
    );
    ensure!(
        engine.navigate(tab, &format!("{base}/second")),
        "no main frame"
    );
    wait_loaded(engine, tab, "/second")?;
    wait_settled(engine, tab, "/second");
    ensure!(
        engine.can_go_back(tab) && !engine.can_go_forward(tab),
        "history after a navigation"
    );
    engine.go_back(tab);
    wait_loaded(engine, tab, "/page")?;
    wait_settled(engine, tab, "/page");
    ensure!(engine.can_go_forward(tab), "history after going back");
    // Going back loaded the page again, with blocking back on for the site.
    let page = settle(engine, tab, |page| page["ok"] == true)?;
    ensure!(
        page["adProbe"] == false,
        "the blocked script ran after going back: {page}"
    );
    println!("PASS go_back, can_go_back, can_go_forward and is_loading follow CEF's history");

    // The lifecycle call freezing uses, then freeze and thaw through the engine.
    engine.set_visible(tab, false);
    for state in ["frozen", "active"] {
        let params = format!(r#"{{"state":"{state}"}}"#);
        let reply = call(engine, tab, "Page.setWebLifecycleState", Some(&params))?;
        ensure!(
            reply.0,
            "Page.setWebLifecycleState {state} failed: {}",
            reply.1
        );
    }
    ensure!(
        engine.freeze_tab(tab) && engine.is_frozen(tab),
        "freeze_tab"
    );
    engine.set_visible(tab, true);
    ensure!(!engine.is_frozen(tab), "showing a frozen tab thaws it");
    let page = settle(engine, tab, |page| page["ok"] == true)?;
    ensure!(
        page["visibility"] == "visible",
        "the thawed tab is {}",
        page["visibility"]
    );
    println!(
        "PASS Page.setWebLifecycleState froze and resumed the page; freeze_tab and thaw on show"
    );
    Ok(())
}

fn wait_loaded(engine: &mut Engine, tab: &TabId, path: &str) -> Result<()> {
    loop {
        while let Some(message) = engine.next_event() {
            if &message.key.tab != tab {
                continue;
            }
            match message.event {
                Event::Loaded { url, .. } if url.ends_with(path) => return Ok(()),
                Event::LoadFailed { url, code } => bail!("{url} failed to load: {code}"),
                _ => {}
            }
        }
        pump();
    }
}

/// Pumps until the tab shows `path` and CEF reports it done loading, which can trail its last
/// load end.
fn wait_settled(engine: &mut Engine, tab: &TabId, path: &str) {
    while engine.is_loading(tab) || !engine.url(tab).is_some_and(|url| url.ends_with(path)) {
        while engine.next_event().is_some() {}
        pump();
    }
}

/// One CDP call and its reply: (success, result JSON).
fn call(
    engine: &mut Engine,
    tab: &TabId,
    method: &str,
    params: Option<&str>,
) -> Result<(bool, String)> {
    let id = engine.send_devtools_message(tab, method, params)?;
    loop {
        while let Some(message) = engine.next_devtools_event() {
            if let DevToolsEvent::Result {
                id: reply,
                success,
                result,
            } = message.event
                && reply == id
            {
                return Ok((success, result));
            }
        }
        pump();
    }
}

const EXPRESSION: &str = r#"JSON.stringify({
  webdriver: navigator.webdriver,
  ok: !!window.okLoaded,
  adProbe: !!window.adProbeLoaded,
  cosmetic: getComputedStyle(document.querySelector('.flecha-cosmetic-ad')).display,
  color: getComputedStyle(document.getElementById('boosted')).color,
  boostRan: document.documentElement.dataset.boostRan ?? null,
  boostRuns: window.boostRuns ?? 0,
  visibility: document.visibilityState,
})"#;

/// Evaluates the page's state through CDP until `done` holds for it.
fn settle(engine: &mut Engine, tab: &TabId, done: impl Fn(&Value) -> bool) -> Result<Value> {
    let params = serde_json::json!({ "expression": EXPRESSION, "returnByValue": true }).to_string();
    loop {
        let (success, result) = call(engine, tab, "Runtime.evaluate", Some(&params))?;
        ensure!(success, "Runtime.evaluate failed: {result}");
        let result: Value = serde_json::from_str(&result)?;
        if let Some(text) = result["result"]["value"].as_str() {
            let page: Value = serde_json::from_str(text)?;
            if done(&page) {
                return Ok(page);
            }
        }
        for _ in 0..20 {
            pump();
        }
    }
}

fn boosts() -> Result<Boosts> {
    let mut boosts = Boosts::new();
    boosts
        .save(
            "localhost.css",
            "#boosted { color: rgb(1, 2, 3) !important; }",
            None,
            false,
        )
        .context("save the style Boost")?;
    let code = "window.boostRuns = (window.boostRuns ?? 0) + 1; document.documentElement.dataset.boostRan = 'yes';";
    let build = CompiledBoost {
        code: Some(flecha_boosts::compile::wrap_transformed(
            &flecha_boosts::BoostName::parse("localhost.ts").context("Boost name")?,
            code,
        )),
        error: None,
    };
    boosts
        .save("localhost.ts", code, Some(build), false)
        .context("save the script Boost")?;
    Ok(boosts)
}

/// Cached copies of both lists, fresh enough that loading them downloads nothing.
fn write_lists(cache: &Path) -> Result<()> {
    let easylist = format!("{PROBE}\n##.flecha-cosmetic-ad\n");
    for (id, title, body) in [
        ("easylist", "EasyList", easylist.as_str()),
        ("easyprivacy", "EasyPrivacy", "||tracker.invalid^\n"),
    ] {
        std::fs::write(
            cache.join(format!("{id}.txt")),
            format!("[Adblock Plus 2.0]\n! Title: {title}\n\n{body}"),
        )?;
    }
    Ok(())
}

const PAGE: &str = r#"<!doctype html><html><head><title>engine live</title></head><body>
<p id="boosted">boosted</p>
<div class="flecha-cosmetic-ad">ad</div>
<script src="/flecha-ad-probe.js"></script>
<script src="/ok.js"></script>
</body></html>"#;

/// An HTTP server for the test pages, one thread and one request per connection. Chromium opens
/// connections it may never use, so no connection may hold up another.
fn serve(probe_hits: Arc<AtomicUsize>) -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let probe_hits = probe_hits.clone();
            std::thread::spawn(move || respond(&stream, &probe_hits));
        }
    });
    Ok(port)
}

fn respond(stream: &TcpStream, probe_hits: &AtomicUsize) {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
        header.clear();
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (kind, body) = match path {
        "/page" => ("text/html", PAGE),
        "/second" => ("text/html", "<!doctype html><title>second</title>second"),
        "/ok.js" => ("text/javascript", "window.okLoaded = true;"),
        PROBE => {
            probe_hits.fetch_add(1, Ordering::SeqCst);
            ("text/javascript", "window.adProbeLoaded = true;")
        }
        _ => ("text/plain", ""),
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
