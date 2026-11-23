//! `--mode osr`: Alloy-style windowless browsers. CEF paints BGRA frames through `OnPaint`; each
//! browser (and each `window.open` popup) gets its own winit window drawn with softbuffer. Mouse,
//! wheel and keyboard input go to CEF through `BrowserHost::send_*_event`. The CEF message pump
//! runs on the winit main thread through CEF's external-pump contract.

use crate::{
    common::{self, CursorSink, SpikeDisplayHandler, SpikeLoadHandler},
    keys,
};
use cef::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    io::BufRead,
    num::NonZeroU32,
    path::PathBuf,
    rc::Rc,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent as WinitKeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::KeyLocation,
    platform::{
        modifier_supplement::KeyEventExtModifierSupplement, scancode::PhysicalKeyExtScancode,
    },
    window::{Cursor, CursorIcon, Window, WindowId},
};

/// Longest gap between pump calls. cef-rs examples/tests_shared main_message_loop_external_pump
/// (`MAX_TIMER_DELAY = 1000 / 30`), ported from cefclient's kMaxTimerDelay.
const MAX_TIMER_DELAY_MS: i64 = 1000 / 30;
/// Pixels per wheel line. cefclient browser_window_osr_gtk.cc, `scrollbarPixelsPerGtkTick`.
const WHEEL_PIXELS_PER_LINE: f64 = 40.0;
/// GTK's defaults for gtk-double-click-time (ms) and gtk-double-click-distance (px).
const DOUBLE_CLICK_MS: u128 = 400;
const DOUBLE_CLICK_DISTANCE: f64 = 5.0;
/// cefsimple's window size, as in chrome mode.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(800.0, 600.0);

// EVENTFLAG_CAPS_LOCK_ON (1) is not sent: winit 0.30 does not report lock-key state, and
// guessing it from Caps Lock presses would be wrong whenever it was on before the window opened.
const FLAG_SHIFT: u32 = 2;
const FLAG_CONTROL: u32 = 4;
const FLAG_ALT: u32 = 8;
const FLAG_LEFT_BUTTON: u32 = 16;
const FLAG_MIDDLE_BUTTON: u32 = 32;
const FLAG_RIGHT_BUTTON: u32 = 64;
const FLAG_COMMAND: u32 = 128;
const FLAG_KEYPAD: u32 = 512;
const FLAG_IS_LEFT: u32 = 1024;
const FLAG_IS_RIGHT: u32 = 2048;
const FLAG_REPEAT: u32 = 8192;

#[derive(Debug)]
pub enum UserEvent {
    Wake,
}

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();
static NEXT_PUMP: Mutex<Option<Instant>> = Mutex::new(None);
static CONTEXT_READY: AtomicBool = AtomicBool::new(false);

fn wake() {
    if let Some(proxy) = PROXY.get() {
        let _ = proxy
            .lock()
            .expect("proxy lock")
            .send_event(UserEvent::Wake);
    }
}

/// `OnScheduleMessagePumpWork`. May be called on any thread.
pub fn schedule_pump(delay_ms: i64) {
    let at = Instant::now() + Duration::from_millis(delay_ms.clamp(0, MAX_TIMER_DELAY_MS) as u64);
    {
        let mut next = NEXT_PUMP.lock().expect("pump lock");
        if next.is_none_or(|n| at < n) {
            *next = Some(at);
        }
    }
    wake();
}

pub fn context_ready() {
    CONTEXT_READY.store(true, Ordering::SeqCst);
    wake();
}

pub fn create_event_loop() -> EventLoop<UserEvent> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    event_loop
}

pub struct View {
    browser: Option<Browser>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// Window size in physical pixels.
    size: PhysicalSize<u32>,
    scale: f64,
    frame: Vec<u32>,
    frame_size: (u32, u32),
    popup_rect: Option<Rect>,
    popup: Vec<u32>,
    popup_size: (u32, u32),
    cursor: (f64, f64),
    buttons: u32,
    modifiers: u32,
    last_click: Option<(Instant, MouseButton, (f64, f64), i32)>,
    is_main: bool,
    /// Its window could not be created; its browser is closed instead of shown.
    failed: bool,
}

type ViewRef = Rc<RefCell<View>>;

