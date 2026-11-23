use std::fs;
use std::path::Path;

use flecha_import::{Bookmark, Browser, BrowserLocations, Platform, read_arc, read_browser};
use rusqlite::Connection;
use serde_json::{Value, json};

#[test]
fn overflowing_numbers_only_affect_their_own_fields() {
    let home = tempfile::tempdir().unwrap();
    let (locations, bookmarks) = chromium(home.path());
    fs::write(
        bookmarks,
        chromium_json(r#"{"type":"url","url":"https://example.org/","date_added":1e400}"#),
    )
    .unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    assert_eq!(plan.bookmark_count, 1);
    assert_number_repair(&plan.warnings, "Bookmarks");

    let arc = tempfile::tempdir().unwrap();
    let sidebar = json!({"sidebar":{"containers":[{
        "spaces":[space("space", "root")],
        "items":[container("root", &["tab"]), arc_tab("tab")],
    }]}});
    fs::write(
        arc.path().join("StorableSidebar.json"),
        format!(r#"{{"ignored":1e400,{}"#, &sidebar.to_string()[1..]),
    )
    .unwrap();
    fs::write(
        arc.path().join("StorableArchiveItems.json"),
        r#"{"items":[
            {"archivedAt":1e400,"sidebarItem":{"data":{"tab":{"savedURL":"https://example.org/bad"}}}},
            {"archivedAt":10,"ignored":1e400,"sidebarItem":{"data":{"tab":{"savedURL":"https://example.org/ok"}}}}
        ]}"#,
    )
    .unwrap();
    let (plan, warnings) = read_arc(arc.path()).unwrap();
    assert_eq!(plan.spaces[0].pinned.len(), 1);
    assert_eq!(plan.archive.len(), 1);
    assert_eq!(plan.archive[0].url, "https://example.org/ok");
    assert_number_repair(&warnings, "Arc’s sidebar");
    assert_number_repair(&warnings, "Arc’s archived tabs");
}

#[test]
fn chromium_keeps_200_folders_and_recovers_after_skipping_deeper_subtrees() {
    let home = tempfile::tempdir().unwrap();
    let (locations, bookmarks) = chromium(home.path());
    let leaf = |name| format!(r#"{{"type":"url","url":"https://example.org/{name}"}}"#);
    let folder = r#"{"type":"folder","name":"F","children":["#;
    let chain = |depth| {
        format!(
            "{}{}{}",
            folder.repeat(depth),
            leaf("nested"),
            "]}".repeat(depth)
        )
    };

    // Chromium's JSON depth is 200. A bookmark folder spans two JSON containers.
    for (depth, expected_size) in [(200, 3), (201, 2)] {
        let nodes = format!("{},{},{}", leaf("before"), chain(depth), leaf("after"));
        fs::write(&bookmarks, chromium_json(&nodes)).unwrap();
        let plan = read_browser(Browser::Chrome, &locations);
        assert_eq!(plan.bookmark_count, expected_size, "folder depth {depth}");
        let tabs = plan.bookmarks.as_deref().unwrap();
        assert!(matches!(tabs.first(), Some(Bookmark::Tab(tab)) if tab.url.ends_with("/before")));
        assert!(matches!(tabs.last(), Some(Bookmark::Tab(tab)) if tab.url.ends_with("/after")));
        assert_eq!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("JSON subtrees beyond the nesting limit")),
            depth > 200,
        );
    }
}

#[test]
fn nested_arc_splits_copy_each_tab_at_most_once() {
    let root = tempfile::tempdir().unwrap();
    // The verifier measured the nested layout at these input sizes.
    for count in [1000, 2000] {
        let mut items = vec![container("root", &["s0"])];
        for index in 0..count {
            let id = format!("s{index}");
            let tab = format!("t{index}");
            let mut children = vec![tab.clone()];
            if index + 1 < count {
                children.push(format!("s{}", index + 1));
            }
            items.push(json!({"id":id,"data":{"splitView":{}},"childrenIds":children}));
            items.push(arc_tab(&tab));
        }
        write_arc(root.path(), items, vec![space("space", "root")]);
        let (arc, _) = read_arc(root.path()).unwrap();
        assert_eq!(arc.spaces[0].pinned.len(), count);
        let splits = &arc.spaces[0].splits;
        assert_eq!(splits.iter().map(Vec::len).sum::<usize>(), 2);
        assert_eq!(
            splits[0],
            [
                format!("https://example.org/t{}", count - 2),
                format!("https://example.org/t{}", count - 1)
            ]
        );
        assert_eq!(arc.skipped["overlapping split"], count - 2);
    }
}

