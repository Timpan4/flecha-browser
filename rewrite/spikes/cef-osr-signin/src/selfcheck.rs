//! `--self-check`: loads a fixed list of pages in the first browser, collects what each page
//! reports through `console.log`, inspects this process tree in /proc, prints one JSON report and
//! closes the browsers. It signs in to nothing. Its only click is "Reject all" on Google's cookie
//! consent page, when the Web Store redirects there.
//!
//! Reports carry a per-run nonce and are accepted only from the first browser, so console
//! output from anything else cannot complete a step. Command lines and the home directory are
//! redacted from the report.

use crate::{
    cli::{self, SelfCheckOptions},
    common, runtime,
};
use cef::*;
use serde_json::{Map, Value, json};
use std::{cell::RefCell, collections::HashMap, fs, path::Path};

const MARKER: &str = "FLECHA_STEP ";
const NOTE_MARKER: &str = "FLECHA_NOTE ";
pub const REPORT_MARKER: &str = "FLECHA_SELFCHECK_REPORT ";

/// uBlock Origin Lite: a Manifest V3 extension with a popup and content scripts.
pub const WEBSTORE_URL: &str =
    "https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh";

/// Switches whose values the report keeps, when the value is made only of letters, digits and
/// `_.,:-` (feature lists, process types, platform and store names). Every other value is
/// replaced, since `--` passes arbitrary switches and some carry paths.
const SHOWN_SWITCH_VALUES: &[&str] = &[
    "type",
    "utility-sub-type",
    "lang",
    "ozone-platform",
    "disable-features",
    "enable-features",
    "password-store",
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
    nonce: String,
    runtime: Value,
}

thread_local! {
    static RUN: RefCell<Option<Run>> = const { RefCell::new(None) };
}

/// Prepares the run and returns the URL the first browser must open.
pub fn init(
    options: &SelfCheckOptions,
    profile: &Path,
    runtime: &runtime::Runtime,
) -> std::io::Result<String> {
    let page = profile.join("flecha-selfcheck.html");
    fs::create_dir_all(profile)?;
    fs::write(
        &page,
        "<!doctype html><meta charset=utf-8><title>Flecha self-check</title><p>self-check</p>",
    )?;
    let page_url = file_url(&page);
    let nonce = nonce();

    let steps = vec![
        Step {
            name: "page",
            url: page_url.clone(),
            js: script("page", &nonce, PAGE_COLLECT, "() => true"),
        },
        Step {
            name: "version",
            url: "chrome://version".into(),
            js: script(
                "version",
                &nonce,
                TEXT_COLLECT,
                r"() => /\d+\.\d+\.\d+\.\d+/.test(document.body?.innerText ?? '')",
            ),
        },
        Step {
            name: "sandbox",
            url: "chrome://sandbox".into(),
            js: script(
                "sandbox",
                &nonce,
                TEXT_COLLECT,
                r"() => /adequately sandboxed/i.test(document.body?.innerText ?? '')",
            ),
        },
        Step {
            name: "webstore",
            url: WEBSTORE_URL.into(),
            js: script(
                "webstore",
                &nonce,
                WEBSTORE_COLLECT,
                &WEBSTORE_READY.replace("__NONCE__", &nonce),
            ),
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
            nonce,
            runtime: runtime.report(),
        })
    });
    Ok(page_url)
}

