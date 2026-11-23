//! Differential test of the per-site switch against the Chromium shell's TypeScript, recorded by
//! `scripts/generate-site-fixture.ts` under Bun: `URL.hostname`, `isSiteHost`, `blocksAdsOn`,
//! `withAdBlocking` and `decodeState`.
//!
//! The Rust side gets the raw strings and goes through the public entry points: `site_host`,
//! `AdBlocker::should_block` and flecha-core's settings and storage.
//!
//! Network filter matching isn't compared here. The shell matched with declarativeNetRequest,
//! which runs only inside Chromium; `engine.rs` and `lists.rs` cover the filters.
//!
//! Deliberate divergences, both from the shell accepting hostnames Chromium then refuses:
//! - An allowed entry that `isSiteHost` refuses (`news.example.`) is dropped, so it allows
//!   nothing. The shell appended it unchecked and declarativeNetRequest refused the whole rule.
//! - Turning blocking off or on for such a host is refused. The shell appended it, and its
//!   saved list was dropped on the next launch.
//! - A saved list with refused entries keeps its valid ones. The shell dropped the whole list.

mod common;

use flecha_adblock::{AdBlocker, Request, RequestType, SiteAllowList, site_host};
use flecha_core::{BrowserState, SpaceId, WindowId, blocks_ads_on, is_site_host, storage};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/site-parity.jsonl");

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap().to_owned())
        .collect()
}

fn rows() -> Vec<Value> {
    let mut lines = FIXTURE.lines();
    let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    let rows: Vec<Value> = lines
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut total = 0;
    for kind in ["url", "host", "saved"] {
        let expected = header["counts"][kind].as_u64().unwrap() as usize;
        assert!(expected > 0, "{kind}");
        assert_eq!(
            rows.iter().filter(|row| row["kind"] == kind).count(),
            expected,
            "{kind}"
        );
        total += expected;
    }
    assert_eq!(rows.len(), total);
    rows
}

#[test]
fn the_corpus_covers_the_classes_that_broke_earlier_ports() {
    let rows = rows();
    let has = |kind: &str, field: &str, needle: &str| {
        rows.iter()
            .any(|row| row["kind"] == kind && row[field].as_str().unwrap().contains(needle))
    };
    // U+017F, U+212A, fullwidth letters and dot, ideographic dot, and whitespace JS and Rust
    // trim differently.
    for needle in [
        "\u{17f}", "\u{212a}", "\u{ff4e}", "\u{ff0e}", "\u{3002}", "\u{feff}", "\u{85}",
        "\u{3000}", "\t", "%2e",
    ] {
        assert!(has("url", "url", needle), "url {needle:?}");
        assert!(has("host", "host", needle), "host {needle:?}");
    }
}

#[test]
fn page_hosts_and_decisions_match_url_hostname_and_blocks_ads_on() {
    let (blocker, _cache) = common::loaded(true, &[]);
    for row in rows().iter().filter(|row| row["kind"] == "url") {
        let url = row["url"].as_str().unwrap();
        let allowed = strings(&row["allowed"]);
        let ts_host = row["tsHost"].as_str().map(str::to_owned);
        assert_eq!(site_host(url), ts_host, "{url:?}");

        let expected = row["tsBlocksValidOnly"].as_bool().unwrap();
        if allowed.iter().all(|site| is_site_host(site)) {
            assert_eq!(row["tsBlocks"], row["tsBlocksValidOnly"], "{row}");
        }
        assert_eq!(
            SiteAllowList::new(&allowed).0.blocks_ads_on_page(url),
            expected,
            "{row}"
        );
        blocker.set_allowed_sites(&allowed);
        let ad = Request {
            url: "https://ads.example/a.js",
            source_url: url,
            top_level_url: url,
            frame_parent_url: None,
            request_type: RequestType::Script,
        };
        assert_eq!(blocker.should_block(&ad), expected, "{row}");
    }
}

#[test]
fn raw_hosts_and_edits_match_is_site_host_and_with_ad_blocking() {
    for row in rows().iter().filter(|row| row["kind"] == "host") {
        let host = row["host"].as_str().unwrap();
        let allowed = strings(&row["allowed"]);
        let valid = row["tsSiteHost"].as_bool().unwrap();
        assert_eq!(is_site_host(host), valid, "{host:?}");
        assert_eq!(
            blocks_ads_on(Some(&allowed), host),
            row["tsBlocks"].as_bool().unwrap(),
            "{row}"
        );
        for (block, ts) in [(false, "tsOff"), (true, "tsOn")] {
            let mut settings = BrowserState::new(SpaceId::new("s"), WindowId::new("w")).settings;
            settings.ad_block_allowed = Some(allowed.clone());
            let accepted = settings.set_ad_blocking_on(host, block);
            assert_eq!(accepted, valid, "{row}");
            let expected = if valid {
                strings(&row[ts])
            } else {
                allowed.clone()
            };
            assert_eq!(settings.ad_block_allowed.unwrap(), expected, "{row} {ts}");
        }
        let (blocker, dropped) = AdBlocker::new(true, &[host.to_owned()]);
        assert_eq!(dropped, usize::from(!valid), "{host:?}");
        drop(blocker);
    }
}

#[test]
fn saved_lists_decode_like_decode_state_except_that_bad_entries_cost_only_themselves() {
    let mut state = BrowserState::new(SpaceId::new("s"), WindowId::new("w"));
    state.settings.ad_block_allowed = Some(Vec::new());
    let template = storage::encode(&state).unwrap();
    let placeholder = "\"adBlockAllowed\": []";
    assert_eq!(template.matches(placeholder).count(), 1);
    for row in rows().iter().filter(|row| row["kind"] == "saved") {
        let value = &row["value"];
        let json = template.replace(placeholder, &format!("\"adBlockAllowed\": {value}"));
        let decoded = storage::decode_with_repairs(&json).unwrap();
        let rust = decoded.state.settings.ad_block_allowed;
        match row["tsValidEntries"].as_array() {
            None => {
                assert_eq!(row["tsDecoded"], Value::Null, "{row}");
                assert_eq!(rust, None, "{row}");
            }
            Some(_) => {
                let valid = strings(&row["tsValidEntries"]);
                if !row["tsDecoded"].is_null() {
                    assert_eq!(strings(&row["tsDecoded"]), valid, "{row}");
                }
                assert_eq!(rust.as_deref(), Some(valid.as_slice()), "{row}");
                let total = value.as_array().unwrap().len();
                assert_eq!(
                    decoded.repairs.skipped_ad_block_sites,
                    total - valid.len(),
                    "{row}"
                );
            }
        }
    }
}
