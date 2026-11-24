use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use cef::{rc::Rc as _, *};
use flecha_surface::{Frame as SurfaceFrame, SurfaceTarget};
use gpui::{Bounds, Pixels, SharedString, point, px, size};

use crate::compositor::{self, Delivery};
use crate::ime::Ime;
use crate::popup::{self, Popups};
use crate::{BrowserEvent, BrowserKey, Event, Partition, Viewport};
use crate::{adblock, boosts, context_menu, dialogs, downloads, find, permissions, services};

pub(crate) type Queue = Rc<RefCell<VecDeque<BrowserEvent>>>;

wrap_request_context_handler! {
    pub(crate) struct ContextReady {
        ready: Rc<Cell<bool>>,
        changed: Rc<Cell<bool>>,
    }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _context: Option<&mut RequestContext>) {
            self.ready.set(true);
            self.changed.set(true);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// CEF hasn't called `on_after_created` yet.
    Pending,
    Live,
    Closed,
}

pub(crate) struct Slot {
    pub key: RefCell<BrowserKey>,
    pub browser: RefCell<Option<Browser>>,
    pub viewport: Cell<Viewport>,
    pub surface: RefCell<Frames>,
    pub retired: Cell<bool>,
    pub paint_pending: Cell<bool>,
    pub cursor: Cell<CursorType>,
    pub status: RefCell<Option<SharedString>>,
    pub events: Queue,
    pub popups: Rc<Popups>,
    pub stage: Cell<Stage>,
    pub finding: Cell<bool>,
    pub menu: context_menu::Menu,
    pub dialogs: dialogs::Dialogs,
    pub certificate_error: RefCell<Option<String>>,
    pub committed_navigation: Arc<AtomicU64>,
    /// Popups this browser has asked CEF for that CEF hasn't created yet, by popup ID.
    pub opening: RefCell<BTreeMap<i32, Rc<Slot>>>,
    /// For a popup still being created: its opener and its popup ID there.
    pub origin: RefCell<Option<(Weak<Slot>, i32)>>,
    /// CEF's browser identifier, from `on_after_created`.
    pub identifier: Cell<Option<i32>>,
    /// The request context the engine placed this browser in. Permission decisions stay in it.
    pub partition: RefCell<Option<Partition>>,
    /// Requests the IO thread's ad blocker cancelled on the current page.
    pub blocked: Arc<AtomicU64>,
    /// The DevTools observer, registered by the first CDP call.
    pub devtools: RefCell<Option<Registration>>,
    /// Permission answers Chromium holds for the current document only.
    pub one_time: RefCell<Vec<permissions::OneTime>>,
    /// Whether CEF created this browser in shared-texture mode.
    pub shares_textures: Cell<bool>,
    /// Why a shared texture could not be imported. The engine reopens the page with CPU
    /// copies, and the render handler ignores shared textures until then.
    pub import_failure: RefCell<Option<anyhow::Error>>,
    pub ime: Ime,
    /// Whether the page became audible since its document loaded ([`crate::Engine::is_audible`]).
    pub audible: Cell<bool>,
    delivery: Cell<Option<Delivery>>,
}

impl Slot {
    pub fn new(key: BrowserKey, viewport: Viewport, events: Queue, popups: Rc<Popups>) -> Self {
        Self {
            key: RefCell::new(key),
            browser: RefCell::new(None),
            viewport: Cell::new(viewport),
            surface: RefCell::new(Frames::default()),
            retired: Cell::new(false),
            paint_pending: Cell::new(false),
            cursor: Cell::new(CursorType::POINTER),
            status: RefCell::new(None),
            events,
            popups,
            stage: Cell::new(Stage::Pending),
            finding: Cell::new(false),
            menu: context_menu::Menu::default(),
            dialogs: dialogs::Dialogs::default(),
            certificate_error: RefCell::new(None),
            committed_navigation: Arc::new(AtomicU64::new(0)),
            opening: RefCell::new(BTreeMap::new()),
            origin: RefCell::new(None),
            identifier: Cell::new(None),
            partition: RefCell::new(None),
            blocked: Arc::new(AtomicU64::new(0)),
            devtools: RefCell::new(None),
            one_time: RefCell::new(Vec::new()),
            shares_textures: Cell::new(false),
            import_failure: RefCell::new(None),
            ime: Ime::default(),
            audible: Cell::new(false),
            delivery: Cell::new(None),
        }
    }

