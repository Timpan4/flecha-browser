use flecha_core::storage::{
    DecodeError, Recovery, SCHEMA_VERSION, Slot, decode, encode, load, migrate_legacy,
};
use flecha_core::{
    ArchivePolicy, PinnedNode, SearchEngine, Settings, SidebarWidth, SpaceId, TabId, Theme,
    WindowId,
};
use serde_json::{Value, json};

const V2: &str = include_str!("fixtures/chromium-v2.json");

#[test]
fn saved_surrogates_are_replaced_per_code_unit_without_losing_archive_entries() {
    let cases = [
        (r"Cut \ud83d", "Cut �"),
        (r"\ude00 tail", "� tail"),
        (r"\ud83d\ud83d\ude00\ude00", "�😀�"),
        (r"\uD83D\uDE00", "😀"),
        (r"\\ud83d \ud83d", r"\ud83d �"),
    ];
    for (encoded, title) in cases {
        let saved = format!(
            r#"{{
            "version":2,"tabs":{{}},"spaces":[{{"id":"s"}}],
            "archive":[{{"url":"https://example.org","title":"{encoded}","closedAt":1}}]
        }}"#
        );
        let state = migrate_legacy(&saved).unwrap();
        assert_eq!(state.archive()[0].title, title, "{encoded}");
        let saved = json!(saved);
        let mut loaded = load(Some(&saved), None).unwrap();
        let ready = loaded
            .ready()
            .expect("a repaired title must not trigger recovery");
        assert_eq!(ready.state.as_ref(), Some(&state));
    }
}

