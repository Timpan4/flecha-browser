//! The schedule, the warm start from `engine.dat`, and the updater thread.

mod common;

use std::{
    fs, io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};

use common::{EASYLIST, EASYPRIVACY, FnDownload, RETRY, offline, response, serve, subscription};
use flecha_adblock::{
    AdBlocker, DownloadRequest, EngineOutcome, LISTS, MAX_AGE, Request, RequestType, Updater,
    UpdaterConfig, WireResponse, engine::CachedEngineError, update_once, warm_start,
};

fn ad_blocked(blocker: &AdBlocker, url: &str) -> bool {
    blocker.should_block(&Request {
        url,
        source_url: "https://news.example/",
        top_level_url: "https://news.example/",
        request_type: RequestType::Script,
    })
}

fn age(path: &Path, by: Duration) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - by).unwrap();
}

/// Serves the fixture lists and counts requests per list.
fn counting(
    counts: Arc<[AtomicUsize; 2]>,
) -> FnDownload<impl Fn(&str, &DownloadRequest) -> io::Result<WireResponse> + Send + Sync> {
    FnDownload(move |url: &str, _: &DownloadRequest| {
        let index = usize::from(url != LISTS[0].url);
        counts[index].fetch_add(1, Ordering::SeqCst);
        Ok(response(if index == 0 { EASYLIST } else { EASYPRIVACY }))
    })
}

#[test]
fn a_cold_start_parses_and_saves_and_the_next_launch_deserializes_without_downloading() {
    let cache = tempfile::tempdir().unwrap();
    let (first, _) = AdBlocker::new(true, &[]);
    assert!(matches!(
        warm_start(&first, cache.path()),
        Err(CachedEngineError::Missing)
    ));
    let counts = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let mut installed = None;
    let report = update_once(
        &first,
        cache.path(),
        false,
        &counting(Arc::clone(&counts)),
        &mut installed,
        RETRY,
    );
    assert!(
        matches!(
            report.engine,
            EngineOutcome::Parsed {
                cached: CachedEngineError::Missing,
                save_error: None,
                ..
            }
        ),
        "{report:?}"
    );
    assert!(report.next_check > MAX_AGE - Duration::from_secs(60) && report.next_check <= MAX_AGE);
    assert!(cache.path().join("engine.dat").exists());
    assert!(ad_blocked(&first, "https://ads.example/a.js"));

    // The next launch: blocking from engine.dat before any download, and nothing is due.
    let (second, _) = AdBlocker::new(true, &[]);
    let key = warm_start(&second, cache.path()).unwrap();
    assert!(ad_blocked(&second, "https://ads.example/a.js"));
    // Page exceptions come back with the engine.
    let on_trusted = Request {
        url: "https://ads.example/a.js",
        source_url: "https://trusted.example/",
        top_level_url: "https://trusted.example/",
        request_type: RequestType::Script,
    };
    assert!(!second.should_block(&on_trusted));
    let mut installed = Some(key);
    let report = update_once(
        &second,
        cache.path(),
        false,
        &offline(),
        &mut installed,
        RETRY,
    );
    assert!(
        matches!(report.engine, EngineOutcome::Unchanged),
        "{report:?}"
    );
    assert!(report.warnings.is_empty() && report.failures.is_empty());
    assert_eq!(
        counts[0].load(Ordering::SeqCst) + counts[1].load(Ordering::SeqCst),
        2
    );
}

#[test]
fn a_list_is_downloaded_again_once_its_copy_is_four_days_old() {
    let cache = tempfile::tempdir().unwrap();
    let (blocker, _) = AdBlocker::new(true, &[]);
    let counts = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let download = counting(Arc::clone(&counts));
    let mut installed = None;
    update_once(
        &blocker,
        cache.path(),
        false,
        &download,
        &mut installed,
        RETRY,
    );

    // Just under four days: still current, and due when it turns four days old.
    age(
        &cache.path().join("easylist.txt"),
        MAX_AGE - Duration::from_secs(3600),
    );
    let report = update_once(
        &blocker,
        cache.path(),
        false,
        &download,
        &mut installed,
        RETRY,
    );
    assert_eq!(counts[0].load(Ordering::SeqCst), 1);
    assert!(
        report.next_check <= Duration::from_secs(3600)
            && report.next_check > Duration::from_secs(3500),
        "{report:?}"
    );

    age(&cache.path().join("easylist.txt"), MAX_AGE);
    let report = update_once(
        &blocker,
        cache.path(),
        false,
        &download,
        &mut installed,
        RETRY,
    );
    assert_eq!(counts[0].load(Ordering::SeqCst), 2);
    assert_eq!(counts[1].load(Ordering::SeqCst), 1);
    // Same text again: the installed engine stays.
    assert!(
        matches!(report.engine, EngineOutcome::Unchanged),
        "{report:?}"
    );
    // A copy dated in the future counts as current.
    let future = fs::File::options()
        .write(true)
        .open(cache.path().join("easyprivacy.txt"))
        .unwrap();
    future
        .set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    update_once(
        &blocker,
        cache.path(),
        false,
        &download,
        &mut installed,
        RETRY,
    );
    assert_eq!(counts[1].load(Ordering::SeqCst), 1);
}

