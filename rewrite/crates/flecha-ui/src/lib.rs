mod about;
mod boosts;
mod clipboard;
mod command_bar;
mod context_menu;
mod dialogs;
mod downloads_view;
mod empty_page;
mod favicons;
mod find_bar;
mod flechinha;
mod fullscreen;
mod history_view;
mod icons;
mod incognito;
mod mcp;
mod model;
mod page_input;
mod permissions_view;
mod popup;
#[cfg(target_os = "linux")]
mod printing;
mod responsive;
mod settings_view;
mod shell_menu;
mod shortcuts;
mod sidebar;
mod site_controls;
mod space_editor;
mod split_view;
mod tab_drag;
mod tab_menu;
mod tab_sleep;
mod text_field;
mod theme;
mod tokens;
mod verify;
mod view;
mod window_chrome;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Context as _, Result, ensure};
use flecha_adblock::{
    DOWNLOAD_TIMEOUT, Download, EngineOutcome, UpdateReport, Updater, UpdaterConfig, UreqDownload,
    load_cached_rules,
};
use flecha_boosts::Boosts;
use flecha_cef::{Engine, Event, IncognitoSession, Owner, Partition, Runtime, Viewport};
use flecha_core::{
    ArchivedTab, Reopened, SpaceId, TabId, WindowId,
    storage::{LoadedState, ReadyState},
    suggestions::pinnable_address,
};
use flecha_services::StateStore;
use flecha_services::local_servers::LocalServer;
use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowHandle, WindowOptions, px, size};

use boosts::ProfileBoosts;
use downloads_view::{DownloadCommand, Downloads};
use history_view::{ClearRange, History};
use model::{Model, now};
use permissions_view::PermissionStore;
use shortcuts::{Navigation, Toolbar};
use sidebar::SidebarAction;
use site_controls::{Site, SiteChange};
use tab_sleep::TabSleep;
use view::Shell;

pub struct Options {
    pub initial: Option<String>,
    pub verify: bool,
    pub adblock_fixture_port: Option<NonZeroU16>,
}

pub use icons::Assets;

pub fn validate_initial_address(input: &str) -> Result<String> {
    pinnable_address(input).context("--open requires a browser address")
}

/// The window Flecha opens before CEF starts. GPUI picks its GPU when it opens a window,
/// and its windows share that GPU, so this window decides where page frames are shared.
/// A launch that CEF forwards to a running Flecha closes it by exiting. Otherwise the first
/// main window takes it over.
pub struct FirstWindow {
    handle: gpui::AnyWindowHandle,
    compositing: flecha_cef::Compositing,
}

impl FirstWindow {
    pub fn open(cx: &mut App) -> Result<Self> {
        // The first main window takes it over, so it is decorated as one.
        let options = window_chrome::client_decorated(WindowOptions::default());
        let handle = cx.open_window(options, |window, cx| {
            window.set_window_title("Flecha");
            cx.new(|_| view::Starting)
        })?;
        let compositing = handle.update(cx, |_, window, _| {
            flecha_cef::Compositing::for_window(window)
        })?;
        Ok(Self {
            handle: handle.into(),
            compositing,
        })
    }

    pub fn compositing(&self) -> &flecha_cef::Compositing {
        &self.compositing
    }
}

#[allow(clippy::too_many_arguments)]
pub fn start(
    cx: &mut App,
    runtime: &mut Runtime,
    first: Option<FirstWindow>,
    cef_profile: PathBuf,
    store: StateStore,
    loaded: LoadedState,
    options: Options,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
) -> Result<()> {
    let controller = Rc::new(RefCell::new(Controller::new(
        cef_profile,
        store,
        loaded,
        options.verify,
        options.adblock_fixture_port,
        failure,
    )?));
    match first {
        Some(first) => {
            controller
                .borrow()
                .engine
                .start_compositing(&first.compositing);
            controller.borrow_mut().first_window = Some(first.handle);
        }
        // The profile looked held, so no window probed the GPU, yet CEF started here.
        None => {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "page frames: CPU copies, another process held the profile when Flecha started"
            );
        }
    }
    sync_windows(&controller, cx)?;
    if options.verify {
        verify::install(&controller, cx);
    }
    if let Some(input) = options.initial {
        let url = validate_initial_address(&input)?;
        let window = controller.borrow().model.core.focused_window().id.clone();
        controller.borrow_mut().act(&window, Action::Open(url))?;
    }
    let relaunches = runtime.relaunches();
    runtime.drive(cx, move |ready, cx| {
        for address in relaunches.take() {
            let mut controller = controller.borrow_mut();
            if let Err(error) = controller.relaunch(address.as_deref()) {
                controller.status.set(None, error.to_string());
            }
            cx.refresh_windows();
        }
        let keep_running = controller.borrow_mut().tick(ready, cx);
        if keep_running && let Err(error) = sync_windows(&controller, cx) {
            controller.borrow_mut().fail(error, cx);
        }
        keep_running
    });
    Ok(())
}

