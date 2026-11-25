use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;

use anyhow::{Context as _, Result, ensure};
use cef::{rc::Rc as _, *};

use crate::compositor::Compositing;
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
        unpacked: Rc<RefCell<Option<String>>>,
        // GPUI's GPU, where Chromium must allocate the textures it shares.
        render_node: Rc<RefCell<Option<String>>>,
        // GPUI's display connection, once `initialize` knows it.
        ozone: Rc<Cell<Option<&'static str>>>,
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
                // Route HTTP challenges to GetAuthCredentials in the windowless shell.
                command_line.append_switch(Some(&"disable-chrome-login-prompt".into()));
                if process_type.is_none() && let Some(folders) = self.unpacked.borrow().as_deref() {
                    command_line.append_switch_with_value(Some(&"load-extension".into()), Some(&folders.into()));
                }
                if process_type.is_none()
                    && let Some(open) = self.open.borrow().as_deref()
                {
                    command_line
                        .append_switch_with_value(Some(&OPEN_SWITCH.into()), Some(&open.into()));
                }
                if let Some(store) = password_store(std::env::var_os("FLECHA_LAUNCHER").as_deref()) {
                    command_line.append_switch_with_value(
                        Some(&"password-store".into()),
                        Some(&store.into()),
                    );
                }
                // Child processes inherit the browser process's platform.
                #[cfg(target_os = "linux")]
                if command_line.has_switch(Some(&"ozone-platform".into())) == 0 {
                    let ozone = self
                        .ozone
                        .get()
                        .unwrap_or_else(|| Compositing::software().ozone());
                    command_line.append_switch_with_value(
                        Some(&"ozone-platform".into()),
                        Some(&ozone.into()),
                    );
                }
                if let Some(node) = self.render_node.borrow().as_deref() {
                    command_line.append_switch_with_value(
                        Some(&"render-node-override".into()),
                        Some(&node.into()),
                    );
                }
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
    unpacked: Rc<RefCell<Option<String>>>,
    chrome_policy: Option<PathBuf>,
    render_node: Rc<RefCell<Option<String>>>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    ozone: Rc<Cell<Option<&'static str>>>,
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
        let unpacked = Rc::new(RefCell::new(None));
        let render_node = Rc::new(RefCell::new(None));
        let ozone = Rc::new(Cell::new(None));
        let mut app = FlechaApp::new(
            schedule,
            ready.clone(),
            relaunches.clone(),
            open.clone(),
            unpacked.clone(),
            render_node.clone(),
            ozone.clone(),
        );
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
            unpacked,
            chrome_policy: None,
            render_node,
            ozone,
            pump: Some(pump),
            initialized: false,
            #[cfg(target_os = "macos")]
            _loader: loader,
        })
    }

    /// Reads the unpacked folders and prepares the managed policy directory. Each part that
    /// fails is skipped and returned, so a broken extension setting never stops the browser.
    pub fn configure_extensions(&mut self, profile: &Path) -> Vec<anyhow::Error> {
        let settings = crate::extensions::ExtensionSettings::new(profile);
        let mut problems = Vec::new();
        match settings.startup_folders() {
            Ok(folders) => {
                let folders = folders
                    .iter()
                    .filter_map(|path| path.to_str())
                    .collect::<Vec<_>>();
                *self.unpacked.borrow_mut() = (!folders.is_empty()).then(|| folders.join(","));
            }
            Err(error) => problems.push(error.context("Skipped unpacked extensions")),
        }
        if crate::extensions::MANAGED_POLICY_SUPPORTED {
            let policy = settings.policy_directory();
            match std::fs::create_dir_all(policy.join("managed")) {
                Ok(()) => self.chrome_policy = Some(policy),
                Err(error) => problems.push(anyhow::Error::new(error).context(format!(
                    "Skipped managed extensions: create {}",
                    policy.display()
                ))),
            }
        }
        problems
    }

    /// Starts CEF on `profile`. `open` is the address this launch asks for, which reaches the
    /// running Flecha when CEF forwards the launch. Chromium starts its GPU process here and
    /// reads `compositing`'s render node from its command line, so the GPU it shares
    /// textures on is fixed for the process.
    pub fn initialize(
        &mut self,
        profile: &Path,
        open: Option<&str>,
        compositing: &Compositing,
    ) -> Result<Start> {
        *self.open.borrow_mut() = open.map(str::to_owned);
        *self.render_node.borrow_mut() = compositing
            .render_node()
            .map(|node| node.to_str().context("render node path must be UTF-8"))
            .transpose()?
            .map(str::to_owned);
        #[cfg(target_os = "linux")]
        self.ozone.set(Some(compositing.ozone()));
        let mut settings = Settings {
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            command_line_args_disabled: 1,
            root_cache_path: path_string(profile)?,
            cache_path: path_string(&shared_cache_path(profile)?)?,
            persist_session_cookies: 1,
            log_severity: LogSeverity::DISABLE,
            ..Default::default()
        };
        if let Some(policy) = &self.chrome_policy {
            settings.chrome_policy_id = path_string(policy)?;
        }
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

fn password_store(launcher: Option<&OsStr>) -> Option<&'static str> {
    (launcher == Some(OsStr::new("isolated"))).then_some("basic")
}

fn shared_cache_path(profile: &Path) -> Result<PathBuf> {
    // Existing rewrite installs already have their shared context under this name.
    let legacy = profile.join("shared");
    if legacy
        .try_exists()
        .context("check existing shared CEF profile")?
    {
        return Ok(legacy);
    }
    Ok(profile.join(
        crate::Partition::Shared
            .directory()
            .expect("the shared partition keeps its data on disk"),
    ))
}

/// Whether a process on this host holds `profile`, so CEF would forward a launch on it.
/// Chromium's process singleton links `SingletonLock` in the profile to `<hostname>-<pid>`
/// while it runs. A stale link to a live, unrelated process reads as held; that launch then
/// starts with CPU copies.
#[cfg(target_os = "linux")]
pub fn profile_in_use(profile: &Path) -> bool {
    let Ok(target) = std::fs::read_link(profile.join("SingletonLock")) else {
        return false;
    };
    let Some((host, pid)) = target.to_str().and_then(|target| target.rsplit_once('-')) else {
        return false;
    };
    let Ok(hostname) = std::fs::read_to_string("/proc/sys/kernel/hostname") else {
        return false;
    };
    host == hostname.trim()
        && pid
            .parse::<u32>()
            .is_ok_and(|pid| Path::new("/proc").join(pid.to_string()).exists())
}

#[cfg(not(target_os = "linux"))]
pub fn profile_in_use(_profile: &Path) -> bool {
    false
}

pub(crate) fn path_string(path: &Path) -> Result<CefString> {
    Ok(path
        .to_str()
        .context("CEF profile or resource path must be UTF-8")?
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_password_store_is_only_for_the_isolated_launcher() {
        assert_eq!(password_store(Some(OsStr::new("isolated"))), Some("basic"));
        assert_eq!(password_store(Some(OsStr::new("desktop"))), None);
        assert_eq!(password_store(None), None);
    }

    #[test]
    fn uses_default_for_new_profiles_and_preserves_existing_shared_profiles() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let default = profile.path().join("Default");
        assert_eq!(shared_cache_path(profile.path())?, default);
        std::fs::create_dir(default)?;
        let legacy = profile.path().join("shared");
        std::fs::create_dir(&legacy)?;
        assert_eq!(shared_cache_path(profile.path())?, legacy);
        Ok(())
    }
}
