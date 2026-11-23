use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Alloy-style windowless (off-screen) browser painted into a winit window.
    Osr,
    /// Windowed Chrome-style browser in CEF Views, the control.
    Chrome,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Osr => "osr",
            Mode::Chrome => "chrome",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    pub profile: PathBuf,
    pub url: String,
    pub self_check: Option<SelfCheckOptions>,
    /// Arguments after `--`, passed to CEF unchanged.
    pub cef_args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SelfCheckOptions {
    /// How long each self-check step may take before it is recorded as timed out.
    pub step_timeout_ms: i64,
    pub report: Option<PathBuf>,
}

/// cefsimple's start page (cef-rs examples/cefsimple/src/shared/simple_app.rs).
const DEFAULT_URL: &str = "https://www.google.com/";

pub const USAGE: &str = "\
usage: cef-osr-signin --mode <osr|chrome> --profile <absolute dir> [--url <url>]
                      [--self-check --self-check-timeout-secs <secs> [--report <file>]]
                      [-- <extra CEF switches>]";

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut mode = None;
    let mut profile = None;
    let mut url = None;
    let mut self_check = false;
    let mut timeout_secs = None;
    let mut report = None;
    let mut cef_args = Vec::new();

    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        if arg == "--" {
            cef_args.extend(it.by_ref().cloned());
            break;
        }
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match key {
            "--mode" => {
                mode = Some(match value("--mode")?.as_str() {
                    "osr" => Mode::Osr,
                    "chrome" => Mode::Chrome,
                    other => return Err(format!("unknown mode {other:?}")),
                })
            }
            "--profile" => profile = Some(PathBuf::from(value("--profile")?)),
            "--url" => url = Some(value("--url")?),
            "--self-check" => self_check = true,
            "--self-check-timeout-secs" => {
                let v = value("--self-check-timeout-secs")?;
                let secs: i64 = v
                    .parse()
                    .map_err(|_| format!("--self-check-timeout-secs: not a number: {v}"))?;
                if secs <= 0 {
                    return Err("--self-check-timeout-secs must be positive".into());
                }
                timeout_secs = Some(secs);
            }
            "--report" => report = Some(PathBuf::from(value("--report")?)),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let mode = mode.ok_or("--mode is required")?;
    let profile = profile.ok_or("--profile is required")?;
    if !profile.is_absolute() {
        return Err("--profile must be an absolute path".into());
    }
    let self_check = if self_check {
        let secs = timeout_secs.ok_or("--self-check needs --self-check-timeout-secs")?;
        Some(SelfCheckOptions {
            step_timeout_ms: secs * 1000,
            report,
        })
    } else {
        if timeout_secs.is_some() || report.is_some() {
            return Err("--self-check-timeout-secs and --report need --self-check".into());
        }
        None
    };
    Ok(Options {
        mode,
        profile,
        url: url.unwrap_or_else(|| DEFAULT_URL.to_string()),
        self_check,
        cef_args,
    })
}
