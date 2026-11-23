use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Alloy-style windowless (off-screen) browser painted into a winit window.
    Osr,
    /// Chrome-style browser in a native Chrome window, the control.
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
    /// Relay console messages from every page, not only from local `file://` pages.
    pub log_console: bool,
    /// Arguments after `--`, passed to CEF unchanged.
    pub cef_args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SelfCheckOptions {
    /// How long each self-check step may take before it is recorded as timed out.
    pub step_timeout_ms: i64,
    pub report: Option<PathBuf>,
}

/// Switches that make the browser differ from a shipping build: remote control, automation,
/// sandbox off, a spoofed user agent, weakened web security, test mode. The self-check scans
/// every process for them, and they are refused after `--`. `--type` is refused there too,
/// because it would turn the browser process into a subprocess.
pub const FORBIDDEN_SWITCHES: &[&str] = &[
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

pub fn is_forbidden(arg: &str) -> bool {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    FORBIDDEN_SWITCHES.contains(&name)
}

/// cefsimple's start page (cef-rs examples/cefsimple/src/shared/simple_app.rs).
const DEFAULT_URL: &str = "https://www.google.com/";

pub const USAGE: &str = "\
usage: cef-osr-signin --mode <osr|chrome> --profile <absolute dir> [--url <url>] [--log-console]
                      [--self-check --self-check-timeout-secs <secs> [--report <file>]]
                      [-- <extra CEF switches>]";

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut mode = None;
    let mut profile = None;
    let mut url = None;
    let mut self_check = false;
    let mut log_console = false;
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
        let flag = |name: &str| -> Result<bool, String> {
            match inline {
                None => Ok(true),
                Some(_) => Err(format!("{name} takes no value")),
            }
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
            "--self-check" => self_check = flag("--self-check")?,
            "--log-console" => log_console = flag("--log-console")?,
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

    if let Some(bad) = cef_args
        .iter()
        .find(|a| is_forbidden(a) || a.starts_with("--type="))
    {
        return Err(format!("refusing CEF switch {bad:?} after --"));
    }
    let mode = mode.ok_or("--mode is required")?;
    let profile = profile.ok_or("--profile is required")?;
    if !profile.is_absolute() {
        return Err("--profile must be an absolute path".into());
    }
    let self_check = if self_check {
        let secs = timeout_secs.ok_or("--self-check needs --self-check-timeout-secs")?;
        Some(SelfCheckOptions {
            step_timeout_ms: secs
                .checked_mul(1000)
                .ok_or("--self-check-timeout-secs is too large")?,
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
        log_console,
        cef_args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(rest: &[&str]) -> Vec<String> {
        ["cef-osr-signin", "--mode", "osr", "--profile", "/tmp/p"]
            .iter()
            .chain(rest)
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn refuses_forbidden_and_type_switches_after_separator() {
        for bad in [
            "--no-sandbox",
            "--remote-debugging-port=9222",
            "--type=renderer",
        ] {
            assert!(parse(&args(&["--", bad])).is_err(), "{bad}");
        }
        for ok in ["--password-store=basic", "--ozone-platform=wayland"] {
            assert!(parse(&args(&["--", ok])).is_ok(), "{ok}");
        }
    }

    #[test]
    fn presence_flags_take_no_value() {
        assert!(parse(&args(&["--log-console=false"])).is_err());
        assert!(
            parse(&args(&[
                "--self-check=false",
                "--self-check-timeout-secs",
                "1"
            ]))
            .is_err()
        );
        assert!(parse(&args(&["--log-console"])).unwrap().log_console);
    }

    #[test]
    fn timeout_overflow_is_an_error() {
        let huge = i64::MAX.to_string();
        assert!(parse(&args(&["--self-check", "--self-check-timeout-secs", &huge])).is_err());
    }
}
