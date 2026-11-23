//! State and handlers shared by both modes. Everything here runs on the CEF UI thread, which is
//! the main thread in both modes (CEF's own loop in chrome mode, the external pump in osr mode).

use crate::{cli::Mode, selfcheck};
use cef::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub static MODE: OnceLock<Mode> = OnceLock::new();

static STARTUP_FAILED: AtomicBool = AtomicBool::new(false);

/// Records that the first browser or its window could not be created and stops the run;
/// `main` then exits with a failure status after `cef::shutdown`. In chrome mode no browser
/// exists to close, so the message loop is quit through a task (this may run before the
/// loop has started).
pub fn fail_startup(message: &str) {
    eprintln!("startup failed: {message}");
    STARTUP_FAILED.store(true, Ordering::SeqCst);
    if mode() == Mode::Chrome {
        post_task(ThreadId::UI, Some(&mut QuitTask::new()));
    }
}

pub fn startup_failed() -> bool {
    STARTUP_FAILED.load(Ordering::SeqCst)
}

wrap_task! {
    struct QuitTask;

    impl Task {
        fn execute(&self) {
            quit_message_loop();
        }
    }
}

/// `--log-console`: relay console messages from every page. Off by default, so nothing a
/// sign-in page logs reaches the terminal or a log file.
pub static LOG_CONSOLE: OnceLock<bool> = OnceLock::new();

/// A URL as the terminal may show it: local and internal pages in full, web pages as their
/// origin only, because paths and queries can carry OAuth codes or account identifiers.
pub fn display_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.split(['?', '#']).next().unwrap_or("").to_string();
    };
    match scheme {
        "file" | "chrome" | "devtools" => url.to_string(),
        _ => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            // Drop any user:password@ part.
            let host = authority.rsplit('@').next().unwrap_or("");
            format!("{scheme}://{host}")
        }
    }
}

/// Console output is relayed only for pages loaded from local files (the spike's fixtures),
/// judged by the browser's main-frame URL, unless `--log-console` is set.
fn relay_console(browser: Option<&Browser>) -> bool {
    if LOG_CONSOLE.get().copied().unwrap_or(false) {
        return true;
    }
    browser
        .and_then(|b| b.main_frame())
        .is_some_and(|f| userfree_to_string(&f.url()).starts_with("file://"))
}

pub fn mode() -> Mode {
    *MODE.get().expect("mode is set before CEF starts")
}

thread_local! {
    static BROWSERS: RefCell<Vec<Browser>> = const { RefCell::new(Vec::new()) };
    static ALL_CLOSED: RefCell<bool> = const { RefCell::new(false) };
}

pub fn browser_created(browser: &Browser) {
    let open = BROWSERS.with_borrow_mut(|list| {
        list.push(browser.clone());
        list.len()
    });
    eprintln!("[browser] created id={} open={open}", browser.identifier());
    selfcheck::on_browser_created(browser);
}

pub fn browser_closed(browser: &Browser) {
    let id = browser.identifier();
    let open = BROWSERS.with_borrow_mut(|list| {
        list.retain(|b| b.identifier() != id);
        list.len()
    });
    eprintln!("[browser] closed id={id} open={open}");
    let empty = open == 0;
    if empty {
        ALL_CLOSED.with_borrow_mut(|closed| *closed = true);
        if mode() == Mode::Chrome {
            quit_message_loop();
        }
    }
}

pub fn all_closed() -> bool {
    ALL_CLOSED.with_borrow(|closed| *closed)
}

pub fn browsers() -> Vec<Browser> {
    BROWSERS.with_borrow(|list| list.clone())
}

/// Closes every browser. `on_before_close` then drives shutdown, so cookies and the profile are
/// flushed before `cef::shutdown()`.
pub fn close_all() {
    for browser in browsers() {
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }
}

pub type CursorSink = Rc<dyn Fn(CursorType)>;

wrap_display_handler! {
    pub struct SpikeDisplayHandler {
        cursor: Option<CursorSink>,
    }

    impl DisplayHandler {
        fn on_console_message(
            &self,
            browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let browser = browser.map(|b| b.clone());
            let message = message.map(|m| m.to_string()).unwrap_or_default();
            if !selfcheck::on_console_message(browser.as_ref(), &message)
                && relay_console(browser.as_ref())
            {
                let source = source.map(|s| s.to_string()).unwrap_or_default();
                eprintln!("[console] {message} ({}:{line})", display_url(&source));
            }
            0
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: ::std::os::raw::c_ulong,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            match &self.cursor {
                Some(sink) => {
                    sink(type_);
                    1
                }
                None => 0,
            }
        }
    }
}

wrap_load_handler! {
    pub struct SpikeLoadHandler;

    impl LoadHandler {
        fn on_load_end(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: ::std::os::raw::c_int,
        ) {
            if let (Some(browser), Some(frame)) = (browser, frame)
                && frame.is_main() != 0 {
                    eprintln!("[load-end] {}", display_url(&userfree_to_string(&frame.url())));
                    selfcheck::on_main_frame_load_end(browser, frame);
                }
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            let main = frame.map(|f| f.is_main() != 0).unwrap_or(false);
            // The error text is CEF's description of the error code, not page content.
            let text = error_text.map(|s| s.to_string()).unwrap_or_default();
            let url = display_url(&failed_url.map(|s| s.to_string()).unwrap_or_default());
            eprintln!("[load-error] main_frame={main} code={error_code:?} {text} {url}");
            if main {
                selfcheck::on_main_frame_load_error(&url, &format!("{error_code:?} {text}"));
            }
        }
    }
}

/// Opaque white (ARGB). With alpha 0, windowless browsers paint transparent pixels where a page
/// sets no background. Used for every browser in both modes.
pub const BACKGROUND: u32 = 0xFFFF_FFFF;

/// The same browser settings in both modes.
pub fn browser_settings() -> BrowserSettings {
    BrowserSettings {
        background_color: BACKGROUND,
        ..Default::default()
    }
}

pub fn userfree_to_string(value: &CefStringUserfree) -> String {
    CefString::from(value).to_string()
}
