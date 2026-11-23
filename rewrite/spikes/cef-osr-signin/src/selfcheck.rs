//! `--self-check`: loads a fixed list of pages in the first browser, collects what each page
//! reports through `console.log`, inspects this process tree in /proc, prints one JSON report and
//! closes the browsers. It signs in to nothing. Its only click is "Reject all" on Google's cookie
//! consent page, when the Web Store redirects there.

use crate::{cli::SelfCheckOptions, common};
use cef::*;
use serde_json::{json, Map, Value};
use std::{cell::RefCell, collections::HashMap, fs, path::Path};

const MARKER: &str = "FLECHA_STEP ";
const NOTE_MARKER: &str = "FLECHA_NOTE ";
pub const REPORT_MARKER: &str = "FLECHA_SELFCHECK_REPORT ";

/// uBlock Origin Lite: a Manifest V3 extension with a popup and content scripts.
pub const WEBSTORE_URL: &str =
    "https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh";

/// Switches that would make the build differ from a shipping browser.
const FORBIDDEN_SWITCHES: &[&str] = &[
    "--remote-debugging-port",
    "--remote-debugging-pipe",
    "--remote-debugging-address",
    "--enable-automation",
    "--headless",
    "--no-sandbox",
    "--disable-setuid-sandbox",
    "--disable-seccomp-filter-sandbox",
    "--disable-namespace-sandbox",
    "--user-agent",
    "--disable-web-security",
    "--ignore-certificate-errors",
    "--use-mock-keychain",
    "--test-type",
];

struct Step {
    name: &'static str,
    url: String,
    js: String,
}

struct Run {
    steps: Vec<Step>,
    index: usize,
    injected: bool,
    flushed: bool,
    generation: u64,
    step_timeout_ms: i64,
    results: Map<String, Value>,
    primary: Option<Browser>,
    report_path: Option<std::path::PathBuf>,
    finished: bool,
}

thread_local! {
    static RUN: RefCell<Option<Run>> = const { RefCell::new(None) };
}

/// Prepares the run and returns the URL the first browser must open.
pub fn init(options: &SelfCheckOptions, profile: &Path) -> std::io::Result<String> {
    let page = profile.join("flecha-selfcheck.html");
    fs::create_dir_all(profile)?;
    fs::write(
        &page,
        "<!doctype html><meta charset=utf-8><title>Flecha self-check</title><p>self-check</p>",
    )?;
    let page_url = format!("file://{}", page.display());

    let steps = vec![
        Step {
            name: "page",
            url: page_url.clone(),
            js: script("page", PAGE_COLLECT, "() => true"),
        },
        Step {
            name: "version",
            url: "chrome://version".into(),
            js: script("version", TEXT_COLLECT, r"() => /\d+\.\d+\.\d+\.\d+/.test(document.body?.innerText ?? '')"),
        },
        Step {
            name: "sandbox",
            url: "chrome://sandbox".into(),
            js: script("sandbox", TEXT_COLLECT, r"() => /adequately sandboxed/i.test(document.body?.innerText ?? '')"),
        },
        Step {
            name: "webstore",
            url: WEBSTORE_URL.into(),
            js: script("webstore", WEBSTORE_COLLECT, WEBSTORE_READY),
        },
    ];
    RUN.with_borrow_mut(|run| {
        *run = Some(Run {
            steps,
            index: 0,
            injected: false,
            flushed: false,
            generation: 0,
            step_timeout_ms: options.step_timeout_ms,
            results: Map::new(),
            primary: None,
            report_path: options.report.clone(),
            finished: false,
        })
    });
    Ok(page_url)
}

fn active() -> bool {
    RUN.with_borrow(|run| run.as_ref().is_some_and(|r| !r.finished))
}

pub fn on_browser_created(browser: &Browser) {
    let first = RUN.with_borrow_mut(|run| match run {
        Some(run) if run.primary.is_none() => {
            run.primary = Some(browser.clone());
            true
        }
        _ => false,
    });
    if first {
        arm_timeout();
    }
}

fn is_primary(browser: &Browser) -> bool {
    RUN.with_borrow(|run| {
        run.as_ref()
            .and_then(|r| r.primary.as_ref())
            .is_some_and(|p| p.identifier() == browser.identifier())
    })
}

pub fn on_main_frame_load_end(browser: &mut Browser, frame: &mut Frame) {
    if !active() || !is_primary(browser) {
        return;
    }
    // Inject on every main-frame load of the step: a step may pass through redirects or an
    // interstitial (Google's consent page) before reaching its page.
    let js = RUN.with_borrow_mut(|run| {
        let run = run.as_mut()?;
        run.injected = true;
        Some(run.steps[run.index].js.clone())
    });
    if let Some(js) = js {
        let url = common::userfree_to_string(&frame.url());
        frame.execute_java_script(Some(&CefString::from(js.as_str())), Some(&CefString::from(url.as_str())), 0);
    }
}