#[derive(Clone)]
enum Action {
    Open(String),
    OpenContext(TabId, String, context_menu::Destination),
    Navigate(String),
    Browser(Navigation),
    Print,
    ViewSource,
    Download(flecha_cef::DownloadHandle, DownloadCommand),
    ClearDownloads,
    RetryDownload(flecha_downloads::DownloadId),
    ResolvePermission(
        flecha_permissions::RequestId,
        flecha_permissions::PromptOutcome,
    ),
    RetryPermissionSave,
    Site(Site, SiteChange),
    Select(TabId),
    Split(TabId),
    SplitNew(String),
    DropTab(tab_drag::TabDrag, tab_drag::TabDrop),
    ExitSplit,
    /// The split divider was dragged to this share for the first pane, or reset to half.
    SetSplitRatio(f64),
    /// The responsive view (`responsive`): a phone-sized mirror of the shown tab beside it, or
    /// the tab alone again.
    ToggleResponsive,
    Peek(String),
    PeekTab(TabId),
    Promote,
    ClosePeek,
    Detach,
    DetachTab(TabId),
    Return,
    Close,
    CloseTab(TabId),
    /// The window's close button: the window closes for good, or the app quits with
    /// its last Flecha window (patches 0059 and 0062).
    CloseWindow,
    /// Chromium's "New window", Ctrl+N, from any window.
    NewWindow,
    /// "New incognito window" (`ui/app.ts`), Ctrl+Shift+N.
    NewIncognitoWindow,
    /// "New Flechinha window" (`ui/app.ts`): a Flechinha showing the address, in this
    /// regular window's Space.
    OpenFlechinha(String),
    Switch(SpaceId),
    NewSpace,
    Isolate,
    /// A tab's menu, shortcut or command-bar action on that tab.
    Tab(TabId, tab_menu::TabAction),
    /// A Space's menu or editor.
    Space(SpaceId, space_editor::SpaceAction),
    /// "Clear Today" (`clearToday`): archives the shown Space's Today tabs.
    ClearToday,
    Sidebar(SidebarAction),
    ResizeSidebar(flecha_core::SidebarWidth),
    Reopen,
    RestoreArchive(ArchivedTab),
    ClearArchive,
    DeleteHistory(String),
    ClearHistory(ClearRange),
    Quit,
}

/// Whether this launch keeps the profile closed to the network's list updates and to agents:
/// `--verify`, or `rewrite/tools/isolated-run.sh` (`FLECHA_LAUNCHER=isolated`).
fn isolated_launch(verify: bool, launcher: Option<&std::ffi::OsStr>) -> bool {
    verify || launcher == Some("isolated".as_ref())
}

enum AdblockLists {
    Update(Box<dyn Download>),
    Cached,
}

fn report_adblock(verify: bool, report: &UpdateReport) {
    for issue in &report.warnings {
        eprintln!("{issue}");
    }
    for issue in &report.failures {
        eprintln!("{issue}");
    }
    if let EngineOutcome::Parsed {
        save_error: Some(error),
        ..
    } = &report.engine
    {
        eprintln!("adblock: could not save the engine: {error}");
    }
    if verify {
        verify::adblock_report(report);
    }
}

struct Controller {
    model: Model,
    history: History,
    boosts: ProfileBoosts,
    engine: Engine,
    _adblock_updater: Option<Updater>,
    downloads: Downloads,
    permissions: PermissionStore,
    sleep: TabSleep,
    favicons: favicons::Favicons,
    fullscreen: fullscreen::PageFullscreen,
    agents: mcp::Agents,
    toolbars: BTreeMap<WindowId, Toolbar>,
    copy_link_flashes: BTreeMap<WindowId, Instant>,
    pending_reloads: BTreeMap<i32, flecha_cef::BrowserKey>,
    store: StateStore,
    session_dir: PathBuf,
    snapshots: LoadedState,
    windows: BTreeMap<WindowId, WindowHandle<Shell>>,
    sidebar_deadlines: BTreeMap<WindowId, Instant>,
    /// Where each window shows its pages, even while it shows none.
    page_areas: BTreeMap<WindowId, Viewport>,
    /// The window opened before CEF started, until a main window takes it over.
    first_window: Option<gpui::AnyWindowHandle>,
    front: Option<WindowId>,
    ready: bool,
    quitting: bool,
    verify: bool,
    /// Set by the verify harness: the shell handles page clipboard edits on every display, so a
    /// display without a Wayland input serial can still check them.
    shell_clipboard: bool,
    /// Page input waiting behind a selection read, per browser.
    held: BTreeMap<flecha_cef::BrowserKey, clipboard::Hold>,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
    status: Status,
    /// The profile directory: the agent token and the history live in it.
    profile: PathBuf,
    /// `--verify` or the isolated launcher: no agent bridge, whatever Settings say.
    isolated: bool,
    /// The dev servers on this machine, and the scan that finds them.
    local_servers: Vec<LocalServer>,
    local_watch: empty_page::LocalWatch,
    /// "Jump back in" on the empty page.
    top_sites: empty_page::TopSites,
}

/// The status line. Regular windows share one; each incognito session has its own, so what goes
/// wrong in a private window never shows in a regular one.
#[derive(Default)]
pub(crate) struct Status(BTreeMap<Option<IncognitoSession>, String>);

impl Status {
    pub(crate) fn text(&self, session: Option<IncognitoSession>) -> Option<&str> {
        self.0
            .get(&session)
            .map(String::as_str)
            .filter(|text| !text.is_empty())
    }

    pub(crate) fn set(&mut self, session: Option<IncognitoSession>, text: impl Into<String>) {
        self.0.insert(session, text.into());
    }

    pub(crate) fn clear(&mut self, session: Option<IncognitoSession>) {
        self.0.remove(&session);
    }
}

impl Controller {
    fn new(
        cef_profile: PathBuf,
        store: StateStore,
        snapshots: LoadedState,
        verify: bool,
        adblock_fixture_port: Option<NonZeroU16>,
        failure: Rc<RefCell<Option<anyhow::Error>>>,
    ) -> Result<Self> {
        let isolated = isolated_launch(verify, std::env::var_os("FLECHA_LAUNCHER").as_deref());
        let lists = if verify {
            adblock_fixture_port.map_or(AdblockLists::Cached, |port| {
                AdblockLists::Update(verify::adblock_download(port))
            })
        } else if isolated {
            AdblockLists::Cached
        } else {
            AdblockLists::Update(Box::new(UreqDownload::default()))
        };
        Self::with_profile(
            cef_profile,
            store,
            snapshots,
            verify,
            isolated,
            Some(lists),
            failure,
        )
    }

