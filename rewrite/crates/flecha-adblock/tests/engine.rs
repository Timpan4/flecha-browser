//! Blocking and element hiding with the fixture lists, through the API the CEF hook calls.

mod common;

use std::collections::HashSet;

use common::{EASYLIST, EASYPRIVACY, loaded, serve, update};
use flecha_adblock::{AdBlocker, EngineOutcome, Request, RequestType};

const PAGE: &str = "https://news.example/story";

fn request<'a>(url: &'a str, page: &'a str, request_type: RequestType) -> Request<'a> {
    Request {
        url,
        source_url: page,
        top_level_url: page,
        request_type,
    }
}

fn blocks(blocker: &AdBlocker, url: &str, page: &str, request_type: RequestType) -> bool {
    blocker.should_block(&request(url, page, request_type))
}

#[test]
fn network_filters_from_both_lists_block_and_exceptions_win() {
    let (blocker, _cache) = loaded(true, &[]);
    use RequestType::*;
    assert!(blocks(&blocker, "https://ads.example/a.js", PAGE, Script));
    assert!(blocks(
        &blocker,
        "https://sub.ads.example/a.png",
        PAGE,
        Image
    ));
    assert!(!blocks(
        &blocker,
        "https://ads.example/allowed/a.js",
        PAGE,
        Script
    ));
    assert!(blocks(
        &blocker,
        "https://metrics.example/collect",
        PAGE,
        XmlHttpRequest
    ));
    assert!(!blocks(
        &blocker,
        "https://metrics.example/ok.js",
        PAGE,
        Script
    ));
    assert!(blocks(
        &blocker,
        "https://cdn.example/pixel.gif?id=1",
        PAGE,
        Image
    ));
    assert!(!blocks(
        &blocker,
        "https://cdn.example/story.js",
        PAGE,
        Script
    ));
    // Options: resource type, third-party, domain=.
    assert!(blocks(
        &blocker,
        "https://cdn.example/banner/x.png",
        PAGE,
        Image
    ));
    assert!(!blocks(
        &blocker,
        "https://cdn.example/banner/x.js",
        PAGE,
        Script
    ));
    assert!(blocks(&blocker, "https://third.example/t.js", PAGE, Script));
    assert!(!blocks(
        &blocker,
        "https://third.example/t.js",
        "https://www.third.example/",
        Script
    ));
    assert!(blocks(
        &blocker,
        "https://onlyon.example/x.js",
        PAGE,
        Script
    ));
    assert!(!blocks(
        &blocker,
        "https://onlyon.example/x.js",
        "https://other.example/",
        Script
    ));
    assert!(blocks(&blocker, "https://frames.example/", PAGE, SubFrame));
    assert!(!blocks(&blocker, "https://frames.example/", PAGE, Script));
}

#[test]
fn regular_expression_filters_apply_unlike_the_chromium_shell() {
    // build-adblock.mjs left regular expressions out (declarativeNetRequest caps them).
    let (blocker, _cache) = loaded(true, &[]);
    assert!(blocks(
        &blocker,
        "https://regex.example/ad42.js",
        PAGE,
        RequestType::Script
    ));
    assert!(!blocks(
        &blocker,
        "https://regex.example/adx.js",
        PAGE,
        RequestType::Script
    ));
}

#[test]
fn a_page_itself_is_never_blocked() {
    let (blocker, _cache) = loaded(true, &[]);
    for url in [
        "https://whole.example/",
        "https://ads.example/",
        "https://metrics.example/",
    ] {
        assert!(!blocks(&blocker, url, url, RequestType::MainFrame), "{url}");
    }
    // `$document` doesn't reach a page's own subresources either.
    assert!(!blocks(
        &blocker,
        "https://whole.example/app.js",
        PAGE,
        RequestType::Script
    ));
}