#[test]
fn out_of_range_snapshot_values_repair_independently() {
    let fixture = r#"{
      "version":3,
      "tabs":{"desktop":{"url":"https://example.org/d"},"side":{"url":"https://example.org/s"},
              "closed":{"url":"https://example.org/closed"}},
      "spaces":[{"id":"s","pinnedFlat":[
        {"kind":"folder","id":"overflow","depth":1e400},
        {"kind":"folder","id":"keep","depth":0}
      ]}],
      "windows":[{"id":"w","activeSpaceId":"s","spaces":{"s":{
        "today":["desktop","side"],"activeTabId":"desktop","split":["desktop","side"],"splitRatio":1e400
      }}}],
      "closedWindows":[{"id":"bad","activeSpaceId":"s","spaces":{"s":{"today":["closed"]}},"closedAt":1e400}],
      "archive":[{"url":"https://example.org/bad","title":"Bad","closedAt":1e400},
                 {"url":"https://example.org/keep","title":"Keep","closedAt":7}],
      "window":{"x":1,"y":2,"width":1e400,"height":4},
      "sidebarWidth":1e400,"theme":"dark"
    }"#;
    for invalid in ["sidebarWidth", "splitRatio", "depth", "closedAt", "width"] {
        // Leave just one occurrence class malformed so each projection path is tested independently.
        let input = fixture
            .lines()
            .map(|line| {
                if line.contains(&format!(r#""{invalid}":1e400"#)) {
                    line.into()
                } else {
                    line.replace("1e400", "0.5")
                }
            })
            .collect::<String>();
        let state = decode(&input).unwrap_or_else(|error| panic!("{invalid}: {error}"));
        assert_eq!(state.settings.theme, Theme::Dark, "{invalid}");
        match invalid {
            "sidebarWidth" => assert_eq!(state.settings.sidebar_width, SidebarWidth::default()),
            "splitRatio" => assert_eq!(
                state.focused_window().spaces[&SpaceId::new("s")].split_ratio,
                None
            ),
            "depth" => assert_eq!(state.spaces()[0].pinned.len(), 1),
            "closedAt" => assert!(state.closed_windows().is_empty() && state.archive().len() == 1),
            "width" => assert!(state.settings.window.is_none()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn many_unrepresentable_values_preserve_readable_snapshot_state() {
    let saved = json!(
        r#"{
      "version":"\ud83d",
      "tabs":{"t":{"url":"https://example.org","title":"Title\ud83d","lastActive":1e400,
                   "history":{"index":0,"entries":[{"url":"https://example.org","title":"\udfff"}]}}},
      "spaces":[1e400,{"id":"s","name":"Name\ude00",
        "pinned":[{"kind":"folder","id":"f","name":"Folder\ud83d","children":1e400}],
        "today":["t"]}],
      "archive":[1e400,{"url":"https://example.org/archive","title":"Archive\ud83d","closedAt":1}],
      "sitePermissions":{"example.org":1e400},
      "window":1e400,"sidebarWidth":-1e400,"keybindings":{"newTab":1e400},
      "onboarded":true
    }"#
    );
    let mut loaded = load(Some(&saved), None).unwrap();
    let state = loaded
        .ready()
        .expect("one damaged value must not disable saves")
        .state
        .as_ref()
        .unwrap();
    let tab = state.tab(&TabId::new("t")).unwrap();
    assert_eq!(tab.title, "Title�");
    assert_eq!(tab.history.as_ref().unwrap().entries[0].title, "�");
    assert_eq!(tab.last_active, None);
    let space = &state.spaces()[0];
    assert_eq!(space.name, "Name�");
    assert!(
        matches!(&space.pinned[0], PinnedNode::Folder { name, children, .. } if name == "Folder�" && children.is_empty())
    );
    assert_eq!(state.archive()[0].title, "Archive�");
    assert_eq!(state.settings.sidebar_width, SidebarWidth::default());
    assert_eq!(state.settings.onboarded, Some(true));
    assert!(state.settings.site_permissions.is_none());
    assert!(state.settings.window.is_none());
    assert!(state.settings.keybindings.is_empty());
}

#[test]
fn migrates_a_stock_v2_profile_copy_and_round_trips_the_rewrite_snapshot() {
    let state = migrate_legacy(V2).unwrap();
    let work = SpaceId::new("work");
    let personal = SpaceId::new("personal");
    let first = state.focused_window();
    let second = state.window(&WindowId::new("window-home")).unwrap();
    assert_eq!(state.favorites(), &[TabId::new("calendar")]);
    assert_eq!(first.id, WindowId::new("window-work"));
    assert_eq!(first.active_space_id, work);
    assert_eq!(
        first.spaces[&work].today,
        [TabId::new("notes"), TabId::new("review")]
    );
    assert_eq!(
        first.visible_tabs(),
        [TabId::new("review"), TabId::new("wiki")]
    );
    assert_eq!(second.active_space_id, personal);
    assert_eq!(second.spaces[&work].today, [TabId::new("draft")]);
    assert_eq!(second.spaces[&personal].today, [TabId::new("weekend")]);
    assert_eq!(
        state.closed_windows()[0].window.spaces[&work].today,
        [TabId::new("old-plan")]
    );
    assert!(!state.open_tabs().contains_key(&TabId::new("old-plan")));
    let space = state.space(&work).unwrap();
    assert!(space.isolated && space.pinned_collapsed);
    assert_eq!(space.color, "#f6d6cc");
    assert_eq!(space.emoji.as_deref(), Some("💼"));
    assert!(
        matches!(&space.pinned[0], PinnedNode::Folder { name, open: true, children, .. }
        if name == "Project" && matches!(&children[0], PinnedNode::Folder { name, open: false, .. } if name == "Reference"))
    );
    assert_eq!(space.pinned_tabs(), [&TabId::new("wiki")]);
    assert!(!state.sidebar_visible(&first.id));
    assert!(state.sidebar_visible(&second.id));
    let wiki = state.tab(&TabId::new("wiki")).unwrap();
    assert_eq!(wiki.history.as_ref().unwrap().entries[1].title, "Setup");
    assert_eq!(
        wiki.home_url.as_deref(),
        Some("https://work.example.org/wiki")
    );
    assert!(state.tab(&TabId::new("notes")).unwrap().agent);
    assert_eq!(state.settings.search_engine, SearchEngine::Kagi);
    assert_eq!(state.settings.theme, Theme::Dark);
    assert_eq!(state.settings.archive_policy, ArchivePolicy::WEEK);
    assert_eq!(state.settings.sidebar_width.pixels(), 312.0);
    assert_eq!(state.settings.keybindings["closeTab"], None);
    assert_eq!(
        state.settings.environments[0].targets[0].origin,
        "http://localhost:3000"
    );

    let saved = encode(&state).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&saved).unwrap()["version"],
        SCHEMA_VERSION
    );
    assert!(!saved.contains("pageState"));
    assert_eq!(decode(&saved).unwrap(), state);
    assert!(matches!(
        migrate_legacy(&saved),
        Err(DecodeError::AlreadyMigrated)
    ));
}

#[test]
fn v1_and_unversioned_spaces_become_one_window_with_repaired_references() {
    let mut legacy = json!({
        "version": 1,
        "spaces": [
          {"id":"s1", "name":"One", "color":"#fff", "activeTabId":"gone", "split":["t1","gone"], "today":["t1","gone",7,"t2"], "pinned":[
            {"kind":"tab","tabId":"gone"}, {"kind":"folder","id":"f","name":"F","children":[{"kind":"tab","tabId":"t1"},null]}, "junk"
          ]},
          {"id":"s2", "today":["t3"], "activeTabId":"t3"}, "not a space"
        ],
        "activeSpaceId":"missing",
        "favorites":["gone"],
        "tabs":{"t1":{"url":"https://example.org/","id":"bad"},"t2":{"url":"https://example.net/"},
                "t3":{"url":"https://example.com/","lastActive":"42"},"broken":{"title":"no url"}}
    });
    let state = migrate_legacy(&legacy.to_string()).unwrap();
    legacy.as_object_mut().unwrap().remove("version");
    assert_eq!(state, migrate_legacy(&legacy.to_string()).unwrap());
    let s1 = SpaceId::new("s1");
    let s2 = SpaceId::new("s2");
    let window = state.focused_window();
    assert_eq!(state.windows().len(), 1);
    assert_eq!(window.active_space_id, s1);
    assert_eq!(window.spaces[&s1].today, [TabId::new("t2")]);
    assert_eq!(window.spaces[&s1].active_tab_id, None);
    assert_eq!(window.spaces[&s1].split, None);
    assert_eq!(window.spaces[&s2].active_tab_id, Some(TabId::new("t3")));
    assert_eq!(state.space(&s1).unwrap().pinned_tabs(), [&TabId::new("t1")]);
    assert_eq!(
        state.tab(&TabId::new("t3")).unwrap().last_active,
        Some(42.0)
    );
    assert!(state.favorites().is_empty());
    assert!(state.tab(&TabId::new("broken")).is_none());
}

#[test]
fn repair_drops_unknown_and_malformed_fields_independently() {
    let mut input: Value = serde_json::from_str(V2).unwrap();
    input["theme"] = json!("neon");
    input["archiveAfterHours"] = json!(-1);
    input["window"] = json!({"x":1,"y":2,"width":"wide","height":4});
    input["spaces"][1]["isolated"] = json!("yes");
    input["spaces"][1]["extra"] = json!(2);
    input["windows"][0]["spaces"]["work"]["splitRatio"] = json!(3);
    input["tabs"]["wiki"]["title"] = json!(7);
    input["tabs"]["wiki"]["history"]["entries"] = json!("none");
    input["tabs"]["wiki"]["extra"] = json!(1);
    input["tabs"]["review"]["audible"] = json!(true);
    input["archive"][0]["extra"] = json!(3);
    input["keybindings"]["newTab"] = json!(42);
    input["environments"][0]["targets"][0]["autoLabel"] = json!("bad");
    let input = input.to_string();
    let input = format!(
        "{},\"unexpected\":{}{}}}",
        input.strip_suffix('}').unwrap(),
        "[".repeat(20_000),
        "]".repeat(20_000)
    );
    let state = decode(&input).unwrap();
    let defaults = Settings::default();
    assert_eq!(state.settings.keybindings, defaults.keybindings);
    assert_eq!(state.settings.theme, defaults.theme);
    assert_eq!(state.settings.archive_policy, defaults.archive_policy);
    assert_eq!(state.settings.search_engine, SearchEngine::Kagi);
    assert_eq!(state.settings.window, None);
    assert_eq!(state.settings.environments[0].targets[0].auto_label, None);
    let wiki = state.tab(&TabId::new("wiki")).unwrap();
    assert_eq!(wiki.title, wiki.url);
    assert_eq!(wiki.history, None);
    assert_eq!(state.tab(&TabId::new("review")).unwrap().muted, Some(true));
    assert!(!state.spaces()[1].isolated);
    assert_eq!(
        state.focused_window().spaces[&SpaceId::new("work")].split_ratio,
        None
    );
    let saved = encode(&state).unwrap();
    for field in ["\"extra\"", "\"unexpected\"", "\"audible\""] {
        assert!(!saved.contains(field));
    }
}

#[test]
fn invalid_saved_shortcuts_fall_back_without_unbinding_other_commands() {
    use flecha_core::shortcuts::{Platform, keys_for};
    let raw = r#"{"tabs":{},"spaces":[{"id":"s"}],
        "keybindings":{"newTab":"T","find":null,"reload":"F24"}}"#;
    let state = decode(raw).unwrap();
    let bindings = &state.settings.keybindings;
    assert!(!bindings.contains_key("newTab"));
    assert_eq!(
        keys_for(bindings, "newTab", Platform::Linux),
        Some("CmdOrCtrl+T")
    );
    assert_eq!(keys_for(bindings, "find", Platform::Linux), None);
    assert_eq!(keys_for(bindings, "reload", Platform::Linux), Some("F24"));
}