    fn with_profile(
        cef_profile: PathBuf,
        store: StateStore,
        mut snapshots: LoadedState,
        verify: bool,
        isolated: bool,
        lists: Option<AdblockLists>,
        failure: Rc<RefCell<Option<anyhow::Error>>>,
    ) -> Result<Self> {
        let profile = cef_profile
            .parent()
            .context("CEF data has no profile directory")?
            .to_owned();
        let ready = snapshots.apply_recovery(|write| store.write(write))?;
        let mut state = Self {
            model: Model::new(ready.state.take()),
            history: History::open(&profile),
            boosts: ProfileBoosts::open(profile.join("boosts")),
            session_dir: profile.join("session"),
            engine: Engine::new(cef_profile, model::tab_id),
            _adblock_updater: None,
            downloads: Downloads::new(&profile, isolated),
            permissions: PermissionStore::open(&profile),
            sleep: TabSleep::new(Instant::now()),
            favicons: favicons::Favicons::default(),
            fullscreen: fullscreen::PageFullscreen::default(),
            agents: mcp::Agents::default(),
            toolbars: BTreeMap::new(),
            copy_link_flashes: BTreeMap::new(),
            pending_reloads: BTreeMap::new(),
            store,
            snapshots,
            windows: BTreeMap::new(),
            sidebar_deadlines: BTreeMap::new(),
            page_areas: BTreeMap::new(),
            first_window: None,
            front: None,
            ready: false,
            quitting: false,
            verify,
            shell_clipboard: false,
            held: BTreeMap::new(),
            failure,
            status: Status::default(),
            profile: profile.clone(),
            isolated,
            local_servers: Vec::new(),
            local_watch: empty_page::LocalWatch::default(),
            top_sites: empty_page::TopSites::default(),
        };
        state.permissions.restore(&mut state.engine);
        state.sync_adblock();
        let cache_dir = profile.join("adblock");
        let blocker = state.engine.adblocker();
        match lists {
            Some(AdblockLists::Update(download)) => {
                let config = UpdaterConfig {
                    cache_dir,
                    // The shell has no runtime retry. Reuse DOWNLOAD_TIMEOUT (30 seconds)
                    // as the wait after an incomplete transport.
                    retry_after_failure: DOWNLOAD_TIMEOUT,
                };
                match Updater::spawn(blocker.clone(), config, download, move |report| {
                    report_adblock(verify, &report);
                }) {
                    Ok(updater) => state._adblock_updater = Some(updater),
                    Err(error) => eprintln!("adblock: could not start the updater: {error}"),
                }
            }
            Some(AdblockLists::Cached) => {
                let blocker = blocker.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("flecha-adblock-cache".into())
                    .spawn(move || {
                        if cache_dir.is_dir() {
                            report_adblock(verify, &load_cached_rules(&blocker, &cache_dir));
                        }
                    })
                {
                    eprintln!("adblock: could not start the cache loader: {error}");
                }
            }
            None => {}
        }
        state.engine.set_boosts(state.boosts().clone());
        state.sync_download_config();
        state.sync_agents();
        Ok(state)
    }

    pub(crate) fn boosts(&self) -> &Boosts {
        self.boosts.enabled()
    }

    pub(crate) fn boosts_revision(&self) -> u64 {
        self.boosts.revision()
    }

    fn snapshots(&mut self) -> &mut ReadyState {
        self.snapshots
            .ready()
            .expect("session recovery ran before opening windows")
    }

    /// The one place the browser state reaches storage. It saves the regular state alone:
    /// incognito windows keep theirs in `Model::incognito`, which never comes here.
    fn save(&mut self) -> Result<()> {
        self.replace_snapshot()?.context("Sync session storage")
    }

    /// [`Self::save`], telling a snapshot that never replaced the saved one (`Err`) from one in
    /// place whose directory entry could not be synced (`Ok(Err)`).
    fn replace_snapshot(&mut self) -> Result<std::io::Result<()>> {
        let write = self
            .snapshots
            .ready()
            .expect("session recovered")
            .save_current(&self.model.core)?;
        self.store.replace_snapshot(&write)
    }

