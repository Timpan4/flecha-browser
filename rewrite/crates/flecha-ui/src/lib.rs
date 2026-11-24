mod boosts;
mod command_bar;
mod downloads_view;
mod history_view;
mod model;
mod permissions_view;
mod popup;
mod settings_view;
mod shortcuts;
mod sidebar;
mod site_controls;
mod tab_sleep;
mod text_field;
mod verify;
mod view;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Context as _, Result, ensure};
use flecha_boosts::Boosts;
use flecha_cef::{Engine, Event, Owner, Runtime, Viewport};
use flecha_core::{
    ArchivedTab, Reopened, SpaceId, TabId, WindowId,
    storage::{LoadedState, ReadyState},
    suggestions::pinnable_address,
};
use flecha_services::StateStore;
use gpui::{App, Bounds, WindowBounds, WindowHandle, WindowOptions, px, size};

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
}

pub fn start(
    cx: &mut App,
    runtime: &mut Runtime,
    cef_profile: PathBuf,
    store: StateStore,
    loaded: LoadedState,
    options: Options,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
) -> Result<()> {
    let controller = Rc::new(RefCell::new(Controller::new(
        cef_profile.clone(),
        store,
        loaded,
        options.verify,
        failure,
    )?));
    sync_windows(&controller, cx)?;
    // Chromium fixes the GPU it shares textures on at startup, so CEF starts after GPUI
    // has picked the GPU for the first window. GPUI's windows share one device.
    let first = controller.borrow().windows.values().next().copied();
    let compositing = first
        .and_then(|window| {
            window
                .update(cx, |_, window, _| {
                    flecha_cef::Compositing::for_window(window)
                })
                .ok()
        })
        .unwrap_or_default();
    runtime.initialize(&cef_profile, &compositing)?;
    controller.borrow().engine.start_compositing(&compositing);
    if options.verify {
        verify::install(&controller, cx);
    }
    if let Some(input) = options.initial {
        let url = pinnable_address(&input).context("--open requires a browser address")?;
        let window = controller.borrow().model.core.focused_window().id.clone();
        controller.borrow_mut().act(&window, Action::Open(url))?;
    }
    runtime.drive(cx, move |ready, cx| {
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
    Navigate(String),
    Browser(Navigation),
    Download(flecha_cef::DownloadHandle, DownloadCommand),
    ResolvePermission(
        flecha_permissions::RequestId,
        flecha_permissions::PromptOutcome,
    ),
    RetryPermissionSave,
    Site(Site, SiteChange),
    Select(TabId),
    Split(TabId),
    SplitNew(String),
    ExitSplit,
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
    Switch(SpaceId),
    NewSpace,
    Isolate,
    Sidebar(SidebarAction),
    Reopen,
    RestoreArchive(ArchivedTab),
    ClearArchive,
    DeleteHistory(String),
    ClearHistory(ClearRange),
    Quit,
}

struct Controller {
    model: Model,
    history: History,
    boosts: ProfileBoosts,
    engine: Engine,
    downloads: Downloads,
    permissions: PermissionStore,
    sleep: TabSleep,
    toolbars: BTreeMap<WindowId, Toolbar>,
    pending_reloads: BTreeMap<i32, flecha_cef::BrowserKey>,
    store: StateStore,
    session_dir: PathBuf,
    snapshots: LoadedState,
    windows: BTreeMap<WindowId, WindowHandle<Shell>>,
    front: Option<WindowId>,
    ready: bool,
    quitting: bool,
    verify: bool,
    failure: Rc<RefCell<Option<anyhow::Error>>>,
    status: String,
}

impl Controller {
    fn new(
        cef_profile: PathBuf,
        store: StateStore,
        snapshots: LoadedState,
        verify: bool,
        failure: Rc<RefCell<Option<anyhow::Error>>>,
    ) -> Result<Self> {
        let isolated =
            verify || std::env::var_os("FLECHA_LAUNCHER").as_deref() == Some("isolated".as_ref());
        Self::with_profile(cef_profile, store, snapshots, verify, isolated, failure)
    }

    fn with_profile(
        cef_profile: PathBuf,
        store: StateStore,
        mut snapshots: LoadedState,
        verify: bool,
        isolated: bool,
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
            downloads: Downloads::new(&profile, isolated),
            permissions: PermissionStore::open(&profile),
            sleep: TabSleep::new(Instant::now()),
            toolbars: BTreeMap::new(),
            pending_reloads: BTreeMap::new(),
            store,
            snapshots,
            windows: BTreeMap::new(),
            front: None,
            ready: false,
            quitting: false,
            verify,
            failure,
            status: String::new(),
        };
        state.permissions.restore(&mut state.engine);
        if profile.join("adblock").is_dir() {
            state.engine.load_adblock_rules(&profile.join("adblock"));
        }
        state.sync_adblock();
        state.engine.set_boosts(state.boosts().clone());
        state.sync_download_config();
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

    fn save(&mut self) -> Result<()> {
        let write = self
            .snapshots
            .ready()
            .expect("session recovered")
            .save_current(&self.model.core)?;
        self.store.write(&write)
    }

    fn act(&mut self, window: &WindowId, action: Action) -> Result<()> {
        if self.quitting {
            return Ok(());
        }
        match action {
            Action::Quit => {
                self.quitting = true;
                self.engine.close_all();
                return self.save();
            }
            Action::Open(url) => {
                self.model.open(window, url, true)?;
            }
            Action::Navigate(url) => self.navigate_tab(window, url)?,
            Action::Browser(navigation) => return self.browser_navigation(window, navigation),
            Action::Download(handle, command) => return self.manage_download(handle, command),
            Action::ResolvePermission(request, outcome) => {
                return self.resolve_permission(request, outcome);
            }
            Action::RetryPermissionSave => return self.save_permissions(),
            Action::Site(site, change) => return self.change_current_site(window, &site, change),
            Action::Select(id) => {
                let change = self.model.core.activate(&id, window, Some(now()));
                for id in change.removed {
                    self.engine.close(&id);
                }
                if let Some(destination) = change.result.filter(|destination| destination != window)
                {
                    self.model.core.focus_window(&destination);
                    self.front = Some(destination);
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
            Action::ExitSplit => {
                if let Some(flecha_core::EndedSplit::Responsive { mirror }) =
                    self.model.core.unsplit(window)
                {
                    self.engine.close(&mirror);
                }
            }
            Action::Peek(url) => {
                let source = self.model.active(window).cloned();
                if let Some(old) = self.model.peek(window.clone(), url, source.as_ref())? {
                    self.engine.close(&old);
                }
                return Ok(());
            }
            Action::PeekTab(id) => {
                let Some(tab) = self.model.core.tab(&id) else {
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
            Action::CloseWindow => {
                if self.model.flechinhas.contains_key(window)
                    || self.model.popups.contains_key(window)
                {
                    return self.act(window, Action::Close);
                }
                if self.model.core.windows().len() <= 1 {
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
                self.engine.close(&tab);
                for removed in self.model.core.close_tab(&tab, now()).removed {
                    self.engine.close(&removed);
                }
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
                for tab in self.model.core.set_isolated(&id, !isolated) {
                    self.reconcile_partition(&tab);
                }
                for tab in self.model.close_space_transients(&id) {
                    self.engine.close(&tab);
                }
            }
            Action::Sidebar(action) => self.sidebar_action(window, action),
            Action::Reopen => {
                let change = self.model.core.reopen_last(model::tab_id(), now());
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
        self.save()
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
        let split = self.model.core.split_to_right(window, left, right, now());
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

    fn fail(&mut self, error: anyhow::Error, cx: &mut App) {
        *self.failure.borrow_mut() = Some(error);
        self.quitting = true;
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
                            tab.url = address;
                            save |= durable;
                            changed = true;
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
                            self.status = error.to_string();
                        }
                        save |= durable;
                        changed = true;
                    }
                }
                Event::Paint => {
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
                Event::Loaded { url, .. } => self.history.loaded(id, &url),
                Event::LoadFailed { url, .. } => self.history.failed(id, &url),
                Event::Closed => self.history.forget(id),
                Event::Created(_) => {}
            }
        }
        changed |= self.finish_history();
        changed |= self.drain_downloads();
        changed |= self.poll_permissions();
        changed |= self.poll_toolbars();
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
            Rc::new(RefCell::new(None)),
        )
    }
}

type Shared = Rc<RefCell<Controller>>;

fn dispatch(controller: &Shared, window: &WindowId, action: Action, cx: &mut App) {
    let mut state = controller.borrow_mut();
    state.status.clear();
    if state.model.core.window(window).is_some() {
        state.model.core.focus_window(window);
    }
    if let Err(error) = state.act(window, action) {
        state.status = error.to_string();
        if state.quitting {
            state.fail(error, cx);
        }
    }
    cx.refresh_windows();
}

#[derive(Clone, Copy)]
enum Kind {
    Main,
    Flechinha,
    Popup(Option<(i32, i32)>),
}

fn sync_windows(controller: &Shared, cx: &mut App) -> Result<()> {
    let (wanted, opened) = {
        let state = controller.borrow();
        let mut wanted = BTreeMap::new();
        for window in state.model.core.windows() {
            wanted.insert(window.id.clone(), Kind::Main);
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
            Kind::Main | Kind::Popup(None) => None,
            Kind::Flechinha => Some(size(px(900.), px(700.))),
            Kind::Popup(Some((width, height))) => Some(size(
                px(width as f32),
                px(height as f32) + view::POPUP_BAR_HEIGHT,
            )),
        };
        let bounds =
            content.map(|content| WindowBounds::Windowed(Bounds::centered(None, content, cx)));
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: bounds,
                ..Default::default()
            },
            {
                let controller = controller.clone();
                let id = id.clone();
                move |window, cx| {
                    window.set_window_title(match kind {
                        Kind::Main => "Flecha",
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
                    Shell::new(controller, id, window, cx)
                }
            },
        )?;
        controller.borrow_mut().windows.insert(id, handle);
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
