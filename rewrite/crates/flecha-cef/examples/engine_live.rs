//! Drives the engine's Phase 2 wiring in real CEF browsers and checks it through the in-process
//! CDP bridge: network blocking with its per-site switch, ad hiding, style and script Boosts,
//! `navigator.webdriver`, navigation helpers, the page lifecycle call tab sleep uses, downloads
//! from a shared and an isolated request context at once, one-time and saved geolocation
//! answers across navigations, and restoring a discarded tab while its context is created.
//!
//! CEF must never reach the desktop session, so this runs only inside the isolated launcher:
//!
//! ```sh
//! cargo build -p flecha-cef --example engine_live
//! tools/isolated-run.sh x11 --binary target/debug/examples/engine_live -- \
//!     ./target/debug/flecha --step-timeout-ms <ms>
//! ```
//!
//! It serves its own pages on 127.0.0.1. `--step-timeout-ms` bounds each wait; a step that
//! takes longer fails the run, which still closes every browser and shuts CEF down. The run
//! prints its longest step, to size the bound from a clean run. It exits nonzero on the first
//! failed check.

use std::cell::Cell;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use flecha_boosts::{Boosts, CompiledBoost};
use flecha_cef::{
    DevToolsEvent, DownloadConfig, DownloadError, DownloadEvent, DownloadHandle, Engine, Event,
    Owner, Partition, Runtime, Viewport,
};
use flecha_core::{SpaceId, TabId, WindowId};
use flecha_downloads::{DownloadSettings, State};
use flecha_permissions::PromptOutcome;
use gpui::{Bounds, point, px, size};
use serde_json::{Value, json};

const PROBE: &str = "/flecha-ad-probe.js";

fn main() -> Result<()> {
    ensure!(
        std::env::var("FLECHA_LAUNCHER").as_deref() == Ok("isolated"),
        "run this only through tools/isolated-run.sh"
    );
    let mut runtime = Runtime::entry()?;
    let steps = Steps::new(step_timeout()?);
    let profile = PathBuf::from(std::env::var_os("FLECHA_PROFILE").context("FLECHA_PROFILE")?);
    let result = run(&mut runtime, &profile, &steps);
    drop(runtime);
    match &result {
        Ok(()) => println!(
            "PASS engine_live: every check passed and CEF shut down; longest step {} ms",
            steps.longest.get().as_millis()
        ),
        Err(_) => eprintln!("FAIL engine_live: CEF shut down after the failure above"),
    }
    result
}

fn step_timeout() -> Result<Duration> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--step-timeout-ms" {
            let ms = args.next().context("--step-timeout-ms needs a value")?;
            return Ok(Duration::from_millis(
                ms.parse().context("--step-timeout-ms")?,
            ));
        }
    }
    bail!("pass --step-timeout-ms <ms>, the longest any one wait may take")
}

/// Bounds each wait and records the longest.
struct Steps {
    budget: Duration,
    longest: Cell<Duration>,
}

impl Steps {
    fn new(budget: Duration) -> Self {
        Self {
            budget,
            longest: Cell::new(Duration::ZERO),
        }
    }

    /// Pumps CEF until `poll` gives a value, for at most the step budget.
    fn wait<T>(&self, what: &str, mut poll: impl FnMut() -> Result<Option<T>>) -> Result<T> {
        let start = Instant::now();
        loop {
            if let Some(value) = poll()? {
                self.longest.set(self.longest.get().max(start.elapsed()));
                return Ok(value);
            }
            ensure!(
                start.elapsed() <= self.budget,
                "waited longer than --step-timeout-ms for {what}"
            );
            pump();
        }
    }
}

fn pump() {
    cef::do_message_loop_work();
    std::thread::sleep(Duration::from_millis(5));
}

struct Live<'a> {
    engine: Engine,
    steps: &'a Steps,
    base: String,
    probe_hits: Arc<AtomicUsize>,
    profile: PathBuf,
}