    /// `on_after_created`. Returns whether the engine already closed this browser, so
    /// the caller must close it now.
    pub fn created(&self, identifier: i32) -> bool {
        self.identifier.set(Some(identifier));
        self.stage.set(Stage::Live);
        self.popups.live.set(self.popups.live.get() + 1);
        let origin = self.origin.take();
        if let Some((opener, popup_id)) = origin
            && let Some(opener) = opener.upgrade()
        {
            opener.opening.borrow_mut().remove(&popup_id);
        }
        self.emit(Event::Created(identifier));
        self.retired.get()
    }

    /// `on_before_close`. Releases every CEF object the engine holds for this browser and
    /// ends what it was waiting on: its downloads are cancelled and its permission requests
    /// denied.
    pub fn destroyed(&self) {
        dialogs::unregister(self);
        self.dialogs.cancel();
        self.browser.take();
        find::forget(self);
        context_menu::cancel(self);
        self.devtools.take();
        // Frames can hold textures on GPUI's device, which must not outlive the windows.
        self.surface.take();
        let services = services::of(self);
        crate::devtools::forget(&services, self);
        let acts = services.downloads.borrow_mut().browser_closed(self);
        downloads::run(&services, acts);
        if let Some(identifier) = self.identifier.get() {
            permissions::leave(&services, &permissions::queue_id(identifier));
        }
        permissions::forget_one_time(self);
        self.closed();
        popup::opener_closed(self);
    }

    /// CEF destroyed this browser or abandoned creating it. The engine closed a retired
    /// slot; anything else was the page (`window.close()`) or CEF.
    pub fn closed(&self) {
        if self.stage.replace(Stage::Closed) == Stage::Live {
            self.popups.live.set(self.popups.live.get() - 1);
        }
        self.emit(if self.retired.get() {
            Event::Closed
        } else {
            Event::ClosedByPage
        });
    }

    pub fn emit(&self, event: Event) {
        self.events.borrow_mut().push_back(BrowserEvent {
            key: self.key.borrow().clone(),
            event,
        });
    }

    /// Records whether the page is audible, and tells the shell when that changes.
    fn set_audible(&self, audible: bool) {
        if !self.retired.get() && self.audible.replace(audible) != audible {
            self.emit(Event::Audible(audible));
        }
    }

    fn paint_changed(&self) {
        if !self.paint_pending.replace(true) {
            self.emit(Event::Paint);
        }
    }

    pub fn set_status(&self, status: Option<SharedString>) {
        if *self.status.borrow() != status {
            self.status.replace(status);
            self.emit(Event::PointerChanged);
        }
    }

    /// Stores a page or `<select>` popup frame from either paint callback.
    fn store(&self, kind: PaintElementType, frame: SurfaceFrame, delivery: Delivery) {
        compositor::delivered(&self.delivery, delivery, self.identifier.get());
        let mut frames = self.surface.borrow_mut();
        if kind == PaintElementType::VIEW {
            frames.view = Some(frame);
        } else {
            frames.popup = frames.popup_bounds.map(|bounds| (bounds, frame));
        }
        drop(frames);
        self.paint_changed();
    }
}

#[derive(Clone, Default)]
pub struct Frames {
    pub view: Option<SurfaceFrame>,
    pub popup: Option<(Bounds<Pixels>, SurfaceFrame)>,
    popup_bounds: Option<Bounds<Pixels>>,
}

impl Frames {
    /// A page frame with no `<select>` popup.
    pub(crate) fn page(view: SurfaceFrame) -> Self {
        Self {
            view: Some(view),
            ..Self::default()
        }
    }
}

fn bounds(rect: &Rect) -> Bounds<Pixels> {
    Bounds::new(
        point(px(rect.x as f32), px(rect.y as f32)),
        size(px(rect.width as f32), px(rect.height as f32)),
    )
}