#[test]
fn saved_platform_reserved_override_leaves_the_command_unbound() {
    use flecha_core::shortcuts::{Platform, keys_for};
    let state =
        decode(r#"{"tabs":{},"spaces":[{"id":"s"}],"keybindings":{"newTab":"Ctrl+Alt+1"}}"#)
            .unwrap();
    let bindings = &state.settings.keybindings;
    assert_eq!(keys_for(bindings, "newTab", Platform::Linux), None);
    assert_eq!(
        keys_for(bindings, "newTab", Platform::Mac),
        Some("Ctrl+Alt+1")
    );
}

#[test]
fn prototype_names_claim_only_actual_tabs_and_only_once() {
    let raw = r#"{"spaces":[{"id":"s","today":["constructor","toString","__proto__","t"],"pinned":[{"kind":"tab","tabId":"__proto__"},{"kind":"tab","tabId":"t"}]}],"favorites":["hasOwnProperty","t","t"],"tabs":{"__proto__":{"url":"https://example.org/"},"t":{"url":"https://example.net/"}}}"#;
    let state = decode(raw).unwrap();
    assert_eq!(state.focused_window().spaces[&SpaceId::new("s")].today, []);
    assert_eq!(state.favorites(), [TabId::new("t")]);
    assert_eq!(state.spaces()[0].pinned_tabs(), [&TabId::new("__proto__")]);
    assert!(state.tab(&TabId::new("__proto__")).is_some());
}

#[test]
fn last_good_is_promoted_only_after_startup() {
    let saved = json!(V2);
    let mut loaded = load(Some(&saved), None).unwrap();
    assert!(recovery_writes(&mut loaded).is_empty());
    let loaded = loaded.ready().unwrap();
    let startup = loaded.state.as_ref().unwrap().clone();
    loaded.state.as_mut().unwrap().settings.theme = Theme::Light;
    let backup = loaded.started().unwrap();
    assert_eq!(backup.slot(), Slot::LastGood);
    assert_eq!(decode(backup.value().as_str().unwrap()).unwrap(), startup);
    assert!(loaded.started().is_none());
    let mut unchanged = load(Some(backup.value()), Some(backup.value())).unwrap();
    assert!(unchanged.ready().unwrap().started().is_none());
}

#[test]
fn unreadable_and_too_deep_state_recovers_without_losing_the_input() {
    let depth = 20_000;
    let pinned = format!(
        "{}{}",
        r#"{"kind":"folder","id":"f","children":["#.repeat(depth),
        "]}".repeat(depth)
    );
    let deep = json!(format!(
        r#"{{"spaces":[{{"id":"s","pinned":[{pinned}]}}],"tabs":{{}}}}"#
    ));
    let fallback = json!(V2);
    for damaged in [json!("{\"spaces\": ["), json!(42), deep] {
        let mut loaded = load(Some(&damaged), Some(&fallback)).unwrap();
        assert!(loaded.ready().is_none());
        assert_eq!(
            recovery_writes(&mut loaded),
            [(Slot::Damaged, damaged), (Slot::Current, fallback.clone())]
        );
        let loaded = loaded.ready().unwrap();
        assert_eq!(loaded.recovered, Some(Recovery::LastGood));
        assert_eq!(loaded.state.as_ref().unwrap().spaces()[0].name, "Personal");
        assert!(loaded.started().is_none());
    }
    let damaged = json!(r#"{"spaces":[]}"#);
    let mut loaded = load(Some(&damaged), Some(&json!("not json"))).unwrap();
    assert!(loaded.ready().is_none());
    assert_eq!(recovery_writes(&mut loaded), [(Slot::Damaged, damaged)]);
    let loaded = loaded.ready().unwrap();
    assert_eq!(loaded.recovered, Some(Recovery::Fresh));
    assert!(loaded.state.is_none() && loaded.started().is_none());
    let mut first = load(None, Some(&fallback)).unwrap();
    assert!(recovery_writes(&mut first).is_empty());
    let first = first.ready().unwrap();
    assert_eq!(first.recovered, None);
    assert!(first.state.is_none() && first.started().is_none());
}

fn recovery_writes(loaded: &mut flecha_core::storage::LoadedState) -> Vec<(Slot, Value)> {
    let mut writes = Vec::new();
    loaded
        .apply_recovery(|write| {
            writes.push((write.slot(), write.value().clone()));
            Ok::<_, std::convert::Infallible>(())
        })
        .unwrap();
    writes
}

#[test]
fn recovery_waits_for_a_successfully_written_damaged_copy() {
    let damaged = json!("{\"spaces\": [");
    let fallback = json!(V2);
    let mut loaded = load(Some(&damaged), Some(&fallback)).unwrap();
    assert!(loaded.ready().is_none());
    let mut writes = Vec::new();
    assert_eq!(
        loaded
            .apply_recovery(|write| {
                writes.push(write.slot());
                if write.slot() == Slot::Damaged {
                    Err("cannot write damaged copy")
                } else {
                    Ok(())
                }
            })
            .unwrap_err(),
        "cannot write damaged copy"
    );
    assert_eq!(writes, [Slot::Damaged]);
    assert!(loaded.ready().is_none());
    assert_eq!(
        loaded
            .apply_recovery(|write| {
                if write.slot() == Slot::Current {
                    Err("cannot write current copy")
                } else {
                    Ok(())
                }
            })
            .unwrap_err(),
        "cannot write current copy"
    );
    assert!(loaded.ready().is_none());
    assert_eq!(recovery_writes(&mut loaded), [(Slot::Current, fallback)]);
    assert!(recovery_writes(&mut loaded).is_empty());
    let ready = loaded.ready().unwrap();
    let state = ready.state.as_mut().unwrap();
    state.settings.theme = Theme::Dark;
    let current = ready.save_current(ready.state.as_ref().unwrap()).unwrap();
    assert_eq!(current.slot(), Slot::Current);
    let saved = decode(current.value().as_str().unwrap()).unwrap();
    assert_eq!(saved.settings.theme, Theme::Dark);
}

#[test]
fn unknown_deep_tab_fields_do_not_trigger_recovery() {
    use flecha_core::WindowId;
    let depth = 20_000;
    let deep = format!("{}null{}", "[".repeat(depth), "]".repeat(depth));
    let saved = json!(format!(
        r#"{{"version":3,
        "tabs":{{"t":{{"url":"https://example.org","lastActive":7,"extra":{deep},
          "history":{{"index":0,"entries":[{{"url":"https://example.org","title":"Example","extra":{deep}}}]}}}}}},
        "spaces":[{{"id":"s","name":"Personal"}}],
        "windows":[{{"id":"w","activeSpaceId":"s","spaces":{{"s":{{"today":["t"]}}}}}}]}}"#
    ));
    let mut loaded = load(Some(&saved), None).unwrap();
    assert!(recovery_writes(&mut loaded).is_empty());
    let loaded = loaded.ready().unwrap();
    assert_eq!(loaded.recovered, None);
    let state = loaded.state.take().unwrap();
    let id = TabId::new("t");
    assert_eq!(state.tab(&id).unwrap().last_active, Some(7.0));
    assert_eq!(
        state.tab(&id).unwrap().history.as_ref().unwrap().entries[0].title,
        "Example"
    );
    assert_eq!(
        state
            .tab_ids(&WindowId::new("w"), &SpaceId::new("s"))
            .unwrap(),
        [&id]
    );
}

