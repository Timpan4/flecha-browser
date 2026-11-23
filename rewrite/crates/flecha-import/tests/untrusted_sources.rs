use std::fs;
use std::path::Path;

use flecha_core::import::ImportMode;
use flecha_core::{BrowserState, SpaceId, WindowId};
use flecha_import::{
    Bookmark, Browser, BrowserLocations, ImportOptions, Platform, read_arc, read_bookmarks_html,
    read_browser,
};
use rusqlite::Connection;
use serde_json::{Value, json};

#[test]
fn unsafe_bookmark_files_do_not_discard_the_other_file() {
    let root = tempfile::tempdir().unwrap();
    let (locations, profile) = chromium(root.path());
    fs::write(profile.join("AccountBookmarks"), chromium_bookmarks("Kept")).unwrap();
    let oversized = fs::File::create(profile.join("Bookmarks")).unwrap();
    // fs.readFile uses the signed 32-bit file size boundary. This fixture is sparse.
    oversized.set_len(i32::MAX as u64 + 1).unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    assert_eq!(plan.bookmark_count, 1);
    assert_eq!(
        plan.warnings,
        ["Couldn’t read bookmarks from Chrome: File exceeds the supported size."]
    );

    // The core migration suite also exercises legacy input with 20,000 nested arrays.
    let depth = 20_000;
    fs::write(
        profile.join("Bookmarks"),
        format!("{}null{}", "[".repeat(depth), "]".repeat(depth)),
    )
    .unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    assert_eq!(plan.bookmark_count, 1);
    assert_eq!(
        plan.warnings,
        ["Skipped 1 JSON subtrees beyond the nesting limit in Bookmarks."]
    );
}

#[test]
fn arc_graph_depth_and_cycles_keep_the_reachable_tab() {
    let root = tempfile::tempdir().unwrap();
    let depth = 20_000;
    let mut items = vec![json!({"id":"root", "childrenIds":["0"]})];
    for index in 0..depth {
        items.push(json!({
            "id": index.to_string(),
            "data": {"list": {}},
            "childrenIds": [(index + 1).to_string()],
        }));
    }
    items.push(json!({
        "id": depth.to_string(),
        "data": {"tab": {"savedURL": "https://example.org/"}},
        "childrenIds": ["0"],
    }));
    let main = json!({"spaces":[{"id":"work","containerIDs":["pinned","root"]}],"items":items});
    let sidebar = json!({"sidebar":{"containers":[main]}});
    fs::write(
        root.path().join("StorableSidebar.json"),
        sidebar.to_string(),
    )
    .unwrap();
    let (arc, warnings) = read_arc(root.path()).unwrap();
    assert!(warnings.is_empty());
    assert!(arc.skipped.is_empty());
    let mut nodes = arc.spaces[0].pinned.as_slice();
    for _ in 0..depth {
        let [Bookmark::Folder { children, .. }] = nodes else {
            panic!("missing folder")
        };
        nodes = children;
    }
    let [Bookmark::Tab(tab)] = nodes else {
        panic!("missing tab at the end of Arc's folder graph")
    };
    assert_eq!(tab.url, "https://example.org/");
}

#[test]
fn bookmarks_follow_the_shipping_count_and_depth_limits() {
    let mut html = "<H3>Folder</H3><DL>".repeat(9);
    for n in 0..2001 {
        html.push_str(&format!(
            "<A HREF=https://example.org/{n}>Synthetic tab</A>"
        ));
    }
    html.push_str(&"</DL>".repeat(9));
    let plan = read_bookmarks_html(&html);
    assert_eq!(plan.bookmark_count, 2000);
    assert_eq!(
        plan.warnings,
        ["Only the first 2000 bookmarks were imported; 1 more were left out."]
    );
    let mut nodes = plan.bookmarks.as_deref().unwrap();
    for _ in 0..8 {
        let [Bookmark::Folder { children, .. }] = nodes else {
            panic!("unexpected folder depth")
        };
        nodes = children;
    }
    assert_eq!(nodes.len(), 2000);
    assert!(nodes.iter().all(|node| matches!(node, Bookmark::Tab(_))));
}

