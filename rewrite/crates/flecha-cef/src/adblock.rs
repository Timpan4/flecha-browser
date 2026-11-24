//! Network blocking in CEF's resource-request hook, with flecha-adblock's matching.
//!
//! CEF calls [`Requests`] and [`Filter`] on its IO thread, so they hold only
//! [`Io`]: the `Send + Sync` blocker handle and the browser's blocked-request counter. They must
//! never hold the UI thread's `Rc<Slot>`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cef::{rc::Rc as _, *};
use flecha_adblock::{
    AdBlocker, Request as AdRequest, RequestType, UpdateReport, load_cached_rules,
};

use crate::callbacks::Slot;
use crate::{devtools, services};

/// What the IO thread may touch for one browser.
#[derive(Clone)]
pub(crate) struct Io {
    pub blocker: AdBlocker,
    /// Requests blocked since the tab's last main-frame navigation.
    pub blocked: Arc<AtomicU64>,
}

const _: fn() = || {
    fn io_thread_safe<T: Send + Sync>() {}
    io_thread_safe::<Io>();
};

/// One resource request, as the IO thread sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Subject<'a> {
    pub url: &'a str,
    pub request_type: RequestType,
    pub top_level_url: &'a str,
    pub source_url: &'a str,
    pub frame_parent_url: Option<&'a str>,
}

/// Whether to cancel the request. Internal and non-web URLs always load: flecha-adblock's lists
/// match web requests only. A main-frame request starts a new page, so it resets the count.
pub(crate) fn check(io: &Io, subject: Subject<'_>) -> bool {
    if !is_web(subject.url) {
        return false;
    }
    if subject.request_type == RequestType::MainFrame {
        io.blocked.store(0, Ordering::Relaxed);
        return false;
    }
    let blocked = io.blocker.should_block(&AdRequest {
        url: subject.url,
        source_url: subject.source_url,
        top_level_url: subject.top_level_url,
        frame_parent_url: subject.frame_parent_url,
        request_type: subject.request_type,
    });
    if blocked {
        io.blocked.fetch_add(1, Ordering::Relaxed);
    }
    blocked
}

fn is_web(url: &str) -> bool {
    ["http:", "https:", "ws:", "wss:"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
    })
}

pub(crate) fn request_type(kind: ResourceType) -> RequestType {
    let pairs = [
        (ResourceType::MAIN_FRAME, RequestType::MainFrame),
        (
            ResourceType::NAVIGATION_PRELOAD_MAIN_FRAME,
            RequestType::MainFrame,
        ),
        (ResourceType::SUB_FRAME, RequestType::SubFrame),
        (
            ResourceType::NAVIGATION_PRELOAD_SUB_FRAME,
            RequestType::SubFrame,
        ),
        (ResourceType::STYLESHEET, RequestType::Stylesheet),
        (ResourceType::SCRIPT, RequestType::Script),
        (ResourceType::WORKER, RequestType::Script),
        (ResourceType::SHARED_WORKER, RequestType::Script),
        (ResourceType::SERVICE_WORKER, RequestType::Script),
        (ResourceType::IMAGE, RequestType::Image),
        (ResourceType::FAVICON, RequestType::Image),
        (ResourceType::FONT_RESOURCE, RequestType::Font),
        (ResourceType::OBJECT, RequestType::Object),
        (ResourceType::PLUGIN_RESOURCE, RequestType::Object),
        (ResourceType::MEDIA, RequestType::Media),
        (ResourceType::XHR, RequestType::XmlHttpRequest),
        (ResourceType::PING, RequestType::Ping),
        (ResourceType::CSP_REPORT, RequestType::CspReport),
    ];
    pairs
        .into_iter()
        .find_map(|(cef, ours)| (cef == kind).then_some(ours))
        .unwrap_or(RequestType::Other)
}

impl crate::Engine {
    /// The blocker every browser's resource filter reads. Hand a clone to
    /// `flecha_adblock::Updater::spawn` to keep EasyList and EasyPrivacy current.
    pub fn adblocker(&self) -> &AdBlocker {
        &self.services().adblock
    }

    /// Settings' "Block ads and trackers".
    pub fn set_adblock_enabled(&self, enabled: bool) {
        self.services().adblock.set_enabled(enabled);
    }