wrap_render_handler! {
    struct Osr { slot: Rc<Slot> }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                *rect = self.slot.viewport.get().view_rect();
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen: Option<&mut ScreenInfo>) -> i32 {
            let Some(screen) = screen else { return 0 };
            let viewport = self.slot.viewport.get();
            screen.device_scale_factor = viewport.scale();
            screen.rect = viewport.screen_rect();
            screen.available_rect = screen.rect.clone();
            1
        }

        fn screen_point(
            &self, _browser: Option<&mut Browser>, view_x: i32, view_y: i32,
            screen_x: Option<&mut i32>, screen_y: Option<&mut i32>,
        ) -> i32 {
            let (x, y) = self.slot.viewport.get().screen_origin();
            if let Some(screen_x) = screen_x { *screen_x = x.saturating_add(view_x); }
            if let Some(screen_y) = screen_y { *screen_y = y.saturating_add(view_y); }
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: i32) {
            if show == 0 && !self.slot.retired.get() {
                let mut frames = self.slot.surface.borrow_mut();
                frames.popup = None;
                frames.popup_bounds = None;
                self.slot.paint_changed();
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if !self.slot.retired.get() {
                let mut frames = self.slot.surface.borrow_mut();
                frames.popup = None;
                frames.popup_bounds = rect.map(bounds);
                self.slot.paint_changed();
            }
        }

        fn on_ime_composition_range_changed(
            &self, _browser: Option<&mut Browser>, range: Option<&Range>, character_bounds: Option<&[Rect]>,
        ) {
            self.slot.ime.update_bounds(range, character_bounds.unwrap_or_default());
        }

        fn on_text_selection_changed(
            &self, _browser: Option<&mut Browser>, _text: Option<&CefString>, range: Option<&Range>,
        ) {
            self.slot.ime.update_selection(range);
        }

        fn on_paint(
            &self, _browser: Option<&mut Browser>, kind: PaintElementType, dirty: Option<&[Rect]>,
            buffer: *const u8, width: i32, height: i32,
        ) {
            if self.slot.retired.get() { return; }
            let frames = self.slot.surface.borrow();
            let previous = match kind {
                PaintElementType::VIEW => frames.view.as_ref(),
                PaintElementType::POPUP => frames.popup.as_ref().map(|(_, frame)| frame),
                _ => return,
            };
            // CEF supplies width * height * 4 bytes valid through this callback.
            let frame = unsafe { SurfaceFrame::copy_bgra(
                buffer, width, height, &SurfaceTarget::default(), previous, dirty.unwrap_or_default(),
            ) };
            drop(frames);
            match frame {
                Ok(frame) => self.slot.store(kind, frame, Delivery::CpuCopy),
                Err(error) => {
                    compositor::log(format_args!("CEF {kind:?} frame copy ({width}x{height}): {error:#}"));
                    self.slot.emit(Event::PaintFailed);
                }
            }
        }

        #[cfg(target_os = "linux")]
        fn on_accelerated_paint(
            &self, _browser: Option<&mut Browser>, kind: PaintElementType, _dirty: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            if self.slot.retired.get() || self.slot.import_failure.borrow().is_some() { return; }
            if kind != PaintElementType::VIEW && kind != PaintElementType::POPUP { return; }
            let target = self.slot.popups.compositor.target();
            let frame = match (info, target) {
                (None, _) => Err(anyhow::anyhow!("CEF sent no shared texture")),
                (_, None) => Err(anyhow::anyhow!("shared textures are off")),
                // The importer copies the dma-buf into GPUI's device before returning, and
                // CEF's render node matches that device.
                (Some(info), Some(target)) => self.slot.popups.compositor.import(info, |info| unsafe {
                    SurfaceFrame::copy_accelerated(info, &target)
                }),
            };
            match frame {
                Ok(frame) => self.slot.store(kind, frame, Delivery::SharedTexture),
                Err(error) => {
                    self.slot.import_failure.replace(Some(error));
                    // Wake the engine, which reopens the page with CPU copies.
                    self.slot.paint_changed();
                }
            }
        }
    }
}

