mod adblock;
mod boosts;
mod callbacks;
mod clipboard;
mod compositor;
mod context_menu;
mod devtools;
mod dialogs;
mod downloads;
mod favicon;
mod find;
mod identity;
mod page_tools;
mod permissions;
mod popup;
#[cfg(target_os = "linux")]
mod printing;
mod pump;
mod runtime;
mod services;
mod sleep;
mod zoom;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use anyhow::{Context as _, Result, ensure};
use cef::{
    Browser, BrowserHost, BrowserSettings, CefString, ImplBrowser as _, ImplBrowserHost as _,
    ImplFrame as _, ImplRequestContext as _, Rect, RequestContext, RequestContextSettings,
    RuntimeStyle, WindowInfo, browser_host_create_browser_sync, request_context_create_context,
};
use flecha_core::TabId;
use gpui::{Bounds, Pixels, Point, point, px};

pub use callbacks::Frames;
use callbacks::{Queue, Slot, Stage};
pub use clipboard::{ClipboardEdit, CopiedSelection};
pub use compositor::Compositing;
use compositor::Compositor;
pub use context_menu::{
    ContextMenuEvent, ContextMenuId, ContextMenuRequest, FrameEdit, MenuContext, MenuFrame,
};
pub use devtools::{DevToolsEvent, DevToolsMessage};
pub use dialogs::{
    ConnectionSecurity, DialogId, DialogKind, DialogRequest, DialogResponse, FileMode,
};
pub use downloads::{
    DownloadConfig, DownloadError, DownloadEvent, DownloadHandle, DownloadMessage,
};
pub use favicon::{FAVICON_SIZE, Favicon, FaviconBitmap};
pub use find::{FindEvent, FindResult};
use identity::Generations;
pub use identity::{BrowserKey, IncognitoSession, Owner, Partition};
pub use page_tools::{Copied, PHONE_WIDTH, PageToolEvent, PageToolMessage};
pub use permissions::{PermissionEvent, PermissionPrompt};
pub use popup::{Disposition, Opened};
use popup::{Pending, Popups};
use runtime::path_string;
pub use runtime::{Relaunches, Runtime, Start, profile_in_use};
use services::Services;
pub use sleep::{Discarded, SleepChange};
pub use zoom::{PageZoom, zoom_percentage};

/// The CEF build Flecha is built against (`pins.toml`'s `cef.version`), such as
/// `158.0.3+g8c29be2+chromium-158.0.8127.18`. cef-rs refuses a runtime with another API hash.
pub fn cef_version() -> &'static str {
    std::ffi::CStr::from_bytes_with_nul(cef::sys::CEF_VERSION)
        .ok()
        .and_then(|version| version.to_str().ok())
        .unwrap_or_default()
}

/// The Chromium version inside that CEF build, such as `158.0.8127.18`.
pub fn chromium_version() -> String {
    use cef::sys::{
        CHROME_VERSION_BUILD, CHROME_VERSION_MAJOR, CHROME_VERSION_MINOR, CHROME_VERSION_PATCH,
    };
    format!(
        "{CHROME_VERSION_MAJOR}.{CHROME_VERSION_MINOR}.{CHROME_VERSION_BUILD}.{CHROME_VERSION_PATCH}"
    )
}