#[test]
fn a_document_exception_exempts_the_page_and_frames_it_covers() {
    let (blocker, _cache) = loaded(true, &[]);
    let ad = |source: &str, top: &str, request_type, url| {
        blocker.should_block(&Request {
            url,
            source_url: source,
            top_level_url: top,
            request_type,
        })
    };
    use RequestType::*;
    for page in ["https://trusted.example/", "https://www.trusted.example/a"] {
        assert!(
            !ad(page, page, Script, "https://ads.example/a.js"),
            "{page}"
        );
        assert!(
            !ad(page, page, Image, "https://metrics.example/p"),
            "{page}"
        );
    }
    // A frame the exception covers, and everything inside it.
    let news = "https://news.example/";
    assert!(!ad(news, news, SubFrame, "https://trusted.example/embed"));
    assert!(!ad(
        "https://trusted.example/embed",
        news,
        Script,
        "https://ads.example/a.js"
    ));
    assert!(ad(news, news, Script, "https://ads.example/a.js"));
    // `$document,subdocument`: the shell dropped it; adblock-rust syntax keeps the page exempt.
    let checker = "https://checker.example/page.htm";
    assert!(!ad(checker, checker, Script, "https://ads.example/a.js"));
    // A plain exception lets its URL load but exempts nothing inside the page.
    let allowed = "https://ads.example/allowed/page";
    assert!(ad(
        allowed,
        allowed,
        Script,
        "https://metrics.example/collect"
    ));
    // Exceptions shaped `||host^` that name no resource type: adblock-rust gives them the
    // document type, but like the shell's priority-2 `allow` rules they only let requests to
    // their host load. Neither the page nor a frame from that host is exempt.
    for page in [
        "https://plain.example/",
        "https://firstparty.example/",
        "https://important.example/",
    ] {
        assert!(ad(page, page, Script, "https://ads.example/a.js"), "{page}");
        assert!(
            ad(page, news, Script, "https://ads.example/a.js"),
            "frame {page}"
        );
    }
    // From the 2026-11-23 lists: EasyList's `@@||cloud.google.com^$~third-party` and
    // `$generichide`, and EasyPrivacy's Google Analytics filter.
    let cloud = "https://cloud.google.com/";
    assert!(ad(
        cloud,
        cloud,
        Script,
        "https://www.google-analytics.com/analytics.js"
    ));
    // `$all` with a type negated still names no `$document`. The shell ignored `$all`.
    let allbut = "https://allbut.example/";
    assert!(ad(allbut, allbut, Script, "https://ads.example/a.js"));
    assert!(ad(allbut, news, Script, "https://ads.example/a.js"));
    // `$document` with a party option: a frame is first-party only to a page of its own site, as
    // the shell matched a frame against the page that loaded it.
    for host in ["partyframe.example", "firstframe.example"] {
        let page = format!("https://{host}/");
        let frame = format!("https://{host}/f");
        assert!(
            !ad(&page, &page, Script, "https://ads.example/a.js"),
            "{host}"
        );
        assert!(
            ad(&frame, news, Script, "https://ads.example/a.js"),
            "{host}"
        );
        assert!(
            ad(&frame, news, SubFrame, "https://frames.example/x"),
            "{host}"
        );
    }
    // The frame request itself: `||blockedframe.example^$subdocument` with
    // `@@||blockedframe.example^$document,~third-party`. Embedded by another site the frame is
    // third-party and blocked; by its own site it is exempt.
    let blocked_frame = "https://blockedframe.example/f";
    assert!(ad(news, news, SubFrame, blocked_frame));
    let own_site = "https://www.blockedframe.example/";
    assert!(!ad(own_site, own_site, SubFrame, blocked_frame));
}

#[test]
fn a_filter_anchored_to_a_host_and_port_matches_as_the_shell_did() {
    let (blocker, _cache) = loaded(true, &[]);
    let script = |url| blocks(&blocker, url, PAGE, RequestType::Script);
    // EasyPrivacy's `||etwun.com:8080/counter.php?`, from the 2026-11-23 list.
    for url in [
        "https://etwun.com:8080/counter.php?id=1",
        "http://etwun.com:8080/counter.php?id=1",
        "wss://etwun.com:8080/counter.php?",
        "https://www.etwun.com:8080/counter.php?id=1",
        "https://etwun.com:8080/COUNTER.php?id=1",
    ] {
        assert!(script(url), "{url}");
    }
    for url in [
        "https://etwun.com:8081/counter.php?id=1",
        "https://etwun.com/counter.php?id=1",
        "https://notetwun.com:8080/counter.php?id=1",
        "https://etwun.com:8080/counter.phpx",
        "https://evil.example/?u=etwun.com:8080/counter.php?",
    ] {
        assert!(!script(url), "{url}");
    }
    // An exception with a port keeps its options: `$script` only.
    assert!(script("https://ported.example:8443/other.js"));
    assert!(!script("https://ported.example:8443/ok/a.js"));
    assert!(blocks(
        &blocker,
        "https://ported.example:8443/ok/a.png",
        PAGE,
        RequestType::Image
    ));
}