wrap_life_span_handler! {
    struct Life { slot: Rc<Slot> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let closed = self.slot.created(browser.identifier());
            if !closed {
                dialogs::register(&self.slot, browser.identifier());
            }
            self.slot.browser.replace(Some(browser.clone()));
            // The engine closed this popup, or its opener left, while CEF was creating it.
            if closed && let Some(host) = browser.host() {
                host.close_browser(1);
            }
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.slot.destroyed();
        }

        fn on_before_popup(
            &self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, popup_id: i32,
            url: Option<&CefString>, _frame_name: Option<&CefString>, disposition: WindowOpenDisposition,
            _gesture: i32, features: Option<&PopupFeatures>, window: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>, settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, _no_javascript: Option<&mut i32>,
        ) -> i32 {
            let allowed = popup::before_popup(&self.slot, popup_id, url, disposition, features, window, client);
            if allowed && let Some(settings) = settings {
                settings.windowless_frame_rate = self.slot.viewport.get().frame_rate();
            }
            i32::from(!allowed)
        }

        fn on_before_popup_aborted(&self, _browser: Option<&mut Browser>, popup_id: i32) {
            popup::aborted(&self.slot, popup_id);
        }

        fn on_before_dev_tools_popup(
            &self, _browser: Option<&mut Browser>, _window: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, use_default_window: Option<&mut i32>,
        ) {
            // Registering a CEF life span handler makes the native DevTools window opt-in.
            if let Some(use_default_window) = use_default_window {
                *use_default_window = 1;
            }
        }
    }
}

wrap_display_handler! {
    struct Display { slot: Rc<Slot> }
    impl DisplayHandler {
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() != 0) && let Some(url) = url {
                self.slot.emit(Event::Address(url.to_string()));
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Some(title) = title {
                self.slot.emit(Event::Title(title.to_string()));
            }
        }

        fn on_favicon_urlchange(&self, _browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            if !self.slot.retired.get() {
                let urls = icon_urls.map(|urls| {
                    std::mem::take(urls).into_iter().filter(|url| crate::favicon::is_web_icon(url)).collect()
                }).unwrap_or_default();
                self.slot.emit(Event::FaviconUrls(urls));
            }
        }

        fn on_cursor_change(
            &self, _browser: Option<&mut Browser>, _cursor: cef::sys::cef_cursor_handle_t,
            kind: CursorType, _custom: Option<&CursorInfo>,
        ) -> i32 {
            if !self.slot.retired.get() && self.slot.cursor.replace(kind) != kind {
                self.slot.emit(Event::PointerChanged);
            }
            1
        }

        fn on_status_message(&self, _browser: Option<&mut Browser>, value: Option<&CefString>) {
            if !self.slot.retired.get() {
                self.slot.set_status(value.map(ToString::to_string)
                    .filter(|text| !text.is_empty()).map(SharedString::from));
            }
        }

        fn on_fullscreen_mode_change(&self, _browser: Option<&mut Browser>, fullscreen: i32) {
            if !self.slot.retired.get() {
                self.slot.emit(Event::Fullscreen(fullscreen != 0));
            }
        }
    }
}

