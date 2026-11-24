use flecha_history::HistoryStore;

#[test]
fn a_title_change_updates_the_same_visit_and_its_search_text() {
    let mut history = HistoryStore::in_memory().unwrap();
    let url = "https://example.test/";
    let old = history.record_navigation(url, "Old", 1).unwrap().unwrap();
    let recent = history
        .record_navigation(url, "Initial", 2)
        .unwrap()
        .unwrap();
    assert!(history.update_visit_title(&recent, "Final title").unwrap());
    assert!(
        history
            .update_visit_title(&old, "Older final title")
            .unwrap()
    );
    assert!(!history.update_visit_title(&recent, "Final title").unwrap());

    let page = history.query("FINAL TITLE", 0).unwrap();
    assert_eq!(page.entries.len(), 1);
    let entry = &page.entries[0];
    assert_eq!(
        (entry.title.as_str(), entry.visit_count, entry.last_visit_ms),
        ("Final title", 2, 2)
    );
    assert!(history.query("Initial", 0).unwrap().entries.is_empty());

    history.clear_since(2).unwrap();
    let page = history.query("Older final title", 0).unwrap();
    assert_eq!(page.entries[0].visit_count, 1);
}

#[test]
fn removed_visits_cannot_receive_title_changes_even_when_sqlite_reuses_the_id() {
    let mut history = HistoryStore::in_memory().unwrap();
    let url = "https://example.test/";
    let handle = history
        .record_navigation(url, "Removed", 1)
        .unwrap()
        .unwrap();
    assert!(history.remove_url(url).unwrap());
    assert!(
        !history
            .update_visit_title(&handle, "No resurrection")
            .unwrap()
    );
    assert!(history.query("", 0).unwrap().entries.is_empty());

    let handle_after_delete = history
        .record_navigation(url, "Replacement", 1)
        .unwrap()
        .unwrap();
    assert!(!history.update_visit_title(&handle, "Stale update").unwrap());
    assert_eq!(
        history.query("", 0).unwrap().entries[0].title,
        "Replacement"
    );

    history.clear_since(1).unwrap();
    assert!(
        !history
            .update_visit_title(&handle_after_delete, "No resurrection")
            .unwrap()
    );
    assert!(history.query("", 0).unwrap().entries.is_empty());
    let handle_after_range = history
        .record_navigation(url, "Cleared", 1)
        .unwrap()
        .unwrap();
    history.clear_all().unwrap();
    assert!(
        !history
            .update_visit_title(&handle_after_range, "No resurrection")
            .unwrap()
    );
    assert!(history.query("", 0).unwrap().entries.is_empty());
}