pub fn on_main_frame_load_error(url: &str, error: &str) {
    push_to_step_list("load_errors", json!({ "url": url, "error": error }));
}

fn add_note(note: &str) {
    let value = serde_json::from_str(note).unwrap_or_else(|_| Value::String(note.to_string()));
    push_to_step_list("notes", value);
}

/// Appends to `<current step>_<suffix>` in the results.
fn push_to_step_list(suffix: &str, value: Value) {
    if !active() {
        return;
    }
    RUN.with_borrow_mut(|run| {
        if let Some(run) = run.as_mut() {
            let name = run.steps[run.index].name;
            let entry = run
                .results
                .entry(format!("{name}_{suffix}"))
                .or_insert_with(|| Value::Array(vec![]));
            if let Value::Array(list) = entry {
                list.push(value);
            }
        }
    });
}

/// Returns true when the message belonged to the self-check.
pub fn on_console_message(message: &str) -> bool {
    if let Some(note) = message.strip_prefix(NOTE_MARKER) {
        add_note(note);
        return true;
    }
    let Some(rest) = message.strip_prefix(MARKER) else {
        return false;
    };
    if !active() {
        return true;
    }
    let (name, payload) = rest.split_once(' ').unwrap_or((rest, ""));
    let value = serde_json::from_str(payload).unwrap_or_else(|_| Value::String(payload.to_string()));
    let current = RUN.with_borrow_mut(|run| {
        let run = run.as_mut()?;
        (run.steps[run.index].name == name).then(|| {
            run.results.insert(name.to_string(), value);
        })
    });
    if current.is_some() {
        advance();
    }
    true
}

fn advance() {
    let next = RUN.with_borrow_mut(|run| {
        let run = run.as_mut()?;
        run.index += 1;
        run.injected = false;
        run.flushed = false;
        run.generation += 1;
        if run.index >= run.steps.len() {
            None
        } else {
            Some((run.primary.clone()?, run.steps[run.index].url.clone()))
        }
    });
    match next {
        Some((browser, url)) => {
            if let Some(frame) = browser.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
            arm_timeout();
        }
        None => finish(),
    }
}

fn arm_timeout() {
    let (generation, delay) = RUN.with_borrow(|run| {
        let run = run.as_ref().expect("self-check run");
        (run.generation, run.step_timeout_ms)
    });
    post_delayed_task(ThreadId::UI, Some(&mut StepTimeout::new(generation)), delay);
}

wrap_task! {
    struct StepTimeout {
        generation: u64,
    }

    impl Task {
        fn execute(&self) {
            on_timeout(self.generation);
        }
    }
}

fn on_timeout(generation: u64) {
    enum Action {
        Flush(Browser),
        Record,
    }
    let action = RUN.with_borrow_mut(|run| {
        let run = run.as_mut()?;
        if run.finished || run.generation != generation {
            return None;
        }
        if run.injected && !run.flushed {
            // The page script is waiting for its ready condition. Ask it to report what it has.
            run.flushed = true;
            run.generation += 1;
            return run.primary.clone().map(Action::Flush);
        }
        let name = run.steps[run.index].name;
        run.results.insert(
            name.to_string(),
            json!({ "timeout": true, "script_injected": run.injected }),
        );
        Some(Action::Record)
    });
    match action {
        Some(Action::Flush(browser)) => {
            if let Some(frame) = browser.main_frame() {
                let js = "window.__flechaFlush && window.__flechaFlush('timeout')";
                frame.execute_java_script(Some(&CefString::from(js)), None, 0);
            }
            arm_timeout();
        }
        Some(Action::Record) => advance(),
        None => {}
    }
}

