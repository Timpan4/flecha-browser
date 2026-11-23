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
    /// `--fake-camera`: Chromium's fake capture device, for the camera prompt check on a host
    /// without a camera. Only local pages may load while it is on; see `common::is_local_url`.
    pub fake_camera: bool,
    /// `--probe`: test-only page probes for the automated checklist; see `probe`.
    pub probe: bool,
}

#[derive(Clone, Debug)]
pub struct SelfCheckOptions {
    /// How long each self-check step may take before it is recorded as timed out.
    pub step_timeout_ms: i64,
    pub report: Option<PathBuf>,
}

/// Switches that make the browser differ from a shipping build: remote control, automation,
/// sandbox off, a spoofed user agent, weakened web security, test mode, fake media devices.
/// The self-check scans every process for them, and they are refused after `--`. `--type` is
/// refused there too, because it would turn the browser process into a subprocess. The fake
/// capture device is reachable only through `--fake-camera`, which keeps every page local.
/// Names are listed without a prefix; see `switch`.
pub const FORBIDDEN_SWITCHES: &[&str] = &[
    "remote-debugging-port",
    "remote-debugging-pipe",
    "remote-debugging-address",
    "enable-automation",
    "headless",
    "no-sandbox",
    "disable-setuid-sandbox",
    "disable-seccomp-filter-sandbox",
    "disable-namespace-sandbox",
    "disable-gpu-sandbox",
    "no-zygote-sandbox",
    "user-agent",
    "disable-web-security",
    "ignore-certificate-errors",
    "use-mock-keychain",
    "test-type",
    FAKE_DEVICE_SWITCH,
    "use-fake-ui-for-media-stream",
    "use-file-for-fake-video-capture",
    "use-file-for-fake-audio-capture",
];

/// Chromium's fake capture device (media/base/media_switches.cc), added only by `--fake-camera`.
pub const FAKE_DEVICE_SWITCH: &str = "use-fake-device-for-media-stream";

/// A command-line switch as Chromium reads it on Linux (base/command_line.cc,
/// `kSwitchPrefixes = {"--", "-"}`): the prefix is `--` or `-`, the name runs to the first `=`,
/// and the rest is the value. Returns `None` for an argument that is not a switch.
pub fn switch(arg: &str) -> Option<(&str, Option<&str>)> {
    let body = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-'))?;
    if body.is_empty() {
        return None;
    }
    Some(match body.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (body, None),
    })
}

pub fn is_forbidden(arg: &str) -> bool {
    switch(arg).is_some_and(|(name, _)| FORBIDDEN_SWITCHES.contains(&name))
}

pub fn is_type_switch(arg: &str) -> bool {
    switch(arg).is_some_and(|(name, _)| name == "type")
}

/// cefsimple's start page (cef-rs examples/cefsimple/src/shared/simple_app.rs).
const DEFAULT_URL: &str = "https://www.google.com/";

pub const USAGE: &str = "\
usage: cef-osr-signin --mode <osr|chrome> --profile <absolute dir> [--url <url>] [--log-console]
                      [--self-check --self-check-timeout-secs <secs> [--report <file>]]
                      [--fake-camera] [--probe] [-- <extra CEF switches>]
  --fake-camera  Chromium's fake camera; only file:// and loopback pages may load
  --probe        test-only page probes for scripts/auto-checks.sh";

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut mode = None;
    let mut profile = None;
    let mut url = None;
    let mut self_check = false;
    let mut log_console = false;
    let mut timeout_secs = None;
    let mut report = None;
    let mut cef_args = Vec::new();
    let mut fake_camera = false;
    let mut probe = false;

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
            "--fake-camera" => fake_camera = flag("--fake-camera")?,
            "--probe" => probe = flag("--probe")?,
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
        .find(|a| is_forbidden(a) || is_type_switch(a))
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
    let url = url.unwrap_or_else(|| DEFAULT_URL.to_string());
    if fake_camera {
        // The fake device must never meet a sign-in page: every page stays local, and the
        // modes that load remote pages by design are refused with it.
        if self_check.is_some() || probe {
            return Err("--fake-camera cannot be combined with --self-check or --probe".into());
        }
        if !crate::common::is_local_url(&url) {
            return Err(format!(
                "--fake-camera needs a local --url (file:// or loopback), not {}",
                crate::common::display_url(&url)
            ));
        }
    }
    if probe && self_check.is_some() {
        return Err("--probe cannot be combined with --self-check".into());
    }
    Ok(Options {
        mode,
        profile,
        url,
        self_check,
        log_console,
        cef_args,
        fake_camera,
        probe,
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
            "-no-sandbox",
            "--remote-debugging-port=9222",
            "-remote-debugging-port=0",
            "--type=renderer",
            "-type=renderer",
            "--disable-gpu-sandbox",
            "-no-zygote-sandbox",
            "--use-fake-device-for-media-stream",
            "-use-fake-ui-for-media-stream",
            "--use-file-for-fake-video-capture=/tmp/x.y4m",
        ] {
            assert!(parse(&args(&["--", bad])).is_err(), "{bad}");
        }
        for ok in [
            "--password-store=basic",
            "-password-store=basic",
            "--ozone-platform=wayland",
            "positional",
        ] {
            assert!(parse(&args(&["--", ok])).is_ok(), "{ok}");
        }
    }

    #[test]
    fn switches_follow_chromium_prefix_rules() {
        assert_eq!(switch("--a=b=c"), Some(("a", Some("b=c"))));
        assert_eq!(switch("-a"), Some(("a", None)));
        assert_eq!(switch("---a"), Some(("-a", None)));
        assert_eq!(switch("-"), None);
        assert_eq!(switch("--"), None);
        assert_eq!(switch("a"), None);
        assert!(!is_forbidden("---no-sandbox"));
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
    fn fake_camera_refuses_google_and_every_other_remote_start() {
        for url in [
            "https://accounts.google.com/",
            "https://mail.google.com/mail/u/0/",
            "https://myaccount.google.com/",
            "https://accounts.youtube.com/",
            "https://example.com/",
            "http://127.0.0.1@accounts.google.com/",
            "http://accounts.google.com\\@127.0.0.1/",
            "http://127.0.0.1.nip.io/",
            "chrome-extension://abc/",
        ] {
            assert!(
                parse(&args(&["--fake-camera", "--url", url])).is_err(),
                "{url}"
            );
        }
        // Without --url the start page is google.com.
        assert!(parse(&args(&["--fake-camera"])).is_err());
        for url in [
            "file:///tmp/camera.html",
            "http://127.0.0.1:8080/camera",
            "http://localhost/camera",
            "http://[::1]:9/camera",
        ] {
            let o = parse(&args(&["--fake-camera", "--url", url])).unwrap();
            assert!(o.fake_camera, "{url}");
        }
        let local = ["--fake-camera", "--url", "file:///tmp/c.html"];
        assert!(parse(&args(&[&local[..], &["--probe"]].concat())).is_err());
        assert!(
            parse(&args(
                &[
                    &local[..],
                    &["--self-check", "--self-check-timeout-secs", "1"]
                ]
                .concat()
            ))
            .is_err()
        );
        assert!(parse(&args(&[&local[..], &["--fake-camera=1"]].concat())).is_err());
    }

    #[test]
    fn timeout_overflow_is_an_error() {
        let huge = i64::MAX.to_string();
        assert!(parse(&args(&["--self-check", "--self-check-timeout-secs", &huge])).is_err());
    }
}