fn run(runtime: &mut Runtime, profile: &Path, steps: &Steps) -> Result<()> {
    let cef_profile = profile.join("cef");
    std::fs::create_dir_all(&cef_profile)?;
    runtime.initialize(&cef_profile)?;
    steps.wait("CEF to initialize", || Ok(runtime.is_ready().then_some(())))?;

    let probe_hits = Arc::new(AtomicUsize::new(0));
    let port = serve(probe_hits.clone())?;
    let engine = Engine::new(cef_profile, || TabId::new("popup"));
    let mut live = Live {
        engine,
        steps,
        base: format!("http://localhost:{port}"),
        probe_hits,
        profile: profile.to_owned(),
    };
    let result = live.checks();
    // CEF can't shut down while a browser is open, so a failed check closes them too.
    if let Err(error) = &result {
        eprintln!("FAIL engine_live: {error:#}");
    }
    live.engine.close_all();
    let engine = &mut live.engine;
    let drained = steps.wait("every browser to close", || {
        while engine.next_event().is_some() {}
        Ok(engine.is_drained().then_some(()))
    });
    if drained.is_ok() {
        live.engine.release_contexts();
    }
    result.and(drained)
}

fn viewport() -> Viewport {
    let window = Bounds::new(point(px(0.), px(0.)), size(px(1024.), px(768.)));
    Viewport::new(window, window, window, 1.)
}

fn owner() -> Owner {
    Owner::Tab(WindowId::new("window"))
}

