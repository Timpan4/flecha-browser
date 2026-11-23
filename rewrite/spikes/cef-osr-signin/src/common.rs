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

/// A URL as the terminal may show it. Paths, queries, fragments and opaque payloads can carry
/// OAuth codes or account identifiers (`data:` content, a `mailto:` address), so only these
/// parts are kept:
/// - a URL with a scheme in HIERARCHICAL_SCHEMES: `scheme://host[:port]`, without any
///   `user:password@`;
/// - `file`, `chrome` and `devtools` URLs: also the path, without query or fragment;
/// - any other scheme (`data:`, `mailto:`, `javascript:`, `blob:`, `about:`), whatever
///   follows it, `//` included: `scheme:` only.
pub fn display_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once(':') else {
        return if url.is_empty() {
            "(none)"
        } else {
            "(not a URL)"
        }
        .to_string();
    };
    // RFC 3986: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
    let valid = scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return "(not a URL)".to_string();
    }
    let scheme = scheme.to_ascii_lowercase();
    // Any other scheme is opaque here: `data://X` or `javascript://X` carry payload, not a host.
    if !HIERARCHICAL_SCHEMES.contains(&scheme.as_str()) {
        return format!("{scheme}:");
    }
    let special = SPECIAL_SCHEMES.contains(&scheme.as_str());
    let hier = if scheme == "file" {
        // `file:` plus `//`, `\\` or a mix, then the path (usually starting with `/`).
        let skip = rest
            .chars()
            .take(2)
            .take_while(|c| matches!(c, '/' | '\\'))
            .count();
        &rest[skip..]
    } else if special {
        // WHATWG URL: special schemes read `\` as `/` and skip any number of leading slashes.
        rest.trim_start_matches(['/', '\\'])
    } else {
        match rest.strip_prefix("//") {
            Some(hier) => hier,
            None => return format!("{scheme}:"),
        }
    };
    let without_query = hier.split(['?', '#']).next().unwrap_or("");
    match scheme.as_str() {
        "file" => format!("file://{}", without_query.replace('\\', "/")),
        "chrome" | "devtools" => format!("{scheme}://{without_query}"),
        _ => {
            let delimiters: &[char] = if special { &['/', '\\'] } else { &['/'] };
            let authority = without_query.split(delimiters).next().unwrap_or("");
            let host = authority.rsplit('@').next().unwrap_or("");
            format!("{scheme}://{host}")
        }
    }
}

/// The WHATWG URL standard's special schemes: `\` counts as `/` and the authority is read
/// however many slashes follow the scheme.
const SPECIAL_SCHEMES: &[&str] = &["http", "https", "ws", "wss", "ftp", "file"];

/// Schemes whose URLs have an authority (and, for local and internal pages, a path) worth
/// showing. Every other scheme is shown as `scheme:` only.
const HIERARCHICAL_SCHEMES: &[&str] = &[
    "http",
    "https",
    "ws",
    "wss",
    "ftp",
    "file",
    "chrome",
    "devtools",
    "chrome-extension",
];

/// Console output is relayed only for the spike's local fixtures unless `--log-console` is
/// set. CEF's console callback does not say which frame logged a message, and the message's
/// source is only a label a script can set (`//# sourceURL=file:///…`). So the frames decide:
/// a message is relayed only while every frame of the browser is a `file://` page, which
/// includes the frame that logged it. A local page that embeds any remote frame relays
/// nothing. The source must also be `file://`, and a message without one is not relayed.
fn relay_console(browser: Option<&Browser>, source: &str) -> bool {
    if LOG_CONSOLE.get().copied().unwrap_or(false) {
        return true;
    }
    let Some(browser) = browser else {
        return false;
    };
    let mut ids = CefStringList::new();
    browser.frame_identifiers(Some(&mut ids));
    let ids: Vec<String> = ids.into_iter().collect();
    source.starts_with("file://")
        && !ids.is_empty()
        && ids.iter().all(|id| {
            browser
                .frame_by_identifier(Some(&CefString::from(id.as_str())))
                .is_some_and(|f| userfree_to_string(&f.url()).starts_with("file://"))
        })
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
            let source = source.map(|s| s.to_string()).unwrap_or_default();
            if !selfcheck::on_console_message(browser.as_ref(), &message)
                && relay_console(browser.as_ref(), &source)
            {
                eprintln!("[console] {message} ({}:{line})", display_url(&source));
            }
            // Handled here in every case. Returning 0 lets CEF also write the message and its
            // full source URL to chrome_debug.log in the profile.
            1
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

#[cfg(test)]
mod tests {
    use super::display_url;

    #[test]
    fn display_url_keeps_no_payload_path_query_or_userinfo() {
        for (url, shown) in [
            ("data:text/html,TOKDATA", "data:"),
            ("data:text/html,<a href=http://x/TOK>x</a>", "data:"),
            ("data:,a://TOKHOST/TOKPATH", "data:"),
            ("mailto:TOK@example.invalid", "mailto:"),
            ("javascript:alert('TOK')", "javascript:"),
            ("blob:https://example.com/TOK-uuid", "blob:"),
            ("about:blank#TOK", "about:"),
            (
                "https://user:TOK@accounts.example:8443/p/TOK?code=TOK#TOK",
                "https://accounts.example:8443",
            ),
            ("HTTPS://example.com/TOK", "https://example.com"),
            ("https:\\\\example.com\\TOK", "https://example.com"),
            (
                "file:///tmp/fixture.html?q=TOK#TOK",
                "file:///tmp/fixture.html",
            ),
            ("chrome://version/?TOK", "chrome://version/"),
            (
                "devtools://devtools/x.html#TOK",
                "devtools://devtools/x.html",
            ),
            ("data://TOK,hi", "data:"),
            ("data:\\\\TOK,hi", "data:"),
            ("DATA://TOK,hi", "data:"),
            ("javascript://TOK%0Avoid(0)", "javascript:"),
            ("mailto://TOK@example.invalid", "mailto:"),
            ("blob:https://example.com/TOK-uuid", "blob:"),
            ("about://TOK", "about:"),
            ("http:\\\\example.com\\TOK?TOK", "http://example.com"),
            ("http:example.com/TOK", "http://example.com"),
            ("wss:///example.com:9/TOK", "wss://example.com:9"),
            ("chrome:\\\\TOK", "chrome:"),
            (
                "chrome-extension://abcdef/popup.html?TOK",
                "chrome-extension://abcdef",
            ),
            (
                "file:\\\\\\tmp\\fixture.html?TOK",
                "file:///tmp/fixture.html",
            ),
            ("", "(none)"),
            ("no scheme TOK", "(not a URL)"),
            ("1x:TOK", "(not a URL)"),
        ] {
            assert_eq!(display_url(url), shown, "{url}");
        }
    }
}