fn finish() {
    let (results, report_path) = RUN.with_borrow_mut(|run| {
        let run = run.as_mut().expect("self-check run");
        run.finished = true;
        (std::mem::take(&mut run.results), run.report_path.clone())
    });

    let browsers: Vec<Value> = common::browsers()
        .iter()
        .map(|b| {
            let host = b.host();
            json!({
                "id": b.identifier(),
                "is_popup": b.is_popup() != 0,
                "runtime_style": host.as_ref().map(|h| style_name(h.runtime_style())),
                "windowless": host.as_ref().map(|h| h.is_window_rendering_disabled() != 0),
            })
        })
        .collect();

    let processes = inspect_processes();
    let forbidden: Vec<Value> = processes
        .iter()
        .flat_map(|p| {
            let pid = p["pid"].clone();
            p["cmdline"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |arg| {
                    let arg = arg.as_str()?.to_string();
                    FORBIDDEN_SWITCHES
                        .iter()
                        .any(|s| arg == *s || arg.starts_with(&format!("{s}=")))
                        .then(|| json!({ "pid": pid, "switch": arg }))
                })
        })
        .collect();

    let report = json!({
        "mode": common::mode().name(),
        "cef": cef_versions(),
        "browsers": browsers,
        "steps": results,
        "processes": processes,
        "forbidden_switches_found": forbidden,
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize report");
    println!("{REPORT_MARKER}{text}");
    if let Some(path) = report_path {
        if let Err(e) = fs::write(&path, &text) {
            eprintln!("cannot write report {}: {e}", path.display());
        }
    }
    common::close_all();
}

fn style_name(style: RuntimeStyle) -> &'static str {
    if style == RuntimeStyle::ALLOY {
        "alloy"
    } else if style == RuntimeStyle::CHROME {
        "chrome"
    } else {
        "default"
    }
}

fn cef_versions() -> Value {
    let compiled = String::from_utf8_lossy(cef::sys::CEF_VERSION)
        .trim_end_matches('\0')
        .to_string();
    // SAFETY: api_hash returns a NUL-terminated string owned by libcef.
    let runtime_commit = unsafe {
        std::ffi::CStr::from_ptr(api_hash(cef::sys::CEF_API_VERSION_LAST, 2))
            .to_string_lossy()
            .into_owned()
    };
    let libcef = fs::read_to_string("/proc/self/maps").ok().and_then(|maps| {
        maps.lines()
            .filter_map(|l| l.split_whitespace().nth(5))
            .find(|p| p.ends_with("/libcef.so"))
            .map(str::to_string)
    });
    let archive = libcef.as_ref().and_then(|p| {
        let dir = Path::new(p).parent()?;
        let text = fs::read_to_string(dir.join("archive.json")).ok()?;
        serde_json::from_str::<Value>(&text).ok()
    });
    json!({
        "compiled_cef_version": compiled,
        "runtime_cef_commit": runtime_commit,
        "loaded_libcef": libcef,
        "archive": archive,
    })
}

/// Lists this process and its descendants with their sandbox state from /proc.
fn inspect_processes() -> Vec<Value> {
    let root = std::process::id();
    let mut parents: HashMap<u32, u32> = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            // Field 4 (ppid) follows the parenthesised command name, which may contain spaces.
            if let Some(after) = stat.rsplit_once(')').map(|(_, a)| a) {
                if let Some(ppid) = after.split_whitespace().nth(1).and_then(|v| v.parse().ok()) {
                    parents.insert(pid, ppid);
                }
            }
        }
    }
    let is_descendant = |mut pid: u32| loop {
        if pid == root {
            return true;
        }
        match parents.get(&pid) {
            Some(&ppid) if ppid != 0 && ppid != pid => pid = ppid,
            _ => return false,
        }
    };
    let own_ns = |kind: &str| fs::read_link(format!("/proc/{root}/ns/{kind}")).ok();
    let mut pids: Vec<u32> = parents.keys().copied().filter(|&p| is_descendant(p)).collect();
    pids.sort_unstable();
    pids.into_iter()
        .map(|pid| {
            let cmdline: Vec<String> = fs::read(format!("/proc/{pid}/cmdline"))
                .map(|raw| {
                    raw.split(|b| *b == 0)
                        .filter(|a| !a.is_empty())
                        .map(|a| String::from_utf8_lossy(a).into_owned())
                        .collect()
                })
                .unwrap_or_default();
            let process_type = cmdline
                .iter()
                .find_map(|a| a.strip_prefix("--type="))
                .unwrap_or(if pid == root { "browser" } else { "?" })
                .to_string();
            let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
            let field = |name: &str| {
                status
                    .lines()
                    .find_map(|l| l.strip_prefix(name))
                    .map(|v| v.trim().to_string())
            };
            let ns: Map<String, Value> = ["user", "pid", "net"]
                .iter()
                .map(|kind| {
                    let value = match fs::read_link(format!("/proc/{pid}/ns/{kind}")) {
                        Ok(link) => json!(if Some(&link) == own_ns(kind).as_ref() {
                            "same as browser"
                        } else {
                            "separate"
                        }),
                        Err(e) => json!(format!("unreadable: {e}")),
                    };
                    (kind.to_string(), value)
                })
                .collect();
            json!({
                "pid": pid,
                "type": process_type,
                // Zygote children keep the zygote's argv, so the name tells them apart.
                "name": field("Name:"),
                "seccomp": field("Seccomp:"),
                "no_new_privs": field("NoNewPrivs:"),
                "namespaces": ns,
                "cmdline": cmdline,
            })
        })
        .collect()
}