wrap_load_handler! {
    struct Load { slot: Rc<Slot> }
    impl LoadHandler {
        fn on_loading_state_change(
            &self, _browser: Option<&mut Browser>, is_loading: i32, _can_go_back: i32, _can_go_forward: i32,
        ) {
            if !self.slot.retired.get() {
                self.slot.emit(Event::Loading(is_loading != 0));
            }
        }

        fn on_load_start(
            &self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType,
        ) {
            let Some(frame) = frame.filter(|frame| frame.is_main() != 0) else { return };
            self.slot.committed_navigation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.slot.ime.reset_document();
            // A new document. CEF reports when a page becomes audible, never when it stops.
            self.slot.set_audible(false);
            context_menu::cancel(&self.slot);
            if self.slot.dialogs.cancel() {
                self.slot.emit(Event::DialogChanged);
            }
            let services = services::of(&self.slot);
            // The page this tab's permission requests came from is gone.
            if let Some(identifier) = self.slot.identifier.get() {
                permissions::leave(&services, &permissions::queue_id(identifier));
            }
            permissions::forget_one_time(&self.slot);
            if !self.slot.retired.get() {
                self.slot.set_status(None);
                boosts::page_committed(&services, frame, &CefString::from(&frame.url()).to_string());
            }
        }

        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, status: i32) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                let url = CefString::from(&frame.url()).to_string();
                if !self.slot.retired.get() {
                    let host = browser.and_then(|browser| browser.host());
                    crate::adblock::hide_ads(&self.slot, host, frame, &url);
                    boosts::page_loaded(&services::of(&self.slot), frame, &url);
                }
                self.slot.emit(Event::Loaded { url, status });
            }
        }

        fn on_load_error(
            &self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, code: Errorcode,
            _error: Option<&CefString>, url: Option<&CefString>,
        ) {
            if frame.is_some_and(|frame| frame.is_main() != 0) && let Some(url) = url {
                self.slot.emit(Event::LoadFailed { url: url.to_string(), code: code.get_raw() });
            }
        }
    }
}

wrap_audio_handler! {
    struct Audio { slot: Rc<Slot> }
    impl AudioHandler {
        // Alloy requests parameters when a page becomes audible. Its loopback capture mutes
        // the source (CEF 8127, CefAudioLoopbackStreamCreator). Declining preserves playback.
        fn audio_parameters(&self, _browser: Option<&mut Browser>, _params: Option<&mut AudioParameters>) -> i32 {
            self.slot.set_audible(true);
            0
        }
    }
}

#[derive(Clone)]
struct PageHandlers {
    display: DisplayHandler,
    load: LoadHandler,
    audio: AudioHandler,
    find: FindHandler,
    context_menu: ContextMenuHandler,
    js_dialog: JsdialogHandler,
    file_dialog: DialogHandler,
    #[cfg(target_os = "linux")]
    print: PrintHandler,
}

wrap_client! {
    pub(crate) struct OsrClient {
        render: RenderHandler,
        life: LifeSpanHandler,
        request: RequestHandler,
        download: DownloadHandler,
        permission: PermissionHandler,
        page: PageHandlers,
    }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.render.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.life.clone()) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(self.page.display.clone()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(self.page.load.clone()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(self.request.clone()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(self.download.clone()) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(self.permission.clone()) }
        fn find_handler(&self) -> Option<FindHandler> { Some(self.page.find.clone()) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(self.page.context_menu.clone()) }
        fn audio_handler(&self) -> Option<AudioHandler> { Some(self.page.audio.clone()) }
        fn jsdialog_handler(&self) -> Option<JsdialogHandler> { Some(self.page.js_dialog.clone()) }
        fn dialog_handler(&self) -> Option<DialogHandler> { Some(self.page.file_dialog.clone()) }
        #[cfg(target_os = "linux")]
        fn print_handler(&self) -> Option<PrintHandler> {
            Some(self.page.print.clone())
        }
    }
}

/// Every browser the engine owns gets these handlers, popups included.
pub(crate) fn client(slot: Rc<Slot>) -> Client {
    let io = adblock::Io {
        blocker: slot.popups.services.adblock.clone(),
        blocked: slot.blocked.clone(),
    };
    OsrClient::new(
        Osr::new(slot.clone()),
        Life::new(slot.clone()),
        adblock::handler(io, slot.committed_navigation.clone()),
        downloads::handler(slot.clone()),
        permissions::handler(slot.clone()),
        PageHandlers {
            display: Display::new(slot.clone()),
            load: Load::new(slot.clone()),
            audio: Audio::new(slot.clone()),
            find: find::handler(slot.clone()),
            context_menu: context_menu::handler(slot.clone()),
            js_dialog: dialogs::JsDialogs::new(slot.clone()),
            #[cfg(target_os = "linux")]
            print: crate::printing::LinuxPrinting::new(),
            file_dialog: dialogs::FileDialogs::new(slot),
        },
    )
}