    fn act(&mut self, window: &WindowId, action: Action) -> Result<()> {
        if self.quitting {
            return Ok(());
        }
        if self.model.is_incognito(window)
            && let Some(refusal) = incognito::refusal(&action)
        {
            anyhow::bail!(refusal);
        }
        match action {
            Action::Quit => {
                self.quitting = true;
                self.agents.stop();
                self.engine.close_all();
                return self.save();
            }
            Action::Open(url) => {
                self.model.open(window, url, true)?;
            }
            Action::OpenContext(source, url, target) => {
                if let Some(owner) = self.engine.owner(&source).cloned() {
                    self.open_context_url(owner, &source, &url, target)?;
                }
            }
            Action::Navigate(url) => self.navigate_tab(window, url)?,
            Action::Browser(navigation) => return self.browser_navigation(window, navigation),
            Action::Print => {
                if let Some((_, tab)) = self.model.keyboard_pane(window) {
                    self.engine.print(tab)?;
                }
                return Ok(());
            }
            Action::ViewSource => {
                let Some((_, tab)) = self.model.keyboard_pane(window) else {
                    return Ok(());
                };
                let Some(address) = self.engine.url(tab).filter(|url| {
                    ["https:", "http:", "file:"]
                        .iter()
                        .any(|scheme| url.starts_with(scheme))
                }) else {
                    return Ok(());
                };
                let partition = self
                    .engine
                    .partition(tab)
                    .cloned()
                    .context("The source page has closed")?;
                let destination = self.window_for_partition(window, &partition)?;
                if let Some(peek) = self.model.peeks.remove(&destination) {
                    self.engine.close(&peek.id);
                }
                self.model
                    .open(&destination, format!("view-source:{address}"), true)?;
                if &destination != window {
                    self.front = Some(destination);
                }
            }
            Action::Download(handle, command) => return self.manage_download(handle, command),
            Action::ClearDownloads => return self.clear_completed_downloads(window),
            Action::RetryDownload(id) => return self.retry_download(window, id),
            Action::ResolvePermission(request, outcome) => {
                return self.resolve_permission(request, outcome);
            }
            Action::RetryPermissionSave => return self.save_permissions(),
            Action::Site(site, change) => return self.change_current_site(window, &site, change),
            Action::Select(id) => {
                for id in self.select(window, &id)? {
                    self.engine.close(&id);
                }
            }
            Action::Split(right) => {
                if let Some(left) = self.model.active(window).cloned() {
                    self.split(window, left, right)?;
                }
            }
            Action::SplitNew(url) => {
                let left = self.model.active(window).cloned();
                let right = self.model.open(window, url, left.is_none())?;
                if let Some(left) = left {
                    self.split(window, left, right)?;
                }
            }
            Action::DropTab(drag, target) => self.drop_tab(window, drag, target),
            Action::ExitSplit => {
                if let Some(flecha_core::EndedSplit::Responsive { mirror }) = self
                    .model
                    .state_mut(window)
                    .and_then(|state| state.unsplit(window))
                {
                    self.engine.close(&mirror);
                }
            }
            Action::SetSplitRatio(ratio) => {
                self.model
                    .state_mut(window)
                    .context("missing Flecha window")?
                    .set_split_ratio(window, ratio);
            }
            Action::ToggleResponsive => {
                let state = self
                    .model
                    .state_mut(window)
                    .context("missing Flecha window")?;
                let responsive = state.window(window).is_some_and(|current| {
                    current
                        .spaces
                        .get(&current.active_space_id)
                        .is_some_and(|space| {
                            space.split.is_some()
                                && space.split_mode == Some(flecha_core::SplitMode::Responsive)
                        })
                });
                if responsive {
                    // Ending the split closes the mirror and shows the desktop side.
                    return self.act(window, Action::ExitSplit);
                }
                ensure!(
                    state
                        .open_responsive(window, model::tab_id(), now())
                        .is_some(),
                    "Responsive view needs a page"
                );
            }
            Action::Peek(url) => {
                let source = self.model.active(window).cloned();
                if let Some(old) = self.model.peek(window.clone(), url, source.as_ref())? {
                    self.engine.close(&old);
                }
                return Ok(());
            }
            Action::PeekTab(id) => {
                let Some(tab) = self.model.tab(&id) else {
                    return Ok(());
                };
                if let Some(old) = self
                    .model
                    .peek(window.clone(), tab.url.clone(), Some(&id))?
                {
                    self.engine.close(&old);
                }
                return Ok(());
            }
            Action::Promote => {
                let tab = self.model.promote(window)?;
                self.reconcile_partition(&tab);
                self.engine
                    .transfer(&tab, tab.clone(), Owner::Tab(window.clone()));
            }
            Action::ClosePeek => {
                if let Some(page) = self.model.peeks.remove(window) {
                    self.engine.close(&page.id);
                }
                return Ok(());
            }
            Action::Detach => {
                if let Some(tab) = self.model.active(window).cloned() {
                    return self.act(window, Action::DetachTab(tab));
                }
            }
            Action::DetachTab(tab) => {
                let detached = self.model.detach(window, &tab)?;
                self.history.forget(&tab);
                if self.engine.discarded(&tab).is_some() {
                    self.engine.close(&tab);
                }
                self.engine
                    .transfer(&tab, detached.tab, Owner::Flechinha(detached.window));
                for removed in detached.removed {
                    self.engine.close(&removed);
                }
            }
            Action::Return => {
                let tab = self
                    .model
                    .flechinhas
                    .get(window)
                    .context("no Flechinha page")?
                    .id
                    .clone();
                let (destination, returned) = self.model.return_page(window)?;
                self.engine
                    .transfer(&tab, returned.clone(), Owner::Tab(destination.clone()));
                self.reconcile_partition(&returned);
                self.front = Some(destination);
            }
            Action::NewWindow => self.front = Some(self.model.open_regular_window()?),
            Action::NewIncognitoWindow => {
                self.front = Some(self.model.open_incognito_window());
                return Ok(());
            }
            Action::OpenFlechinha(url) => {
                self.front = Some(self.model.open_flechinha(window, url)?);
                return Ok(());
            }
            Action::CloseWindow => {
                if self.model.flechinhas.contains_key(window)
                    || self.model.popups.contains_key(window)
                {
                    return self.act(window, Action::Close);
                }
                if self.model.is_incognito(window) {
                    self.close_incognito_window(window);
                    if self.model.regular_closed() && self.model.incognito.is_none() {
                        return self.act(window, Action::Quit);
                    }
                    return Ok(());
                }
                if self.model.core.windows().len() <= 1 {
                    if self.model.incognito.is_some() {
                        return self.close_last_regular_window();
                    }
                    return self.act(window, Action::Quit);
                }
                if let Some(page) = self.model.peeks.remove(window) {
                    self.engine.close(&page.id);
                }
                let shown = self
                    .engine
                    .ids()
                    .filter(|id| self.engine.owner(id) == Some(&Owner::Tab(window.clone())))
                    .cloned()
                    .collect::<Vec<_>>();
                let change = self.model.core.close_window(window, now());
                for id in shown.iter().chain(&change.removed) {
                    self.engine.close(id);
                }
            }
            Action::Close => {
                if self.model.popups.contains_key(window) {
                    self.close_popup(window);
                    return Ok(());
                }
                if let Some(page) = self.model.flechinhas.remove(window) {
                    self.engine.close(&page.id);
                    return Ok(());
                }
                if self.model.peeks.contains_key(window) {
                    return self.act(window, Action::ClosePeek);
                }
                if let Some(tab) = self.model.active(window).cloned() {
                    return self.act(window, Action::CloseTab(tab));
                }
            }
            Action::CloseTab(tab) => {
                if self.model.active(window) == Some(&tab) && self.engine.request_tab_close(&tab) {
                    return Ok(());
                }
                self.close_tab(&tab);
            }
            Action::Switch(space) => {
                self.model
                    .core
                    .switch_space(window, &space)
                    .removed
                    .iter()
                    .for_each(|id| self.engine.close(id));
            }
            Action::NewSpace => {
                let id = model::space_id();
                ensure!(
                    self.model.core.add_space(id.clone()),
                    "could not create Space"
                );
                return self.act(window, Action::Switch(id));
            }
            Action::Isolate => {
                let id = self
                    .model
                    .core
                    .window(window)
                    .context("missing Flecha window")?
                    .active_space_id
                    .clone();
                let isolated = self
                    .model
                    .core
                    .space(&id)
                    .context("missing active Space")?
                    .isolated;
                self.set_space_isolated(&id, !isolated);
            }
            Action::Tab(id, action) => self.tab_action(window, &id, action)?,
            Action::Space(id, action) => {
                if let Some(opened) = self.space_action(&id, action)? {
                    self.front = Some(opened);
                }
            }
            Action::ClearToday => {
                let state = self
                    .model
                    .state_mut(window)
                    .context("missing Flecha window")?;
                for id in state.clear_today(window, now()) {
                    self.engine.close(&id);
                }
            }
            Action::Sidebar(action) => self.sidebar_action(window, action),
            Action::ResizeSidebar(width) => self.model.core.settings.sidebar_width = width,
            Action::Reopen => {
                let state = self
                    .model
                    .state_mut(window)
                    .context("missing Flecha window")?;
                let change = state.reopen_last(model::tab_id(), now());
                for id in change.removed {
                    self.engine.close(&id);
                }
                if let Some(Reopened::Window(window)) = change.result {
                    self.front = Some(window);
                }
            }
            Action::RestoreArchive(entry) => {
                self.model
                    .core
                    .restore_archived(&entry, window, model::tab_id(), now());
            }
            Action::ClearArchive => self.model.core.clear_archive(),
            Action::DeleteHistory(url) => {
                self.history.remove(&url)?;
                return Ok(());
            }
            Action::ClearHistory(range) => {
                self.history.clear(range, now())?;
                return Ok(());
            }
        }
        self.update_visibility();
        self.favicons_stale();
        self.save()
    }

