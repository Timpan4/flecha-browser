//! CEF side: the app, the browser process handler and per-browser clients.
//!
//! Every callback here runs inside `CefDoMessageLoopWork()` on GPUI's main
//! thread, except `OnScheduleMessagePumpWork`, which can arrive on any thread
//! and only forwards to `pump::request_from_cef`. Callbacks write to `Hub` or
//! send on channels; they never touch GPUI state directly.

use crate::pump;
use cef::*;
use futures::channel::mpsc;
use std::cell::{Cell, RefCell};
use std::rc::Rc as StdRc;
use std::sync::OnceLock;

static CONTEXT_READY_TX: OnceLock<mpsc::UnboundedSender<()>> = OnceLock::new();

/// Must run before `CefInitialize`.
pub fn install_context_ready() -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded();
    CONTEXT_READY_TX.set(tx).expect("installed twice");
    rx
}

wrap_app! {
    pub struct FlechaApp;

    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(PumpBrowserProcessHandler::new())
        }
    }
}

pub fn build_app() -> App {
    FlechaApp::new()
}

wrap_browser_process_handler! {
    struct PumpBrowserProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            if let Some(tx) = CONTEXT_READY_TX.get() {
                let _ = tx.unbounded_send(());
            }
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            pump::request_from_cef(delay_ms);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Off-screen page browser with this index. Index 0 is drawn by GPUI.
    Page(usize),
    DevTools,
}

/// A browser between `OnAfterCreated` and `OnBeforeClose`.
pub struct Tracked {
    pub kind: Kind,
    pub browser: Browser,
}

/// Latest BGRA frame from `OnPaint` for the drawn browser.
pub struct PaintedFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// State shared between CEF callbacks and the GPUI side, main thread only.
pub struct Hub {
    pub view_width: i32,
    pub view_height: i32,
    pub scale_factor: Cell<f32>,
    pub frame: RefCell<Option<PaintedFrame>>,
    /// `OnPaint` frames replaced in `frame` before the view took them, so
    /// never uploaded to GPUI.
    pub frames_overwritten: Cell<u64>,
    pub frame_tx: mpsc::UnboundedSender<()>,
    /// `OnPaint` calls per page index.
    pub paints: RefCell<Vec<u64>>,
    pub loaded: RefCell<Vec<bool>>,
    pub tracked: RefCell<Vec<Tracked>>,
    pub lifecycle_tx: mpsc::UnboundedSender<()>,
    pub render_process_terminations: RefCell<Vec<String>>,
    /// Time and dirty rects (x, y, w, h) of every OnPaint for page 0.
    pub paint_log: RefCell<Vec<(std::time::Instant, Vec<[i32; 4]>)>>,
}

impl Hub {
    pub fn paints(&self, index: usize) -> u64 {
        self.paints.borrow().get(index).copied().unwrap_or(0)
    }

    pub fn page(&self, index: usize) -> Option<Browser> {
        self.tracked
            .borrow()
            .iter()
            .find(|t| t.kind == Kind::Page(index))
            .map(|t| t.browser.clone())
    }

    pub fn devtools_open(&self) -> bool {
        self.tracked
            .borrow()
            .iter()
            .any(|t| t.kind == Kind::DevTools)
    }

    pub fn open_count(&self) -> usize {
        self.tracked.borrow().len()
    }

    fn ping(&self) {
        let _ = self.lifecycle_tx.unbounded_send(());
    }
}

wrap_client! {
    struct PageClient {
        render: RenderHandler,
        life_span: LifeSpanHandler,
        load: LoadHandler,
        display: DisplayHandler,
        request: RequestHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }
    }
}

wrap_client! {
    struct DevToolsClient {
        life_span: LifeSpanHandler,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }
    }
}

pub fn page_client(hub: &StdRc<Hub>, index: usize) -> Client {
    {
        let mut paints = hub.paints.borrow_mut();
        let mut loaded = hub.loaded.borrow_mut();
        if paints.len() <= index {
            paints.resize(index + 1, 0);
            loaded.resize(index + 1, false);
        }
    }
    PageClient::new(
        OsrRender::new(hub.clone(), index),
        Lifecycle::new(hub.clone(), Kind::Page(index)),
        Loading::new(hub.clone(), index),
        Console::new(index),
        Requests::new(hub.clone(), index),
    )
}

pub fn devtools_client(hub: &StdRc<Hub>) -> Client {
    DevToolsClient::new(Lifecycle::new(hub.clone(), Kind::DevTools))
}