/// `Opened`: CEF is creating this browser for a page, and the engine already owns it.
/// `ClosedByPage`: the page closed its browser (`window.close()`), or CEF abandoned
/// creating it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    SecurityChanged,
    DialogChanged,
    Created(i32),
    Closed,
    Address(String),
    Title(String),
    Loaded {
        url: String,
        status: i32,
    },
    LoadFailed {
        url: String,
        code: i32,
    },
    Paint,
    PaintFailed,
    PointerChanged,
    Opened(Opened),
    ClosedByPage,
    /// The page entered (`true`) or left HTML fullscreen. With Alloy-style browsers the
    /// client makes the window transition and handles Esc ([`Engine::exit_fullscreen`]).
    Fullscreen(bool),
    /// The icons the main frame's document declares, in its order. Empty when it has none.
    FaviconUrls(Vec<String>),
    /// The browser started (`true`) or finished loading ([`Engine::is_loading`]).
    Loading(bool),
    /// The page became audible (`true`), or a new document replaced the audible one
    /// ([`Engine::is_audible`]).
    Audible(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserEvent {
    pub key: BrowserKey,
    pub event: Event,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Viewport {
    pane_origin: Point<Pixels>,
    origin: (i32, i32),
    size: (i32, i32),
    screen: (i32, i32, i32, i32),
    scale: f32,
}

impl Viewport {
    pub fn new(
        pane: Bounds<Pixels>,
        window: Bounds<Pixels>,
        screen: Bounds<Pixels>,
        scale: f32,
    ) -> Self {
        let pixels = |px| f32::from(px).round() as i32;
        Self {
            pane_origin: pane.origin,
            origin: (
                pixels(window.origin.x + pane.origin.x),
                pixels(window.origin.y + pane.origin.y),
            ),
            size: (
                pixels(pane.size.width).max(1),
                pixels(pane.size.height).max(1),
            ),
            screen: (
                pixels(screen.origin.x),
                pixels(screen.origin.y),
                pixels(screen.size.width),
                pixels(screen.size.height),
            ),
            scale,
        }
    }

    fn view_rect(self) -> Rect {
        Rect {
            width: self.size.0,
            height: self.size.1,
            ..Rect::default()
        }
    }

    fn screen_rect(self) -> Rect {
        let (x, y, width, height) = self.screen;
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn scale(self) -> f32 {
        self.scale
    }

    fn screen_origin(self) -> (i32, i32) {
        self.origin
    }

    fn window_point(self, (x, y): (i32, i32)) -> Point<Pixels> {
        self.pane_origin + point(px(x as f32), px(y as f32))
    }
}

struct Session {
    owner: Owner,
    partition: Partition,
    slot: Rc<Slot>,
    hidden: bool,
    opener: Option<Weak<Slot>>,
    /// Whether tab sleep froze this browser. A frozen browser stays muted.
    frozen: bool,
    /// Whether the tab's own audio is muted, which a thawed browser returns to.
    muted: bool,
}

struct PartitionContext {
    handle: RequestContext,
    ready: Rc<Cell<bool>>,
    /// Whether its content settings hold the saved permission decisions, which they must
    /// before a browser opens in it.
    reconciled: bool,
}

pub struct Engine {
    profile: PathBuf,
    contexts: BTreeMap<Partition, PartitionContext>,
    contexts_changed: Rc<Cell<bool>>,
    generations: Generations,
    browsers: BTreeMap<TabId, Session>,
    closing: BTreeSet<BrowserKey>,
    events: Queue,
    popups: Rc<Popups>,
    /// Tabs tab sleep discarded, with what each needs to load again.
    discarded: BTreeMap<TabId, Discarded>,
    /// Sites where ad blocking is off (`Settings::ad_block_allowed`).
    adblock_allowed: Vec<String>,
    /// The last page frame of tabs whose browser is reopening with CPU copies, shown
    /// until the new browser paints.
    retained: BTreeMap<TabId, flecha_surface::Frame>,
    /// Set when pages closed for CPU copies, until the shell takes it.
    reopened: bool,
}

impl Engine {
    /// `mint` names the tabs of browsers that pages open.
    pub fn new(profile: PathBuf, mint: fn() -> TabId) -> Self {
        let generations = Generations::default();
        let popups = Rc::new(Popups {
            counter: generations.counter(),
            mint,
            pending: RefCell::new(Vec::new()),
            live: Cell::new(0),
            services: Rc::new(Services::new()),
            compositor: Compositor::default(),
        });
        Self {
            profile,
            contexts: BTreeMap::new(),
            contexts_changed: Rc::new(Cell::new(false)),
            generations,
            browsers: BTreeMap::new(),
            closing: BTreeSet::new(),
            events: Queue::default(),
            popups,
            discarded: BTreeMap::new(),
            adblock_allowed: Vec::new(),
            retained: BTreeMap::new(),
            reopened: false,
        }
    }

    /// Starts giving new browsers frames as `compositing` says. Call once CEF runs with
    /// the same `compositing`.
    pub fn start_compositing(&self, compositing: &Compositing) {
        self.popups.compositor.start(compositing);
    }

    /// Whether browsers created now receive page frames as shared GPU textures, rather than
    /// CPU copies.
    pub fn shares_textures(&self) -> bool {
        self.popups.compositor.shares_textures()
    }

    /// Follows the GPUI device of a window that shows pages. When GPUI recreates its
    /// device on the same GPU, browsers repaint for the new one. When it moves to another
    /// GPU, browsers that share textures reopen with CPU copies.
    #[cfg(target_os = "linux")]
    pub fn follow_window_device(&self, target: flecha_surface::SurfaceTarget) {
        let change = self.popups.compositor.follow(target);
        if change == compositor::DeviceChange::None {
            return;
        }
        for session in self.browsers.values() {
            let slot = &session.slot;
            if !slot.shares_textures.get() {
                continue;
            }
            if change == compositor::DeviceChange::Lost {
                slot.import_failure
                    .replace(Some(anyhow::anyhow!("GPUI changed GPU")));
            } else if let Some(host) = slot.browser.borrow().as_ref().and_then(|b| b.host()) {
                host.invalidate(cef::PaintElementType::VIEW);
            }
        }
    }

    /// Makes the next shared-texture import fail, so the display harness can check the
    /// switch to CPU copies.
    #[doc(hidden)]
    pub fn reject_next_shared_texture(&self) {
        self.popups.compositor.reject_next_import();
    }

    /// Whether pages closed for CPU copies since the last call. The shell must draw them
    /// again so its panes recreate their browsers, because the closed browsers' own
    /// queued events no longer reach it.
    pub fn take_reopened(&mut self) -> bool {
        std::mem::take(&mut self.reopened)
    }

    /// Closes each page whose shared texture could not be imported, for the shell to
    /// reopen with CPU copies. Runs only once the shell has every queued event, so a
    /// popup's placement reaches it before its browser closes.
    fn reopen_failed_imports(&mut self) {
        let failed: Vec<_> = self
            .browsers
            .iter()
            .filter_map(|(tab, session)| {
                let error = session.slot.import_failure.borrow_mut().take()?;
                Some((tab.clone(), error))
            })
            .collect();
        self.reopened |= !failed.is_empty();
        for (tab, error) in failed {
            self.popups.compositor.turn_off(&format!("{error:#}"));
            let frame = self.browsers[&tab].slot.surface.borrow().view.clone();
            compositor::log(format_args!(
                "page frames: reopening a page with CPU copies: {error:#}"
            ));
            // A page with no address yet opens again at the tab's address.
            if self.discard_tab(&tab).is_none() {
                self.close(&tab);
            }
            if let Some(frame) = frame {
                self.retained.insert(tab, frame);
            }
        }
    }

    fn services(&self) -> &Services {
        &self.popups.services
    }

    pub fn key(&self, tab: &TabId) -> Option<BrowserKey> {
        self.generations.current(tab)
    }

    pub fn owner(&self, tab: &TabId) -> Option<&Owner> {
        self.browsers.get(tab).map(|session| &session.owner)
    }

    pub fn partition(&self, tab: &TabId) -> Option<&Partition> {
        self.browsers.get(tab).map(|session| &session.partition)
    }

    pub fn ensure(
        &mut self,
        tab: TabId,
        owner: Owner,
        partition: Partition,
        url: &str,
        viewport: Viewport,
    ) -> Result<()> {
        if let Some(session) = self.browsers.get_mut(&tab)
            && session.partition == partition
        {
            session.owner = owner;
            self.resize(&tab, viewport);
            return Ok(());
        }
        self.close_browser(&tab);
        let Some(mut context) = self.context(&partition)? else {
            return Ok(());
        };
        let key = self.generations.create(tab.clone());
        let slot = Rc::new(Slot::new(
            key.clone(),
            viewport,
            self.events.clone(),
            self.popups.clone(),
        ));
        slot.partition.replace(Some(partition.clone()));
        let shared = self.popups.compositor.shares_textures();
        slot.shares_textures.set(shared);
        slot.surface.borrow_mut().view = self.retained.remove(&tab);
        let mut client = callbacks::client(slot.clone());
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            windowless_rendering_enabled: 1,
            shared_texture_enabled: i32::from(shared),
            ..Default::default()
        };
        let browser = browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&BrowserSettings::default()),
            None,
            Some(&mut context),
        );
        if browser.is_none() {
            self.generations.retire(&key);
            anyhow::bail!("CEF could not create an OSR browser");
        }
        self.browsers.insert(
            tab.clone(),
            Session {
                slot,
                owner,
                partition,
                hidden: false,
                opener: None,
                frozen: false,
                muted: false,
            },
        );
        self.discarded.remove(&tab);
        let alloy = browser
            .and_then(|browser| browser.host())
            .is_some_and(|host| host.runtime_style() == RuntimeStyle::ALLOY);
        ensure!(alloy, "CEF did not create an Alloy OSR browser");
        Ok(())
    }

    /// The partition's request context once it is ready and its permission settings are
    /// reconciled, which happens once per context, before its first browser.
    fn context(&mut self, partition: &Partition) -> Result<Option<RequestContext>> {
        if !self.contexts.contains_key(partition) {
            let context = if *partition == Partition::Shared {
                PartitionContext {
                    handle: cef::request_context_get_global_context()
                        .context("get shared CEF request context")?,
                    // CEF initializes the global context before the app's first callback.
                    ready: Rc::new(Cell::new(true)),
                    reconciled: false,
                }
            } else {
                let settings = match partition.directory() {
                    Some(directory) => RequestContextSettings {
                        cache_path: path_string(&self.profile.join(directory))?,
                        persist_session_cookies: 1,
                        ..Default::default()
                    },
                    // An empty cache_path makes an "incognito mode" context: in-memory caches,
                    // and no profile-specific data persisted to disk
                    // (include/internal/cef_types.h, cef_request_context_settings_t).
                    None => RequestContextSettings::default(),
                };
                let ready = Rc::new(Cell::new(false));
                let mut handler =
                    callbacks::ContextReady::new(ready.clone(), self.contexts_changed.clone());
                let handle = request_context_create_context(Some(&settings), Some(&mut handler))
                    .context("create CEF request context")?;
                // GetCachePath() is empty for an in-memory context (cef_request_context.h).
                ensure!(
                    partition.directory().is_some()
                        || CefString::from(&handle.cache_path()).to_string().is_empty(),
                    "CEF gave the incognito request context a folder on disk"
                );
                PartitionContext {
                    handle,
                    ready,
                    reconciled: false,
                }
            };
            self.contexts.insert(partition.clone(), context);
        }
        let services = self.popups.services.clone();
        let Some(context) = self
            .contexts
            .get_mut(partition)
            .filter(|context| context.ready.get())
        else {
            return Ok(None);
        };
        if !context.reconciled {
            permissions::reconcile(
                &services,
                &permissions::partition(partition),
                &context.handle,
            )?;
            context.reconciled = true;
        }
        Ok(Some(context.handle.clone()))
    }

    /// The request contexts browsers can open in, with their permission partitions.
    fn permission_contexts(&self) -> Vec<(flecha_permissions::Partition, RequestContext)> {
        self.contexts
            .iter()
            .filter(|(_, context)| context.reconciled)
            .map(|(partition, context)| (permissions::partition(partition), context.handle.clone()))
            .collect()
    }

    pub fn contexts_changed(&self) -> bool {
        self.contexts_changed.replace(false)
    }

    pub fn resize(&self, tab: &TabId, viewport: Viewport) {
        let Some(session) = self.browsers.get(tab) else {
            return;
        };
        let old = session.slot.viewport.replace(viewport);
        if old != viewport
            && let Some(host) = self.host(tab)
        {
            if (old.scale, old.screen) != (viewport.scale, viewport.screen) {
                host.notify_screen_info_changed();
            }
            if (old.size, old.scale) != (viewport.size, viewport.scale) {
                host.was_resized();
            }
        }
    }

    pub fn frames(&self, tab: &TabId) -> Option<Frames> {
        match self.browsers.get(tab) {
            Some(session) => Some(session.slot.surface.borrow().clone()),
            None => self.retained.get(tab).cloned().map(Frames::page),
        }
    }

    pub fn pointer_state(&self, tab: &TabId) -> (gpui::CursorStyle, Option<gpui::SharedString>) {
        self.browsers
            .get(tab)
            .map_or_else(Default::default, |session| {
                (
                    flecha_surface::input::cursor_style(session.slot.cursor.get()),
                    session.slot.status.borrow().clone(),
                )
            })
    }

    pub fn clear_pointer_status(&self, tab: &TabId) {
        if let Some(session) = self.browsers.get(tab) {
            session.slot.set_status(None);
        }
    }

    pub fn host(&self, tab: &TabId) -> Option<BrowserHost> {
        self.browsers
            .get(tab)?
            .slot
            .browser
            .borrow()
            .as_ref()?
            .host()
    }

    pub fn browser_id(&self, tab: &TabId) -> Option<i32> {
        Some(
            self.browsers
                .get(tab)?
                .slot
                .browser
                .borrow()
                .as_ref()?
                .identifier(),
        )
    }

    pub fn viewport(&self, tab: &TabId) -> Option<Viewport> {
        Some(self.browsers.get(tab)?.slot.viewport.get())
    }

    /// Exercise the same queue and dispatch boundary as CEF's callbacks.
    #[doc(hidden)]
    pub fn queue_verification_callbacks(&self, tab: &TabId) -> Option<BrowserKey> {
        let slot = &self.browsers.get(tab)?.slot;
        slot.emit(Event::Paint);
        slot.emit(Event::Title("retired verification title".into()));
        slot.emit(Event::Address("about:blank#retired-verification".into()));
        Some(slot.key.borrow().clone())
    }

    pub fn ids(&self) -> impl Iterator<Item = &TabId> {
        self.browsers.keys()
    }

    pub fn focused(&self, tab: &TabId, focused: bool) {
        if let Some(host) = self.host(tab) {
            host.set_focus(i32::from(focused));
        }
    }

    pub fn set_visible(&mut self, tab: &TabId, visible: bool) {
        if visible {
            self.thaw_tab(tab);
        }
        let Some(session) = self.browsers.get_mut(tab) else {
            return;
        };
        if session.hidden == visible {
            session.hidden = !visible;
            if !visible {
                session.slot.set_status(None);
            }
            if let Some(host) = self.host(tab) {
                host.was_hidden(i32::from(!visible));
            }
        }
    }

    pub fn release_contexts(&mut self) {
        assert!(self.is_drained());
        self.contexts.clear();
    }

    /// Ends an incognito session, when its last window closes: closes the browsers still open
    /// in it, forgets its permission decisions and drops the engine's handle on its in-memory
    /// request context, which CEF frees once those browsers finish closing. A later incognito
    /// window must use a new session.
    pub fn end_incognito(&mut self, session: IncognitoSession) {
        let partition = Partition::Incognito(session);
        let open = self
            .browsers
            .iter()
            .filter(|(_, browser)| browser.partition == partition)
            .map(|(tab, _)| tab.clone())
            .collect::<Vec<_>>();
        for tab in open {
            self.close(&tab);
        }
        self.discarded.retain(|_, page| page.partition != partition);
        self.contexts.remove(&partition);
        self.end_download_session(session);
        let ephemeral = permissions::partition(&partition);
        self.edit_permissions(|manager| ((), manager.clear_partition(&ephemeral)));
    }

    /// The partitions the engine holds a request context for.
    pub fn context_partitions(&self) -> impl Iterator<Item = &Partition> {
        self.contexts.keys()
    }

    pub fn transfer(&mut self, from: &TabId, to: TabId, owner: Owner) {
        let Some(mut session) = self.browsers.remove(from) else {
            return;
        };
        if from != &to || session.owner != owner {
            if session.slot.finding.get() {
                find::stop(&session.slot, true);
            }
            context_menu::cancel(&session.slot);
        }
        if from != &to {
            let mut key = session.slot.key.borrow_mut();
            *key = self.generations.transfer(&key, to.clone());
            session.slot.paint_pending.set(false);
        }
        session.owner = owner;
        self.browsers.insert(to, session);
    }

    pub fn execute_script(&self, tab: &TabId, script: &str) -> bool {
        let frame = self
            .browsers
            .get(tab)
            .and_then(|session| session.slot.browser.borrow().as_ref()?.main_frame());
        if let Some(frame) = frame {
            frame.execute_java_script(Some(&CefString::from(script)), None, 0);
            true
        } else {
            false
        }
    }

    fn browser(&self, tab: &TabId) -> Option<Browser> {
        self.browsers.get(tab)?.slot.browser.borrow().clone()
    }

    /// Loads `url` in the tab's main frame as a browser-initiated navigation, as the address bar
    /// does. Returns whether the tab has a main frame.
    pub fn navigate(&self, tab: &TabId, url: &str) -> bool {
        let frame = self.browser(tab).and_then(|browser| browser.main_frame());
        if let Some(frame) = &frame {
            frame.load_url(Some(&CefString::from(url)));
        }
        frame.is_some()
    }

    pub fn go_back(&self, tab: &TabId) {
        if let Some(browser) = self.browser(tab) {
            browser.go_back();
        }
    }

    pub fn go_forward(&self, tab: &TabId) {
        if let Some(browser) = self.browser(tab) {
            browser.go_forward();
        }
    }

    pub fn reload(&self, tab: &TabId) {
        if let Some(browser) = self.browser(tab) {
            browser.reload();
        }
    }

    pub fn print(&self, tab: &TabId) -> Result<()> {
        let host = self.host(tab).context("The page has closed")?;
        #[cfg(target_os = "linux")]
        {
            self.print_for_portal(tab, &host)
        }
        #[cfg(not(target_os = "linux"))]
        {
            host.print();
            Ok(())
        }
    }

    pub fn stop_load(&self, tab: &TabId) {
        if let Some(browser) = self.browser(tab) {
            browser.stop_load();
        }
    }

    pub fn can_go_back(&self, tab: &TabId) -> bool {
        self.browser(tab)
            .is_some_and(|browser| browser.can_go_back() != 0)
    }

    pub fn can_go_forward(&self, tab: &TabId) -> bool {
        self.browser(tab)
            .is_some_and(|browser| browser.can_go_forward() != 0)
    }

    pub fn is_loading(&self, tab: &TabId) -> bool {
        self.browser(tab)
            .is_some_and(|browser| browser.is_loading() != 0)
    }

    /// Whether the tab's page has played sound since its document loaded. CEF reports only when
    /// a page becomes audible, so this stays set until the next document or browser.
    pub fn is_audible(&self, tab: &TabId) -> bool {
        self.browsers
            .get(tab)
            .is_some_and(|session| session.slot.audible.get())
    }

    /// Whether CEF has the tab's page muted now, by the shell or by tab sleep.
    pub fn is_audio_muted(&self, tab: &TabId) -> bool {
        self.host(tab)
            .is_some_and(|host| host.is_audio_muted() != 0)
    }

    /// Whether the tab's page is in HTML fullscreen, as its renderer has it now.
    pub fn is_fullscreen(&self, tab: &TabId) -> bool {
        self.host(tab).is_some_and(|host| host.is_fullscreen() != 0)
    }

    /// Asks the page to leave HTML fullscreen; [`Event::Fullscreen`] follows. Alloy-style
    /// clients call this for the user's exit actions, such as Esc (`cef_browser_capi.h`).
    pub fn exit_fullscreen(&self, tab: &TabId) {
        if let Some(host) = self.host(tab) {
            host.exit_fullscreen(1);
        }
    }

    /// The main frame's URL, as CEF has it now.
    pub fn url(&self, tab: &TabId) -> Option<String> {
        let frame = self.browser(tab)?.main_frame()?;
        Some(CefString::from(&frame.url()).to_string())
    }

    /// Closes the tab's browser and forgets the tab, discarded page included.
    pub fn close(&mut self, tab: &TabId) {
        self.discarded.remove(tab);
        self.retained.remove(tab);
        self.close_browser(tab);
    }

    /// Closes the tab's browser. A discarded page's record stays for [`Self::restore_tab`].
    fn close_browser(&mut self, tab: &TabId) {
        let Some(session) = self.browsers.remove(tab) else {
            return;
        };
        self.shut(&session.slot);
    }

    fn shut(&mut self, slot: &Slot) {
        let key = slot.key.borrow().clone();
        self.generations.retire(&key);
        slot.retired.set(true);
        find::forget(slot);
        context_menu::cancel(slot);
        slot.dialogs.cancel();
        match slot.stage.get() {
            // CEF already destroyed it. Its queued ClosedByPage now has a retired key.
            Stage::Closed => {}
            // on_after_created closes it, or CEF reports the aborted creation.
            Stage::Pending => {
                self.closing.insert(key);
            }
            Stage::Live => {
                self.closing.insert(key);
                let host = slot
                    .browser
                    .borrow()
                    .as_ref()
                    .and_then(|browser| browser.host());
                if let Some(host) = host {
                    host.close_browser(1);
                }
            }
        }
    }

    /// Registers the browsers CEF started creating for pages since the last call. A
    /// popup whose opener has gone since, or that was already closed/retired, is shut
    /// without registering so its queued events never reach the shell.
    fn adopt_popups(&mut self) {
        let pending = std::mem::take(&mut *self.popups.pending.borrow_mut());
        for Pending { child, opener } in pending {
            if child.retired.get()
                || child.stage.get() == Stage::Closed
                || opener.retired.get()
                || opener.stage.get() == Stage::Closed
            {
                self.shut(&child);
                continue;
            }
            let source = opener.key.borrow().tab.clone();
            let session = self
                .browsers
                .get(&source)
                .filter(|session| Rc::ptr_eq(&session.slot, &opener));
            let Some((owner, partition)) =
                session.map(|session| (session.owner.clone(), session.partition.clone()))
            else {
                self.shut(&child);
                continue;
            };
            let key = child.key.borrow().clone();
            self.generations.register(&key);
            self.browsers.insert(
                key.tab,
                Session {
                    owner,
                    // CEF creates a popup in its opener's request context.
                    partition,
                    slot: child,
                    hidden: false,
                    opener: Some(Rc::downgrade(&opener)),
                    frozen: false,
                    muted: false,
                },
            );
        }
    }

    /// The tab whose page opened this one, while that page is still open.
    pub fn opener(&self, tab: &TabId) -> Option<TabId> {
        let opener = self.browsers.get(tab)?.opener.as_ref()?.upgrade()?;
        if opener.retired.get() || opener.stage.get() == Stage::Closed {
            return None;
        }
        let source = opener.key.borrow().tab.clone();
        self.browsers
            .get(&source)
            .filter(|session| Rc::ptr_eq(&session.slot, &opener))
            .map(|_| source)
    }

    /// CEF browsers between `on_after_created` and `on_before_close`, counted by the
    /// callbacks themselves.
    pub fn live_browsers(&self) -> usize {
        self.popups.live.get()
    }

    pub fn close_all(&mut self) {
        self.adopt_popups();
        while let Some(tab) = self.browsers.first_key_value().map(|(tab, _)| tab.clone()) {
            self.close(&tab);
        }
        self.retained.clear();
    }

    /// CEF must call `on_before_close` for every browser before the app exits.
    pub fn is_drained(&self) -> bool {
        self.browsers.is_empty()
            && self.closing.is_empty()
            && self.popups.pending.borrow().is_empty()
            && self.live_browsers() == 0
    }

    pub fn next_event(&mut self) -> Option<BrowserEvent> {
        self.adopt_popups();
        loop {
            let next = self.events.borrow_mut().pop_front();
            let Some(message) = next else {
                self.reopen_failed_imports();
                return None;
            };
            if message.event == Event::Closed && self.closing.remove(&message.key) {
                // Always finish a retired generation's shutdown bookkeeping.
                if self.generations.current(&message.key.tab).is_none() {
                    return Some(message);
                }
                continue;
            }
            if !self.generations.accepts(&message.key) {
                continue;
            }
            if message.event == Event::Paint
                && let Some(session) = self.browsers.get(&message.key.tab)
            {
                session.slot.paint_pending.set(false);
            }
            if message.event == Event::ClosedByPage {
                self.browsers.remove(&message.key.tab);
                self.generations.retire(&message.key);
            }
            return Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! callback_samples {
        ($($pattern:pat => $sample:expr),+ $(,)?) => {{
            fn cover(event: &Event) {
                match event { $($pattern => (),)+ }
            }
            let samples = [$($sample,)+];
            samples.iter().for_each(cover);
            samples
        }};
    }

    #[test]
    fn rejects_every_callback_from_a_retired_generation() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let tab = TabId::new("replaced-tab");
        let retired = engine.generations.create(tab.clone());
        let current = engine.generations.create(tab.clone());
        engine.closing.insert(retired.clone());

        let url = "https://example.test/old";
        for event in callback_samples![
            Event::Created(_) => Event::Created(1),
            Event::DialogChanged => Event::DialogChanged,
            Event::SecurityChanged => Event::SecurityChanged,
            Event::Closed => Event::Closed,
            Event::Address(_) => Event::Address(url.into()),
            Event::Title(_) => Event::Title("Old document".into()),
            Event::Loaded { .. } => Event::Loaded { url: url.into(), status: 200 },
            Event::LoadFailed { .. } => Event::LoadFailed { url: url.into(), code: -106 },
            Event::Paint => Event::Paint,
            Event::PaintFailed => Event::PaintFailed,
            Event::PointerChanged => Event::PointerChanged,
            Event::Opened(_) => Event::Opened(Opened {
                url: url.into(),
                disposition: Disposition::Popup,
                size: Some((480, 360)),
            }),
            Event::ClosedByPage => Event::ClosedByPage,
            Event::Fullscreen(_) => Event::Fullscreen(true),
            Event::FaviconUrls(_) => Event::FaviconUrls(vec![format!("{url}/icon.png")]),
            Event::Loading(_) => Event::Loading(true),
            Event::Audible(_) => Event::Audible(true),
        ] {
            engine.events.borrow_mut().push_back(BrowserEvent {
                key: retired.clone(),
                event: event.clone(),
            });
            assert_eq!(
                engine.next_event(),
                None,
                "retired callback reached the shell: {event:?}"
            );
        }

        assert!(engine.closing.is_empty());
        let live = BrowserEvent {
            key: current,
            event: Event::Paint,
        };
        engine.events.borrow_mut().push_back(live.clone());
        assert_eq!(engine.next_event(), Some(live));
    }

    const VIEWPORT: Viewport = Viewport {
        pane_origin: point(px(0.), px(0.)),
        origin: (0, 0),
        size: (800, 600),
        screen: (0, 0, 800, 600),
        scale: 1.,
    };

    /// A slot no CEF browser backs, in an engine of its own.
    pub(crate) fn slot(tab: &str) -> Rc<Slot> {
        slots(&[tab]).remove(0)
    }

    /// Browsers of one engine, so they share its services.
    pub(crate) fn slots(tabs: &[&str]) -> Vec<Rc<Slot>> {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        tabs.iter()
            .map(|tab| {
                Rc::new(Slot::new(
                    engine.generations.create(TabId::new(*tab)),
                    VIEWPORT,
                    engine.events.clone(),
                    engine.popups.clone(),
                ))
            })
            .collect()
    }

    /// A live opener with a popup CEF has started creating, and the popup's slot.
    fn opener_with_pending_popup(engine: &mut Engine) -> (Rc<Slot>, Rc<Slot>) {
        let tab = TabId::new("opener");
        let opener = Rc::new(Slot::new(
            engine.generations.create(tab.clone()),
            VIEWPORT,
            engine.events.clone(),
            engine.popups.clone(),
        ));
        assert!(!opener.created(1));
        engine.browsers.insert(
            tab,
            Session {
                owner: Owner::Tab(flecha_core::WindowId::new("window")),
                partition: Partition::Shared,
                slot: opener.clone(),
                hidden: false,
                opener: None,
                frozen: false,
                muted: false,
            },
        );
        let child = popup::open(&opener, 7, None);
        (opener, child)
    }

    fn drain(engine: &mut Engine) -> Vec<Event> {
        std::iter::from_fn(|| engine.next_event())
            .map(|message| message.event)
            .collect()
    }

    #[test]
    fn quitting_does_not_wait_for_a_popup_its_closed_opener_abandoned() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let (opener, child) = opener_with_pending_popup(&mut engine);
        engine.close_all();
        assert!(!engine.is_drained());

        // CEF destroys the opener and never calls on_before_popup_aborted.
        opener.destroyed();
        assert_eq!(drain(&mut engine), [Event::Closed, Event::Closed]);
        assert!(engine.is_drained(), "quit waits for an abandoned popup");

        // A popup CEF creates anyway is closed, and quit waits for that.
        assert!(child.created(2), "a late popup stays open");
        assert!(!engine.is_drained());
        child.destroyed();
        assert!(drain(&mut engine).is_empty());
        assert!(engine.is_drained());
    }

    #[test]
    fn a_page_closing_itself_releases_its_pending_popup() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let (opener, child) = opener_with_pending_popup(&mut engine);
        assert_eq!(drain(&mut engine), [Event::Created(1)]);
        assert!(engine.browsers.contains_key(&TabId::new("popup")));

        // window.close() in the opener, and CEF never creates or aborts the popup.
        opener.destroyed();
        assert_eq!(
            drain(&mut engine),
            [Event::ClosedByPage, Event::ClosedByPage]
        );
        assert!(engine.browsers.is_empty());
        assert!(engine.is_drained());

        assert!(child.created(2), "a late popup has no tab to show it");
        child.destroyed();
        assert!(drain(&mut engine).is_empty());
        assert!(engine.is_drained());
    }

    #[test]
    fn a_failed_import_closes_the_browser_and_keeps_its_last_frame() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let (opener, _child) = opener_with_pending_popup(&mut engine);
        let tab = TabId::new("opener");
        let pixel = [0x10_u8, 0x20, 0x30, 0xff];
        // SAFETY: `pixel` holds width * height * 4 bytes.
        let frame = unsafe {
            flecha_surface::Frame::copy_bgra(
                pixel.as_ptr(),
                1,
                1,
                &flecha_surface::SurfaceTarget::default(),
                None,
                &[],
            )
        }
        .unwrap();
        opener.surface.borrow_mut().view = Some(frame);
        opener.shares_textures.set(true);
        opener
            .import_failure
            .replace(Some(anyhow::anyhow!("import failed")));

        drain(&mut engine);

        assert!(
            !engine.browsers.contains_key(&tab),
            "the browser stays open"
        );
        assert!(
            engine
                .frames(&tab)
                .is_some_and(|frames| frames.view.is_some()),
            "the page goes blank until its new browser paints"
        );
    }

    #[test]
    fn a_popup_whose_first_import_fails_is_placed_before_it_closes() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let (_opener, child) = opener_with_pending_popup(&mut engine);
        let popup = TabId::new("popup");
        let opened = popup::Opened {
            url: "https://example.test/popup".into(),
            disposition: popup::Disposition::Popup,
            size: Some((480, 360)),
        };
        // CEF's order: OnBeforePopup, OnAfterCreated, then a first frame that fails.
        child.emit(Event::Opened(opened.clone()));
        assert!(!child.created(2));
        child.shares_textures.set(true);
        child
            .import_failure
            .replace(Some(anyhow::anyhow!("import failed")));

        let mut events = Vec::new();
        while let Some(message) = engine.next_event() {
            if matches!(message.event, Event::Opened(_)) {
                assert_eq!(message.key.tab, popup);
                assert!(
                    engine.opener(&popup).is_some(),
                    "the shell cannot place a popup whose browser closed"
                );
            }
            events.push(message.event);
        }

        assert_eq!(
            events,
            [Event::Created(1), Event::Opened(opened), Event::Created(2)]
        );
        assert!(
            !engine.browsers.contains_key(&popup),
            "the popup kept the browser that failed"
        );
        assert!(engine.take_reopened(), "the shell is not told to redraw");
        assert!(!engine.take_reopened());
    }

    #[test]
    fn abandoned_popup_before_first_poll_never_reaches_shell() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let (opener, child) = opener_with_pending_popup(&mut engine);
        child.emit(Event::Opened(popup::Opened {
            url: "https://example.test/popup".into(),
            disposition: popup::Disposition::ForegroundTab,
            size: None,
        }));

        // Opener closes before the engine's first poll adopts the popup.
        opener.destroyed();

        let events = drain(&mut engine);
        assert_eq!(events, [Event::Created(1), Event::ClosedByPage]);
        assert!(!engine.browsers.contains_key(&TabId::new("popup")));
        assert_eq!(engine.opener(&TabId::new("popup")), None);
        assert!(engine.is_drained());

        assert!(child.created(2), "a late popup still closes immediately");
        child.destroyed();
        assert!(drain(&mut engine).is_empty());
        assert!(engine.is_drained());
    }
}