    fn window_for_partition(
        &mut self,
        requested: &WindowId,
        partition: &Partition,
    ) -> Result<WindowId> {
        if let Some(window) = self
            .model
            .state(requested)
            .and_then(|state| state.window(requested))
            && self
                .model
                .window_partition(requested, &window.active_space_id)
                == *partition
        {
            return Ok(requested.clone());
        }
        if partition.is_incognito() {
            return self
                .model
                .focused_window_for(partition)
                .cloned()
                .context("The incognito session has ended");
        }
        if let Some(window) = self.model.regular_windows().iter().find(|window| {
            self.model
                .window_partition(&window.id, &window.active_space_id)
                == *partition
        }) {
            return Ok(window.id.clone());
        }
        let space = match partition {
            Partition::Shared => self
                .model
                .core
                .spaces()
                .iter()
                .find(|space| !space.isolated),
            Partition::Isolated(id) => self.model.core.space(id).filter(|space| space.isolated),
            Partition::Incognito(_) => None,
        }
        .context("No Space uses this page's request context")?
        .id
        .clone();
        let window = self.model.open_regular_window()?;
        for tab in self.model.core.switch_space(&window, &space).removed {
            self.engine.close(&tab);
        }
        Ok(window)
    }

    /// Activates a tab from `window`, bringing forward the window that shows it. Returns the
    /// pages the activation removed, whose browsers the caller closes.
    fn select(&mut self, window: &WindowId, id: &TabId) -> Result<Vec<TabId>> {
        let state = self
            .model
            .state_mut(window)
            .context("missing Flecha window")?;
        let change = state.activate(id, window, Some(now()));
        if let Some(destination) = change.result.filter(|destination| destination != window) {
            self.model.focus_window(&destination);
            self.front = Some(destination);
        }
        Ok(change.removed)
    }

    fn update_visibility(&mut self) {
        let ids = self.engine.ids().cloned().collect::<Vec<_>>();
        for id in ids {
            let visible = self
                .engine
                .owner(&id)
                .is_some_and(|owner| self.model.page(owner, &id).is_some());
            self.engine.set_visible(&id, visible);
        }
        self.note_shown_tabs(Instant::now());
    }