#[test]
fn arc_expands_shared_roots_and_descendants_once() {
    let home = tempfile::tempdir().unwrap();
    for count in [32, 64] {
        let tabs: Vec<_> = (0..count).map(|index| format!("t{index}")).collect();
        let mut items: Vec<_> = tabs.iter().map(|id| arc_tab(id)).collect();
        items.push(json!({"id":"shared-folder","data":{"list":{}},"childrenIds":tabs}));
        items.push(container("root", &["shared-folder"]));
        let mut spaces = vec![space("first", "root")];
        for index in 1..count {
            let id = format!("dup{index}");
            spaces.push(space(&id, "root"));
            items.push(container(&id, &["shared-folder"]));
            spaces.push(space(&id, &id));
        }
        write_arc(home.path(), items, spaces);
        let (arc, warnings) = read_arc(home.path()).unwrap();
        assert_eq!(arc.spaces.len(), 1 + (count - 1) * 2);
        let [Bookmark::Folder { children, .. }] = arc.spaces[0].pinned.as_slice() else {
            panic!("first location keeps the shared folder")
        };
        assert_eq!(children.len(), count);
        assert!(arc.spaces[1..].iter().all(|space| space.pinned.is_empty()));
        assert_eq!(
            warnings,
            [format!(
                "Kept {} shared Arc sidebar references at their first location.",
                (count - 1) * 2
            )]
        );
    }
}

#[test]
fn arc_surrogate_ids_and_literal_replacement_chars_stay_distinct() {
    let home = tempfile::tempdir().unwrap();
    let entries = [
        (
            r#""\uFFFD""#,
            r#"{"id":"�","data":{"tab":{"savedURL":"https://example.org/real","savedTitle":"\ud800"}}}"#,
            "https://example.org/real",
        ),
        (
            r#""\uD800""#,
            r#"{"id":"\ud800","data":{"tab":{"savedURL":"https://example.org/surrogate","savedTitle":"\ud800"}}}"#,
            "https://example.org/surrogate",
        ),
        (
            r#""\u0000eda080""#,
            r#"{"id":"\u0000eda080","data":{"tab":{"savedURL":"https://example.org/prefix","savedTitle":"\ud800"}}}"#,
            "https://example.org/prefix",
        ),
    ];
    for order in [[0, 1, 2], [1, 0, 2]] {
        let entries = order.map(|index| entries[index]);
        let children = entries.map(|entry| entry.0).join(",");
        let items = entries.map(|entry| entry.1).join(",");
        let sidebar = format!(
            r#"{{"sidebar":{{"containers":[{{
                "spaces":[{{"id":"space","containerIDs":["pinned","root"]}}],
                "items":[{{"id":"root","childrenIds":[{children}]}},{items}]
            }}]}}}}"#
        );
        fs::write(home.path().join("StorableSidebar.json"), sidebar).unwrap();
        let (arc, warnings) = read_arc(home.path()).unwrap();
        assert!(warnings.is_empty());
        let pinned = &arc.spaces[0].pinned;
        assert_eq!(pinned.len(), entries.len());
        for (node, (_, _, expected)) in pinned.iter().zip(entries) {
            assert!(matches!(node, Bookmark::Tab(tab) if tab.url == expected && tab.title == "�"));
        }
    }
}

