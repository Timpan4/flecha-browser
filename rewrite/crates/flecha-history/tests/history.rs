use std::fs;

use flecha_history::{HistoryError, HistoryStore, HistoryVisit, PAGE};
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn records_one_entry_per_url_and_counts_each_navigation() {
    let mut history = HistoryStore::in_memory().unwrap();
    let url = "https://example.test/";
    let with_credentials = "HTTPS://user:secret@EXAMPLE.TEST:443";
    assert!(
        history
            .record_visit(with_credentials, with_credentials, 20)
            .unwrap()
    );
    let initial = history.query("", 0).unwrap().entries.remove(0);
    assert_eq!(initial.url, url);
    assert_eq!(initial.title, url);

    assert!(history.record_visit(url, "Most recent title", 30).unwrap());
    assert!(history.record_visit(url, "Out-of-order title", 10).unwrap());
    assert!(history.record_visit(url, "Latest title", 30).unwrap());
    let page = history.query("", 0).unwrap();
    assert!(page.exhausted);
    assert_eq!(page.entries.len(), 1);
    let entry = &page.entries[0];
    assert_eq!(entry.url, url);
    assert_eq!(entry.title, "Latest title");
    assert_eq!(entry.visit_count, 4);
    assert_eq!(entry.last_visit_ms, 30);
}

#[test]
fn skips_invalid_urls_and_non_web_schemes() {
    let mut history = HistoryStore::in_memory().unwrap();
    for url in [
        "about:blank",
        "chrome://settings/",
        "chrome-extension://abc123/history.html",
        "data:text/html,sensitive",
        "JaVaScRiPt:alert(1)",
        "blob:https://example.test/id",
        "file:///home/user/document.html",
        "mailto:user@example.test",
        "https://",
        "unqualified/path",
    ] {
        assert!(!history.record_visit(url, "", 1).unwrap(), "{url}");
    }
    assert!(history.query("", 0).unwrap().entries.is_empty());
    assert!(
        history
            .record_visit("http://example.test/", "HTTP", 1)
            .unwrap()
    );
}

#[test]
fn matches_literal_substrings_and_pages_unique_urls_in_stable_order() {
    let mut history = HistoryStore::in_memory().unwrap();
    for index in 0..=PAGE * 2 {
        let url = format!("https://example.test/{index:03}");
        history.record_visit(&url, "earlier", 1).unwrap();
        history.record_visit(&url, "Résumé %_", 2).unwrap();
    }
    history
        .record_visit("https://another.test/elsewhere", "different", 3)
        .unwrap();

    let first = history.query("  RÉSUMÉ %_  ", 0).unwrap();
    let second = history.query("résumé %_", PAGE).unwrap();
    let last = history.query("résumé %_", PAGE * 2).unwrap();
    assert_eq!(first.entries.len(), 100);
    assert_eq!(second.entries.len(), 100);
    assert_eq!(last.entries.len(), 1);
    assert!(!first.exhausted && !second.exhausted && last.exhausted);

    let entries: Vec<_> = first
        .entries
        .into_iter()
        .chain(second.entries)
        .chain(last.entries)
        .collect();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.url.clone())
            .collect::<Vec<_>>(),
        (0..=200)
            .map(|index| format!("https://example.test/{index:03}"))
            .collect::<Vec<_>>(),
    );
    assert!(history.query("résumé %_", 201).unwrap().entries.is_empty());
    assert_eq!(
        history.query("ANOTHER.TEST/ELSE", 0).unwrap().entries[0].last_visit_ms,
        3
    );
    assert!(history.query("%__%", 0).unwrap().entries.is_empty());

    history.remove_url("https://example.test/200").unwrap();
    assert!(!history.query("résumé", PAGE).unwrap().exhausted);
    assert!(history.query("résumé", PAGE * 2).unwrap().exhausted);
}

#[test]
fn suggestions_limit_recent_matching_candidates_and_include_ranking_fields() {
    let mut history = HistoryStore::in_memory().unwrap();
    history
        .record_visit("https://examples.test/docs", "Docs", 10)
        .unwrap();
    history
        .record_visit("https://examples.test/docs", "Docs", 20)
        .unwrap();
    history
        .record_visit("https://docs.test/", "Reference", 30)
        .unwrap();
    history
        .record_visit("https://other.test/", "Unrelated", 40)
        .unwrap();

    let results = history.suggest(" DoCs ", 2).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].url, "https://docs.test/");
    assert_eq!(results[1].url, "https://examples.test/docs");
    assert_eq!(results[1].title, "Docs");
    assert_eq!(results[1].visit_count, 2);
    assert_eq!(results[1].last_visit_ms, 20);
    assert_eq!(history.suggest("docs", 1).unwrap(), results[..1]);
    assert!(history.suggest("docs", 0).unwrap().is_empty());
}

#[test]
fn clear_since_removes_inclusive_range_and_recounts_retained_visits() {
    let mut history = HistoryStore::in_memory().unwrap();
    let mixed = "https://mixed.test/";
    for time in [40, 10, 30, 20, 20] {
        history.record_visit(mixed, "", time).unwrap();
    }
    history
        .record_visit("https://expired.test/", "", 30)
        .unwrap();
    history.record_visit("https://older.test/", "", 25).unwrap();

    history.clear_since(30).unwrap();
    let page = history.query("", 0).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].url, "https://older.test/");
    assert_eq!(page.entries[0].visit_count, 1);
    assert_eq!(page.entries[0].last_visit_ms, 25);
    assert_eq!(page.entries[1].url, mixed);
    assert_eq!(page.entries[1].visit_count, 3);
    assert_eq!(page.entries[1].last_visit_ms, 20);

    history.clear_since(20).unwrap();
    let page = history.query("", 0).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].url, mixed);
    assert_eq!(page.entries[0].visit_count, 1);
    assert_eq!(page.entries[0].last_visit_ms, 10);
}