#[test]
fn nothing_is_blocked_or_hidden_before_the_first_engine() {
    let (blocker, _) = AdBlocker::new(true, &[]);
    assert!(!blocker.has_engine());
    assert!(!blocks(
        &blocker,
        "https://ads.example/a.js",
        PAGE,
        RequestType::Script
    ));
    let filters = blocker.cosmetic_filters(PAGE);
    assert!(filters.hide_selectors.is_empty() && filters.generic_hide);
}

#[test]
fn the_global_switch_turns_off_blocking_and_hiding() {
    let (blocker, _cache) = loaded(true, &[]);
    blocker.set_enabled(false);
    assert!(!blocks(
        &blocker,
        "https://ads.example/a.js",
        PAGE,
        RequestType::Script
    ));
    let filters = blocker.cosmetic_filters(PAGE);
    assert!(filters.hide_selectors.is_empty() && filters.generic_hide);
    let classes = ["ad-banner".to_owned()];
    assert!(
        blocker
            .hidden_class_id_selectors(PAGE, &classes, &[], &HashSet::new())
            .is_empty()
    );
    blocker.set_enabled(true);
    assert!(blocks(
        &blocker,
        "https://ads.example/a.js",
        PAGE,
        RequestType::Script
    ));
}

#[test]
fn an_allowed_site_covers_its_subdomains_and_every_frame_in_its_pages() {
    let (blocker, _cache) = loaded(true, &["news.example"]);
    for page in [
        "https://news.example/",
        "https://www.news.example/a",
        "http://NEWS.example:8080/",
    ] {
        let ad = Request {
            url: "https://ads.example/a.js",
            source_url: "https://frames.example/embed",
            top_level_url: page,
            request_type: RequestType::Script,
        };
        assert!(!blocker.should_block(&ad), "{page}");
        let filters = blocker.cosmetic_filters(page);
        assert!(
            filters.hide_selectors.is_empty() && filters.generic_hide,
            "{page}"
        );
    }
    // A frame from an allowed site inside another page is still blocked in.
    let framed = Request {
        url: "https://ads.example/a.js",
        source_url: "https://news.example/embed",
        top_level_url: "https://other.example/",
        request_type: RequestType::Script,
    };
    assert!(blocker.should_block(&framed));
    for page in ["https://notnews.example/", "https://news.example.org/"] {
        assert!(
            blocks(
                &blocker,
                "https://ads.example/a.js",
                page,
                RequestType::Script
            ),
            "{page}"
        );
    }
    assert_eq!(blocker.set_allowed_sites(&[]), 0);
    assert!(blocks(
        &blocker,
        "https://ads.example/a.js",
        PAGE,
        RequestType::Script
    ));
}

#[test]
fn invalid_allowed_entries_are_dropped_one_by_one() {
    let entries: Vec<String> = [
        "news.example",
        "News.example",
        "[::1]",
        "ſite.example",
        "\u{212a}.example",
        "ｓｉｔｅ.example",
        " site.example",
        "other.example",
    ]
    .map(str::to_owned)
    .to_vec();
    let (blocker, dropped) = AdBlocker::new(true, &entries);
    assert_eq!(dropped, 6);
    let cache = tempfile::tempdir().unwrap();
    update(&blocker, cache.path(), &serve(EASYLIST, EASYPRIVACY));
    assert!(!blocks(
        &blocker,
        "https://ads.example/a.js",
        "https://other.example/",
        RequestType::Script
    ));
    assert!(!blocks(
        &blocker,
        "https://ads.example/a.js",
        PAGE,
        RequestType::Script
    ));
    // The URL parser folds U+017F, U+212A and fullwidth forms; the dropped entries never match.
    assert!(blocks(
        &blocker,
        "https://ads.example/a.js",
        "https://ſite.example/",
        RequestType::Script
    ));
    assert!(blocks(
        &blocker,
        "https://ads.example/a.js",
        "https://\u{212a}.example/",
        RequestType::Script
    ));
}