#[test]
fn history_failure_keeps_committed_spaces_and_redacts_adapter_errors() {
    let root = tempfile::tempdir().unwrap();
    let (locations, profile) = chromium(root.path());
    fs::write(profile.join("Bookmarks"), chromium_bookmarks("Bookmark")).unwrap();
    let db = Connection::open(profile.join("History")).unwrap();
    db.execute_batch("CREATE TABLE urls (url, title, visit_count, last_visit_time, hidden);
        INSERT INTO urls VALUES ('https://example.org/private?test=1', 'Synthetic title', 2, 11644473600001000, 0);").unwrap();
    db.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    let window = WindowId::new("w");
    let mut state = BrowserState::new(SpaceId::new("home"), window.clone());
    let mut id = 0;
    let outcome = plan
        .apply(
            &mut state,
            &window,
            1,
            ImportOptions {
                mode: ImportMode::Merge,
                isolate: &[],
                spaces_key: None,
            },
            || {
                id += 1;
                id.to_string()
            },
            |history| {
                assert_eq!(history.len(), 1);
                Err("https://example.org/private?test=1 Synthetic title")
            },
        )
        .unwrap();
    assert_eq!(
        outcome.warnings,
        ["Couldn’t finish importing browser history."]
    );
    assert!(outcome.changed);
    assert_eq!(outcome.history, 0);
    assert_eq!(state.spaces()[0].pinned_tabs().len(), 1);
}

#[test]
fn history_keeps_the_newest_5000_rows_from_each_profile() {
    let root = tempfile::tempdir().unwrap();
    let (locations, profile) = chromium(root.path());
    let db = Connection::open(profile.join("History")).unwrap();
    db.execute_batch(
        "CREATE TABLE urls (url, title, visit_count, last_visit_time, hidden);
         WITH RECURSIVE visits(n) AS (VALUES(1) UNION ALL SELECT n + 1 FROM visits WHERE n <= 5000)
         INSERT INTO urls SELECT 'https://example.org/' || n, '', 1, 11644473600000000 + n * 1000, 0 FROM visits;",
    ).unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    assert!(plan.warnings.is_empty());
    assert_eq!(plan.history.len(), 5000);
    assert_eq!(plan.history.first().unwrap().last_visit, 5001.0);
    assert_eq!(plan.history.last().unwrap().last_visit, 2.0);
}

#[test]
fn source_diagnostics_and_sqlite_views_cannot_expose_private_fields() {
    let root = tempfile::tempdir().unwrap();
    let (locations, profile) = chromium(root.path());
    let private = "https://example.org/private-path?fixture=synthetic-secret";
    let title = "Synthetic private title";
    let db = Connection::open(profile.join("History")).unwrap();
    db.execute_batch(
        "CREATE TABLE cookies (url, title, visit_count, last_visit_time, hidden);
        CREATE VIEW urls AS SELECT * FROM cookies;",
    )
    .unwrap();
    db.execute(
        "INSERT INTO cookies VALUES (?1, ?2, 1, 11644473600001000, 0)",
        (private, title),
    )
    .unwrap();
    fs::write(
        profile.join("Bookmarks"),
        format!("invalid JSON {private} {title}"),
    )
    .unwrap();
    fs::write(profile.join("Cookies"), title).unwrap();
    fs::write(profile.join("Login Data"), title).unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    assert_eq!(plan.bookmark_count, 0);
    assert!(plan.history.is_empty());
    assert_eq!(plan.warnings.len(), 2);
    assert!(
        !plan
            .warnings
            .iter()
            .any(|warning| warning.contains(private) || warning.contains(title))
    );

    let data = serde_json::Map::from_iter([(private.to_owned(), json!(title))]);
    let sidebar = json!({"sidebar":{"containers":[{
        "spaces":[{"containerIDs":["pinned","container"]}],
        "items":[{"id":"container","childrenIds":["item"]}, {"id":"item","data":data}],
    }]}});
    fs::write(
        root.path().join("StorableSidebar.json"),
        sidebar.to_string(),
    )
    .unwrap();
    fs::write(
        root.path().join("StorableArchiveItems.json"),
        format!("invalid JSON {private} {title}"),
    )
    .unwrap();
    let (arc, warnings) = read_arc(root.path()).unwrap();
    assert_eq!(
        arc.skipped.keys().map(String::as_str).collect::<Vec<_>>(),
        ["unknown"]
    );
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.contains(private) && !warning.contains(title))
    );
}