/// A `file://` URL for an absolute path, percent-encoding everything but unreserved
/// characters and `/`, so `#`, `%` or `?` in a directory name stay part of the path.
fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(Some(hi)), Some(Some(lo))) = (
                bytes.get(i + 1).map(|&b| hex(b)),
                bytes.get(i + 2).map(|&b| hex(b)),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replaces home directories with `~` in every string of a report value, including
/// percent-encoded ones (a `file://` href): a string whose decoded form contains a home
/// directory is reported decoded.
fn redact_homes(value: &mut Value, homes: &[String]) {
    match value {
        Value::String(text) => {
            let decoded = percent_decode(text);
            let base = if homes.iter().any(|h| decoded.contains(h.as_str())) {
                decoded
            } else {
                text.clone()
            };
            *text = homes.iter().fold(base, |t, h| t.replace(h.as_str(), "~"));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact_homes(v, homes)),
        Value::Object(map) => map.values_mut().for_each(|v| redact_homes(v, homes)),
        _ => {}
    }
}

/// Not a secret: it only keeps console output that did not come from the injected collector
/// from completing a step.
fn nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", nanos, std::process::id())
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
        frame.execute_java_script(
            Some(&CefString::from(js.as_str())),
            Some(&CefString::from(url.as_str())),
            0,
        );
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

/// Returns true when the message was addressed to the self-check (accepted or rejected).
/// Messages are `FLECHA_STEP <nonce> <step> <json object>` and `FLECHA_NOTE <nonce> <json>`.
pub fn on_console_message(browser: Option<&Browser>, message: &str) -> bool {
    let (note, rest) = if let Some(rest) = message.strip_prefix(NOTE_MARKER) {
        (true, rest)
    } else if let Some(rest) = message.strip_prefix(MARKER) {
        (false, rest)
    } else {
        return false;
    };
    if !active() || !browser.is_some_and(is_primary) {
        return true;
    }
    let nonce = RUN.with_borrow(|run| run.as_ref().map(|r| r.nonce.clone()).unwrap_or_default());
    let Some(rest) = rest
        .strip_prefix(nonce.as_str())
        .and_then(|r| r.strip_prefix(' '))
    else {
        return true;
    };
    if note {
        add_note(rest);
        return true;
    }
    let Some((name, payload)) = rest.split_once(' ') else {
        return true;
    };
    let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(payload) else {
        return true;
    };
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
    if post_delayed_task(ThreadId::UI, Some(&mut StepTimeout::new(generation)), delay) != 1 {
        // Without a deadline a step that never reports would wait forever.
        eprintln!("self-check: cannot schedule the step timeout; stopping");
        RUN.with_borrow_mut(|run| {
            if let Some(run) = run.as_mut() {
                run.results
                    .insert("error".into(), json!("cannot schedule the step timeout"));
            }
        });
        finish();
    }
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
    let Some((results, report_path, runtime)) = RUN.with_borrow_mut(|run| {
        let run = run.as_mut()?;
        if run.finished {
            return None;
        }
        run.finished = true;
        Some((
            std::mem::take(&mut run.results),
            run.report_path.clone(),
            run.runtime.clone(),
        ))
    }) else {
        return;
    };

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

    let scan = inspect_processes();
    let forbidden: Vec<Value> = scan
        .processes
        .iter()
        .flat_map(|p| {
            p.cmdline
                .iter()
                .filter(|a| cli::is_forbidden(a))
                .filter_map(|a| redact_switch(a))
                .map(|switch| json!({ "pid": p.pid, "switch": switch }))
        })
        .collect();
    let processes: Vec<Value> = scan.processes.iter().map(|p| p.report.clone()).collect();

    let mut results = results;
    if let Some(Value::Object(version)) = results.get_mut("version")
        && let Some(Value::String(text)) = version.get_mut("text")
    {
        *text = redact_version_text(text);
    }

    let failures = verdict(&results, &forbidden, &scan.errors);
    let mut report = json!({
        "mode": common::mode().name(),
        "pass": failures.is_empty(),
        "failures": failures,
        "cef": runtime,
        "browsers": browsers,
        "steps": results,
        "processes": processes,
        // A scan with read errors cannot show that no forbidden switch was present.
        "process_scan": { "complete": scan.errors.is_empty(), "errors": scan.errors },
        "forbidden_switches_found": forbidden,
    });
    redact_homes(&mut report, &homes());
    let text = serde_json::to_string_pretty(&report).expect("serialize report");
    println!("{REPORT_MARKER}{text}");
    for failure in &failures {
        eprintln!("self-check failed: {failure}");
    }
    if !failures.is_empty() {
        FAILED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    if let Some(path) = report_path
        && let Err(e) = fs::write(&path, &text)
    {
        eprintln!("cannot write report {}: {e}", path.display());
    }
    common::close_all();
}

static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True after a self-check found the sandbox off, a forbidden switch, `navigator.webdriver`
/// not false, or an incomplete process scan; `main` then exits with a failure status.
pub fn failed() -> bool {
    FAILED.load(std::sync::atomic::Ordering::SeqCst)
}

/// The conditions that make the browser differ from a shipping one. Each is a failure, so the
/// exit status shows it without reading the report.
fn verdict(
    results: &Map<String, Value>,
    forbidden: &[Value],
    scan_errors: &[String],
) -> Vec<String> {
    let mut failures = Vec::new();
    // The sandbox step reports "ready" only once chrome://sandbox says "adequately sandboxed".
    if results.get("sandbox").and_then(|s| s.get("reason")) != Some(&json!("ready")) {
        failures.push("chrome://sandbox did not report an adequate sandbox".to_string());
    }
    if !forbidden.is_empty() {
        failures.push(format!(
            "forbidden switches found: {}",
            Value::from(forbidden.to_vec())
        ));
    }
    match results.get("page").and_then(|p| p.get("webdriver")) {
        Some(Value::Bool(false)) => {}
        other => failures.push(format!("navigator.webdriver is {other:?}, not false")),
    }
    if !scan_errors.is_empty() {
        failures.push(format!(
            "process scan incomplete ({} errors), so switches and sandbox state are unproven",
            scan_errors.len()
        ));
    }
    failures
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

/// The home directories to hide in the report: `$HOME` and the user's passwd entry (the Xvfb
/// harness gives the app a scratch `$HOME`).
fn homes() -> Vec<String> {
    let mut homes: Vec<String> = std::env::var("HOME").into_iter().collect();
    let uid = fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix("Uid:"))
            .and_then(|v| v.split_whitespace().next().map(str::to_string))
    });
    if let (Some(uid), Ok(passwd)) = (uid, fs::read_to_string("/etc/passwd")) {
        homes.extend(passwd.lines().find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 5 && f[2] == uid).then(|| f[5].to_string())
        }));
    }
    // Longest first, so a scratch HOME under the real home is replaced whole.
    homes.retain(|h| h.len() > 1);
    homes.sort_by_key(|h| std::cmp::Reverse(h.len()));
    homes
}