    /// Replaces the sites where blocking is off (`Settings::ad_block_allowed`). Returns how
    /// many entries were dropped as invalid.
    pub fn set_adblock_allowed_sites(&mut self, sites: &[String]) -> usize {
        self.adblock_allowed = sites.to_vec();
        self.services().adblock.set_allowed_sites(sites)
    }

    /// The site shield: turns blocking off (`disabled`) or back on for one site host. Returns
    /// how many saved entries were dropped as invalid.
    pub fn set_adblock_site_disabled(&mut self, host: &str, disabled: bool) -> usize {
        self.adblock_allowed.retain(|site| site != host);
        if disabled {
            self.adblock_allowed.push(host.to_owned());
        }
        self.services()
            .adblock
            .set_allowed_sites(&self.adblock_allowed)
    }

    /// The sites where blocking is off, as last set.
    pub fn adblock_allowed_sites(&self) -> &[String] {
        &self.adblock_allowed
    }

    /// Installs the engine for the lists cached in `cache` without downloading. Parsing full
    /// lists takes seconds, so the app loads them on `flecha_adblock::Updater`'s thread instead;
    /// this is for a warm cache and for tests.
    pub fn load_adblock_rules(&self, cache: &Path) -> UpdateReport {
        load_cached_rules(&self.services().adblock, cache)
    }

    /// Requests blocked on the tab's current page.
    pub fn blocked_count(&self, tab: &flecha_core::TabId) -> u64 {
        self.browsers
            .get(tab)
            .map_or(0, |session| session.slot.blocked.load(Ordering::Relaxed))
    }
}

fn text(value: &CefStringUserfree) -> String {
    CefString::from(value).to_string()
}

wrap_resource_request_handler! {
    pub(crate) struct Filter { io: Io }
    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self, browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            request: Option<&mut Request>, _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let Some(request) = request else { return ReturnValue::CONTINUE };
            let url = text(&request.url());
            let request_type = request_type(request.resource_type());
            // Service workers and CefURLRequest have no browser or frame. Their pages are
            // unknown, so they load.
            let (Some(browser), Some(frame)) = (browser, frame) else {
                return ReturnValue::CONTINUE;
            };
            let Some(top) = browser.main_frame() else { return ReturnValue::CONTINUE };
            let top_level_url = text(&top.url());
            // A frame's own navigation comes from the document that embeds it.
            let (source, embedder) = if request_type == RequestType::SubFrame {
                let parent = frame.parent();
                let grandparent = parent.as_ref().and_then(|parent| parent.parent());
                (parent.map(|parent| text(&parent.url())), grandparent.map(|frame| text(&frame.url())))
            } else {
                let parent = (frame.is_main() == 0).then(|| frame.parent()).flatten();
                (Some(text(&frame.url())), parent.map(|parent| text(&parent.url())))
            };
            let source_url = source.unwrap_or_else(|| top_level_url.clone());
            let blocked = check(&self.io, Subject {
                url: &url,
                request_type,
                top_level_url: &top_level_url,
                source_url: &source_url,
                frame_parent_url: embedder.as_deref(),
            });
            if blocked { ReturnValue::CANCEL } else { ReturnValue::CONTINUE }
        }
    }
}

wrap_request_handler! {
    pub(crate) struct Requests { filter: ResourceRequestHandler }
    impl RequestHandler {
        fn resource_request_handler(
            &self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
            _request: Option<&mut Request>, _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int, _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            Some(self.filter.clone())
        }
    }
}

pub(crate) fn handler(io: Io) -> RequestHandler {
    Requests::new(Filter::new(io))
}

/// The page registry that holds ad hiding's adopted style sheets.
const HIDING_SHEETS: &str = "flecha.adblock/styles";

/// Collects the page's distinct classes and ids. The value returns by CDP's own serialization,
/// so a page that replaces `JSON.stringify` can't change its shape.
const REPORT_CLASSES_AND_IDS: &str = "(() => {
  const classes = new Set(), ids = new Set();
  for (const element of document.querySelectorAll('[class], [id]')) {
    for (const name of element.classList) classes.add(name);
    if (element.id) ids.add(element.id);
  }
  return { classes: [...classes], ids: [...ids] };
})()";

