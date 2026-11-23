use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use cef::{rc::Rc as _, *};
use flecha_surface::{Frame as SurfaceFrame, SurfaceTarget};
use gpui::{Bounds, Pixels, point, px, size};

use crate::{BrowserEvent, BrowserKey, Event, Viewport};

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

pub(crate) struct Slot {
    pub key: RefCell<BrowserKey>,
    pub browser: RefCell<Option<Browser>>,
    pub viewport: Cell<Viewport>,
    pub surface: RefCell<Frames>,
    pub retired: Cell<bool>,
    pub paint_pending: Cell<bool>,
    pub events: Queue,
}

impl Slot {
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
            if let Some(browser) = browser {
                self.slot.emit(Event::Created(browser.identifier()));
                self.slot.browser.replace(Some(browser.clone()));
            }
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.slot.browser.take();
            self.slot.emit(Event::Closed);
        }

        fn on_before_popup(
            &self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id: i32,
            url: Option<&CefString>, _frame_name: Option<&CefString>, disposition: WindowOpenDisposition,
            gesture: i32, _features: Option<&PopupFeatures>, _window: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, _no_javascript: Option<&mut i32>,
        ) -> i32 {
            if gesture != 0
                && matches!(disposition, WindowOpenDisposition::NEW_FOREGROUND_TAB | WindowOpenDisposition::NEW_BACKGROUND_TAB)
                && let Some(url) = url
            {
                self.slot.emit(Event::OpenLink {
                    url: url.to_string(),
                    foreground: disposition == WindowOpenDisposition::NEW_FOREGROUND_TAB,
                });
            }
            // Phase 1 routes user-initiated tab links through the shell. The
            // product popup lifecycle is a separate rewrite phase.
            1
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
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, status: i32) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() != 0) {
                self.slot.emit(Event::Loaded { url: CefString::from(&frame.url()).to_string(), status });
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
    }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.render.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.life.clone()) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(self.display.clone()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(self.load.clone()) }
    }
}

pub(crate) fn client(slot: Rc<Slot>) -> Client {
    OsrClient::new(
        Osr::new(slot.clone()),
        Life::new(slot.clone()),
        Display::new(slot.clone()),
        Load::new(slot),
    )
}