#[cfg(unix)]
#[test]
fn profile_files_and_directories_cannot_redirect_import_through_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("Cookies"),
        chromium_bookmarks("Only reached through a link"),
    )
    .unwrap();
    fs::write(
        outside.path().join("Bookmarks"),
        chromium_bookmarks("Only reached through a link"),
    )
    .unwrap();
    let (locations, profile) = chromium(root.path());
    symlink(outside.path().join("Cookies"), profile.join("Bookmarks")).unwrap();
    symlink(outside.path(), profile.parent().unwrap().join("Profile 9")).unwrap();
    let plan = read_browser(Browser::Chrome, &locations);
    assert_eq!(plan.bookmark_count, 0);
    assert!(plan.bookmarks.is_none());
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.contains("without symbolic links"))
    );
}

#[test]
fn safari_imports_xml_and_binary_bookmark_exports() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("Library/Safari");
    fs::create_dir_all(&root).unwrap();
    let leaf = |url: &str, title: &str| {
        json!({
            "WebBookmarkType": "WebBookmarkTypeLeaf",
            "URLString": url,
            "URIDictionary": {"title": title},
        })
    };
    let value: Value = json!({ "Children": [
        {"Title":"BookmarksBar","Children":[leaf("https://example.org/bar","Bar & title")]},
        {"Title":"BookmarksMenu","WebBookmarkType":"WebBookmarkTypeList",
         "Children":[leaf("https://example.org/menu","Menu title")]},
        {"Title":"History","WebBookmarkType":"WebBookmarkTypeProxy"},
    ]});
    let plist = plist_value(value);
    let file = root.join("Bookmarks.plist");
    let locations = BrowserLocations::new(Platform::MacOs, home.path());
    plist.to_file_xml(&file).unwrap();
    let xml = read_browser(Browser::Safari, &locations);
    plist.to_file_binary(&file).unwrap();
    let binary = read_browser(Browser::Safari, &locations);
    assert_eq!(xml.bookmark_count, 2);
    assert!(xml.warnings.is_empty());
    assert!(serde_json::to_value(xml).unwrap() == serde_json::to_value(binary).unwrap());
}

fn plist_value(value: Value) -> plist::Value {
    match value {
        Value::Object(fields) => plist::Value::Dictionary(
            fields
                .into_iter()
                .map(|(key, value)| (key, plist_value(value)))
                .collect(),
        ),
        Value::Array(values) => plist::Value::Array(values.into_iter().map(plist_value).collect()),
        Value::String(value) => plist::Value::String(value),
        _ => panic!("unsupported type in Safari fixture"),
    }
}

fn chromium(home: &Path) -> (BrowserLocations, std::path::PathBuf) {
    let profile = home.join(".config/google-chrome/Default");
    fs::create_dir_all(&profile).unwrap();
    (BrowserLocations::new(Platform::Linux, home), profile)
}

fn chromium_bookmarks(title: &str) -> String {
    json!({"roots":{"bookmark_bar":{"children":[{"type":"url","url":"https://example.org/","name":title}]}}}).to_string()
}