#[test]
fn a_failed_refresh_waits_for_the_retry_instead_of_looping() {
    let cache = tempfile::tempdir().unwrap();
    let (blocker, _) = AdBlocker::new(true, &[]);
    let retry = Duration::from_secs(1234);
    let mut installed = None;
    let report = update_once(
        &blocker,
        cache.path(),
        false,
        &offline(),
        &mut installed,
        retry,
    );
    assert!(matches!(report.engine, EngineOutcome::NotBuilt));
    assert_eq!(report.failures.len(), 2);
    assert_eq!(report.next_check, retry);
    assert!(!blocker.has_engine());

    update_once(
        &blocker,
        cache.path(),
        false,
        &serve(EASYLIST, EASYPRIVACY),
        &mut installed,
        retry,
    );
    age(&cache.path().join("easylist.txt"), MAX_AGE);
    let report = update_once(
        &blocker,
        cache.path(),
        false,
        &offline(),
        &mut installed,
        retry,
    );
    assert!(report.failures.is_empty());
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.next_check, retry);
    assert!(ad_blocked(&blocker, "https://ads.example/a.js"));
}

#[test]
fn a_damaged_or_outdated_engine_file_is_rebuilt_from_the_lists() {
    let cache = tempfile::tempdir().unwrap();
    let (blocker, _) = AdBlocker::new(true, &[]);
    update_once(
        &blocker,
        cache.path(),
        false,
        &serve(EASYLIST, EASYPRIVACY),
        &mut None,
        RETRY,
    );
    let engine_file = cache.path().join("engine.dat");
    let good = fs::read(&engine_file).unwrap();

    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xff;
    let truncated = good[..good.len() / 2].to_vec();
    for damaged in [flipped, truncated] {
        fs::write(&engine_file, &damaged).unwrap();
        let (fresh, _) = AdBlocker::new(true, &[]);
        assert!(matches!(
            warm_start(&fresh, cache.path()),
            Err(CachedEngineError::Deserialize(_))
        ));
        assert!(!fresh.has_engine());
        let report = update_once(&fresh, cache.path(), false, &offline(), &mut None, RETRY);
        assert!(
            matches!(
                report.engine,
                EngineOutcome::Parsed {
                    cached: CachedEngineError::Deserialize(_),
                    ..
                }
            ),
            "{report:?}"
        );
        assert!(ad_blocked(&fresh, "https://ads.example/a.js"));
        assert_eq!(fs::read(&engine_file).unwrap(), good);
    }

    // New list text: the old engine file no longer matches.
    let newer = Box::leak(subscription(LISTS[0].url, "||newer.example^").into_boxed_str());
    let mut installed = None;
    let report = update_once(
        &blocker,
        cache.path(),
        true,
        &serve(newer, EASYPRIVACY),
        &mut installed,
        RETRY,
    );
    assert!(
        matches!(
            report.engine,
            EngineOutcome::Parsed {
                cached: CachedEngineError::StaleKey,
                ..
            }
        ),
        "{report:?}"
    );
    assert!(ad_blocked(&blocker, "https://newer.example/a.js"));
    assert!(!ad_blocked(&blocker, "https://ads.example/a.js"));
}

#[test]
fn the_updater_thread_installs_refreshes_and_stops() {
    let cache = tempfile::tempdir().unwrap();
    let (blocker, _) = AdBlocker::new(true, &[]);
    let counts = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let (reports, received) = mpsc::channel();
    let config = UpdaterConfig {
        cache_dir: cache.path().to_owned(),
        retry_after_failure: RETRY,
    };
    let updater = Updater::spawn(
        blocker.clone(),
        config,
        Box::new(counting(Arc::clone(&counts))),
        move |report| {
            let _ = reports.send(report);
        },
    )
    .unwrap();
    let report = received.recv().unwrap();
    assert!(
        matches!(report.engine, EngineOutcome::Parsed { .. }),
        "{report:?}"
    );
    assert!(ad_blocked(&blocker, "https://ads.example/a.js"));

    updater.refresh_now();
    let report = received.recv().unwrap();
    assert!(
        matches!(report.engine, EngineOutcome::Unchanged),
        "{report:?}"
    );
    assert_eq!(counts[0].load(Ordering::SeqCst), 2);
    updater.shutdown();
    assert!(received.recv().is_err());

    // The next launch reports its warm start first.
    let (relaunched, _) = AdBlocker::new(true, &[]);
    let (reports, received) = mpsc::channel();
    let config = UpdaterConfig {
        cache_dir: cache.path().to_owned(),
        retry_after_failure: RETRY,
    };
    let updater = Updater::spawn(
        relaunched.clone(),
        config,
        Box::new(offline()),
        move |report| {
            let _ = reports.send(report);
        },
    )
    .unwrap();
    assert!(matches!(
        received.recv().unwrap().engine,
        EngineOutcome::Deserialized
    ));
    assert!(matches!(
        received.recv().unwrap().engine,
        EngineOutcome::Unchanged
    ));
    assert!(ad_blocked(&relaunched, "https://ads.example/a.js"));
    updater.shutdown();
}