    fn split(&mut self, window: &WindowId, left: TabId, right: TabId) -> Result<()> {
        let split = self
            .model
            .state_mut(window)
            .context("missing Flecha window")?
            .split_to_right(window, left, right, now());
        ensure!(split.result, "could not split these tabs");
        for removed in split.removed {
            self.engine.close(&removed);
        }
        Ok(())
    }

    fn reconcile_partition(&mut self, tab: &TabId) {
        if self
            .engine
            .partition(tab)
            .or_else(|| self.engine.discarded(tab).map(|record| &record.partition))
            .is_some_and(|partition| Some(partition) != self.model.partition(tab).as_ref())
        {
            self.engine.close(tab);
        }
    }

    fn pane(&mut self, owner: Owner, id: TabId, viewport: Viewport) -> Result<()> {
        if !self.ready || self.quitting {
            return Ok(());
        }
        let Some(tab) = self.model.page(&owner, &id) else {
            return Ok(());
        };
        let partition = self
            .model
            .partition(&id)
            .context("page lacks a request context")?;
        tab_sleep::ensure_visible(&mut self.engine, &id, owner, partition, &tab.url, viewport)?;
        self.engine.set_visible(&id, true);
        Ok(())
    }

    fn page_area(&mut self, window: WindowId, viewport: Viewport) {
        self.page_areas.insert(window, viewport);
    }

    fn fail(&mut self, error: anyhow::Error, cx: &mut App) {
        *self.failure.borrow_mut() = Some(error);
        self.quitting = true;
        self.agents.stop();
        self.engine.close_all();
        cx.refresh_windows();
    }

    fn tick(&mut self, ready: bool, cx: &mut App) -> bool {
        if ready && !self.ready {
            self.ready = true;
            if let Some(write) = self.snapshots().started()
                && let Err(error) = self.store.write(&write)
            {
                self.fail(error, cx);
            }
            cx.refresh_windows();
            if self.verify {
                verify::ready();
            }
        }
        let mut changed = self.engine.contexts_changed();
        let mut save = false;
        while let Some(message) = self.engine.next_event() {
            let id = &message.key.tab;
            if self.verify {
                verify::event(&message);
            }
            match message.event {
                Event::Address(address) => {
                    if let Some((tab, durable)) = self.model.tab_mut(id) {
                        if durable {
                            self.history.begin(&message.key, &address);
                        }
                        if tab.url != address {
                            let moved = view::site(&tab.url) != view::site(&address);
                            tab.url = address;
                            save |= durable;
                            changed = true;
                            if moved {
                                self.page_site_changed(id);
                            }
                            save |= self.follow_responsive(id);
                        }
                    }
                }
                Event::Title(title) => {
                    if let Some((tab, durable)) = self.model.tab_mut(id)
                        && tab.title != title
                    {
                        tab.title = title;
                        if durable && let Err(error) = self.history.title(&message.key, &tab.title)
                        {
                            self.status.set(None, error.to_string());
                        }
                        save |= durable;
                        changed = true;
                    }
                }
                Event::Paint | Event::PointerChanged => {
                    changed = true;
                }
                Event::PaintFailed => {
                    self.fail(anyhow::anyhow!("CEF could not copy the page frame"), cx)
                }
                Event::Opened(opened) => {
                    if let Err(error) = self.opened(id, opened) {
                        self.fail(error, cx);
                    }
                    changed = true;
                }
                Event::ClosedByPage => {
                    self.history.forget(id);
                    if let Err(error) = self.page_closed(id) {
                        self.fail(error, cx);
                    }
                    changed = true;
                }
                Event::Loaded { url, .. } => {
                    self.history.loaded(id, &url);
                    changed = true;
                }
                Event::LoadFailed { url, .. } => {
                    self.history.failed(id, &url);
                    changed = true;
                }
                Event::SecurityChanged | Event::DialogChanged => changed = true,
                Event::Closed => self.history.forget(id),
                Event::Created(_) => {
                    // A tab muted while it had no browser opens muted.
                    if self
                        .model
                        .tab(id)
                        .is_some_and(|tab| tab.muted == Some(true))
                    {
                        self.engine.set_audio_muted(id, true);
                    }
                    self.emulate_mirror(id);
                    self.start_download_retries(id);
                    self.favicons_stale();
                }
                Event::FaviconUrls(urls) => {
                    let (tab_changed, durable) = self.page_favicons(id, urls);
                    changed |= tab_changed;
                    save |= tab_changed && durable;
                }
                Event::Loading(loading) => {
                    if loading {
                        self.page_loading(id);
                    }
                    changed = true;
                }
                Event::Audible(_) => changed = true,
                Event::Fullscreen(fullscreen) => {
                    self.page_fullscreen(&message.key, fullscreen);
                    changed = true;
                }
            }
        }
        let (icons_changed, icons_save) = self.drain_favicons();
        changed |= icons_changed;
        save |= icons_save;
        if save {
            self.favicons_stale();
        }
        self.sweep_favicons();
        changed |= self.engine.take_reopened();
        changed |= self.sweep_fullscreen();
        changed |= self.serve_agents();
        changed |= self.agents.status_changed(Instant::now());
        changed |= self.poll_local_servers();
        changed |= self.refresh_top_sites();
        changed |= self.finish_history();
        changed |= self.drain_downloads();
        changed |= self.poll_permissions();
        changed |= self.poll_toolbars();
        if !self.copy_link_flashes.is_empty() {
            let pending = self.copy_link_flashes.len();
            let now = Instant::now();
            self.copy_link_flashes.retain(|_, until| *until > now);
            changed |= pending != self.copy_link_flashes.len();
        }
        self.poll_page_overlays(cx);
        changed |= self.poll_page_tools(cx);
        self.poll_clipboard(cx);
        #[cfg(target_os = "linux")]
        self.poll_print_jobs(cx);
        self.poll_sidebars(cx);
        if self.ready && !self.quitting {
            changed |= self.poll_sleep(Instant::now());
        }
        if save && let Err(error) = self.save() {
            self.fail(error, cx);
        }
        if changed {
            cx.refresh_windows();
        }
        if self.quitting && self.engine.is_drained() {
            if self.verify {
                verify::drained(self.engine.live_browsers());
            }
            self.engine.release_contexts();
            cx.quit();
            return false;
        }
        true
    }
}

