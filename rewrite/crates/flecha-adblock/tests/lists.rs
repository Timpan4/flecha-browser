//! Ports of `chromium/tests/build-adblock.test.mjs`.
//!
//! Not ported: "reverse download completion preserves EasyList priority, rule IDs and the shared
//! budget" tests declarativeNetRequest rule IDs and Chromium's 30,000-rule limit, which don't
//! exist with adblock-rust. The redirect and partial-body test is in `transport.rs`, against a
//! local server.

mod common;

use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    sync::Mutex,
};

use common::{FnDownload, loaded, offline, response, response_with, subscription};
use flecha_adblock::{
    AdBlocker, DownloadRequest, LISTS, WireResponse,
    fetch::{FetchReport, ListFailure, ListSource, ListWarning, fetch_lists},
};

const EASYLIST: &str = LISTS[0].url;
const EASYPRIVACY: &str = LISTS[1].url;

/// A bad EasyList response and the reason it must report.
type Invalid = (Box<dyn Fn() -> WireResponse + Send + Sync>, &'static str);
/// A Content-Encoding and its encoder.
type Encoder = (&'static str, fn(&str) -> Vec<u8>);

fn cached(cache: &std::path::Path, id: &str) -> String {
    fs::read_to_string(cache.join(format!("{id}.txt"))).unwrap()
}

fn used_cache(report: &FetchReport) -> Vec<String> {
    report
        .warnings
        .iter()
        .filter(|warning| matches!(warning, ListWarning::UsedCache { .. }))
        .map(ToString::to_string)
        .collect()
}

/// Every list's download failed for `reason`, and its cached copy was used.
fn assert_used_cache(report: &FetchReport, reason: &str) {
    let warnings = used_cache(report);
    assert_eq!(warnings.len(), LISTS.len(), "{report:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains(reason) && w.contains("using the cached copy")),
        "{reason}: {warnings:?}"
    );
}

/// Every list's download failed with `reason`, with no cached copy to fall back on.
fn assert_no_cache(report: FetchReport, reason: &str) {
    let failures = report.lists.unwrap_err();
    assert_eq!(failures.len(), LISTS.len(), "{reason}: {failures:?}");
    assert!(
        failures
            .iter()
            .all(|f| matches!(f, ListFailure::NoCache { download, .. } if download == reason)),
        "{reason}: {failures:?}"
    );
}

#[test]
fn cosmetic_filters_keep_independent_selectors_cross_list_exceptions_and_site_exemptions() {
    let list = "/network\n##.ad\n## .ad\n##.unsupported:unknown()\n##.remove-me\nsite.example##.domain-only\n@@||site.example^$generichide";
    let privacy = "/tracker\n##.ad\n##[data-text=\"two words\"]\nsite.example#@#.remove-me";
    let (blocker, _) = AdBlocker::new(true, &[]);
    let cache = tempfile::tempdir().unwrap();
    let download = FnDownload(move |url: &str, _: &DownloadRequest| {
        Ok(response(subscription(
            url,
            if url == EASYLIST { list } else { privacy },
        )))
    });
    let report = common::update(&blocker, cache.path(), &download);
    assert!(report.failures.is_empty(), "{report:?}");

    let page = "https://other.example/";
    let filters = blocker.cosmetic_filters(page);
    assert!(!filters.generic_hide);
    assert_eq!(filters.hide_selectors, ["[data-text=\"two words\"]"]);
    // Selectors that start with a class come back when the page reports that class.
    let classes = [
        "ad".to_owned(),
        "remove-me".to_owned(),
        "unsupported".to_owned(),
    ];
    // Divergence: an exception applies on its own site, not everywhere.
    assert_eq!(
        blocker.hidden_class_id_selectors(page, &classes, &[], &filters.exceptions),
        [".ad", ".remove-me", ".unsupported:unknown()"]
    );

    // A site the lists exempt from generic hiding keeps only its own selectors. Divergence:
    // build-adblock.mjs left out `site##sel`.
    for site in ["https://site.example/", "https://www.site.example/"] {
        let filters = blocker.cosmetic_filters(site);
        assert!(filters.generic_hide, "{site}");
        assert_eq!(filters.hide_selectors, [".domain-only"], "{site}");
    }
    let filters = blocker.cosmetic_filters("https://site.example/");
    assert!(filters.exceptions.contains(".remove-me"));
}

#[test]
fn failed_refresh_requires_a_cached_list_with_supported_network_rules() {
    let cache = tempfile::tempdir().unwrap();
    let cache = cache.path();
    let report = fetch_lists(cache, false, &offline());
    let failures = report.lists.unwrap_err();
    assert_eq!(failures.len(), 2);
    assert!(failures.iter().all(
        |failure| matches!(failure, ListFailure::NoCache { download, .. } if download == "offline")
    ));
    assert!(failures[0].to_string().contains("no cache exists"));

    let list = |url: &str| {
        subscription(
            url,
            if url == EASYLIST {
                "||ads.example^\n/banner"
            } else {
                "@@/allow"
            },
        )
    };
    let serve = FnDownload(move |url: &str, _: &DownloadRequest| Ok(response(list(url))));
    assert!(fetch_lists(cache, false, &serve).lists.is_ok());
    let report = fetch_lists(cache, true, &offline());
    let lists = report.lists.unwrap();
    assert_eq!(lists.len(), LISTS.len());
    assert!(
        lists
            .iter()
            .all(|list| list.source == ListSource::CacheAfterFailedRefresh)
    );

    let prefix = subscription(EASYLIST, "||ads.example^");
    let full_length = list(EASYLIST).len().to_string();
    let invalid: [Invalid; 3] = [
        (
            Box::new({
                let prefix = prefix.clone();
                move || response_with(prefix.clone(), &[])
            }),
            "no verifiable length",
        ),
        (
            Box::new({
                let prefix = prefix.clone();
                move || {
                    response_with(
                        prefix.clone(),
                        &[
                            ("content-encoding", "identity"),
                            ("content-length", &full_length),
                        ],
                    )
                }
            }),
            "could not verify Content-Length",
        ),
        (
            Box::new({
                let prefix = prefix.replace("! Title:", "! Checksum: invalid\n! Title:");
                move || response(prefix.clone())
            }),
            "ABP checksum mismatch",
        ),
    ];
    for (bad, cause) in invalid {
        let download = FnDownload(move |url: &str, _: &DownloadRequest| {
            Ok(if url == EASYLIST {
                bad()
            } else {
                response(list(url))
            })
        });
        let report = fetch_lists(cache, true, &download);
        assert!(report.lists.is_ok(), "{report:?}");
        let warnings = used_cache(&report);
        assert!(
            warnings.iter().any(|warning| warning.contains(cause) && warning.contains("using the cached copy")),
            "{warnings:?}"
        );
        assert_eq!(cached(cache, "easylist"), list(EASYLIST));
    }

    fs::write(
        cache.join("easyprivacy.txt"),
        subscription(EASYPRIVACY, "! truncated"),
    )
    .unwrap();
    let report = fetch_lists(cache, false, &offline());
    assert!(used_cache(&report).is_empty());
    let failures = report.lists.unwrap_err();
    assert_eq!(
        failures,
        [ListFailure::InvalidCache {
            id: "easyprivacy",
            download: "offline".into(),
            cache: "filter list contains no supported network rules".into(),
        }]
    );
    assert!(failures[0].to_string().contains("easyprivacy"));
}

#[test]
fn an_abp_checksum_validates_normalized_text_without_a_length_and_identity_verifies_utf8() {
    let signed = "[Adblock Plus 2.0]\r\n! Checksum: z2RKQ5QVHnDHGokvvaKKdg\r\n! Title: EasyList\r\n\r\n||ads.example^\r\n";
    let cache = tempfile::tempdir().unwrap();
    let download = FnDownload(move |url: &str, request: &DownloadRequest| {
        assert_eq!(request.accept_encoding, "gzip");
        Ok(if url == EASYLIST {
            response_with(signed, &[])
        } else {
            response(subscription(url, "! précision\n@@/allow"))
        })
    });
    assert!(fetch_lists(cache.path(), false, &download).lists.is_ok());
    assert_eq!(cached(cache.path(), "easylist"), signed);
}

fn gzip(text: &str) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

fn deflate(text: &str) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

fn brotli(text: &str) -> Vec<u8> {
    // Made with Node's brotliCompressSync: no brotli compressor is in the dependency tree.
    let file = if text.contains("EasyList") {
        "easylist"
    } else {
        "easyprivacy"
    };
    assert!(text.ends_with("/old-rule\n"));
    fs::read(format!(
        "{}/tests/fixtures/compressed/{file}-old-rule.txt.br",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn compressed(bytes: Vec<u8>, encoding: &str) -> WireResponse {
    let length = bytes.len().to_string();
    response_with(
        bytes,
        &[("content-encoding", encoding), ("content-length", &length)],
    )
}

#[test]
fn compressed_wire_data_is_decoded_and_a_truncated_refresh_keeps_the_verified_cache() {
    let cache = tempfile::tempdir().unwrap();
    let cache = cache.path();
    let original = |url: &str| subscription(url, "/old-rule");
    let encoders: [Encoder; 3] = [("gzip", gzip), ("deflate", deflate), ("br", brotli)];
    for (encoding, compress) in encoders {
        let download = FnDownload(move |url: &str, _: &DownloadRequest| {
            Ok(compressed(compress(&original(url)), encoding))
        });
        let report = fetch_lists(cache, true, &download);
        assert!(report.lists.is_ok(), "{encoding}: {report:?}");
        assert_eq!(cached(cache, "easylist"), original(EASYLIST), "{encoding}");
    }
    // Header values are trimmed and compared without ASCII case.
    let download = FnDownload(move |url: &str, _: &DownloadRequest| {
        Ok(compressed(gzip(&original(url)), " GZip\t"))
    });
    assert!(fetch_lists(cache, true, &download).lists.is_ok());

    // A stream cut short, with a Content-Length that matches what was sent: by its last byte, and
    // by its whole gzip (CRC-32 and size, 8 bytes) or zlib (Adler-32, 4 bytes) trailer.
    let cuts = [
        ("gzip", 1),
        ("gzip", 8),
        ("deflate", 1),
        ("deflate", 4),
        ("br", 1),
    ];
    for (encoding, cut) in cuts {
        let compress = encoders
            .iter()
            .find(|(name, _)| *name == encoding)
            .unwrap()
            .1;
        let truncated = FnDownload(move |url: &str, _: &DownloadRequest| {
            let mut bytes = compress(&original(url).replace("/old-rule", "/new-rule\n/old-rule"));
            bytes.truncate(bytes.len() - cut);
            Ok(compressed(bytes, encoding))
        });
        assert_used_cache(
            &fetch_lists(cache, true, &truncated),
            "unexpected end of file",
        );
        for list in LISTS {
            assert_eq!(
                cached(cache, list.id),
                original(list.url),
                "{encoding} cut by {cut}"
            );
        }
        let empty = tempfile::tempdir().unwrap();
        assert_no_cache(
            fetch_lists(empty.path(), false, &truncated),
            "unexpected end of file",
        );
    }
}

#[test]
fn an_unknown_or_repeated_content_encoding_is_refused() {
    let cache = tempfile::tempdir().unwrap();
    for encoding in ["compress", "gzip, gzip", ""] {
        let download = FnDownload(move |url: &str, _: &DownloadRequest| {
            Ok(compressed(gzip(&subscription(url, "/rule")), encoding))
        });
        let failures = fetch_lists(cache.path(), true, &download)
            .lists
            .unwrap_err();
        let expected = format!("unsupported Content-Encoding: {encoding}");
        assert!(
            failures.iter().all(
                |f| matches!(f, ListFailure::NoCache { download, .. } if *download == expected)
            ),
            "{failures:?}"
        );
    }
    // Node joins repeated headers with ", ".
    let download = FnDownload(|url: &str, _: &DownloadRequest| {
        let bytes = gzip(&subscription(url, "/rule"));
        let length = bytes.len().to_string();
        Ok(response_with(
            bytes,
            &[
                ("Content-Encoding", "gzip"),
                ("content-encoding", "br"),
                ("content-length", &length),
            ],
        ))
    });
    let failures = fetch_lists(cache.path(), true, &download)
        .lists
        .unwrap_err();
    assert!(
        failures[0]
            .to_string()
            .contains("unsupported Content-Encoding: gzip, br"),
        "{failures:?}"
    );
}

struct Oversized(bool);

impl Read for Oversized {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        assert!(!self.0, "must stop reading once Content-Length is exceeded");
        self.0 = true;
        let chunk = b"oversized chunk";
        buf[..chunk.len()].copy_from_slice(chunk);
        Ok(chunk.len())
    }
}

#[test]
fn wire_data_exceeding_content_length_is_refused_before_reading_more() {
    let cache = tempfile::tempdir().unwrap();
    let download = FnDownload(|_: &str, _: &DownloadRequest| {
        Ok(WireResponse {
            status: 200,
            headers: vec![("content-length".into(), "1".into())],
            body: Box::new(Oversized(false)),
        })
    });
    assert_no_cache(
        fetch_lists(cache.path(), false, &download),
        "response exceeded Content-Length",
    );
    for length in ["+1", "1.0", " 1", "١", "0x1"] {
        let download = FnDownload(move |url: &str, _: &DownloadRequest| {
            Ok(response_with(
                subscription(url, "/rule"),
                &[("content-length", length)],
            ))
        });
        assert_no_cache(
            fetch_lists(cache.path(), false, &download),
            "could not verify Content-Length",
        );
    }
}

#[test]
fn redirects_stay_on_https_share_one_deadline_and_stop_after_twenty() {
    let cache = tempfile::tempdir().unwrap();
    let deadlines = Mutex::new(HashSet::new());
    let seen = Mutex::new(Vec::new());
    let download = FnDownload(|url: &str, request: &DownloadRequest| {
        deadlines
            .lock()
            .unwrap()
            .insert((url.contains("privacy"), request.deadline));
        seen.lock().unwrap().push(url.to_owned());
        let hops = url.matches("/hop").count();
        Ok(if hops < 2 {
            WireResponse {
                status: [301, 302, 303, 307, 308][hops],
                headers: vec![("location".into(), format!("{url}/hop"))],
                body: Box::new(io::empty()),
            }
        } else {
            response(subscription(
                if url.contains("privacy") {
                    EASYPRIVACY
                } else {
                    EASYLIST
                },
                "/rule",
            ))
        })
    });
    assert!(fetch_lists(cache.path(), false, &download).lists.is_ok());
    // One deadline per list, shared by its redirects.
    assert_eq!(deadlines.lock().unwrap().len(), 2);
    assert!(
        seen.lock()
            .unwrap()
            .contains(&format!("{EASYLIST}/hop/hop"))
    );

    let to_http = FnDownload(|_: &str, _: &DownloadRequest| {
        Ok(WireResponse {
            status: 302,
            headers: vec![("location".into(), "http://easylist.to/x".into())],
            body: Box::new(io::empty()),
        })
    });
    assert_used_cache(
        &fetch_lists(cache.path(), true, &to_http),
        "filter list redirect requires HTTPS",
    );

    let count = Mutex::new(0);
    let endless = FnDownload(|url: &str, _: &DownloadRequest| {
        if url.contains("easylist.txt") {
            *count.lock().unwrap() += 1;
        }
        Ok(WireResponse {
            status: 307,
            headers: vec![("location".into(), url.to_owned())],
            body: Box::new(io::empty()),
        })
    });
    assert_used_cache(
        &fetch_lists(cache.path(), true, &endless),
        "too many HTTP redirects",
    );
    // The first request and twenty redirects.
    assert_eq!(*count.lock().unwrap(), 21);

    // A redirect status without Location is the response.
    let bare = FnDownload(|_: &str, _: &DownloadRequest| {
        Ok(WireResponse {
            status: 302,
            headers: Vec::new(),
            body: Box::new(io::empty()),
        })
    });
    assert_used_cache(&fetch_lists(cache.path(), true, &bare), "HTTP 302");
}

#[test]
fn an_invalid_fresh_cache_is_downloaded_again() {
    let (_, cache) = loaded(true, &[]);
    fs::write(cache.path().join("easylist.txt"), "not a list").unwrap();
    let serve = common::serve(common::EASYLIST, common::EASYPRIVACY);
    let report = fetch_lists(cache.path(), false, &serve);
    let lists = report.lists.unwrap();
    assert_eq!(lists[0].source, ListSource::Downloaded);
    assert_eq!(lists[1].source, ListSource::Cache);
    assert_eq!(
        report.warnings,
        [ListWarning::InvalidCache {
            id: "easylist",
            error: "not a filter list".into()
        }]
    );
    assert_eq!(cached(cache.path(), "easylist"), common::EASYLIST);
}