impl View {
    fn new(size: PhysicalSize<u32>, scale: f64, is_main: bool) -> ViewRef {
        Rc::new(RefCell::new(View {
            browser: None,
            window: None,
            surface: None,
            size,
            scale,
            frame: Vec::new(),
            frame_size: (0, 0),
            popup_rect: None,
            popup: Vec::new(),
            popup_size: (0, 0),
            cursor: (0.0, 0.0),
            buttons: 0,
            modifiers: 0,
            last_click: None,
            is_main,
            failed: false,
        }))
    }

    fn logical_size(&self) -> (i32, i32) {
        let w = (self.size.width as f64 / self.scale).round() as i32;
        let h = (self.size.height as f64 / self.scale).round() as i32;
        (w.max(1), h.max(1))
    }
}

thread_local! {
    static VIEWS: RefCell<Vec<ViewRef>> = const { RefCell::new(Vec::new()) };
}

fn host_of(view: &ViewRef) -> Option<BrowserHost> {
    let browser = view.borrow().browser.clone();
    browser.and_then(|b| b.host())
}

fn request_redraw(view: &ViewRef) {
    let window = view.borrow().window.clone();
    if let Some(window) = window {
        window.request_redraw();
    }
}

fn redraw(view: &ViewRef) {
    let mut v = view.borrow_mut();
    let View {
        surface,
        size,
        scale,
        frame,
        frame_size,
        popup_rect,
        popup,
        popup_size,
        ..
    } = &mut *v;
    let (Some(surface), Some(w), Some(h)) = (
        surface.as_mut(),
        NonZeroU32::new(size.width),
        NonZeroU32::new(size.height),
    ) else {
        return;
    };
    if surface.resize(w, h).is_err() {
        return;
    }
    let Ok(mut buffer) = surface.buffer_mut() else {
        return;
    };
    let (w, h) = (size.width as usize, size.height as usize);
    let (fw, fh) = (frame_size.0 as usize, frame_size.1 as usize);
    for y in 0..h {
        let row = &mut buffer[y * w..(y + 1) * w];
        if y < fh {
            let n = w.min(fw);
            row[..n].copy_from_slice(&frame[y * fw..y * fw + n]);
            row[n..].fill(0x00FF_FFFF);
        } else {
            row.fill(0x00FF_FFFF);
        }
    }
    if let Some(rect) = popup_rect {
        let (pw, ph) = (popup_size.0 as usize, popup_size.1 as usize);
        let x0 = (rect.x as f64 * *scale).round() as i64;
        let y0 = (rect.y as f64 * *scale).round() as i64;
        for py in 0..ph {
            let y = y0 + py as i64;
            if y < 0 || y >= h as i64 {
                continue;
            }
            for px in 0..pw {
                let x = x0 + px as i64;
                if x >= 0 && x < w as i64 {
                    buffer[y as usize * w + x as usize] = popup[py * pw + px];
                }
            }
        }
    }
    let _ = buffer.present();
}