#[cfg(test)]
impl Controller {
    fn open_test_profile(profile: &std::path::Path) -> Result<Self> {
        let (store, loaded) = StateStore::open(&profile.join("session"))?;
        Self::with_profile(
            profile.join("cef"),
            store,
            loaded,
            false,
            true,
            None,
            Rc::new(RefCell::new(None)),
        )
    }
}

type Shared = Rc<RefCell<Controller>>;

impl Controller {
    /// A later launch of Flecha, which CEF hands to this one. A launch without an address opens a
    /// regular window. Its address is an external link: it opens in a Flechinha on the last
    /// focused regular window's Space while "Open external links in Flechinha" is on, and
    /// otherwise, as in Chromium, in a tab of that window.
    fn relaunch(&mut self, address: Option<&str>) -> Result<()> {
        let focused = self.model.core.focused_window().id.clone();
        let Some(input) = address else {
            return self.act(&focused, Action::NewWindow);
        };
        let url = pinnable_address(input).context("--open requires a browser address")?;
        if self.model.core.settings.opens_links_in_flechinha() {
            self.front = Some(self.model.open_flechinha(&focused, url)?);
            return Ok(());
        }
        let window = if self.model.regular_closed() {
            self.model.open_regular_window()?
        } else {
            focused
        };
        self.act(&window, Action::Open(url))?;
        self.front = Some(window);
        Ok(())
    }

    /// Runs an action from `window` and shows its error on that window's status line.
    fn act_from(&mut self, window: &WindowId, action: Action) -> Result<()> {
        let session = self.model.incognito_session(window);
        self.status.clear(session);
        self.model.focus_window(window);
        let result = self.act(window, action);
        if let Err(error) = &result {
            self.status.set(session, error.to_string());
        }
        result
    }

    /// The status line `window` shows.
    pub(crate) fn status_for(&self, window: &WindowId) -> Option<&str> {
        self.status.text(self.model.incognito_session(window))
    }
}

fn dispatch(controller: &Shared, window: &WindowId, action: Action, cx: &mut App) {
    let mut state = controller.borrow_mut();
    if let Err(error) = state.act_from(window, action)
        && state.quitting
    {
        state.fail(error, cx);
    }
    cx.refresh_windows();
}

#[derive(Clone, Copy)]
enum Kind {
    Main,
    Incognito,
    Flechinha,
    Popup(Option<(i32, i32)>),
}