#[test]
fn deep_unknown_snapshot_values_do_not_discard_other_state() {
    let base = json!({
        "version": 3,
        "tabs": {
            "today": {"url": "https://example.org/today"},
            "recent": {"url": "https://example.org/recent"},
            "pin": {"url": "https://example.org/pin"},
        },
        "spaces": [
            {"id": "s", "pinned": [{"id": "f", "kind": "folder", "children": [{"kind": "tab", "tabId": "pin"}]}]},
            {"id": "other", "pinnedFlat": [{"id": "flat", "kind": "folder", "depth": 0}]},
        ],
        "windows": [{"id": "w", "activeSpaceId": "s", "spaces": {"s": {"today": ["today"], "activeTabId": "today"}}}],
        "closedWindows": [{"id": "closed", "activeSpaceId": "s", "closedAt": 9, "spaces": {"s": {"today": ["recent"]}}}],
        "archive": [{"url": "https://example.org/archived", "title": "Archived", "closedAt": 10}],
        "window": {"x": 1, "y": 2, "width": 3, "height": 4},
        "environments": [{"id": "dev", "name": "Dev", "targets": [{"label": "Local", "origin": "http://localhost"}]}],
        "sitePermissions": {"example.org": {"camera": "block"}},
        "keybindings": {"newTab": "Ctrl+T"},
        "sidebarWidth": 200,
    });
    let deep = format!("{}null{}", "[".repeat(20_000), "]".repeat(20_000));
    for (parent, field) in [
        ("", "extra"),
        ("/spaces/0", "extra"),
        ("/spaces/0/pinned/0", "extra"),
        ("/spaces/0/pinned/0/children/0", "children"),
        ("/spaces/1/pinnedFlat/0", "extra"),
        ("/windows/0", "extra"),
        ("/windows/0/spaces/s", "extra"),
        ("/closedWindows/0", "extra"),
        ("/closedWindows/0/spaces/s", "extra"),
        ("/archive/0", "extra"),
        ("/window", "extra"),
        ("/environments/0", "extra"),
        ("/environments/0/targets/0", "extra"),
        ("/sitePermissions", "extra"),
        ("/sitePermissions/example.org", "extra"),
        ("/keybindings", "extra"),
    ] {
        let mut sample = base.clone();
        let record = sample.pointer_mut(parent).unwrap().as_object_mut().unwrap();
        record.insert(field.into(), json!("deep_fixture"));
        let saved = json!(sample.to_string().replace("\"deep_fixture\"", &deep));
        // For typed dictionaries, an invalid entry drops the setting as a whole.
        if parent.starts_with("/sitePermissions") {
            sample["sitePermissions"] = Value::Null;
        } else if parent == "/keybindings" {
            sample["keybindings"]["extra"] = json!([]);
        } else {
            sample = base.clone();
        }
        let expected = decode(&sample.to_string()).unwrap();
        let mut loaded = load(Some(&saved), None).unwrap();
        let ready = loaded
            .ready()
            .unwrap_or_else(|| panic!("deep value in {parent:?} triggered recovery"));
        assert_eq!(
            ready.state.as_ref(),
            Some(&expected),
            "deep value in {parent:?}"
        );
        assert_eq!(ready.recovered, None);
    }
}