wrap_render_handler! {
    struct OsrRenderHandler {
        view: ViewRef,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (w, h) = self.view.borrow().logical_size();
                *rect = Rect { x: 0, y: 0, width: w, height: h };
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else { return 0 };
            let v = self.view.borrow();
            info.device_scale_factor = v.scale as f32;
            if let Some(monitor) = v.window.as_ref().and_then(|w| w.current_monitor()) {
                let pos = monitor.position();
                let size = monitor.size();
                let rect = Rect {
                    x: (pos.x as f64 / v.scale).round() as i32,
                    y: (pos.y as f64 / v.scale).round() as i32,
                    width: (size.width as f64 / v.scale).round() as i32,
                    height: (size.height as f64 / v.scale).round() as i32,
                };
                info.rect = rect.clone();
                info.available_rect = rect;
            }
            1
        }

        fn screen_point(
            &self,
            _browser: Option<&mut Browser>,
            view_x: ::std::os::raw::c_int,
            view_y: ::std::os::raw::c_int,
            screen_x: Option<&mut ::std::os::raw::c_int>,
            screen_y: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let v = self.view.borrow();
            let Some(origin) = v.window.as_ref().and_then(|w| w.inner_position().ok()) else {
                return 0;
            };
            if let (Some(sx), Some(sy)) = (screen_x, screen_y) {
                *sx = origin.x + (view_x as f64 * v.scale).round() as i32;
                *sy = origin.y + (view_y as f64 * v.scale).round() as i32;
                return 1;
            }
            0
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            if show == 0 {
                {
                    let mut v = self.view.borrow_mut();
                    v.popup_rect = None;
                    v.popup.clear();
                    v.popup_size = (0, 0);
                }
                request_redraw(&self.view);
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(rect) = rect {
                self.view.borrow_mut().popup_rect = Some(rect.clone());
            }
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let Some(bytes) = (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .filter(|&bytes| bytes <= isize::MAX as usize)
            else {
                return;
            };
            // SAFETY: CEF passes a BGRA buffer of width * height pixels, valid for this call;
            // `bytes` is that size and fits the slice length limit.
            let src = unsafe { std::slice::from_raw_parts(buffer, bytes) };
            {
                let mut v = self.view.borrow_mut();
                let (target, size) = if type_ == PaintElementType::POPUP {
                    let v = &mut *v;
                    (&mut v.popup, &mut v.popup_size)
                } else {
                    let v = &mut *v;
                    (&mut v.frame, &mut v.frame_size)
                };
                target.clear();
                target.extend(src.as_chunks::<4>().0.iter().map(|p| u32::from_le_bytes([p[0], p[1], p[2], 0])));
                *size = (width as u32, height as u32);
            }
            request_redraw(&self.view);
        }
    }
}

wrap_life_span_handler! {
    struct OsrLifeSpanHandler {
        view: ViewRef,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // Every popup, including window.open with an opener, becomes its own windowless
            // browser in its own window. The opener relationship is kept by CEF.
            let (parent_size, scale) = {
                let v = self.view.borrow();
                (v.size, v.scale)
            };
            let mut size = parent_size;
            if let Some(f) = popup_features {
                if f.width_set != 0 && f.width > 0 {
                    size.width = (f.width as f64 * scale).round() as u32;
                }
                if f.height_set != 0 && f.height > 0 {
                    size.height = (f.height as f64 * scale).round() as u32;
                }
            }
            let url = target_url.map(|u| u.to_string()).unwrap_or_default();
            eprintln!("[popup] {}", common::display_url(&url));
            if common::FAKE_CAMERA.get().copied().unwrap_or(false) && !common::is_local_url(&url) {
                eprintln!("[blocked] popup to {} (--fake-camera)", common::display_url(&url));
                return 1;
            }
            let view = View::new(size, scale, false);
            VIEWS.with_borrow_mut(|views| views.push(view.clone()));
            if let Some(info) = window_info {
                *info = windowless_info();
            }
            if let Some(client) = client {
                *client = Some(make_client(&view));
            }
            if let Some(settings) = settings {
                settings.background_color = common::BACKGROUND;
            }
            wake();
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let failed = {
                let mut v = self.view.borrow_mut();
                v.browser = Some(browser.clone());
                v.failed
            };
            common::browser_created(browser);
            if let Some(host) = browser.host() {
                if failed {
                    host.close_browser(1);
                    return;
                }
                host.was_resized();
                host.set_focus(1);
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            {
                let mut v = self.view.borrow_mut();
                v.browser = None;
                v.surface = None;
                v.window = None;
            }
            let id = Rc::as_ptr(&self.view);
            VIEWS.with_borrow_mut(|views| views.retain(|v| Rc::as_ptr(v) != id));
            if let Some(browser) = browser {
                drop_pending(browser.identifier());
                common::browser_closed(browser);
            }
            wake();
        }
    }
}

enum Request {
    Media(MediaAccessCallback, u32),
    Prompt(PermissionPromptCallback, u64),
}

/// A request waiting for `allow`/`deny`, with the browser that asked, so closing that browser
/// drops its requests.
struct Pending {
    browser: i32,
    request: Request,
}

thread_local! {
    static PENDING: RefCell<HashMap<u32, Pending>> = RefCell::new(HashMap::new());
    static NEXT_PENDING: RefCell<u32> = const { RefCell::new(1) };
}

fn add_pending(browser: Option<&Browser>, request: Request, origin: &str, what: String) {
    let id = NEXT_PENDING.with_borrow_mut(|n| {
        let id = *n;
        *n += 1;
        id
    });
    let browser = browser.map_or(0, |b| b.identifier());
    PENDING.with_borrow_mut(|p| p.insert(id, Pending { browser, request }));
    let origin = common::display_url(origin);
    eprintln!(
        "[permission {id}] {origin} requests {what}. Type `allow {id}` or `deny {id}` and press Enter."
    );
}

/// Cancels the media requests of a closing browser. CEF dismisses its permission prompts
/// itself (`on_dismiss_permission_prompt`).
fn drop_pending(browser: i32) {
    let dropped: Vec<Pending> = PENDING.with_borrow_mut(|p| {
        let ids: Vec<u32> = p
            .iter()
            .filter(|(_, v)| v.browser == browser)
            .map(|(id, _)| *id)
            .collect();
        ids.iter().filter_map(|id| p.remove(id)).collect()
    });
    for pending in dropped {
        if let Request::Media(callback, _) = pending.request {
            callback.cancel();
        }
    }
}

fn media_names(mask: u32) -> String {
    let names: Vec<&str> = [
        (1, "microphone"),
        (2, "camera"),
        (4, "desktop audio"),
        (8, "desktop video"),
    ]
    .iter()
    .filter(|(bit, _)| mask & bit != 0)
    .map(|(_, name)| *name)
    .collect();
    format!("media access ({}; mask {mask:#x})", names.join(", "))
}

wrap_permission_handler! {
    struct OsrPermissionHandler;

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 0 };
            let origin = requesting_origin.map(|o| o.to_string()).unwrap_or_default();
            add_pending(
                browser.as_deref(),
                Request::Media(callback.clone(), requested_permissions),
                &origin,
                media_names(requested_permissions),
            );
            1
        }

        fn on_show_permission_prompt(
            &self,
            browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else { return 0 };
            let origin = requesting_origin.map(|o| o.to_string()).unwrap_or_default();
            add_pending(
                browser.as_deref(),
                Request::Prompt(callback.clone(), prompt_id),
                &origin,
                format!("permission types {requested_permissions:#x} (cef_permission_request_types_t)"),
            );
            1
        }

        fn on_dismiss_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            _result: PermissionRequestResult,
        ) {
            PENDING.with_borrow_mut(|p| {
                p.retain(|_, v| !matches!(v.request, Request::Prompt(_, id) if id == prompt_id))
            });
        }
    }
}