/// Ad hiding waiting for the page at `url` to report its classes and ids.
pub(crate) struct Generic {
    url: String,
    /// The page's own hiding, which stays applied next to the generic sheet.
    own: String,
    exceptions: std::collections::HashSet<String>,
}

/// Hides ads on the main frame that finished loading `url`: the selectors the lists give the
/// page right away, then generic class and id selectors once the page reports what it has,
/// through an internal CDP call. Off the web it does nothing, as Boosts.
pub(crate) fn hide_ads(
    slot: &std::rc::Rc<Slot>,
    host: Option<BrowserHost>,
    frame: &Frame,
    url: &str,
) {
    if !crate::boosts::is_web_page(url) {
        return;
    }
    let filters = services::of(slot).adblock.cosmetic_filters(url);
    let own = filters.stylesheet();
    apply(frame, &own, "");
    let Some(host) = host.filter(|_| !filters.generic_hide) else {
        return;
    };
    let params = serde_json::json!({ "expression": REPORT_CLASSES_AND_IDS, "returnByValue": true });
    let generic = Generic {
        url: url.to_owned(),
        own,
        exceptions: filters.exceptions,
    };
    // Without the report the page keeps its own hiding only.
    let _ = devtools::send(
        slot,
        &host,
        "Runtime.evaluate",
        Some(&params.to_string()),
        Some(devtools::Internal::Generic(generic)),
    );
}

/// The page answered [`hide_ads`]'s report: adds the generic sheet unless it navigated since.
pub(crate) fn reported(blocker: &AdBlocker, frame: &Frame, generic: Generic, reply: Option<&[u8]>) {
    if text(&frame.url()) != generic.url {
        return;
    }
    let sheet = generic_sheet(blocker, &generic, reply);
    apply(frame, &generic.own, &sheet);
}

