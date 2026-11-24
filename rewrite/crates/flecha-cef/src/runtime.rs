use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;

use anyhow::{Context as _, Result, ensure};
use cef::{rc::Rc as _, *};

use crate::pump::{Pump, Schedule};

/// The switch that carries a launch's `--open` address. CEF forwards a second launch's command
/// line to the Flecha already running without its own arguments (`command_line_args_disabled`),
/// but with the switches added here.
const OPEN_SWITCH: &str = "flecha-open";

/// Later launches of Flecha that CEF handed to this one, each with the address it asked for.
#[derive(Clone, Default)]
pub struct Relaunches(Rc<RefCell<VecDeque<Option<String>>>>);

impl Relaunches {
    pub fn take(&self) -> Vec<Option<String>> {
        self.0.borrow_mut().drain(..).collect()
    }
}

/// What `Runtime::initialize` made of this launch.
pub enum Start {
    Running,
    /// A Flecha with this profile is running, and CEF handed it this launch. CEF requires the
    /// process to exit at once, without calling CEF again (`cef_app.h` `CefInitialize`).
    Forwarded,
}

wrap_app! {
    struct FlechaApp {
        schedule: Schedule,
        ready: Rc<Cell<bool>>,
        relaunches: Relaunches,
        open: Rc<RefCell<Option<String>>>,
    }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ProcessHandler::new(
                self.schedule.clone(),
                self.ready.clone(),
                self.relaunches.clone(),
            ))
        }

        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            if let Some(command_line) = command_line {
                command_line.append_switch(Some(&"no-startup-window".into()));
                if process_type.is_none()
                    && let Some(open) = self.open.borrow().as_deref()
                {
                    command_line
                        .append_switch_with_value(Some(&OPEN_SWITCH.into()), Some(&open.into()));
                }
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
        relaunches: Relaunches,
    }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            self.ready.set(true);
        }

        fn on_already_running_app_relaunch(
            &self,
            command_line: Option<&mut CommandLine>,
            _current_directory: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            let switch = CefString::from(OPEN_SWITCH);
            let address = command_line
                .filter(|line| line.has_switch(Some(&switch)) == 1)
                .map(|line| CefString::from(&line.switch_value(Some(&switch))).to_string());
            self.relaunches.0.borrow_mut().push_back(address);
            1
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.schedule.request(delay_ms);
        }
    }
}

pub struct Runtime {
    app: cef::App,
    ready: Rc<Cell<bool>>,
    relaunches: Relaunches,
    open: Rc<RefCell<Option<String>>>,
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
        let relaunches = Relaunches::default();
        let open = Rc::new(RefCell::new(None));
        let mut app = FlechaApp::new(schedule, ready.clone(), relaunches.clone(), open.clone());
        let args = args::Args::new();
        let code = execute_process(Some(args.as_main_args()), Some(&mut app), ptr::null_mut());
        if code >= 0 {
            std::process::exit(code);
        }
        Ok(Self {
            app,
            ready,
            relaunches,
            open,
            pump: Some(pump),
            initialized: false,
            #[cfg(target_os = "macos")]
            _loader: loader,
        })
    }

    /// Starts CEF on `profile`. `open` is the address this launch asks for, which reaches the
    /// running Flecha when CEF forwards the launch.
    pub fn initialize(&mut self, profile: &Path, open: Option<&str>) -> Result<Start> {
        *self.open.borrow_mut() = open.map(str::to_owned);
        let mut settings = Settings {
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            command_line_args_disabled: 1,
            root_cache_path: path_string(profile)?,
            cache_path: path_string(
                &profile.join(
                    crate::Partition::Shared
                        .directory()
                        .expect("the shared partition keeps its data on disk"),
                ),
            )?,
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
        if !self.initialized
            && get_exit_code()
                == sys::cef_resultcode_t::CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED as i32
        {
            return Ok(Start::Forwarded);
        }
        ensure!(self.initialized, "CEF initialization failed");
        Ok(Start::Running)
    }

    /// Later launches of Flecha that CEF hands to this one.
    pub fn relaunches(&self) -> Relaunches {
        self.relaunches.clone()
    }

    /// Whether CEF's global context is initialized, so browsers can be created.
    pub fn is_ready(&self) -> bool {
        self.ready.get()
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
