use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;

use anyhow::{Context as _, Result, ensure};
use cef::{rc::Rc as _, *};

use crate::pump::{Pump, Schedule};

wrap_app! {
    struct FlechaApp {
        schedule: Schedule,
        ready: Rc<Cell<bool>>,
    }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ProcessHandler::new(self.schedule.clone(), self.ready.clone()))
        }

        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            if let Some(command_line) = command_line {
                command_line.append_switch(Some(&"no-startup-window".into()));
                command_line.append_switch_with_value(
                    Some(&"password-store".into()),
                    Some(&"basic".into()),
                );
                #[cfg(target_os = "linux")]
                command_line.append_switch_with_value(
                    Some(&"ozone-platform".into()),
                    Some(&if std::env::var_os("WAYLAND_DISPLAY").is_some() { "wayland" } else { "x11" }.into()),
                );
            }
        }
    }
}

wrap_browser_process_handler! {
    struct ProcessHandler {
        schedule: Schedule,
        ready: Rc<Cell<bool>>,
    }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            self.ready.set(true);
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.schedule.request(delay_ms);
        }
    }
}

pub struct Runtime {
    app: cef::App,
    ready: Rc<Cell<bool>>,
    pump: Option<Pump>,
    initialized: bool,
    #[cfg(target_os = "macos")]
    _loader: library_loader::LibraryLoader,
}

impl Runtime {
    pub fn entry() -> Result<Self> {
        #[cfg(target_os = "linux")]
        ensure!(
            std::env::var("FLECHA_CEF_RUNTIME_SHA1").as_deref() == Ok(sys::FLECHA_CEF_MINIMAL_SHA1),
            "the launcher runtime stamp differs from the build"
        );
        #[cfg(target_os = "macos")]
        let loader = {
            let exe = std::env::current_exe()?;
            let helper = std::env::args().any(|arg| arg.starts_with("--type="));
            let loader = library_loader::LibraryLoader::new(&exe, helper);
            ensure!(loader.load(), "could not load CEF from the app bundle");
            loader
        };
        ensure!(
            !api_hash(sys::CEF_API_VERSION_LAST, 0).is_null()
                && api_version() == sys::CEF_API_VERSION_LAST,
            "libcef does not expose the compiled CEF API version"
        );
        let (schedule, pump) = Schedule::new();
        let ready = Rc::new(Cell::new(false));
        let mut app = FlechaApp::new(schedule, ready.clone());
        let args = args::Args::new();
        let code = execute_process(Some(args.as_main_args()), Some(&mut app), ptr::null_mut());
        if code >= 0 {
            std::process::exit(code);
        }
        Ok(Self {
            app,
            ready,
            pump: Some(pump),
            initialized: false,
            #[cfg(target_os = "macos")]
            _loader: loader,
        })
    }

    pub fn initialize(&mut self, profile: &Path) -> Result<()> {
        let mut settings = Settings {
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            command_line_args_disabled: 1,
            root_cache_path: path_string(profile)?,
            cache_path: path_string(&profile.join(crate::Partition::Shared.directory()))?,
            persist_session_cookies: 1,
            log_severity: LogSeverity::DISABLE,
            ..Default::default()
        };
        if let Some(runtime) = std::env::var_os("FLECHA_CEF_RUNTIME_DIR").map(PathBuf::from) {
            settings.resources_dir_path = path_string(&runtime)?;
            settings.locales_dir_path = path_string(&runtime.join("locales"))?;
        }
        let args = args::Args::new();
        self.initialized = initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut self.app),
            ptr::null_mut(),
        ) == 1;
        ensure!(self.initialized, "CEF initialization failed");
        Ok(())
    }

    pub fn drive(
        &mut self,
        cx: &mut gpui::App,
        mut tick: impl FnMut(bool, &mut gpui::App) -> bool + 'static,
    ) {
        let pump = self.pump.take().expect("one CEF pump per process");
        let ready = self.ready.clone();
        cx.spawn(async move |cx| pump.run(cx, move |cx| tick(ready.get(), cx)).await)
            .detach();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if self.initialized {
            cef::shutdown();
        }
    }
}

pub(crate) fn path_string(path: &Path) -> Result<CefString> {
    Ok(path
        .to_str()
        .context("CEF profile or resource path must be UTF-8")?
        .into())
}
