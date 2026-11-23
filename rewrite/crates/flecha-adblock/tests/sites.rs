//! Ports of `chromium/tests/ad-block-sites.test.mjs`: the site panel's shield, through
//! flecha-core's settings and storage and this crate's `AdBlocker`.
//!
//! Not ported: `site-ad-block-shell.test.mjs` checks which tab the shell reloads, which belongs
//! to the GPUI site panel.

use flecha_adblock::SiteAllowList;
use flecha_core::{
    BrowserState, Settings, SpaceId, WindowId, is_site_host,
    storage::{decode, decode_with_repairs, encode},
};

fn state() -> BrowserState {
    BrowserState::new(SpaceId::new("s"), WindowId::new("w"))
}

fn allow_list(settings: &Settings) -> SiteAllowList {
    SiteAllowList::new(settings.ad_block_allowed.as_deref().unwrap_or_default()).0
}

#[test]
fn turning_blocking_off_for_a_site_covers_its_subdomains_and_leaves_other_sites_blocked() {
    let mut state = state();
    assert!(state.settings.set_ad_blocking_on("example.com", false));
    let allowed = allow_list(&state.settings);
    assert!(!allowed.blocks_ads_on("example.com"));
    assert!(!allowed.blocks_ads_on("news.example.com"));
    assert!(allowed.blocks_ads_on("notexample.com"));
    assert!(allowed.blocks_ads_on("example.org"));
}

#[test]
fn turning_it_off_twice_keeps_one_entry_and_a_covered_subdomain_adds_none() {
    let mut state = state();
    state.settings.set_ad_blocking_on("example.com", false);
    state.settings.set_ad_blocking_on("example.com", false);
    state.settings.set_ad_blocking_on("www.example.com", false);
    assert_eq!(state.settings.ad_block_allowed.unwrap(), ["example.com"]);
}

#[test]
fn turning_blocking_back_on_from_a_subdomain_removes_the_entry_that_covered_it() {
    let mut state = state();
    state.settings.set_ad_blocking_on("example.com", false);
    state.settings.set_ad_blocking_on("other.org", false);
    state.settings.set_ad_blocking_on("www.example.com", true);
    assert!(allow_list(&state.settings).blocks_ads_on("www.example.com"));
    assert_eq!(state.settings.ad_block_allowed.unwrap(), ["other.org"]);
}

#[test]
fn an_emptied_list_stays_saved_as_empty_so_incognito_windows_follow_it() {
    let mut state = state();
    state.settings.set_ad_blocking_on("example.com", false);
    state.settings.set_ad_blocking_on("example.com", true);
    assert_eq!(state.settings.ad_block_allowed, Some(Vec::new()));
    let saved = encode(&state).unwrap();
    assert!(saved.contains("\"adBlockAllowed\": []"), "{saved}");
    assert_eq!(
        decode(&saved).unwrap().settings.ad_block_allowed,
        Some(Vec::new())
    );
}

#[test]
fn only_hostnames_chromium_takes_as_request_domains_count_as_sites() {
    for host in [
        "example.com",
        "localhost",
        "127.0.0.1",
        "xn--bcher-kva.example",
    ] {
        assert!(is_site_host(host), "{host}");
    }
    // Hosts compare as ASCII: no Unicode case folding or width mapping.
    for host in [
        "[::1]",
        "Example.com",
        "",
        "ſtory.example",
        "\u{212a}.example",
        "ｅxample.com",
        " example.com",
        "example.com\u{85}",
    ] {
        assert!(!is_site_host(host), "{host:?}");
    }
}

#[test]
fn the_list_survives_a_restart_and_a_hostname_chromium_would_refuse_is_dropped() {
    let mut state = state();
    state.settings.set_ad_blocking_on("example.com", false);
    let saved = encode(&state).unwrap();
    assert_eq!(
        decode(&saved).unwrap().settings.ad_block_allowed.unwrap(),
        ["example.com"]
    );

    state
        .settings
        .ad_block_allowed
        .as_mut()
        .unwrap()
        .push("Bad Host".into());
    let decoded = decode_with_repairs(&encode(&state).unwrap()).unwrap();
    // Divergence: the shell dropped the whole list; only the bad entry goes.
    assert_eq!(
        decoded.state.settings.ad_block_allowed.unwrap(),
        ["example.com"]
    );
    assert_eq!(decoded.repairs.skipped_ad_block_sites, 1);
}