/// A switch as the report shows it, written `--name` whatever prefix it had (see
/// `cli::switch`). Its value is kept only for switches in SHOWN_SWITCH_VALUES and only when
/// it is made of safe characters. `None` for an argument that is not a switch.
fn redact_switch(arg: &str) -> Option<String> {
    let (name, value) = cli::switch(arg)?;
    let safe = |v: &str| {
        v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ',' | ':' | '-'))
    };
    Some(match value {
        None => format!("--{name}"),
        Some(v) if SHOWN_SWITCH_VALUES.contains(&name) && safe(v) => format!("--{name}={v}"),
        Some(_) => format!("--{name}=<redacted>"),
    })
}

/// A command line as the report shows it, from the process's argv (/proc/<pid>/cmdline, so
/// argument boundaries are exact): switches only, values redacted as above, and no program
/// path or positional arguments.
fn redact_cmdline(cmdline: &[String]) -> Vec<String> {
    cmdline
        .iter()
        .skip(1)
        .filter_map(|a| redact_switch(a))
        .collect()
}

/// `chrome://version` lists the executable, profile path and full command line. The command
/// line is a single string there, joined with spaces, so argument boundaries are lost; it is
/// dropped (the process scan reports each process's switches from argv), and so are the paths.
fn redact_version_text(text: &str) -> String {
    text.lines()
        .map(|line| match line.split_once('\t') {
            Some(("Command Line", _)) => {
                "Command Line\t<omitted; see processes[].switches>".to_string()
            }
            // "Executable Path", "Module Path", "User Data Path [*]", "Profile Path [*]".
            Some((label, _)) if label.contains(" Path") => format!("{label}\t<redacted>"),
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Process {
    pid: u32,
    /// Raw, for the forbidden-switch scan only; never written to the report.
    cmdline: Vec<String>,
    report: Value,
}

struct Scan {
    processes: Vec<Process>,
    errors: Vec<String>,
}

/// Linux `ESRCH` (include/uapi/asm-generic/errno-base.h): reading /proc of a process that is
/// exiting.
const ESRCH: i32 = 3;

/// Lists this process and its descendants with their sandbox state from /proc.
fn inspect_processes() -> Scan {
    let root = std::process::id();
    let mut errors = Vec::new();
    let mut parents: HashMap<u32, u32> = HashMap::new();
    match fs::read_dir("/proc") {
        Ok(entries) => {
            for entry in entries.flatten() {
                let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                    continue;
                };
                // A process can exit between listing and reading (NotFound, or ESRCH); that is
                // not a scan gap. Any other read error is.
                let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Ok(stat) => stat,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::NotFound
                            || e.raw_os_error() == Some(ESRCH) =>
                    {
                        continue;
                    }
                    Err(e) => {
                        errors.push(format!("/proc/{pid}/stat: {e}"));
                        continue;
                    }
                };
                // Field 4 (ppid) follows the parenthesised command name, which may contain spaces.
                match stat
                    .rsplit_once(')')
                    .and_then(|(_, after)| after.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
                {
                    Some(ppid) => {
                        parents.insert(pid, ppid);
                    }
                    None => errors.push(format!("/proc/{pid}/stat: no parent pid")),
                }
            }
        }
        Err(e) => errors.push(format!("/proc: {e}")),
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
    let own_ns: HashMap<&str, std::io::Result<std::path::PathBuf>> = ["user", "pid", "net"]
        .into_iter()
        .map(|kind| (kind, fs::read_link(format!("/proc/{root}/ns/{kind}"))))
        .collect();
    for (kind, link) in &own_ns {
        if let Err(e) = link {
            errors.push(format!("/proc/{root}/ns/{kind}: {e}"));
        }
    }
    let mut pids: Vec<u32> = parents
        .keys()
        .copied()
        .filter(|&p| is_descendant(p))
        .collect();
    pids.sort_unstable();
    let processes = pids
        .into_iter()
        .map(|pid| {
            let cmdline: Vec<String> = match fs::read(format!("/proc/{pid}/cmdline")) {
                Ok(raw) => raw
                    .split(|b| *b == 0)
                    .filter(|a| !a.is_empty())
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .collect(),
                Err(e) => {
                    errors.push(format!("/proc/{pid}/cmdline: {e}"));
                    Vec::new()
                }
            };
            let process_type = cmdline
                .iter()
                .find_map(|a| match cli::switch(a) {
                    Some(("type", value)) => value,
                    _ => None,
                })
                .unwrap_or(if pid == root { "browser" } else { "?" })
                .to_string();
            let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_else(|e| {
                errors.push(format!("/proc/{pid}/status: {e}"));
                String::new()
            });
            let field = |name: &str| {
                status
                    .lines()
                    .find_map(|l| l.strip_prefix(name))
                    .map(|v| v.trim().to_string())
            };
            let ns: Map<String, Value> = ["user", "pid", "net"]
                .iter()
                .map(|kind| {
                    // Either side unreadable means the comparison is unknown, not "separate".
                    let link = fs::read_link(format!("/proc/{pid}/ns/{kind}"));
                    let value = match (&link, &own_ns[kind]) {
                        (Ok(link), Ok(own)) => json!(if link == own {
                            "same as browser"
                        } else {
                            "separate"
                        }),
                        (Err(e), _) => {
                            errors.push(format!("/proc/{pid}/ns/{kind}: {e}"));
                            json!("unknown")
                        }
                        (Ok(_), Err(_)) => json!("unknown"),
                    };
                    (kind.to_string(), value)
                })
                .collect();
            let report = json!({
                "pid": pid,
                "type": process_type,
                // Zygote children keep the zygote's argv, so the name tells them apart.
                "name": field("Name:"),
                "seccomp": field("Seccomp:"),
                "no_new_privs": field("NoNewPrivs:"),
                "namespaces": ns,
                "switches": redact_cmdline(&cmdline),
            });
            Process {
                pid,
                cmdline,
                report,
            }
        })
        .collect();
    Scan { processes, errors }
}

/// Wraps a collector so that it reports once, when `ready()` first holds or when the host
/// calls `window.__flechaFlush` after the step timeout.
fn script(name: &str, nonce: &str, collect: &str, ready: &str) -> String {
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
    console.log('{MARKER}{nonce} ' + NAME + ' ' + JSON.stringify(r));
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
        console.log('FLECHA_NOTE __NONCE__ ' + JSON.stringify({ consent: 'clicked Reject all' }));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_are_reported_by_name_whatever_the_prefix() {
        assert_eq!(
            redact_switch("-no-sandbox").as_deref(),
            Some("--no-sandbox")
        );
        assert_eq!(
            redact_switch("-remote-debugging-port=0").as_deref(),
            Some("--remote-debugging-port=<redacted>")
        );
        assert_eq!(
            redact_switch("--type=renderer").as_deref(),
            Some("--type=renderer")
        );
        assert_eq!(
            redact_switch("--lang=/home/x/evil").as_deref(),
            Some("--lang=<redacted>")
        );
        assert_eq!(redact_switch("/opt/bin/app"), None);
        let argv: Vec<String> = ["/opt/app", "-no-sandbox", "/home/x/--fake", "--lang=en-US"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(redact_cmdline(&argv), ["--no-sandbox", "--lang=en-US"]);
    }

    #[test]
    fn home_paths_are_redacted_even_when_percent_encoded() {
        let homes = vec!["/home/a b".to_string()];
        let mut report = json!({
            "href": "file:///home/a%20b/p%23x/page.html",
            "list": ["/home/a b/x", "plain"],
        });
        redact_homes(&mut report, &homes);
        assert_eq!(report["href"], "file://~/p#x/page.html");
        assert_eq!(report["list"], json!(["~/x", "plain"]));
    }

    #[test]
    fn verdict_fails_on_sandbox_switches_webdriver_and_scan_gaps() {
        let good: Map<String, Value> = serde_json::from_value(json!({
            "sandbox": { "reason": "ready" },
            "page": { "webdriver": false },
        }))
        .unwrap();
        assert!(verdict(&good, &[], &[]).is_empty());
        let bad: Map<String, Value> = serde_json::from_value(json!({
            "sandbox": { "reason": "timeout" },
            "page": { "webdriver": true },
        }))
        .unwrap();
        let failures = verdict(
            &bad,
            &[json!({"pid": 1, "switch": "--no-sandbox"})],
            &["x".into()],
        );
        assert_eq!(failures.len(), 4, "{failures:?}");
    }
}