#[test]
fn firefox_reports_duplicate_place_ids_and_keeps_the_lowest_rowid() {
    let home = tempfile::tempdir().unwrap();
    let profile = home.path().join(".mozilla/firefox/example.default");
    fs::create_dir_all(&profile).unwrap();
    let db = Connection::open(profile.join("places.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE moz_places (id, url, title, visit_count, last_visit_date, hidden);
         CREATE TABLE moz_bookmarks (id, parent, type, title, guid, fk, position);
         INSERT INTO moz_places (rowid, id, url) VALUES
             (30, 1, 'https://example.org/last'), (10, 1, 'https://example.org/first'),
             (20, 1, 'https://example.org/middle'), (1, 2, 'https://example.org/unreferenced');
         INSERT INTO moz_bookmarks VALUES (100, 0, 2, 'Toolbar', 'toolbar_____', NULL, 0),
             (101, 100, 1, 'Bookmark', 'bookmark', 1, 0);",
    )
    .unwrap();
    drop(db);

    let config = BrowserLocations::new(Platform::Linux, home.path());
    let plan = read_browser(Browser::Firefox, &config);
    assert_eq!(plan.warnings, ["Skipped 2 duplicate Firefox place rows."]);
    assert_eq!(plan.bookmark_count, 1);
    assert!(matches!(
        plan.bookmarks.as_deref(),
        Some([Bookmark::Tab(tab)]) if tab.url == "https://example.org/first"
    ));
}

#[test]
fn firefox_expands_each_folder_id_once_per_database() {
    // The verifier also exercised 24 duplicate levels. The smaller case catches a
    // regression without running the larger, exponential input on the old reader.
    for depth in [10, 24] {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join(".mozilla/firefox/example.default");
        fs::create_dir_all(&profile).unwrap();
        let db = Connection::open(profile.join("places.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE moz_places (id, url, title, visit_count, last_visit_date, hidden);
             CREATE TABLE moz_bookmarks (id, parent, type, title, guid, fk, position);
             INSERT INTO moz_places VALUES (1, 'https://example.org/leaf', 'Leaf', 1, 1, 0);
             INSERT INTO moz_bookmarks VALUES (0, -1, 2, 'Toolbar', 'toolbar_____', NULL, 0);",
        )
        .unwrap();
        for level in 1..=depth {
            db.execute(
                "INSERT INTO moz_bookmarks VALUES
                    (?1, ?2, 2, 'First', ?3, NULL, 0), (?1, ?2, 2, 'Duplicate', ?4, NULL, 1)",
                (level, level - 1, format!("{level}a"), format!("{level}b")),
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO moz_bookmarks VALUES (-1, ?1, 1, 'Leaf', 'leaf', 1, 0)",
            [depth],
        )
        .unwrap();
        let config = BrowserLocations::new(Platform::Linux, home.path());
        let plan = read_browser(Browser::Firefox, &config);
        assert_eq!(plan.bookmark_count, 1);
        assert_eq!(
            plan.warnings,
            [format!("Skipped {depth} malformed bookmark records.")]
        );
        assert!(matches!(
            &plan.bookmarks.unwrap()[0],
            Bookmark::Folder { name, .. } if name == "First"
        ));
    }
}

fn assert_number_repair(warnings: &[String], source: &str) {
    assert!(
        warnings.iter().any(
            |warning| warning.contains("out-of-range JSON numbers") && warning.contains(source)
        )
    );
}

fn chromium(home: &Path) -> (BrowserLocations, std::path::PathBuf) {
    let profile = home.join(".config/google-chrome/Default");
    fs::create_dir_all(&profile).unwrap();
    (
        BrowserLocations::new(Platform::Linux, home),
        profile.join("Bookmarks"),
    )
}

fn chromium_json(nodes: &str) -> String {
    format!(r#"{{"roots":{{"bookmark_bar":{{"children":[{nodes}]}}}}}}"#)
}

fn write_arc(path: &Path, items: Vec<Value>, spaces: Vec<Value>) {
    let sidebar = json!({"sidebar":{"containers":[{"spaces":spaces,"items":items}]}});
    fs::write(path.join("StorableSidebar.json"), sidebar.to_string()).unwrap();
}

fn space(id: &str, container: &str) -> Value {
    json!({"id":id,"containerIDs":["pinned",container]})
}

fn container(id: &str, children: &[&str]) -> Value {
    json!({"id":id,"childrenIds":children})
}

fn arc_tab(id: &str) -> Value {
    json!({"id":id,"data":{"tab":{"savedURL":format!("https://example.org/{id}")}}})
}