fn answer_permission(id: u32, allow: bool) {
    match PENDING
        .with_borrow_mut(|p| p.remove(&id))
        .map(|p| p.request)
    {
        Some(Request::Media(callback, mask)) => {
            if allow {
                callback.cont(mask);
            } else {
                callback.cancel();
            }
        }
        Some(Request::Prompt(callback, _)) => callback.cont(if allow {
            PermissionRequestResult::ACCEPT
        } else {
            PermissionRequestResult::DENY
        }),
        None => eprintln!("no pending permission request {id}"),
    }
}

wrap_pdf_print_callback! {
    struct PdfDone;

    impl PdfPrintCallback {
        fn on_pdf_print_finished(&self, path: Option<&CefString>, ok: ::std::os::raw::c_int) {
            let path = path.map(|p| p.to_string()).unwrap_or_default();
            eprintln!("[pdf] ok={} {path}", ok != 0);
        }
    }
}

// Without a print handler CEF does not support printing on Linux (include/cef_client.h,
// GetPrintHandler), so Ctrl+P and `print` would do nothing visible. The spike has no print
// dialog: this handler logs that printing started (OnPrintStart runs however printing was
// requested) and refuses the rest. It supplies no printer settings, so Chromium then logs that
// no printer is available. `pdf` covers printed output.
wrap_print_handler! {
    struct SpikePrintHandler;

    impl PrintHandler {
        fn on_print_start(&self, _browser: Option<&mut Browser>) {
            eprintln!("[print] start");
        }

        fn on_print_dialog(
            &self,
            _browser: Option<&mut Browser>,
            _has_selection: ::std::os::raw::c_int,
            callback: Option<&mut PrintDialogCallback>,
        ) -> ::std::os::raw::c_int {
            eprintln!("[print] dialog requested; cancelled (no print dialog in the spike)");
            if let Some(callback) = callback {
                callback.cancel();
            }
            1
        }

        fn on_print_job(
            &self,
            _browser: Option<&mut Browser>,
            _document_name: Option<&CefString>,
            _pdf_file_path: Option<&CefString>,
            _callback: Option<&mut PrintJobCallback>,
        ) -> ::std::os::raw::c_int {
            eprintln!("[print] job refused");
            0
        }
    }
}