#[test]
fn saved_history_requires_objects_and_uses_the_last_duplicate_fields() {
    let history = |input: &str| {
        let saved = format!(
            r#"{{"tabs":{{"t":{{"url":"https://example.org","history":{input}}}}},"spaces":[{{"id":"s"}}]}}"#
        );
        decode(&saved)
            .unwrap()
            .tab(&TabId::new("t"))
            .unwrap()
            .history
            .clone()
    };
    for input in [
        r#"[[],0]"#,
        r#"{"entries":[["https://example.org","title"]],"index":0}"#,
        r#"{"entries":[{"url":"https://example.org","title":"title","pageState":null}],"index":0}"#,
        r#"{"entries":[],"index":0,"index":"bad"}"#,
    ] {
        assert_eq!(history(input), None, "{input}");
    }
    let empty = history(r#"{"entries":[],"index":0}"#).unwrap();
    assert!(empty.entries.is_empty());
    let saved = history(r#"{"entries":null,"index":"bad","entries":[{"url":"old","url":"https://example.org","title":"Setup","pageState":null,"pageState":"opaque"},{"url":"https://example.org/docs","title":"Docs"}],"index":1}"#).unwrap();
    assert_eq!(saved.index, 1.0);
    assert_eq!(saved.entries[0].url, "https://example.org");
    assert_eq!(saved.entries[0].title, "Setup");
    assert_eq!(saved.entries[1].title, "Docs");
}

#[test]
fn missing_or_invalid_activity_remains_unknown_after_restart() {
    use flecha_core::sleep::{DISCARD_AFTER_WALL_CLOCK, SleepState, SleepTab, sleep_state};
    let saved = r#"{"version":3,"tabs":{
        "missing":{"url":"https://example.org/missing"},
        "invalid":{"url":"https://example.org/invalid","lastActive":"recently"},
        "boolean":{"url":"https://example.org/boolean","lastActive":false},
        "epoch":{"url":"https://example.org/epoch","lastActive":0}
    },"spaces":[{"id":"s","today":["missing","invalid","boolean","epoch"]}]}"#;
    let mut state = decode(saved).unwrap();
    state = decode(&encode(&state).unwrap()).unwrap();
    let now = DISCARD_AFTER_WALL_CLOCK + 1;
    for (id, timestamp, expected) in [
        ("missing", None, SleepState::Alive),
        ("invalid", None, SleepState::Alive),
        ("boolean", None, SleepState::Alive),
        ("epoch", Some(0.0), SleepState::Discarded),
    ] {
        let tab = state.tab(&TabId::new(id)).unwrap();
        assert_eq!(tab.last_active, timestamp);
        assert_eq!(
            sleep_state(
                SleepTab {
                    url: Some(&tab.url),
                    last_active: tab.last_active.map(|time| time as i64),
                    ..SleepTab::default()
                },
                now,
                now,
                false
            ),
            expected
        );
    }
    assert_eq!(state.archive_stale(now), [TabId::new("epoch")]);
}

