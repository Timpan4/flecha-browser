use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use flecha_core::import::ImportMode;
use flecha_core::{BrowserState, PinnedNode, SpaceId, TabId, WindowId, storage};
use flecha_import::{
    Browser, BrowserLocations, FlechaExport, ImportOptions, ImportPlan, Platform, ReplaceError,
    detect_browsers, read_bookmarks_html, read_bookmarks_html_file, read_browser,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case<'a> {
    name: String,
    profile: Option<Profile>,
    found: Option<Value>,
    html: Option<String>,
    html_fixture: Option<PathBuf>,
    export_text: Option<String>,
    #[serde(borrow)]
    ts: &'a RawValue,
    rust: Option<Divergence>,
    starter_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Divergence {
    reason: String,
    expected: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    platform: String,
    browser: String,
    #[serde(default)]
    files: BTreeMap<PathBuf, String>,
    #[serde(default)]
    databases: Vec<Database>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Database {
    path: PathBuf,
    schema: String,
    wal: Option<String>,
}

#[test]
fn shipping_importers_produce_the_same_plans_and_state_changes() {
    let fixture = include_str!("fixtures/import-parity.jsonl");
    assert!(!fixture.trim().is_empty());
    for line in fixture.lines() {
        let case: Case<'_> = serde_json::from_str(line).unwrap();
        let expected = if let Some(rust) = case.rust {
            assert!(!rust.reason.trim().is_empty(), "{}", case.name);
            rust.expected
        } else {
            serde_json::from_str(case.ts.get()).unwrap()
        };

        let window = WindowId::new("test-window");
        let initial = SpaceId::new("starter-space");
        let mut state = BrowserState::new(initial.clone(), window.clone());
        if let Some(name) = &case.starter_name {
            assert!(state.update_space(&initial, Some(name), None, None, None));
        }
        let base = state.clone();
        if let Some(export) = case.export_text {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("export.json");
            fs::write(&path, export).unwrap();
            let import = FlechaExport::read(&path).unwrap();
            assert_equivalent(
                &case.name,
                "decoded export",
                &project(import.state()),
                &expected,
            );
            assert_eq!(
                import.apply(&mut state, |_| Err("backup failed")),
                Err(ReplaceError::Save("backup failed"))
            );
            assert_eq!(state, base);
            let import = FlechaExport::read(&path).unwrap();
            let mut backup = None;
            import
                .apply(&mut state, |write| {
                    backup = storage::state_before_import(Some(write.value())).unwrap();
                    Ok::<_, ()>(())
                })
                .unwrap();
            assert_eq!(backup, Some(base));
            assert_equivalent(&case.name, "applied export", &project(&state), &expected);
            continue;
        }

        let plan = match (case.profile, case.html_fixture, case.html) {
            (Some(profile), None, None) => from_profile(profile, &case.name, case.found.unwrap()),
            (None, Some(fixture), None) => {
                let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../..")
                    .join(fixture);
                read_bookmarks_html_file(&path).unwrap()
            }
            (None, None, Some(html)) => read_bookmarks_html(&html),
            _ => panic!("fixture {} must specify exactly one source", case.name),
        };
        assert_equivalent(&case.name, "plan", &value(&plan), &expected["data"]);
        let preview = plan.preview(&state, true);
        assert_equivalent(
            &case.name,
            "preview",
            &value(&preview),
            &expected["preview"],
        );
        let mut next = 0;
        let mut id = || {
            next += 1;
            format!("generated-{next}")
        };
        for (field, mode, key) in [
            ("stale", ImportMode::Merge, "stale"),
            ("merge", ImportMode::Merge, preview.spaces_key.as_str()),
            ("again", ImportMode::Merge, preview.spaces_key.as_str()),
            ("append", ImportMode::Append, preview.spaces_key.as_str()),
        ] {
            let outcome = plan
                .apply(
                    &mut state,
                    &window,
                    1,
                    ImportOptions {
                        mode,
                        isolate: &[1],
                        spaces_key: Some(key),
                    },
                    &mut id,
                    |history| {
                        assert_ne!(field, "stale", "a stale preview must not write history");
                        assert!(history == plan.history.as_slice());
                        Ok::<_, ()>((history.len(), 0))
                    },
                )
                .unwrap();
            assert_equivalent(&case.name, field, &value(&outcome), &expected[field]);
            match field {
                "stale" => assert_eq!(state, base),
                "merge" => assert_equivalent(
                    &case.name,
                    "merged state",
                    &project(&state),
                    &expected["mergedState"],
                ),
                "append" => assert_equivalent(
                    &case.name,
                    "appended state",
                    &project(&state),
                    &expected["appendedState"],
                ),
                _ => {}
            }
        }
    }
}

fn from_profile(profile: Profile, name: &str, found: Value) -> ImportPlan {
    let home = tempfile::tempdir().unwrap();
    let root = home.path();
    let platform = match profile.platform.as_str() {
        "linux" => Platform::Linux,
        "darwin" => Platform::MacOs,
        "win32" => Platform::Windows,
        unexpected => panic!("unknown test platform {unexpected}"),
    };
    let browser = match profile.browser.as_str() {
        "chrome" => Browser::Chrome,
        "firefox" => Browser::Firefox,
        "helium" => Browser::Helium,
        "opera-gx" => Browser::OperaGx,
        "arc" => Browser::Arc,
        "safari" => Browser::Safari,
        unexpected => panic!("unknown fixture browser {unexpected}"),
    };
    let mut config = BrowserLocations::new(platform, root);
    config.xdg_config_home = Some(root.join("config"));
    config.chrome_config_home = Some(root.join("chrome-config"));
    config.helium_config_home = Some(root.join("helium-config"));
    config.local_app_data = Some(root.join("local"));
    config.roaming_app_data = Some(root.join("roaming"));

    let mut before = BTreeMap::new();
    for (path, text) in profile.files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &text).unwrap();
        before.insert(path, text.into_bytes());
    }
    let dbs: Vec<_> = profile
        .databases
        .into_iter()
        .map(|db| {
            let path = root.join(db.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(&db.schema).unwrap();
            if let Some(sql) = db.wal {
                connection
                    .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                    .unwrap();
                connection.execute_batch(&sql).unwrap();
                let wal = path.with_file_name(format!(
                    "{}-wal",
                    path.file_name().unwrap().to_str().unwrap()
                ));
                before.insert(wal.clone(), fs::read(wal).unwrap());
            }
            before.insert(path.clone(), fs::read(path).unwrap());
            connection
        })
        .collect();
    assert_equivalent(name, "discovery", &value(&detect_browsers(&config)), &found);
    let plan = read_browser(browser, &config);
    for (path, expected) in before {
        assert!(
            fs::read(path).unwrap() == expected,
            "source changed in fixture {name}"
        );
    }
    drop(dbs);
    plan
}

fn project(state: &BrowserState) -> Value {
    let tab = |id: &TabId| {
        let tab = state
            .tab(id)
            .expect("projection references an installed tab");
        json!({ "url": tab.url, "title": tab.title, "homeUrl": tab.home_url })
    };
    let window = state.focused_window();
    let spaces: Vec<_> = state.spaces().iter().map(|space| {
        let ws = window.spaces.get(&space.id);
        let today: Vec<_> = ws.into_iter().flat_map(|ws| &ws.today).map(tab).collect();
        let split: Vec<_> = ws.and_then(|ws| ws.split.as_ref()).into_iter().flatten()
            .map(|id| &state.tab(id).unwrap().url).collect();
        json!({
            "name": space.name, "emoji": space.emoji, "color": space.color, "isolated": space.isolated,
            "pinned": project_pins(&space.pinned, &tab), "today": today, "split": split,
        })
    }).collect();
    json!({
        "spaces": spaces,
        "activeSpace": state.space(&window.active_space_id).unwrap().name,
        "favorites": state.favorites().iter().map(tab).collect::<Vec<_>>(),
        "archive": state.archive(),
    })
}

fn project_pins(pins: &[PinnedNode], tab: &impl Fn(&TabId) -> Value) -> Vec<Value> {
    pins.iter()
        .map(|node| match node {
            PinnedNode::Tab { tab_id } => tab(tab_id),
            PinnedNode::Folder { name, children, .. } => {
                json!({ "name": name, "children": project_pins(children, tab) })
            }
        })
        .collect()
}

fn value(actual: &impl Serialize) -> Value {
    serde_json::to_value(actual).unwrap()
}

fn assert_equivalent(case: &str, part: &str, actual: &Value, expected: &Value) {
    fn equivalent(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(l), Value::Number(r)) => l.as_f64() == r.as_f64(),
            (Value::Array(l), Value::Array(r)) => {
                l.len() == r.len() && l.iter().zip(r).all(|(l, r)| equivalent(l, r))
            }
            (Value::Object(l), Value::Object(r)) => {
                l.len() == r.len()
                    && l.iter()
                        .all(|(key, l)| r.get(key).is_some_and(|r| equivalent(l, r)))
            }
            _ => left == right,
        }
    }
    assert!(equivalent(actual, expected), "{case}: {part} mismatch");
}