/// The generic class and id hiding for the classes and ids in a `Runtime.evaluate` reply.
fn generic_sheet(blocker: &AdBlocker, generic: &Generic, reply: Option<&[u8]>) -> String {
    let report: serde_json::Value = reply
        .and_then(|reply| serde_json::from_slice(reply).ok())
        .unwrap_or_default();
    let names = |key: &str| -> Vec<String> {
        report["result"]["value"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect()
    };
    let selectors = blocker.hidden_class_id_selectors(
        &generic.url,
        &names("classes"),
        &names("ids"),
        &generic.exceptions,
    );
    flecha_adblock::stylesheet(&selectors)
}

fn apply(frame: &Frame, own: &str, generic: &str) {
    let sheets: Vec<(String, String)> = [("own", own), ("generic", generic)]
        .into_iter()
        .filter(|(_, css)| !css.is_empty())
        .map(|(name, css)| (name.to_owned(), css.to_owned()))
        .collect();
    crate::boosts::run(
        frame,
        "flecha-adblock/cosmetic",
        &crate::boosts::style_payload(HIDING_SHEETS, &sheets),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cached EasyList and EasyPrivacy copies that hold `rules`, fresh enough that loading them
    /// downloads nothing.
    pub(crate) fn write_lists(cache: &Path, rules: &str) {
        for (id, title, body) in [
            ("easylist", "EasyList", rules),
            ("easyprivacy", "EasyPrivacy", "||tracker.invalid^\n"),
        ] {
            let text = format!("[Adblock Plus 2.0]\n! Title: {title}\n\n{body}");
            std::fs::write(cache.join(format!("{id}.txt")), text).unwrap();
        }
    }

    /// A blocker with these network rules, installed the way the engine loads cached lists.
    pub(crate) fn blocker_with(rules: &str, cache: &Path) -> AdBlocker {
        write_lists(cache, rules);
        let (blocker, _) = AdBlocker::new(true, &[]);
        let report = load_cached_rules(&blocker, cache);
        assert!(report.failures.is_empty(), "{report:?}");
        assert!(blocker.has_engine());
        blocker
    }

    fn subject(url: &str, request_type: RequestType) -> Subject<'_> {
        Subject {
            url,
            request_type,
            top_level_url: "https://news.example/article",
            source_url: "https://news.example/article",
            frame_parent_url: None,
        }
    }

    #[test]
    fn the_io_thread_filter_blocks_matches_and_counts_them_per_page() {
        let cache = tempdir();
        let io = Io {
            blocker: blocker_with("||ads.example^\n/flecha-ad-probe.js\n", cache.path()),
            blocked: Arc::new(AtomicU64::new(0)),
        };
        // The filter runs on CEF's IO thread, not the thread that built it.
        let worker = io.clone();
        std::thread::spawn(move || {
            assert!(check(
                &worker,
                subject("https://ads.example/banner.png", RequestType::Image)
            ));
            assert!(check(
                &worker,
                subject(
                    "https://cdn.example/flecha-ad-probe.js",
                    RequestType::Script
                )
            ));
            assert!(!check(
                &worker,
                subject("https://cdn.example/app.js", RequestType::Script)
            ));
        })
        .join()
        .unwrap();
        assert_eq!(io.blocked.load(Ordering::Relaxed), 2);

        // Never a top-level navigation, and a new page starts a new count.
        assert!(!check(
            &io,
            subject("https://ads.example/", RequestType::MainFrame)
        ));
        assert_eq!(io.blocked.load(Ordering::Relaxed), 0);

        // Internal and non-web URLs always load.
        for url in [
            "chrome://ads.example/",
            "data:text/plain,ads.example",
            "file:///ads.example",
        ] {
            assert!(!check(&io, subject(url, RequestType::Image)), "{url}");
        }
    }

    #[test]
    fn switching_blocking_off_or_allowing_the_site_lets_ads_load() {
        let cache = tempdir();
        let io = Io {
            blocker: blocker_with("||ads.example^\n", cache.path()),
            blocked: Arc::new(AtomicU64::new(0)),
        };
        let ad = subject("https://ads.example/banner.png", RequestType::Image);
        assert!(check(&io, ad));

        io.blocker.set_allowed_sites(&["news.example".into()]);
        assert!(
            !check(&io, ad),
            "the site shield allows ads on news.example"
        );
        let elsewhere = Subject {
            top_level_url: "https://other.example/",
            source_url: "https://other.example/",
            ..ad
        };
        assert!(check(&io, elsewhere), "the allowlist is per site");

        io.blocker.set_allowed_sites(&[]);
        io.blocker.set_enabled(false);
        assert!(!check(&io, ad));
    }

    #[test]
    fn generic_hiding_uses_the_classes_and_ids_the_page_reports() {
        let cache = tempdir();
        let rules = "||ads.example^\n##.ad-banner\n###sponsor\n##.unused\nnews.example##.site-ad\n";
        let blocker = blocker_with(rules, cache.path());
        let url = "https://news.example/article";
        let filters = blocker.cosmetic_filters(url);
        assert!(!filters.generic_hide);
        assert_eq!(filters.stylesheet(), ".site-ad{display:none!important}");

        let generic = Generic {
            url: url.into(),
            own: filters.stylesheet(),
            exceptions: filters.exceptions,
        };
        // A `Runtime.evaluate` reply with `returnByValue`.
        let reply = br#"{"result":{"type":"object","value":{"classes":["ad-banner","story"],"ids":["sponsor",7]}}}"#;
        assert_eq!(
            generic_sheet(&blocker, &generic, Some(reply)),
            "#sponsor{display:none!important}\n.ad-banner{display:none!important}"
        );
        // A failed or malformed reply hides nothing generic.
        assert_eq!(generic_sheet(&blocker, &generic, None), "");
        assert_eq!(
            generic_sheet(&blocker, &generic, Some(b"{\"result\":{}}")),
            ""
        );
        assert_eq!(generic_sheet(&blocker, &generic, Some(b"not json")), "");

        // The site shield turns generic hiding off as well.
        blocker.set_allowed_sites(&["news.example".into()]);
        assert_eq!(generic_sheet(&blocker, &generic, Some(reply)), "");
        assert!(blocker.cosmetic_filters(url).generic_hide);
    }

    #[test]
    fn cef_resource_types_map_onto_the_lists_request_types() {
        assert_eq!(request_type(ResourceType::SCRIPT), RequestType::Script);
        assert_eq!(request_type(ResourceType::XHR), RequestType::XmlHttpRequest);
        assert_eq!(
            request_type(ResourceType::MAIN_FRAME),
            RequestType::MainFrame
        );
        assert_eq!(request_type(ResourceType::SUB_RESOURCE), RequestType::Other);
        assert_eq!(request_type(ResourceType::PREFETCH), RequestType::Other);
    }

    pub(crate) fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
}