#[test]
fn rejects_unstartable_shapes_and_future_schemas() {
    for raw in [
        "",
        "null",
        "[]",
        "{}",
        r#"{"spaces":[],"tabs":{}}"#,
        r#"{"spaces":[{"name":"no id"}],"tabs":{}}"#,
        r#"{"spaces":[{"id":"s"}]}"#,
    ] {
        assert!(decode(raw).is_err(), "{raw}");
    }
    let future = json!(format!(
        r#"{{"version":{},"spaces":[{{"id":"s"}}],"tabs":{{}}}}"#,
        SCHEMA_VERSION + 1
    ));
    assert!(matches!(
        load(Some(&future), Some(&json!(V2))),
        Err(DecodeError::UnsupportedVersion(_))
    ));
}

#[test]
fn sidebar_width_is_clamped_on_edit_and_repaired_on_load() {
    let mut input: Value = serde_json::from_str(V2).unwrap();
    for (requested, width) in [
        (90.0, 126.0),
        (1000.0, 400.0),
        (300.4, 300.0),
        (300.5, 301.0),
    ] {
        let mut state = decode(V2).unwrap();
        state.settings.sidebar_width = SidebarWidth::clamp(requested).unwrap();
        assert_eq!(
            decode(&encode(&state).unwrap())
                .unwrap()
                .settings
                .sidebar_width
                .pixels(),
            width
        );
    }
    for invalid in [json!(125), json!(401), json!("300")] {
        input["sidebarWidth"] = invalid;
        assert_eq!(
            decode(&input.to_string()).unwrap().settings.sidebar_width,
            SidebarWidth::default()
        );
    }
}

