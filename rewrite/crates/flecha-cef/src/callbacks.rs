use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use cef::{rc::Rc as _, *};
use flecha_surface::{Frame as SurfaceFrame, SurfaceTarget};
use gpui::{Bounds, Pixels, point, px, size};

use crate::popup::{self, Popups};
use crate::{BrowserEvent, BrowserKey, Event, Partition, Viewport};
use crate::{adblock, boosts, downloads, permissions, services};

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
    pub events: Queue,
    pub popups: Rc<Popups>,
    pub stage: Cell<Stage>,
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
            events,
            popups,
            stage: Cell::new(Stage::Pending),
            opening: RefCell::new(BTreeMap::new()),
            origin: RefCell::new(None),
            identifier: Cell::new(None),
            partition: RefCell::new(None),
            blocked: Arc::new(AtomicU64::new(0)),
            devtools: RefCell::new(None),
            one_time: RefCell::new(Vec::new()),
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
        self.browser.take();
        self.devtools.take();
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

    fn paint_changed(&self) {
        if !self.paint_pending.replace(true) {
            self.emit(Event::Paint);
        }
    }
}

#[derive(Clone, Default)]
pub struct Frames {
    pub view: Option<SurfaceFrame>,
    pub popup: Option<(Bounds<Pixels>, SurfaceFrame)>,
    popup_bounds: Option<Bounds<Pixels>>,
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

        fn on_paint(
            &self, _browser: Option<&mut Browser>, kind: PaintElementType, dirty: Option<&[Rect]>,
            buffer: *const u8, width: i32, height: i32,
        ) {
            if self.slot.retired.get() { return; }
            let mut frames = self.slot.surface.borrow_mut();
            let previous = match kind {
                PaintElementType::VIEW => frames.view.as_ref(),
                PaintElementType::POPUP => frames.popup.as_ref().map(|(_, frame)| frame),
                _ => return,
            };
            // CEF supplies width * height * 4 bytes valid through this callback.
            let frame = unsafe { SurfaceFrame::copy_bgra(
                buffer, width, height, &SurfaceTarget::default(), previous, dirty.unwrap_or_default(),
            ) };
            match frame {
                Ok(frame) => {
                    if kind == PaintElementType::VIEW {
                        frames.view = Some(frame);
                    } else {
                        frames.popup = frames.popup_bounds.map(|bounds| (bounds, frame));
                    }
                    self.slot.paint_changed();
                }
                Err(_) => self.slot.emit(Event::PaintFailed),
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
            client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, _no_javascript: Option<&mut i32>,
        ) -> i32 {
            let allowed = popup::before_popup(&self.slot, popup_id, url, disposition, features, window, client);
            i32::from(!allowed)
        }

        fn on_before_popup_aborted(&self, _browser: Option<&mut Browser>, popup_id: i32) {
            popup::aborted(&self.slot, popup_id);
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
    }
}

wrap_load_handler! {
    struct Load { slot: Rc<Slot> }
    impl LoadHandler {
        fn on_load_start(
            &self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType,
        ) {
            let Some(frame) = frame.filter(|frame| frame.is_main() != 0) else { return };
            let services = services::of(&self.slot);
            // The page this tab's permission requests came from is gone.
            if let Some(identifier) = self.slot.identifier.get() {
                permissions::leave(&services, &permissions::queue_id(identifier));
            }
            permissions::forget_one_time(&self.slot);
            if !self.slot.retired.get() {
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

wrap_client! {
    pub(crate) struct OsrClient {
        render: RenderHandler,
        life: LifeSpanHandler,
        display: DisplayHandler,
        load: LoadHandler,
        request: RequestHandler,
        download: DownloadHandler,
        permission: PermissionHandler,
    }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.render.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.life.clone()) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(self.display.clone()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(self.load.clone()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(self.request.clone()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(self.download.clone()) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(self.permission.clone()) }
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
        Display::new(slot.clone()),
        Load::new(slot.clone()),
        adblock::handler(io),
        downloads::handler(slot.clone()),
        permissions::handler(slot),
    )
}