fn sync_windows(controller: &Shared, cx: &mut App) -> Result<()> {
    let (wanted, opened) = {
        let state = controller.borrow();
        let mut wanted = BTreeMap::new();
        for window in state.model.regular_windows() {
            wanted.insert(window.id.clone(), Kind::Main);
        }
        for window in state.model.incognito_windows() {
            wanted.insert(window.id.clone(), Kind::Incognito);
        }
        for id in state.model.flechinhas.keys() {
            wanted.insert(id.clone(), Kind::Flechinha);
        }
        for (id, popup) in &state.model.popups {
            wanted.insert(id.clone(), Kind::Popup(popup.size));
        }
        (wanted, state.windows.clone())
    };
    for (id, handle) in &opened {
        if !wanted.contains_key(id) {
            {
                let mut state = controller.borrow_mut();
                state.windows.remove(id);
                state.sidebar_deadlines.remove(id);
                state.forget_download_window(id);
                state.page_areas.remove(id);
                state.sleep.window_activation(id, false);
            }
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
    for (id, kind) in wanted {
        if opened.contains_key(&id) {
            continue;
        }
        // Flechinha uses the shipping shell's 900 x 700 size. A popup's page gets
        // the size it asked for, below its address bar. The main window and popups
        // without a size use GPUI's default size.
        let content = match kind {
            Kind::Main | Kind::Incognito | Kind::Popup(None) => None,
            Kind::Flechinha => Some(size(px(900.), px(700.))),
            Kind::Popup(Some((width, height))) => Some(size(
                px(width as f32),
                px(height as f32) + view::POPUP_BAR_HEIGHT,
            )),
        };
        let bounds =
            content.map(|content| WindowBounds::Windowed(Bounds::centered(None, content, cx)));
        let shell = {
            let controller = controller.clone();
            let id = id.clone();
            move |window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
                window.set_window_title(match kind {
                    Kind::Main => "Flecha",
                    Kind::Incognito => "Flecha Incognito",
                    Kind::Flechinha => "Flechinha",
                    // The Shell names it after its page.
                    Kind::Popup(_) => "",
                });
                window.on_window_should_close(cx, {
                    let controller = controller.clone();
                    let id = id.clone();
                    move |_, cx| {
                        dispatch(&controller, &id, Action::CloseWindow, cx);
                        false
                    }
                });
                Shell::build(controller, id, window, cx)
            }
        };
        // The first main window takes over the window that opened before CEF, which
        // has GPUI's default size like every main window.
        let first = matches!(kind, Kind::Main)
            .then(|| controller.borrow_mut().first_window.take())
            .flatten();
        let handle = match first {
            Some(first) => {
                first.update(cx, |_, window, cx| {
                    window.replace_root(cx, shell);
                })?;
                // The root is a Shell now, which `WindowHandle::new` does not check.
                WindowHandle::<Shell>::new(first.window_id())
            }
            None => {
                let options = WindowOptions {
                    window_bounds: bounds,
                    ..Default::default()
                };
                // A popup has no toolbar to hold the window controls.
                let options = match kind {
                    Kind::Popup(_) => options,
                    Kind::Main | Kind::Incognito | Kind::Flechinha => {
                        window_chrome::client_decorated(options)
                    }
                };
                cx.open_window(options, |window, cx| cx.new(|cx| shell(window, cx)))?
            }
        };
        controller.borrow_mut().windows.insert(id, handle);
    }
    // No main window wanted it, as when only an incognito window is restored. The GPU
    // stays in use by the windows that just opened.
    let unused = controller.borrow_mut().first_window.take();
    if let Some(first) = unused {
        let _ = first.update(cx, |_, window, _| window.remove_window());
    }
    let front = {
        let mut state = controller.borrow_mut();
        state
            .front
            .take()
            .and_then(|id| Some((state.windows.get(&id).copied()?, id, state.verify)))
    };
    if let Some((front, id, verify)) = front {
        let _ = front.update(cx, |_, window, _| window.activate_window());
        if verify {
            verify::activated(&id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "external.example/page";
    const URL: &str = "https://external.example/page";

    fn controller(profile: &tempfile::TempDir) -> (Controller, WindowId) {
        let controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        (controller, window)
    }

    #[test]
    fn a_page_action_uses_a_window_in_its_original_request_context() {
        let profile = tempfile::tempdir().unwrap();
        let (mut state, window) = controller(&profile);
        let isolated = model::space_id();
        assert!(state.model.core.add_space(isolated.clone()));
        state.model.core.set_isolated(&isolated, true);
        assert!(state.model.core.switch_space(&window, &isolated).result);

        let shared = state
            .window_for_partition(&window, &Partition::Shared)
            .unwrap();
        assert_ne!(shared, window);
        let shared_space = &state.model.core.window(&shared).unwrap().active_space_id;
        assert_eq!(
            state.model.window_partition(&shared, shared_space),
            Partition::Shared
        );
        assert_eq!(
            state
                .window_for_partition(&shared, &Partition::Isolated(isolated))
                .unwrap(),
            window
        );
    }

    #[test]
    fn a_forwarded_address_opens_in_a_flechinha_while_the_setting_is_on() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        assert!(controller.model.core.settings.opens_links_in_flechinha());
        let tabs = controller.model.core.open_tabs().len();

        controller.relaunch(Some(LINK)).unwrap();

        let [(mini, page)] = controller.model.flechinhas.iter().collect::<Vec<_>>()[..] else {
            panic!("expected one Flechinha");
        };
        assert_eq!(page.tab.url, URL);
        assert_eq!(page.partition, flecha_cef::Partition::Shared);
        assert_eq!(
            &page.space,
            &controller
                .model
                .core
                .window(&regular)
                .unwrap()
                .active_space_id
        );
        assert_eq!(controller.front.as_ref(), Some(mini));
        assert_eq!(
            controller.model.core.open_tabs().len(),
            tabs,
            "the link also opened a tab"
        );

        // Open in Flecha moves it into the regular window as a tab.
        let mini = mini.clone();
        controller.act(&mini, Action::Return).unwrap();
        assert!(controller.model.flechinhas.is_empty());
        let active = controller.model.active(&regular).unwrap();
        assert_eq!(controller.model.core.tab(active).unwrap().url, URL);
    }

    #[test]
    fn open_in_flecha_brings_back_the_regular_window_an_incognito_one_outlived() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        controller
            .act(&regular, Action::NewIncognitoWindow)
            .unwrap();
        let private = controller.front.clone().unwrap();
        controller.act(&regular, Action::CloseWindow).unwrap();
        assert!(controller.model.regular_closed());

        controller.relaunch(Some(LINK)).unwrap();
        let mini = controller.model.flechinhas.keys().next().unwrap().clone();
        controller.act(&mini, Action::Return).unwrap();

        assert!(controller.model.flechinhas.is_empty());
        assert!(!controller.model.regular_closed());
        assert_eq!(controller.front.as_ref(), Some(&regular));
        let active = controller.model.active(&regular).unwrap();
        assert_eq!(controller.model.core.tab(active).unwrap().url, URL);
        assert!(controller.model.is_incognito(&private));
    }

    #[test]
    fn a_forwarded_address_opens_a_regular_tab_while_the_setting_is_off() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        controller.model.core.settings.open_links_in_flechinha = Some(false);

        controller.relaunch(Some(LINK)).unwrap();

        assert!(controller.model.flechinhas.is_empty());
        let active = controller.model.active(&regular).unwrap();
        assert_eq!(controller.model.core.tab(active).unwrap().url, URL);
        assert_eq!(controller.front.as_ref(), Some(&regular));
    }

    #[test]
    fn a_forwarded_address_opens_its_flechinha_in_an_isolated_spaces_context() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        controller.act(&regular, Action::Isolate).unwrap();
        let space = controller
            .model
            .core
            .window(&regular)
            .unwrap()
            .active_space_id
            .clone();

        controller.relaunch(Some(LINK)).unwrap();

        let page = controller.model.flechinhas.values().next().unwrap();
        assert_eq!(page.partition, flecha_cef::Partition::Isolated(space));
    }
}