#[test]
fn damaged_state_is_preserved_even_when_its_backup_needs_a_newer_reader() {
    let damaged = json!("broken JSON");
    let future = json!(r#"{"version":4,"spaces":[{"id":"s"}],"tabs":{}}"#);
    let mut loaded = load(Some(&damaged), Some(&future)).unwrap();
    assert_eq!(recovery_writes(&mut loaded), [(Slot::Damaged, damaged)]);
    let loaded = loaded.ready().unwrap();
    assert_eq!(loaded.recovered, Some(Recovery::UnsupportedBackup));
    assert!(loaded.state.is_none());
    assert!(loaded.started().is_none());
    let mut loaded = load(Some(&json!(V2)), Some(&future)).unwrap();
    let loaded = loaded.ready().unwrap();
    assert!(loaded.state.is_some() && loaded.started().is_none());
}

#[test]
fn decimal_and_exponent_future_versions_are_never_replaced() {
    for version in ["4.0", "4e0", "\"4\""] {
        let future = json!(format!(
            r#"{{"version":{version},"spaces":[{{"id":"s"}}],"tabs":{{}}}}"#
        ));
        assert!(
            matches!(
                load(Some(&future), Some(&json!(V2))),
                Err(DecodeError::UnsupportedVersion(_))
            ),
            "{version}"
        );
    }
}

#[test]
fn quoted_legacy_version_loads_and_fractional_values_survive() {
    let mut input: Value = serde_json::from_str(V2).unwrap();
    input["version"] = json!("2");
    assert!(migrate_legacy(&input.to_string()).is_ok());
    input["version"] = json!(2);
    input["sidebarWidth"] = json!(200.6);
    input["tabs"]["review"]["lastActive"] = json!(12.7);
    let encoded: Value =
        serde_json::from_str(&encode(&decode(&input.to_string()).unwrap()).unwrap()).unwrap();
    assert_eq!(encoded["sidebarWidth"], 200.6);
    assert_eq!(encoded["tabs"]["review"]["lastActive"], 12.7);
}

#[test]
fn duplicate_ids_keep_both_spaces_and_folders_with_their_contents() {
    let raw = r#"{
        "spaces": [
            {"id":"s", "name":"First","today":["a"],"pinned":[{"kind":"folder","id":"f","name":"First folder","children":[{"kind":"tab","tabId":"t"}]}]},
            {"id":"s", "name":"Second","today":["b"],"pinned":[{"kind":"folder","id":"f","name":"Second folder","children":[{"kind":"tab","tabId":"u"}]}]}
        ],
        "tabs":{"a":{"url":"a"},"b":{"url":"b"},"t":{"url":"t"},"u":{"url":"u"}}
    }"#;
    let state = decode(raw).unwrap();
    assert_eq!(state.spaces().len(), 2);
    let [first, second] = state.spaces() else {
        panic!("expected two Spaces")
    };
    assert_ne!(first.id, second.id);
    assert_eq!(second.name, "Second");
    assert_eq!(second.pinned_tabs(), [&TabId::new("u")]);
    let folder_ids = [first, second].map(|space| match &space.pinned[0] {
        PinnedNode::Folder { id, .. } => id,
        _ => panic!("expected the saved folder"),
    });
    assert_ne!(folder_ids[0], folder_ids[1]);
    assert_eq!(
        state.focused_window().spaces[&second.id].today,
        [TabId::new("b")]
    );
}