wrap_client! {
    struct OsrClient {
        render: RenderHandler,
        display: DisplayHandler,
        life_span: LifeSpanHandler,
        permission: PermissionHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.permission.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(SpikeLoadHandler::new())
        }

        fn print_handler(&self) -> Option<PrintHandler> {
            Some(SpikePrintHandler::new())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            common::request_handler()
        }
    }
}

fn cursor_icon(cursor: CursorType) -> Option<CursorIcon> {
    let icon = match cursor {
        c if c == CursorType::HAND => CursorIcon::Pointer,
        c if c == CursorType::IBEAM => CursorIcon::Text,
        c if c == CursorType::VERTICALTEXT => CursorIcon::VerticalText,
        c if c == CursorType::WAIT => CursorIcon::Wait,
        c if c == CursorType::PROGRESS => CursorIcon::Progress,
        c if c == CursorType::HELP => CursorIcon::Help,
        c if c == CursorType::CROSS => CursorIcon::Crosshair,
        c if c == CursorType::MOVE => CursorIcon::Move,
        c if c == CursorType::NOTALLOWED || c == CursorType::NODROP => CursorIcon::NotAllowed,
        c if c == CursorType::GRAB => CursorIcon::Grab,
        c if c == CursorType::GRABBING => CursorIcon::Grabbing,
        c if c == CursorType::EASTWESTRESIZE || c == CursorType::COLUMNRESIZE => {
            CursorIcon::EwResize
        }
        c if c == CursorType::NORTHSOUTHRESIZE || c == CursorType::ROWRESIZE => {
            CursorIcon::NsResize
        }
        c if c == CursorType::CONTEXTMENU => CursorIcon::ContextMenu,
        c if c == CursorType::CELL => CursorIcon::Cell,
        c if c == CursorType::COPY => CursorIcon::Copy,
        c if c == CursorType::ALIAS => CursorIcon::Alias,
        c if c == CursorType::ZOOMIN => CursorIcon::ZoomIn,
        c if c == CursorType::ZOOMOUT => CursorIcon::ZoomOut,
        c if c == CursorType::NONE => return None,
        _ => CursorIcon::Default,
    };
    Some(icon)
}

fn make_client(view: &ViewRef) -> Client {
    let weak = Rc::downgrade(view);
    let sink: CursorSink = Rc::new(move |cursor| {
        let Some(view) = weak.upgrade() else { return };
        let window = view.borrow().window.clone();
        if let Some(window) = window {
            match cursor_icon(cursor) {
                Some(icon) => {
                    window.set_cursor_visible(true);
                    window.set_cursor(Cursor::Icon(icon));
                }
                None => window.set_cursor_visible(false),
            }
        }
    });
    OsrClient::new(
        OsrRenderHandler::new(view.clone()),
        SpikeDisplayHandler::new(Some(sink)),
        OsrLifeSpanHandler::new(view.clone()),
        OsrPermissionHandler::new(),
    )
}

fn windowless_info() -> WindowInfo {
    WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: 0,
        external_begin_frame_enabled: 0,
        // Off-screen browsers are always Alloy style; say so explicitly.
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    }
}

struct OsrApp {
    url: String,
    profile: PathBuf,
    windows: HashMap<WindowId, ViewRef>,
    main: Option<ViewRef>,
    browser_requested: bool,
    focused: Option<ViewRef>,
    commands: mpsc::Receiver<String>,
}