wrap_render_handler! {
    struct OsrRender {
        hub: StdRc<Hub>,
        index: usize,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                rect.x = 0;
                rect.y = 0;
                rect.width = self.hub.view_width;
                rect.height = self.hub.view_height;
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else { return 0 };
            info.device_scale_factor = self.hub.scale_factor.get();
            1
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if type_ != PaintElementType::VIEW || buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            if let Some(n) = self.hub.paints.borrow_mut().get_mut(self.index) {
                *n += 1;
            }
            if self.index != 0 {
                return;
            }
            let rects = dirty_rects
                .unwrap_or_default()
                .iter()
                .map(|r| [r.x, r.y, r.width, r.height])
                .collect();
            self.hub.paint_log.borrow_mut().push((std::time::Instant::now(), rects));
            let len = width as usize * height as usize * 4;
            // SAFETY: CEF passes a BGRA buffer of width * height * 4 bytes that
            // stays valid for the duration of this callback.
            let bgra = unsafe { std::slice::from_raw_parts(buffer, len) }.to_vec();
            let replaced = self.hub.frame.borrow_mut().replace(PaintedFrame {
                width: width as u32,
                height: height as u32,
                bgra,
            });
            if replaced.is_some() {
                let n = &self.hub.frames_overwritten;
                n.set(n.get() + 1);
            }
            let _ = self.hub.frame_tx.unbounded_send(());
        }
    }
}

wrap_life_span_handler! {
    struct Lifecycle {
        hub: StdRc<Hub>,
        kind: Kind,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let style = browser.host().map(|h| h.runtime_style());
            eprintln!(
                "[engine] created {:?} id={} runtime_style={:?}",
                self.kind,
                browser.identifier(),
                style
            );
            self.hub.tracked.borrow_mut().push(Tracked {
                kind: self.kind,
                browser: browser.clone(),
            });
            self.hub.ping();
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let id = browser.identifier();
            eprintln!("[engine] before_close {:?} id={id}", self.kind);
            // Drop the handle after the borrow ends: releasing a CEF reference
            // must not run while `tracked` is borrowed.
            let removed: Vec<Tracked> = {
                let mut tracked = self.hub.tracked.borrow_mut();
                let (gone, kept): (Vec<Tracked>, Vec<Tracked>) =
                    tracked.drain(..).partition(|t| t.browser.identifier() == id);
                *tracked = kept;
                gone
            };
            drop(removed);
            self.hub.ping();
        }
    }
}

wrap_load_handler! {
    struct Loading {
        hub: StdRc<Hub>,
        index: usize,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            _can_go_back: ::std::os::raw::c_int,
            _can_go_forward: ::std::os::raw::c_int,
        ) {
            if let Some(l) = self.hub.loaded.borrow_mut().get_mut(self.index) {
                *l = is_loading == 0;
            }
            self.hub.ping();
        }
    }
}

wrap_display_handler! {
    struct Console {
        index: usize,
    }

    impl DisplayHandler {
        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            if let Some(message) = message {
                eprintln!("[page {}] {}", self.index, message);
            }
            0
        }
    }
}

wrap_request_handler! {
    struct Requests {
        hub: StdRc<Hub>,
        index: usize,
    }

    impl RequestHandler {
        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            status: TerminationStatus,
            error_code: ::std::os::raw::c_int,
            error_string: Option<&CefString>,
        ) {
            let line = format!(
                "page {} status={:?} code={} {}",
                self.index,
                status,
                error_code,
                error_string.map(|s| s.to_string()).unwrap_or_default()
            );
            eprintln!("[engine] render process terminated: {line}");
            self.hub.render_process_terminations.borrow_mut().push(line);
            self.hub.ping();
        }
    }
}

/// Creates an off-screen (windowless, so Alloy-style) page browser.
pub fn create_page(hub: &StdRc<Hub>, index: usize, url: &str) -> Option<Browser> {
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        ..Default::default()
    };
    let mut client = page_client(hub, index);
    browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&BrowserSettings::default()),
        None,
        None,
    )
}

/// Opens Chrome-style DevTools for `browser` in its own native window.
pub fn show_devtools(hub: &StdRc<Hub>, browser: &Browser) -> bool {
    let Some(host) = browser.host() else {
        return false;
    };
    let mut client = devtools_client(hub);
    host.show_dev_tools(
        Some(&WindowInfo::default()),
        Some(&mut client),
        Some(&BrowserSettings::default()),
        None,
    );
    true
}
