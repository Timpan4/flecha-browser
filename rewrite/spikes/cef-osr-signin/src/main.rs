//! Spike 0 (docs/cef-gpui-rewrite-plan.md): one binary, two modes.
//!
//! - `--mode osr`: Alloy-style off-screen browser, software `OnPaint` into a winit window.
//! - `--mode chrome`: the control, a Chrome-style browser in a native Chrome window with
//!   Chrome's own tab strip and toolbar.
//!
//! Both run with shipping settings: sandbox on, no remote debugging, no automation switches,
//! CEF's default user agent, and a persistent profile at `--profile`. The only settings that
//! differ are the windowing ones (`windowless_rendering_enabled`, `external_message_pump`).

mod chrome;
mod cli;
mod common;
mod keys;
mod osr;
mod probe;
mod runtime;
mod selfcheck;

use cef::*;
use cli::Mode;
use std::{
    ffi::{CString, c_char},
    process::ExitCode,
};

/// argv in the form CEF's `MainArgs` borrows.
struct Argv {
    _strings: Vec<CString>,
    _pointers: Vec<*mut c_char>,
    main_args: MainArgs,
}

impl Argv {
    fn new(args: &[String]) -> Self {
        let strings: Vec<CString> = args
            .iter()
            .map(|a| CString::new(a.as_str()).expect("argument without NUL"))
            .collect();
        let mut pointers: Vec<*mut c_char> =
            strings.iter().map(|s| s.as_ptr() as *mut c_char).collect();
        let main_args = MainArgs {
            argc: pointers.len() as i32,
            argv: pointers.as_mut_ptr(),
        };
        Argv {
            _strings: strings,
            _pointers: pointers,
            main_args,
        }
    }
}

wrap_app! {
    struct SpikeApp {
        url: String,
    }

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(SpikeBrowserProcessHandler::new(self.url.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct SpikeBrowserProcessHandler {
        url: String,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            match common::mode() {
                Mode::Chrome => chrome::create_browser(&self.url),
                Mode::Osr => osr::context_ready(),
            }
        }

        // Tabs and windows the user opens through Chrome's UI (new tab, a tab an extension
        // opens) get this client. Without it they are unmanaged: no callbacks, and closing the
        // first browser would quit while they are still open.
        fn default_client(&self) -> Option<Client> {
            (common::mode() == Mode::Chrome).then(chrome::ChromeClient::new)
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            if common::mode() == Mode::Osr {
                osr::schedule_pump(delay_ms);
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if let Err(e) = runtime::configure_api() {
        eprintln!("wrong CEF runtime: {e}");
        return ExitCode::FAILURE;
    }

    // CEF relaunches this binary for its renderer, GPU, utility and zygote processes, with
    // `--type=`. The browser process always has `--mode`, which CEF never passes on (it sees
    // only argv[0] and what follows `--`, and `--type` is refused there), so a `--type=` that
    // is the value of one of this program's options does not count.
    let is_subprocess = !args
        .iter()
        .any(|a| a == "--mode" || a.starts_with("--mode="))
        && args.iter().any(|a| cli::is_type_switch(a));
    if is_subprocess {
        let argv = Argv::new(&args);
        let code = execute_process(
            Some(&argv.main_args),
            None::<&mut App>,
            std::ptr::null_mut(),
        );
        if code < 0 {
            eprintln!("execute_process returned {code} in a subprocess");
            std::process::exit(1);
        }
        std::process::exit(code);
    }

    let options = match cli::parse(&args) {
        Ok(options) => options,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("{message}");
            }
            eprintln!("{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    common::MODE.set(options.mode).expect("mode set once");
    common::LOG_CONSOLE
        .set(options.log_console)
        .expect("log_console set once");
    common::FAKE_CAMERA
        .set(options.fake_camera)
        .expect("fake_camera set once");
    if options.probe {
        probe::enable();
    }

    let runtime = match runtime::check() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("wrong CEF runtime: {e}");
            eprintln!(
                "Stage the pinned runtime with spikes/cef-osr-signin/prepare.sh and start through run.sh."
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = std::fs::create_dir_all(&options.profile) {
        eprintln!("cannot create profile {}: {e}", options.profile.display());
        return ExitCode::FAILURE;
    }
    let start_url = match &options.self_check {
        Some(sc) => match selfcheck::init(sc, &options.profile, &runtime) {
            Ok(url) => url,
            Err(e) => {
                eprintln!("cannot prepare self-check: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => options.url.clone(),
    };

    // CEF sees only argv[0] and what follows `--`, never this program's own flags.
    let mut cef_args = vec![args[0].clone()];
    cef_args.extend(options.cef_args.iter().cloned());
    if options.fake_camera {
        // The only way to the fake device: `--` refuses this switch (cli::FORBIDDEN_SWITCHES).
        cef_args.push(format!("--{}", cli::FAKE_DEVICE_SWITCH));
    }
    let argv = Argv::new(&cef_args);
    if execute_process(
        Some(&argv.main_args),
        None::<&mut App>,
        std::ptr::null_mut(),
    ) != -1
    {
        eprintln!("execute_process did not return -1 in the browser process");
        return ExitCode::FAILURE;
    }

    let osr = options.mode == Mode::Osr;
    // The event loop exists before CEF starts so the pump callback can wake it.
    let event_loop = osr.then(osr::create_event_loop);

    let profile = CefString::from(options.profile.to_string_lossy().as_ref());
    let settings = Settings {
        no_sandbox: 0,
        windowless_rendering_enabled: osr as i32,
        external_message_pump: osr as i32,
        root_cache_path: profile.clone(),
        cache_path: profile,
        background_color: common::BACKGROUND,
        // The paks, ICU data and locales are next to the loaded libcef.so, not the binary.
        resources_dir_path: CefString::from(runtime.dir.to_string_lossy().as_ref()),
        locales_dir_path: CefString::from(runtime.dir.join("locales").to_string_lossy().as_ref()),
        // Chromium logs page console messages that no handler consumed at INFO. Keep INFO and
        // below out of chrome_debug.log unless the owner asked for console output.
        log_severity: if options.log_console {
            LogSeverity::DEFAULT
        } else {
            LogSeverity::WARNING
        },
        ..Default::default()
    };
    let mut app = SpikeApp::new(start_url.clone());
    if initialize(
        Some(&argv.main_args),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        eprintln!("cef initialize failed");
        return ExitCode::FAILURE;
    }
    if options.self_check.is_none()
        && let Err(why) = common::check_typed_url(&start_url)
    {
        eprintln!("--url refused: {why}");
        shutdown();
        return ExitCode::from(2);
    }

    match event_loop {
        Some(event_loop) => osr::run(
            event_loop,
            start_url,
            options.profile.clone(),
            options.self_check.is_none(),
        ),
        None => run_message_loop(),
    }
    shutdown();
    if common::startup_failed() || selfcheck::failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