#[test]
fn element_hiding_for_a_page() {
    let (blocker, _cache) = loaded(true, &[]);
    let filters = blocker.cosmetic_filters(PAGE);
    assert!(!filters.generic_hide);
    // Complex generic selectors come with the page; the unclosed one is left out.
    assert_eq!(
        filters.hide_selectors,
        ["[data-ad=\"1\"]", "div.sponsored > span"]
    );
    assert!(
        filters
            .stylesheet()
            .contains("div.sponsored > span{display:none!important}")
    );
    // Plain class and id selectors come from what the page reports.
    let classes = [
        "ad-banner".to_owned(),
        "story".to_owned(),
        "exempted-everywhere".to_owned(),
    ];
    assert_eq!(
        blocker.hidden_class_id_selectors(PAGE, &classes, &[], &filters.exceptions),
        [".ad-banner", ".exempted-everywhere"]
    );
}

#[test]
fn site_specific_hiding_and_exceptions_apply_per_site() {
    // build-adblock.mjs skipped `site##sel` and dropped an excepted selector everywhere.
    let (blocker, _cache) = loaded(true, &[]);
    let site = blocker.cosmetic_filters("https://www.site.example/");
    assert!(site.hide_selectors.contains(&".site-only".to_owned()));
    let classes = ["ad-banner".to_owned()];
    assert!(
        blocker
            .hidden_class_id_selectors("https://www.site.example/", &classes, &[], &site.exceptions)
            .is_empty()
    );
    let elsewhere = blocker.cosmetic_filters(PAGE);
    assert!(!elsewhere.hide_selectors.contains(&".site-only".to_owned()));
    let other = blocker.cosmetic_filters("https://example.org/");
    let classes = ["exempted-everywhere".to_owned()];
    assert!(
        blocker
            .hidden_class_id_selectors("https://example.org/", &classes, &[], &other.exceptions)
            .is_empty()
    );
}

#[test]
fn sites_the_lists_exempt_get_no_generic_hiding() {
    let (blocker, _cache) = loaded(true, &[]);
    for page in ["https://exempt.example/", "https://a.exempt.example/"] {
        let filters = blocker.cosmetic_filters(page);
        assert!(filters.generic_hide, "{page}");
        assert!(
            !filters
                .hide_selectors
                .contains(&"div.sponsored > span".to_owned()),
            "{page}"
        );
    }
    // adblock-rust doesn't take `$elemhide`: the line is skipped and reported (see the
    // malformed-lines test). EasyList and EasyPrivacy had none on 2026-11-23.
    assert!(
        !blocker
            .cosmetic_filters("https://elemhide.example/")
            .generic_hide
    );
}

#[test]
fn malformed_lines_are_skipped_and_reported_without_failing_the_load() {
    let (blocker, dropped) = AdBlocker::new(true, &[]);
    assert_eq!(dropped, 0);
    let cache = tempfile::tempdir().unwrap();
    let report = update(&blocker, cache.path(), &serve(EASYLIST, EASYPRIVACY));
    let EngineOutcome::Parsed { skipped_lines, .. } = &report.engine else {
        panic!("{report:?}");
    };
    let reported: Vec<_> = skipped_lines
        .iter()
        .map(|line| (line.list.as_str(), line.text.as_str()))
        .collect();
    assert!(
        reported.contains(&("easylist", "||bad-option.example^$no-such-option")),
        "{reported:?}"
    );
    assert!(
        reported.contains(&("easylist", "@@||elemhide.example^$elemhide")),
        "{reported:?}"
    );
    assert!(
        reported.contains(&("easylist", "example.com##")),
        "{reported:?}"
    );
    // Redirects are left out as build-adblock.mjs did: without resources adblock-rust would
    // cancel the request instead of serving the stub.
    for redirect in [
        "||stub.example^$redirect=noopjs,script",
        "||stub-rule.example^$redirect-rule=noopjs",
        "||rewritten.example^$rewrite=abp-resource:blank-js",
    ] {
        assert!(reported.contains(&("easylist", redirect)), "{reported:?}");
        let host = redirect[2..].split('^').next().unwrap();
        let url = format!("https://{host}/x.js");
        assert!(!blocks(&blocker, &url, PAGE, RequestType::Script), "{url}");
    }
    assert_eq!(reported.len(), 6, "{reported:?}");
    let line = skipped_lines
        .iter()
        .find(|line| line.text.contains("no-such-option"))
        .unwrap();
    assert_eq!(
        EASYLIST.lines().nth(line.line - 1),
        Some(line.text.as_str())
    );
    // The lines after them still load.
    assert!(blocks(
        &blocker,
        "https://ok-after-bad.example/x.js",
        PAGE,
        RequestType::Script
    ));
    assert!(blocks(
        &blocker,
        "https://metrics.example/collect",
        PAGE,
        RequestType::Script
    ));
}