impl Live<'_> {
    fn checks(&mut self) -> Result<()> {
        let lists = self.profile.join("adblock");
        std::fs::create_dir_all(&lists)?;
        write_lists(&lists)?;
        let report = self.engine.load_adblock_rules(&lists);
        ensure!(
            report.failures.is_empty(),
            "the cached lists failed to load: {report:?}"
        );
        ensure!(
            self.engine.adblocker().has_engine(),
            "no blocking engine after loading the lists"
        );
        self.engine.set_boosts(boosts()?);

        let tab = TabId::new("live");
        let url = format!("{}/page", self.base);
        self.engine
            .ensure(tab.clone(), owner(), Partition::Shared, &url, viewport())?;
        self.wait_loaded(&tab, "/page")?;

        self.blocking_and_boosts(&tab)?;
        self.navigation(&tab)?;
        self.lifecycle(&tab)?;
        let isolated = TabId::new("isolated");
        self.downloads(&tab, &isolated)?;
        self.permissions(&tab)?;
        self.restore(&tab, &isolated)
    }

    fn blocking_and_boosts(&mut self, tab: &TabId) -> Result<()> {
        // Generic hiding lands one CDP round trip after the load.
        let page = self.settle(tab, |page| {
            page["boostRan"] == "yes" && page["cosmetic"] == "none"
        })?;
        ensure!(
            page["webdriver"] == Value::Bool(false),
            "navigator.webdriver is {}",
            page["webdriver"]
        );
        println!(
            "PASS navigator.webdriver is false while CDP evaluates through SendDevToolsMessage"
        );
        ensure!(
            page["ok"] == true,
            "the page's own script didn't load: {page}"
        );
        ensure!(page["adProbe"] == false, "the blocked script ran: {page}");
        ensure!(
            self.probe_hits.load(Ordering::SeqCst) == 0,
            "the blocked script reached the server"
        );
        let blocked = self.engine.blocked_count(tab);
        ensure!(blocked >= 1, "blocked_count is {blocked}");
        println!(
            "PASS the IO-thread filter cancelled {PROBE} before it reached the server, blocked_count={blocked}"
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
        self.engine.set_adblock_site_disabled("localhost", true);
        self.engine.reload(tab);
        self.wait_loaded(tab, "/page")?;
        let page = self.settle(tab, |page| page["adProbe"] == true)?;
        ensure!(
            self.probe_hits.load(Ordering::SeqCst) >= 1,
            "the allowed script never reached the server"
        );
        ensure!(
            self.engine.blocked_count(tab) == 0,
            "a new page starts a new count"
        );
        ensure!(
            page["cosmetic"] != "none",
            "ad hiding stayed on for an allowed site: {page}"
        );
        println!(
            "PASS with blocking off for the site, the same script and element loaded (negative control)"
        );
        self.engine.set_adblock_site_disabled("localhost", false);
        Ok(())
    }

    fn navigation(&mut self, tab: &TabId) -> Result<()> {
        ensure!(
            !self.engine.can_go_back(tab),
            "a reloaded first page has no history"
        );
        ensure!(
            self.engine.navigate(tab, &format!("{}/second", self.base)),
            "no main frame"
        );
        self.wait_loaded(tab, "/second")?;
        self.wait_settled(tab, "/second")?;
        ensure!(
            self.engine.can_go_back(tab) && !self.engine.can_go_forward(tab),
            "history after a navigation"
        );
        self.engine.go_back(tab);
        self.wait_settled(tab, "/page")?;
        ensure!(self.engine.can_go_forward(tab), "history after going back");
        // A page restored from the back/forward cache keeps its old document, so reload it to
        // check blocking is back on for the site.
        self.engine.reload(tab);
        self.wait_loaded(tab, "/page")?;
        let page = self.settle(tab, |page| page["ok"] == true)?;
        ensure!(
            page["adProbe"] == false,
            "the blocked script ran after re-enabling: {page}"
        );
        ensure!(
            self.engine.blocked_count(tab) >= 1,
            "nothing was blocked after re-enabling"
        );
        println!("PASS go_back, can_go_back, can_go_forward and is_loading follow CEF's history");
        Ok(())
    }

    fn lifecycle(&mut self, tab: &TabId) -> Result<()> {
        self.engine.set_visible(tab, false);
        for state in ["frozen", "active"] {
            let params = json!({ "state": state }).to_string();
            let (success, reply) = self.call(tab, "Page.setWebLifecycleState", Some(&params))?;
            ensure!(success, "Page.setWebLifecycleState {state} failed: {reply}");
        }
        ensure!(
            self.engine.freeze_tab(tab) && self.engine.is_frozen(tab),
            "freeze_tab"
        );
        self.engine.set_visible(tab, true);
        ensure!(!self.engine.is_frozen(tab), "showing a frozen tab thaws it");
        let page = self.settle(tab, |page| page["ok"] == true)?;
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

    /// Both request contexts number their first download 1. Each prompt must keep its own
    /// callback, and two prompts can't both save to one path.
    fn downloads(&mut self, tab: &TabId, isolated: &TabId) -> Result<()> {
        let folder = self.profile.join("downloads");
        std::fs::create_dir_all(&folder)?;
        self.engine.set_download_config(Some(DownloadConfig {
            settings: DownloadSettings {
                folder: folder.clone(),
                ask_where_to_save: true,
            },
            https_first_mode: false,
        }));
        let partition = Partition::Isolated(SpaceId::new("live"));
        let url = format!("{}/second", self.base);
        let engine = &mut self.engine;
        self.steps.wait("the isolated request context", || {
            engine.ensure(
                isolated.clone(),
                owner(),
                partition.clone(),
                &url,
                viewport(),
            )?;
            Ok(engine.host(isolated).map(|_| ()))
        })?;
        self.wait_loaded(isolated, "/second")?;

        ensure!(
            self.engine
                .navigate(tab, &format!("{}/shared/a.txt", self.base)),
            "no main frame"
        );
        let shared = self.next_ask(tab)?;
        ensure!(
            self.engine
                .navigate(isolated, &format!("{}/isolated/a.txt", self.base)),
            "no main frame"
        );
        let other = self.next_ask(isolated)?;
        ensure!(
            shared != other,
            "both contexts' download 1 got handle {shared:?}"
        );

        let picked = folder.join("picked.txt");
        self.engine.accept_download(shared, Some(picked.clone()))?;
        ensure!(
            self.engine.accept_download(other, Some(picked.clone()))
                == Err(DownloadError::PathInUse(picked.clone())),
            "a second prompt took a path a download in progress holds"
        );
        let second = folder.join("isolated.txt");
        self.engine.accept_download(other, Some(second.clone()))?;
        for (handle, path, body) in [
            (shared, &picked, "shared download"),
            (other, &second, "isolated download"),
        ] {
            self.wait_download(handle, path)?;
            ensure!(
                std::fs::read_to_string(path)? == body,
                "{} holds the wrong download",
                path.display()
            );
        }
        println!(
            "PASS a shared and an isolated download with the same CEF id kept their own prompts and files"
        );
        Ok(())
    }

    fn next_ask(&mut self, tab: &TabId) -> Result<DownloadHandle> {
        let engine = &mut self.engine;
        self.steps.wait("a download prompt", || {
            while let Some(message) = engine.next_download_event() {
                if message.tab.as_ref() != Some(tab) {
                    continue;
                }
                match message.event {
                    DownloadEvent::AskPath { handle, .. } => return Ok(Some(handle)),
                    event => bail!("expected a download prompt, got {event:?}"),
                }
            }
            Ok(None)
        })
    }

    fn wait_download(&mut self, handle: DownloadHandle, path: &Path) -> Result<()> {
        let engine = &mut self.engine;
        let state = self.steps.wait("a download to finish", || {
            while engine.next_download_event().is_some() {}
            let list = engine.downloads();
            let row = list.iter().find(|row| row.path() == path);
            Ok(row
                .map(|row| row.state())
                .filter(|state| !matches!(state, State::InProgress | State::Paused)))
        })?;
        ensure!(
            state == State::Complete,
            "download {handle:?} ended {state:?}"
        );
        Ok(())
    }

    /// Allow once lasts for the document; Allow always outlives it.
    fn permissions(&mut self, tab: &TabId) -> Result<()> {
        ensure!(
            self.engine.navigate(tab, &format!("{}/page", self.base)),
            "no main frame"
        );
        self.wait_loaded(tab, "/page")?;
        ensure!(
            self.geolocation(tab)? == "prompt",
            "a fresh profile should ask"
        );
        self.ask_geolocation(tab, PromptOutcome::AllowOnce)?;
        ensure!(
            self.geolocation(tab)? == "granted",
            "Allow once should grant the page"
        );
        ensure!(
            self.engine.navigate(tab, &format!("{}/second", self.base)),
            "no main frame"
        );
        self.wait_loaded(tab, "/second")?;
        let after = self.geolocation(tab)?;
        ensure!(
            after == "prompt",
            "Allow once survived a navigation as {after}"
        );
        println!(
            "PASS Allow once granted geolocation to the page only; the next document asks again"
        );

        self.ask_geolocation(tab, PromptOutcome::AllowAlways)?;
        ensure!(
            self.engine.navigate(tab, &format!("{}/page", self.base)),
            "no main frame"
        );
        self.wait_loaded(tab, "/page")?;
        let after = self.geolocation(tab)?;
        ensure!(
            after == "granted",
            "Allow always didn't outlive the document: {after}"
        );
        println!("PASS Allow always stayed granted after a navigation");
        Ok(())
    }

    fn geolocation(&mut self, tab: &TabId) -> Result<String> {
        let state = self.evaluate(
            tab,
            "navigator.permissions.query({ name: 'geolocation' }).then(status => status.state)",
        )?;
        state
            .as_str()
            .map(str::to_owned)
            .context("permission state")
    }

    fn ask_geolocation(&mut self, tab: &TabId, outcome: PromptOutcome) -> Result<()> {
        self.evaluate(
            tab,
            "void navigator.geolocation.getCurrentPosition(() => {}, () => {})",
        )?;
        let engine = &mut self.engine;
        let prompt = self.steps.wait("the geolocation prompt", || {
            Ok(engine.pending_permission_prompt(tab))
        })?;
        self.engine.resolve_permission(prompt.request, outcome)?;
        Ok(())
    }

    /// A tab discarded in an isolated Space comes back while its request context is created
    /// again, which takes more than one try.
    fn restore(&mut self, tab: &TabId, isolated: &TabId) -> Result<()> {
        ensure!(
            self.engine
                .navigate(isolated, &format!("{}/second", self.base)),
            "no main frame"
        );
        self.wait_loaded(isolated, "/second")?;
        let discarded = self
            .engine
            .discard_tab(isolated)
            .context("discard the isolated tab")?;
        // Release the isolated context, so restoring has to create it again.
        self.engine.close(tab);
        let engine = &mut self.engine;
        self.steps.wait("the other browsers to close", || {
            while engine.next_event().is_some() {}
            Ok(engine.is_drained().then_some(()))
        })?;
        self.engine.release_contexts();

        ensure!(
            self.engine.restore_tab(isolated, viewport())?,
            "the tab wasn't discarded"
        );
        // A released context is created again asynchronously, so this first try only starts it.
        ensure!(
            self.engine.host(isolated).is_none(),
            "the isolated context was ready at once"
        );
        ensure!(
            self.engine.discarded(isolated) == Some(&discarded),
            "restoring before the context was ready lost the discarded page"
        );
        let engine = &mut self.engine;
        self.steps.wait("the restored browser", || {
            engine.restore_tab(isolated, viewport())?;
            Ok(engine.host(isolated).map(|_| ()))
        })?;
        self.wait_loaded(isolated, "/second")?;
        ensure!(
            self.engine.discarded(isolated).is_none(),
            "the record outlived the restore"
        );
        println!(
            "PASS a discarded tab came back to its page while its isolated context was created again"
        );
        Ok(())
    }

    fn wait_loaded(&mut self, tab: &TabId, path: &str) -> Result<()> {
        let engine = &mut self.engine;
        self.steps.wait(&format!("{path} to load"), || {
            while let Some(message) = engine.next_event() {
                if &message.key.tab != tab {
                    continue;
                }
                match message.event {
                    Event::Loaded { url, .. } if url.ends_with(path) => return Ok(Some(())),
                    // A download navigation ends as an aborted load; only this page counts.
                    Event::LoadFailed { url, code } if url.ends_with(path) => {
                        bail!("{url} failed to load: {code}")
                    }
                    _ => {}
                }
            }
            Ok(None)
        })
    }

    /// Until the tab shows `path` and CEF reports it done loading, which can trail its last
    /// load end.
    fn wait_settled(&mut self, tab: &TabId, path: &str) -> Result<()> {
        let engine = &mut self.engine;
        self.steps.wait(&format!("{path} to settle"), || {
            while engine.next_event().is_some() {}
            let shown = engine.url(tab).is_some_and(|url| url.ends_with(path));
            Ok((shown && !engine.is_loading(tab)).then_some(()))
        })
    }

    /// One CDP call and its reply: (success, result JSON).
    fn call(&mut self, tab: &TabId, method: &str, params: Option<&str>) -> Result<(bool, String)> {
        let id = self.engine.send_devtools_message(tab, method, params)?;
        let engine = &mut self.engine;
        self.steps.wait(&format!("the reply to {method}"), || {
            while let Some(message) = engine.next_devtools_event() {
                if let DevToolsEvent::Result {
                    id: reply,
                    success,
                    result,
                } = message.event
                    && reply == id
                {
                    return Ok(Some((success, result)));
                }
            }
            Ok(None)
        })
    }

    /// The value of `expression`, awaited when it is a promise.
    fn evaluate(&mut self, tab: &TabId, expression: &str) -> Result<Value> {
        let params =
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true });
        let (success, reply) = self.call(tab, "Runtime.evaluate", Some(&params.to_string()))?;
        let reply: Value = serde_json::from_str(&reply)?;
        ensure!(
            success && reply.get("exceptionDetails").is_none(),
            "{expression} failed: {reply}"
        );
        Ok(reply["result"]["value"].clone())
    }

    /// The page's state, evaluated until `done` holds for it.
    fn settle(&mut self, tab: &TabId, done: impl Fn(&Value) -> bool) -> Result<Value> {
        let start = Instant::now();
        loop {
            let page = self.evaluate(tab, PAGE_STATE)?;
            if done(&page) {
                return Ok(page);
            }
            ensure!(
                start.elapsed() <= self.steps.budget,
                "waited longer than --step-timeout-ms for the page to reach the expected state: {page}"
            );
            pump();
        }
    }
}

const PAGE_STATE: &str = r#"({
  webdriver: navigator.webdriver,
  ok: !!window.okLoaded,
  adProbe: !!window.adProbeLoaded,
  cosmetic: getComputedStyle(document.querySelector('.flecha-cosmetic-ad')).display,
  color: getComputedStyle(document.getElementById('boosted')).color,
  boostRan: document.documentElement.dataset.boostRan ?? null,
  boostRuns: window.boostRuns ?? 0,
  visibility: document.visibilityState,
})"#;

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
    let attachment = "Content-Disposition: attachment; filename=a.txt\r\n";
    let (kind, extra, body) = match path {
        "/page" => ("text/html", "", PAGE),
        "/second" => (
            "text/html",
            "",
            "<!doctype html><title>second</title>second",
        ),
        "/ok.js" => ("text/javascript", "", "window.okLoaded = true;"),
        "/shared/a.txt" => ("text/plain", attachment, "shared download"),
        "/isolated/a.txt" => ("text/plain", attachment, "isolated download"),
        PROBE => {
            probe_hits.fetch_add(1, Ordering::SeqCst);
            ("text/javascript", "", "window.adProbeLoaded = true;")
        }
        _ => ("text/plain", "", ""),
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n{extra}Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
