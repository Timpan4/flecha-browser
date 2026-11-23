//! Spike options, read from `--pump-NAME=VALUE` arguments. CEF sees the same
//! command line and ignores switches it does not know.
//!
//! Durations have no defaults: each run states them on its command line
//! (see scripts/run-checks.sh), so every number in REPORT.md has a source.

use anyhow::{Context as _, Result, bail};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Idle,
    Animate,
    Cdp,
    DevTools,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub scenario: Scenario,
    pub url: String,
    pub out: PathBuf,
    pub cache_dir: PathBuf,
    pub no_sandbox: bool,
    /// Bounds every wait on CEF: load, a DevTools call, DevTools open/close, shutdown.
    pub step_timeout: Duration,
    /// Time after load before measuring.
    pub warmup: Duration,
    /// Length of each measured window.
    pub measure: Duration,
    /// DevTools scenario: delay between DevTools appearing and `CloseDevTools()`.
    pub devtools_close_after: Duration,
    /// DevTools scenario: open a second, uninspected page browser too.
    pub second_browser: bool,
    /// Turn off GPUI's frame-rate cap for inactive windows. Under Xvfb with no
    /// window manager the window never becomes active.
    pub no_inactive_throttle: bool,
    /// Paths the process tree must not have open, mapped or connected to
    /// (the desktop session's runtime dir, the system bus socket, the real home).
    pub forbidden_paths: Vec<String>,
    /// Exceptions inside `forbidden_paths` the run needs: the bundle with
    /// libcef, the test pages, the results dir that holds stderr.
    pub allowed_paths: Vec<String>,
}

impl Config {
    pub fn from_args() -> Result<Self> {
        let mut opts: HashMap<String, String> = HashMap::new();
        for arg in std::env::args().skip(1) {
            if let Some(rest) = arg.strip_prefix("--pump-") {
                let (k, v) = rest.split_once('=').unwrap_or((rest, "1"));
                opts.insert(k.to_string(), v.to_string());
            }
        }
        let get = |k: &str| {
            opts.get(k)
                .cloned()
                .with_context(|| format!("missing --pump-{k}="))
        };
        let ms = |k: &str| -> Result<Duration> {
            let v = get(k)?;
            Ok(Duration::from_millis(
                v.parse().with_context(|| format!("--pump-{k}={v}"))?,
            ))
        };
        let scenario = match get("scenario")?.as_str() {
            "idle" => Scenario::Idle,
            "animate" => Scenario::Animate,
            "cdp" => Scenario::Cdp,
            "devtools" => Scenario::DevTools,
            other => bail!("unknown scenario {other}"),
        };
        // A zero window or wait would let a check pass without observing anything.
        let nonzero_ms = |k: &str| -> Result<Duration> {
            let d = ms(k)?;
            if d.is_zero() {
                bail!("--pump-{k} must be greater than 0");
            }
            Ok(d)
        };
        let devtools = scenario == Scenario::DevTools;
        Ok(Self {
            scenario,
            url: get("url")?,
            out: get("out")?.into(),
            cache_dir: get("cache-dir")?.into(),
            no_sandbox: opts.contains_key("no-sandbox"),
            step_timeout: nonzero_ms("step-timeout-ms")?,
            warmup: ms("warmup-ms")?,
            measure: nonzero_ms("measure-ms")?,
            devtools_close_after: if devtools {
                ms("devtools-close-ms")?
            } else {
                Duration::ZERO
            },
            second_browser: opts.contains_key("second-browser"),
            no_inactive_throttle: opts.contains_key("no-inactive-throttle"),
            forbidden_paths: {
                let paths = path_list("forbid-paths", &get("forbid-paths")?)?;
                if paths.is_empty() {
                    bail!("--pump-forbid-paths names no path");
                }
                paths
            },
            allowed_paths: path_list("allow-paths", &get("allow-paths")?)?,
        })
    }
}

/// A colon-separated list of absolute paths; empty entries are dropped. A
/// relative entry could never match the absolute paths the scan reads.
fn path_list(option: &str, value: &str) -> Result<Vec<String>> {
    cef_gpui_pump::procfs::scan_roots(option, value).map_err(anyhow::Error::msg)
}