impl OsrApp {
    fn create_pending_windows(&mut self, event_loop: &ActiveEventLoop) {
        let pending: Vec<ViewRef> = VIEWS.with_borrow(|views| {
            views
                .iter()
                .filter(|v| v.borrow().window.is_none() && !v.borrow().failed)
                .cloned()
                .collect()
        });
        // A main window that cannot be created ends the run; a popup's browser is closed.
        let fail = |view: &ViewRef, message: String| {
            let (is_main, browser) = {
                let mut v = view.borrow_mut();
                v.failed = true;
                (v.is_main, v.browser.clone())
            };
            if is_main {
                common::fail_startup(&message);
            } else {
                eprintln!("{message}; closing the popup");
                if let Some(host) = browser.and_then(|b| b.host()) {
                    host.close_browser(1);
                }
            }
        };
        for view in pending {
            let (size, scale, is_main) = {
                let v = view.borrow();
                (v.size, v.scale, v.is_main)
            };
            let title = if is_main {
                "Flecha spike: osr"
            } else {
                "Flecha spike: osr popup"
            };
            let mut attrs = Window::default_attributes().with_title(title);
            attrs = if size.width == 0 || size.height == 0 {
                attrs.with_inner_size(INITIAL_SIZE)
            } else {
                attrs.with_inner_size(size.to_logical::<f64>(scale))
            };
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Rc::new(w),
                Err(e) => {
                    fail(&view, format!("cannot create window: {e}"));
                    continue;
                }
            };
            let surface = softbuffer::Context::new(window.clone())
                .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
            let surface = match surface {
                Ok(s) => s,
                Err(e) => {
                    fail(&view, format!("cannot create softbuffer surface: {e}"));
                    continue;
                }
            };
            {
                let mut v = view.borrow_mut();
                v.scale = window.scale_factor();
                v.size = window.inner_size();
                v.surface = Some(surface);
                v.window = Some(window.clone());
            }
            self.windows.insert(window.id(), view.clone());
            if let Some(host) = host_of(&view) {
                host.notify_screen_info_changed();
                host.was_resized();
            }
            window.request_redraw();
        }
    }

    fn create_main_browser(&mut self) {
        if self.browser_requested || !CONTEXT_READY.load(Ordering::SeqCst) {
            return;
        }
        let Some(view) = self.main.clone() else {
            return;
        };
        if view.borrow().window.is_none() {
            return;
        }
        self.browser_requested = true;
        let mut client = make_client(&view);
        let created = browser_host_create_browser(
            Some(&windowless_info()),
            Some(&mut client),
            Some(&CefString::from(self.url.as_str())),
            Some(&common::browser_settings()),
            None,
            None,
        );
        if created != 1 {
            common::fail_startup("browser_host_create_browser failed");
        }
    }

    fn target(&self) -> Option<ViewRef> {
        self.focused
            .as_ref()
            .filter(|v| v.borrow().browser.is_some())
            .cloned()
            .or_else(|| self.main.clone())
    }

    fn command(&mut self, line: &str) {
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { return };
        let arg = parts.next();
        let view = self.target();
        let browser = view.as_ref().and_then(|v| v.borrow().browser.clone());
        let host = browser.as_ref().and_then(|b| b.host());
        match (cmd, arg) {
            ("go", Some(url)) => match common::check_typed_url(url) {
                Ok(()) => {
                    if let Some(frame) = browser.as_ref().and_then(|b| b.main_frame()) {
                        frame.load_url(Some(&CefString::from(url)));
                    }
                }
                Err(why) => eprintln!("[go] refused: {why}"),
            },
            ("back", _) => browser.iter().for_each(|b| b.go_back()),
            ("forward", _) => browser.iter().for_each(|b| b.go_forward()),
            ("reload", _) => browser.iter().for_each(|b| b.reload()),
            ("print", _) => host.iter().for_each(|h| h.print()),
            ("pdf", path) => {
                let path = path
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.profile.join("flecha-spike-print.pdf"));
                if let Some(host) = host {
                    host.print_to_pdf(
                        Some(&CefString::from(path.to_string_lossy().as_ref())),
                        Some(&PdfPrintSettings::default()),
                        Some(&mut PdfDone::new()),
                    );
                }
            }
            ("allow", Some(id)) | ("deny", Some(id)) => match id.parse() {
                Ok(id) => answer_permission(id, cmd == "allow"),
                Err(_) => eprintln!("not a request id: {id}"),
            },
            ("quit", _) => common::close_all(),
            _ => eprintln!("{COMMAND_HELP}"),
        }
    }

    fn mouse_event(view: &ViewRef) -> MouseEvent {
        let v = view.borrow();
        MouseEvent {
            x: v.cursor.0.round() as i32,
            y: v.cursor.1.round() as i32,
            modifiers: v.modifiers | v.buttons,
        }
    }

    fn mouse_button(view: &ViewRef, state: ElementState, button: MouseButton) {
        let (kind, flag) = match button {
            MouseButton::Left => (MouseButtonType::LEFT, FLAG_LEFT_BUTTON),
            MouseButton::Middle => (MouseButtonType::MIDDLE, FLAG_MIDDLE_BUTTON),
            MouseButton::Right => (MouseButtonType::RIGHT, FLAG_RIGHT_BUTTON),
            MouseButton::Back => (MouseButtonType::BACK, 0),
            MouseButton::Forward => (MouseButtonType::FORWARD, 0),
            MouseButton::Other(_) => return,
        };
        let Some(host) = host_of(view) else { return };
        let pressed = state == ElementState::Pressed;
        let count = {
            let mut v = view.borrow_mut();
            if pressed {
                v.buttons |= flag;
                let now = Instant::now();
                let pos = v.cursor;
                let count = match v.last_click {
                    Some((at, b, p, n))
                        if b == button
                            && now.duration_since(at).as_millis() <= DOUBLE_CLICK_MS
                            && (p.0 - pos.0).abs() <= DOUBLE_CLICK_DISTANCE
                            && (p.1 - pos.1).abs() <= DOUBLE_CLICK_DISTANCE =>
                    {
                        n + 1
                    }
                    _ => 1,
                };
                v.last_click = Some((now, button, pos, count));
                count
            } else {
                v.last_click.map(|c| c.3).unwrap_or(1)
            }
        };
        let event = Self::mouse_event(view);
        if pressed {
            host.set_focus(1);
        }
        host.send_mouse_click_event(Some(&event), kind, (!pressed) as i32, count);
        if !pressed {
            view.borrow_mut().buttons &= !flag;
        }
    }

    fn key(view: &ViewRef, event: &WinitKeyEvent) {
        let Some(host) = host_of(view) else { return };
        let modifiers = view.borrow().modifiers;
        let pressed = event.state == ElementState::Pressed;

        let vk =
            keys::windows_key_code(event.physical_key, &event.key_without_modifiers()).unwrap_or(0);
        if pressed
            && modifiers & (FLAG_CONTROL | FLAG_SHIFT | FLAG_ALT) == FLAG_CONTROL
            && vk == 0x50
        {
            // Alloy has no browser accelerators. Ctrl+P (VK_P) prints, as in a browser.
            if !event.repeat {
                host.print();
            }
            return;
        }

        // winit reports evdev scancodes; X11 keycodes are evdev + 8.
        let native = event
            .physical_key
            .to_scancode()
            .map(|s| s as i32 + 8)
            .unwrap_or(0);
        let mut flags = modifiers;
        if keys::is_keypad(event.physical_key) {
            flags |= FLAG_KEYPAD;
        }
        if event.repeat {
            flags |= FLAG_REPEAT;
        }
        match event.location {
            KeyLocation::Left => flags |= FLAG_IS_LEFT,
            KeyLocation::Right => flags |= FLAG_IS_RIGHT,
            _ => {}
        }
        let mut units: Vec<u16> = event
            .text
            .as_ref()
            .map(|t| t.encode_utf16().collect())
            .unwrap_or_default();
        if vk == 0x0D {
            units = vec![u16::from(b'\r')];
        }
        let first = units.first().copied().unwrap_or(0);
        let base = cef::KeyEvent {
            modifiers: flags,
            windows_key_code: vk,
            native_key_code: native,
            character: first,
            unmodified_character: first,
            ..Default::default()
        };

        if !pressed {
            host.send_key_event(Some(&cef::KeyEvent {
                type_: KeyEventType::KEYUP,
                ..base
            }));
            return;
        }
        host.send_key_event(Some(&cef::KeyEvent {
            type_: KeyEventType::RAWKEYDOWN,
            ..base.clone()
        }));
        if modifiers & (FLAG_CONTROL | FLAG_ALT | FLAG_COMMAND) != 0 {
            return;
        }
        for unit in units {
            let control = (unit < 0x20 && unit != u16::from(b'\r')) || unit == 0x7F;
            if control {
                continue;
            }
            host.send_key_event(Some(&cef::KeyEvent {
                type_: KeyEventType::CHAR,
                windows_key_code: unit as i32,
                character: unit,
                unmodified_character: unit,
                ..base.clone()
            }));
        }
    }
}

