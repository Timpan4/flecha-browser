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
    assert_eq!(state.tab(&TabId::new("t3")).unwrap().last_active, 42.0);
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
    assert!(loaded.recovery_writes.is_empty());
    let startup = loaded.state.as_ref().unwrap().clone();
    loaded.state.as_mut().unwrap().settings.theme = Theme::Light;
    let backup = loaded.started().unwrap();
    assert_eq!(backup.slot, Slot::LastGood);
    assert_eq!(decode(backup.value.as_str().unwrap()).unwrap(), startup);
    assert!(loaded.started().is_none());
    let mut unchanged = load(Some(&backup.value), Some(&backup.value)).unwrap();
    assert!(unchanged.started().is_none());
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
        assert_eq!(loaded.recovered, Some(Recovery::LastGood));
        assert_eq!(loaded.state.as_ref().unwrap().spaces()[0].name, "Personal");
        assert_eq!(loaded.recovery_writes[0].slot, Slot::Damaged);
        assert_eq!(loaded.recovery_writes[0].value, damaged);
        assert_eq!(loaded.recovery_writes[1].slot, Slot::Current);
        assert_eq!(loaded.recovery_writes[1].value, fallback);
        assert!(loaded.started().is_none());
    }
    let damaged = json!(r#"{"spaces":[]}"#);
    let mut loaded = load(Some(&damaged), Some(&json!("not json"))).unwrap();
    assert_eq!(loaded.recovered, Some(Recovery::Fresh));
    assert!(loaded.state.is_none() && loaded.started().is_none());
    assert_eq!(loaded.recovery_writes.len(), 1);
    assert_eq!(loaded.recovery_writes[0].value, damaged);
    let mut first = load(None, Some(&fallback)).unwrap();
    assert_eq!(first.recovered, None);
    assert!(first.state.is_none() && first.recovery_writes.is_empty() && first.started().is_none());
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
    assert_eq!(loaded.recovered, Some(Recovery::UnsupportedBackup));
    assert_eq!(loaded.recovery_writes.len(), 1);
    assert_eq!(loaded.recovery_writes[0].slot, Slot::Damaged);
    assert_eq!(loaded.recovery_writes[0].value, damaged);
    assert!(loaded.state.is_none());
    assert!(loaded.started().is_none());
    let mut loaded = load(Some(&json!(V2)), Some(&future)).unwrap();
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
    assert_eq!(state.reopen_window(), Some(WindowId::new("closed")));
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