/// Wraps a collector so that it reports once, when `ready()` first holds or when the host
/// calls `window.__flechaFlush` after the step timeout.
fn script(name: &str, collect: &str, ready: &str) -> String {
    format!(
        r#"(() => {{
  const NAME = {name:?};
  let done = false;
  const collect = {collect};
  const ready = {ready};
  const report = async (reason) => {{
    if (done) return;
    done = true;
    let r;
    try {{ r = await collect(); }} catch (e) {{ r = {{ error: String(e) }}; }}
    r.reason = reason;
    console.log('{MARKER}' + NAME + ' ' + JSON.stringify(r));
  }};
  window.__flechaFlush = report;
  const check = () => {{ try {{ if (!done && ready()) report('ready'); }} catch (e) {{}} }};
  check();
  if (!done) new MutationObserver(check).observe(document, {{ subtree: true, childList: true, characterData: true, attributes: true }});
}})();"#
    )
}

const PAGE_COLLECT: &str = r#"async () => {
  const r = {
    href: location.href,
    isSecureContext: window.isSecureContext,
    webdriver: navigator.webdriver,
    userAgent: navigator.userAgent,
    userAgentDataPresent: !!navigator.userAgentData,
    windowChromeKeys: window.chrome ? Object.keys(window.chrome) : null,
    pdfViewerEnabled: navigator.pdfViewerEnabled,
    pluginCount: navigator.plugins.length,
    languages: navigator.languages,
    screen: { width: screen.width, height: screen.height, availWidth: screen.availWidth,
              availHeight: screen.availHeight, colorDepth: screen.colorDepth },
    devicePixelRatio: window.devicePixelRatio,
    inner: [window.innerWidth, window.innerHeight],
  };
  const d = navigator.userAgentData;
  if (d) {
    r.brands = d.brands;
    r.platform = d.platform;
    try {
      const h = await d.getHighEntropyValues(['fullVersionList', 'uaFullVersion']);
      r.fullVersionList = h.fullVersionList;
      r.uaFullVersion = h.uaFullVersion;
    } catch (e) { r.highEntropyError = String(e); }
  }
  return r;
}"#;

const TEXT_COLLECT: &str = r#"() => ({
  href: location.href,
  title: document.title,
  text: document.body ? document.body.innerText : null,
})"#;

/// From an EU address the store first redirects to Google's cookie consent page. The check
/// rejects all optional cookies there, as a person would, and records that it did. It never
/// signs in and never clicks an install button.
const WEBSTORE_READY: &str = r#"() => {
  const label = el => (el.innerText || el.getAttribute('aria-label') || el.value || '').trim();
  if (location.hostname === 'consent.google.com') {
    if (!window.__flechaConsent) {
      const reject = [...document.querySelectorAll('button, input[type=submit]')]
        .find(b => /^reject all$/i.test(label(b)));
      if (reject) {
        window.__flechaConsent = true;
        console.log('FLECHA_NOTE ' + JSON.stringify({ consent: 'clicked Reject all', href: location.href }));
        reject.click();
      }
    }
    return false;
  }
  // Report early only once an install button is enabled. A button that stays disabled is
  // reported when the step times out.
  return [...document.querySelectorAll('button')]
    .some(b => /(add to|remove from) chrome/i.test(label(b))
      && !b.disabled && b.getAttribute('aria-disabled') !== 'true');
}"#;

const WEBSTORE_COLLECT: &str = r#"() => {
  const label = el => (el.innerText || el.getAttribute('aria-label') || '').trim();
  const buttons = [...new Set([...document.querySelectorAll('button, [role=button]')]
    .map(label).filter(Boolean))];
  const installButtons = [...document.querySelectorAll('button')]
    .filter(b => /chrome/i.test(label(b)))
    .map(b => ({ label: label(b), disabled: b.disabled || b.getAttribute('aria-disabled') === 'true' }));
  const text = document.body ? document.body.innerText : '';
  return {
    href: location.href,
    title: document.title,
    installButtons,
    buttons,
    chromeMentions: text.split('\n').map(s => s.trim()).filter(s => /chrome/i.test(s)),
    webstorePrivate: typeof window.chrome?.webstorePrivate,
    management: typeof window.chrome?.management,
  };
}"#;