#[test]
fn reopened_window_selects_a_space_that_retained_tabs() {
    let raw = r#"{
        "version":2, "spaces":[{"id":"s"},{"id":"other"}],
        "tabs":{"source":{"url":"a"},"phone":{"url":"b","device":"mobile"},"kept":{"url":"c"}},
        "windows":[{"id":"open"}],
        "closedWindows":[{"id":"closed","closedAt":2,"activeSpaceId":"s","spaces":{
            "s":{"today":["phone"],"split":["source","phone"],"splitMode":"responsive","activeTabId":"phone"},
            "other":{"today":["kept"],"activeTabId":"kept"}
        }}]
    }"#;
    let mut state = decode(raw).unwrap();
    assert_eq!(state.reopen_window().result, Some(WindowId::new("closed")));
    assert_eq!(state.focused_window().visible_tabs(), [TabId::new("kept")]);
}

#[test]
fn exact_schema_comparison_protects_current_and_last_good_copies() {
    let future = json!(r#"{"version":3.0000000000000001,"spaces":[{"id":"s"}],"tabs":{}}"#);
    assert!(matches!(
        load(Some(&future), Some(&json!(V2))),
        Err(DecodeError::UnsupportedVersion(_))
    ));
    let mut loaded = load(Some(&json!(V2)), Some(&future)).unwrap();
    let loaded = loaded.ready().unwrap();
    assert!(loaded.state.is_some());
    assert!(loaded.started().is_none());
    for equivalent in ["3.000", "30e-1", "0.0300e+2"] {
        let same = format!(r#"{{"version":{equivalent},"spaces":[{{"id":"s"}}],"tabs":{{}}}}"#);
        assert!(decode(&same).is_ok());
        assert!(matches!(
            migrate_legacy(&same),
            Err(DecodeError::AlreadyMigrated)
        ));
    }
}

#[test]
fn generated_space_ids_do_not_claim_stale_window_memberships() {
    let raw = r#"{
        "version":2, "spaces":[{"id":"s"},{"id":"s"}],
        "tabs":{"open-stale":{"url":"a"},"closed-stale":{"url":"b"},"kept":{"url":"c"}},
        "windows":[{"id":"open","activeSpaceId":"recovered-space-1","spaces":{
          "recovered-space-2":{"today":["open-stale"],"activeTabId":"open-stale"}
        }}],
        "closedWindows":[{"id":"closed","closedAt":2,"activeSpaceId":"recovered-space-3","spaces":{
          "recovered-space-4":{"today":["closed-stale"]},"s":{"today":["kept"]}
        }}]
    }"#;
    let state = decode(raw).unwrap();
    assert_eq!(state.focused_window().active_space_id, SpaceId::new("s"));
    assert!(state.focused_window().spaces.is_empty());
    let closed = &state.closed_windows()[0].window;
    assert_eq!(closed.active_space_id, SpaceId::new("s"));
    assert_eq!(closed.spaces.len(), 1);
    assert!(state.open_tabs().is_empty());
    for id in ["open-stale", "closed-stale"] {
        assert!(state.tab(&TabId::new(id)).is_some());
    }
}

#[test]
fn legacy_root_active_space_cannot_point_to_a_generated_id() {
    let raw = r#"{"spaces":[{"id":"s"},{"id":"s"}],"tabs":{},"activeSpaceId":"recovered-space-1"}"#;
    let state = migrate_legacy(raw).unwrap();
    assert_eq!(state.focused_window().active_space_id, SpaceId::new("s"));
    assert_ne!(state.spaces()[1].id, SpaceId::new("recovered-space-1"));
}