const COMMAND_HELP: &str = "commands: go <url> | back | forward | reload | print | pdf [path] \
| allow <id> | deny <id> | quit";

impl ApplicationHandler<UserEvent> for OsrApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.main.is_none() {
            let view = View::new(PhysicalSize::new(0, 0), 1.0, true);
            VIEWS.with_borrow_mut(|views| views.push(view.clone()));
            self.main = Some(view);
        }
        self.create_pending_windows(event_loop);
        self.create_main_browser();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {}

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(view) = self.windows.get(&id).cloned() else {
            return;
        };
        if view.borrow().window.is_none() {
            self.windows.remove(&id);
            return;
        }
        match event {
            WindowEvent::RedrawRequested => redraw(&view),
            WindowEvent::Resized(size) => {
                view.borrow_mut().size = size;
                if let Some(host) = host_of(&view) {
                    host.was_resized();
                }
                request_redraw(&view);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                view.borrow_mut().scale = scale_factor;
                if let Some(host) = host_of(&view) {
                    host.notify_screen_info_changed();
                    host.was_resized();
                }
            }
            WindowEvent::CloseRequested => {
                if view.borrow().is_main {
                    common::close_all();
                } else if let Some(host) = host_of(&view) {
                    host.close_browser(1);
                }
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    self.focused = Some(view.clone());
                }
                if let Some(host) = host_of(&view) {
                    host.set_focus(focused as i32);
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                let mut flags = 0;
                if s.shift_key() {
                    flags |= FLAG_SHIFT;
                }
                if s.control_key() {
                    flags |= FLAG_CONTROL;
                }
                if s.alt_key() {
                    flags |= FLAG_ALT;
                }
                if s.super_key() {
                    flags |= FLAG_COMMAND;
                }
                let mut v = view.borrow_mut();
                v.modifiers = flags;
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                if !is_synthetic {
                    Self::key(&view, &event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                {
                    let mut v = view.borrow_mut();
                    let scale = v.scale;
                    v.cursor = (position.x / scale, position.y / scale);
                }
                if let Some(host) = host_of(&view) {
                    host.send_mouse_move_event(Some(&Self::mouse_event(&view)), 0);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(host) = host_of(&view) {
                    host.send_mouse_move_event(Some(&Self::mouse_event(&view)), 1);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                Self::mouse_button(&view, state, button)
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = view.borrow().scale;
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (
                        x as f64 * WHEEL_PIXELS_PER_LINE,
                        y as f64 * WHEEL_PIXELS_PER_LINE,
                    ),
                    MouseScrollDelta::PixelDelta(PhysicalPosition { x, y }) => {
                        (x / scale, y / scale)
                    }
                };
                if let Some(host) = host_of(&view) {
                    host.send_mouse_wheel_event(
                        Some(&Self::mouse_event(&view)),
                        dx.round() as i32,
                        dy.round() as i32,
                    );
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(line) = self.commands.try_recv() {
            self.command(&line);
        }

        let now = Instant::now();
        let due = {
            let mut next = NEXT_PUMP.lock().expect("pump lock");
            match *next {
                Some(at) if at <= now => {
                    *next = None;
                    true
                }
                _ => false,
            }
        };
        if due {
            do_message_loop_work();
        }

        self.create_pending_windows(event_loop);
        self.create_main_browser();
        self.windows.retain(|_, v| v.borrow().window.is_some());

        if common::all_closed() {
            event_loop.exit();
            return;
        }
        if common::startup_failed() {
            // No main browser exists or will exist. Close any popup and leave once none is open.
            if common::browsers().is_empty() {
                event_loop.exit();
                return;
            }
            common::close_all();
        }

        // CEF asks for work through OnScheduleMessagePumpWork; without a request, still pump at
        // the cefclient maximum interval.
        let next = {
            let mut next = NEXT_PUMP.lock().expect("pump lock");
            *next.get_or_insert(Instant::now() + Duration::from_millis(MAX_TIMER_DELAY_MS as u64))
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }
}

pub fn run(event_loop: EventLoop<UserEvent>, url: String, profile: PathBuf, read_stdin: bool) {
    let (tx, rx) = mpsc::channel();
    if read_stdin {
        eprintln!("{COMMAND_HELP}");
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
                wake();
            }
        });
    }
    let mut app = OsrApp {
        url,
        profile,
        windows: HashMap::new(),
        main: None,
        browser_requested: false,
        focused: None,
        commands: rx,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
    }
}