#[test]
fn remove_url_and_clear_all_delete_visits_as_well_as_url_rows() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut history = HistoryStore::open(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    let visit_count = || -> i64 {
        db.query_row("SELECT COUNT(*) FROM visits", [], |row| row.get(0))
            .unwrap()
    };
    let removed = HistoryVisit {
        url: "https://removed.test/",
        title: "",
        visited_at_ms: 10,
    };
    let retained = HistoryVisit {
        url: "https://retained.test/",
        title: "",
        visited_at_ms: 20,
    };
    history.import_visits(&[removed, retained]).unwrap();

    assert!(
        history
            .remove_url("https://user:secret@REMOVED.test:443")
            .unwrap()
    );
    assert!(!history.remove_url(removed.url).unwrap());
    assert_eq!(history.query("", 0).unwrap().entries[0].url, retained.url);
    assert_eq!(visit_count(), 1);

    history.clear_all().unwrap();
    assert!(history.query("", 0).unwrap().entries.is_empty());
    assert_eq!(visit_count(), 0);
}

#[test]
fn imports_visit_timestamps_idempotently_across_reopens() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let url = "https://example.test/";
    let visits = [
        HistoryVisit {
            url,
            title: "Imported newer",
            visited_at_ms: 30,
        },
        HistoryVisit {
            url,
            title: "Imported older",
            visited_at_ms: 10,
        },
        HistoryVisit {
            url: "https://EXAMPLE.test:443",
            title: "Duplicate",
            visited_at_ms: 30,
        },
        HistoryVisit {
            url,
            title: "Already recorded",
            visited_at_ms: 20,
        },
        HistoryVisit {
            url: "about:blank",
            title: "",
            visited_at_ms: 4,
        },
    ];
    {
        let mut history = HistoryStore::open(&path).unwrap();
        history.record_visit(url, "Local", 20).unwrap();
        assert_eq!(history.import_visits(&visits).unwrap(), (2, 3));
    }
    let mut history = HistoryStore::open(&path).unwrap();
    assert_eq!(history.import_visits(&visits).unwrap(), (0, visits.len()));
    let page = history.query("", 0).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].title, "Imported newer");
    assert_eq!(page.entries[0].visit_count, 3);
    assert_eq!(page.entries[0].last_visit_ms, 30);
    history.clear_since(20).unwrap();
    let page = history.query("", 0).unwrap();
    assert_eq!(page.entries[0].visit_count, 1);
    assert_eq!(page.entries[0].last_visit_ms, 10);
}

#[test]
fn wal_store_is_shared_across_connections_but_private_stores_are_separate() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let url = "https://example.test/";
    let mut first = HistoryStore::open(&path).unwrap();
    let mut second = HistoryStore::open(&path).unwrap();
    let inspector = Connection::open(&path).unwrap();
    let mode: String = inspector
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    first.record_visit(url, "First", 1).unwrap();
    second.record_visit(url, "Second", 2).unwrap();
    assert_eq!(first.query("", 0).unwrap().entries[0].visit_count, 2);

    let mut private = HistoryStore::in_memory().unwrap();
    assert!(private.query("", 0).unwrap().entries.is_empty());
    private.record_visit(url, "Private", 3).unwrap();
    assert_eq!(second.query("", 0).unwrap().entries[0].title, "Second");
    assert!(
        HistoryStore::in_memory()
            .unwrap()
            .query("", 0)
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn newer_schema_and_unrecognized_database_are_left_unchanged() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("newer.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE future (content TEXT); INSERT INTO future VALUES ('preserve');")
        .unwrap();
    drop(db);
    let original = fs::read(&path).unwrap();
    assert!(matches!(
        HistoryStore::open(&path),
        Err(HistoryError::UnrecognizedDatabase)
    ));
    assert_eq!(fs::read(&path).unwrap(), original);

    let db = Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    let original = fs::read(&path).unwrap();
    assert!(matches!(
        HistoryStore::open(&path),
        Err(HistoryError::UnsupportedSchema { found: 2 })
    ));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn corrupt_database_and_invalid_paths_return_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("corrupt.sqlite");
    let corrupted = b"not a SQLite database";
    fs::write(&path, corrupted).unwrap();
    assert!(matches!(
        HistoryStore::open(&path),
        Err(HistoryError::Database(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), corrupted);
    assert!(matches!(
        HistoryStore::open(dir.path()),
        Err(HistoryError::Database(_))
    ));
}

#[test]
fn failed_import_rolls_back_both_visits_and_url_summaries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut history = HistoryStore::open(&path).unwrap();
    let url = "https://example.test/";
    history.record_visit(url, "Before", 1).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_visit BEFORE INSERT ON visits WHEN NEW.visited_at_ms = 3
         BEGIN SELECT RAISE(ABORT, 'test write failure'); END;",
    )
    .unwrap();
    let visits = [
        HistoryVisit {
            url,
            title: "Updated",
            visited_at_ms: 2,
        },
        HistoryVisit {
            url: "https://another.test/",
            title: "New URL",
            visited_at_ms: 3,
        },
    ];
    assert!(matches!(
        history.import_visits(&visits),
        Err(HistoryError::Database(_))
    ));
    let page = history.query("", 0).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].title, "Before");
    assert_eq!(page.entries[0].visit_count, 1);
    assert_eq!(page.entries[0].last_visit_ms, 1);
    assert_eq!(history.import_visits(&visits[..1]).unwrap(), (1, 0));
}
