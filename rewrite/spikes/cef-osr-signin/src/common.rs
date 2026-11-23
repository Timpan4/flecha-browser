//! State and handlers shared by both modes. Everything here runs on the CEF UI thread, which is
//! the main thread in both modes (CEF's own loop in chrome mode, the external pump in osr mode).

use crate::{cli::Mode, selfcheck};
use cef::*;
use std::{cell::RefCell, rc::Rc, sync::OnceLock};

pub static MODE: OnceLock<Mode> = OnceLock::new();

pub fn mode() -> Mode {
    *MODE.get().expect("mode is set before CEF starts")
}

thread_local! {
    static BROWSERS: RefCell<Vec<Browser>> = const { RefCell::new(Vec::new()) };
    static ALL_CLOSED: RefCell<bool> = const { RefCell::new(false) };
}

pub fn browser_created(browser: &Browser) {
    BROWSERS.with_borrow_mut(|list| list.push(browser.clone()));
    selfcheck::on_browser_created(browser);
}

pub fn browser_closed(browser: &Browser) {
    let id = browser.identifier();
    let empty = BROWSERS.with_borrow_mut(|list| {
        list.retain(|b| b.identifier() != id);
        list.is_empty()
    });
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
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let message = message.map(|m| m.to_string()).unwrap_or_default();
            if !selfcheck::on_console_message(&message) {
                let source = source.map(|s| s.to_string()).unwrap_or_default();
                eprintln!("[console] {message} ({source}:{line})");
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
            if let (Some(browser), Some(frame)) = (browser, frame) {
                if frame.is_main() != 0 {
                    eprintln!("[load-end] {}", userfree_to_string(&frame.url()));
                    selfcheck::on_main_frame_load_end(browser, frame);
                }
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
            let text = error_text.map(|s| s.to_string()).unwrap_or_default();
            let url = failed_url.map(|s| s.to_string()).unwrap_or_default();
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
