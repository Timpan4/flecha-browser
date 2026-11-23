use flecha_core::TabId;

#[test]
fn invalid_keys_cannot_replace_defaults() {
    let platform = Platform::Linux;
    let mut bindings = Keybindings::new();
    for key in ["F99", "Ctrl+Bogus", "T"] {
        assert_eq!(
            set_binding(&mut bindings, "reload", BindingEdit::Set(key), platform),
            Err(BindingError::InvalidAccelerator)
        );
        bindings.insert("reload".into(), Some(key.into()));
        assert_eq!(keys_for(&bindings, "reload", platform), Some("CmdOrCtrl+R"));
        bindings.clear();
    }
}

#[test]
fn shortcut_edits_displace_shadowed_defaults_too() {
    let platform = Platform::Linux;
    let mut bindings = Keybindings::from([("reload".into(), Some("Ctrl+T".into()))]);
    assert_eq!(
        set_binding(&mut bindings, "find", BindingEdit::Set("Ctrl+T"), platform).unwrap(),
        ["newTab", "reload"]
    );
    set_binding(&mut bindings, "find", BindingEdit::Unbind, platform).unwrap();
    assert_eq!(keys_for(&bindings, "newTab", platform), None);
}

#[test]
fn shortcut_conflicts_follow_chromium_modifier_and_key_normalization() {
    let platform = Platform::Linux;
    let bindings = Keybindings::from([("reload".into(), Some("control+1".into()))]);
    assert_eq!(keys_for(&bindings, "reload", platform), Some("CmdOrCtrl+R"));
    assert!(same_keys(
        " control + Ctrl + Return ",
        "CmdOrCtrl+Enter",
        platform
    ));
    assert!(same_keys("Ctrl++", "Control+Plus", platform));
}

#[test]
fn later_command_wins_when_saved_custom_keys_collide() {
    let bindings = Keybindings::from([
        ("reload".into(), Some("Ctrl+J".into())),
        ("find".into(), Some("Ctrl+J".into())),
    ]);
    let platform = Platform::Linux;
    assert_eq!(keys_for(&bindings, "reload", platform), None);
    assert_eq!(keys_for(&bindings, "find", platform), Some("Ctrl+J"));
}

#[test]
fn shortcut_aliases_follow_native_raw_key_order() {
    let linux = Platform::Linux;
    let bindings = Keybindings::from([
        ("reload".into(), Some("control+J".into())),
        ("find".into(), Some("Ctrl+J".into())),
    ]);
    assert_eq!(keys_for(&bindings, "reload", linux), Some("control+J"));
    assert_eq!(keys_for(&bindings, "find", linux), None);
    let map = shortcut_map(&bindings, linux);
    assert_eq!(map.get("control+J").map(String::as_str), Some("reload"));
    assert!(!map.contains_key("Ctrl+J"));
}

#[test]
fn shortcut_parser_matches_chromium_ascii_whitespace() {
    let linux = Platform::Linux;
    let mut bindings = Keybindings::new();
    assert_eq!(
        set_binding(
            &mut bindings,
            "reload",
            BindingEdit::Set("Ctrl+\u{a0}T"),
            linux
        ),
        Err(BindingError::InvalidAccelerator)
    );
    assert!(bindings.is_empty());
    bindings.insert("reload".into(), Some("Ctrl+\u{a0}T".into()));
    assert_eq!(keys_for(&bindings, "newTab", linux), Some("CmdOrCtrl+T"));
    assert_eq!(keys_for(&bindings, "reload", linux), Some("CmdOrCtrl+R"));
    assert!(same_keys("Ctrl+\u{b}T", "Control+T", linux));
}
use flecha_core::shortcuts::{
    BindingEdit, BindingError, Keybindings, Platform, display_keys, keys_for, same_keys,
    set_binding, shortcut_map, space_shortcut_label,
};
use flecha_core::sleep::{
    DISCARD_AFTER, DISCARD_AFTER_WALL_CLOCK, FREEZE_AFTER, PROTECTED_RECENT, STATE_DEBOUNCE,
    SleepState, SleepTab, protected_recent, sleep_state,
};

#[test]
fn platform_defaults_register_without_collisions() {
    let bindings = Keybindings::new();
    for platform in [Platform::Mac, Platform::Linux, Platform::Windows] {
        let map = shortcut_map(&bindings, platform);
        for (key, command) in &map {
            for (other, other_command) in &map {
                if key != other {
                    assert!(
                        !same_keys(key, other, platform),
                        "{command} collides with {other_command}"
                    );
                }
            }
        }
        assert_eq!(map["CmdOrCtrl+1"], "selectTab:1");
        let mac = platform == Platform::Mac;
        assert_eq!(
            map[if mac { "Ctrl+1" } else { "Ctrl+Alt+1" }],
            "selectSpace:1"
        );
        assert_eq!(
            map[if mac {
                "Ctrl+Shift+N"
            } else {
                "Ctrl+Alt+Shift+N"
            }],
            "newSpace"
        );
        assert_eq!(
            map.get("Ctrl+Shift+N").map(String::as_str),
            mac.then_some("newSpace")
        );
        assert_eq!(
            map.get("Ctrl+Shift+Q").map(String::as_str),
            (!mac).then_some("quit")
        );
        assert_eq!(map["CmdOrCtrl+,"], "settings");
        assert_eq!(map["Ctrl+Tab"], "nextTab");
        assert_eq!(map["Ctrl+Shift+Tab"], "previousTab");
        assert_eq!(same_keys("CmdOrCtrl+T", "Ctrl+T", platform), !mac);
        assert_eq!(same_keys("CmdOrCtrl+T", "Cmd+T", platform), mac);
    }
    assert_eq!(
        display_keys(Some("Alt+CmdOrCtrl+Shift+T"), Platform::Linux),
        "Ctrl+Alt+Shift+T"
    );
    assert_eq!(
        display_keys(Some("Alt+CmdOrCtrl+Shift+T"), Platform::Mac),
        "⌥⇧⌘T"
    );
    assert_eq!(
        space_shortcut_label(Platform::Linux),
        "Ctrl+Alt+1 … Ctrl+Alt+9"
    );
}

#[test]
fn shortcut_edits_displace_conflicts_and_protect_fixed_families() {
    let linux = Platform::Linux;
    let mut keys = Keybindings::new();
    assert_eq!(
        set_binding(&mut keys, "reload", BindingEdit::Set("Ctrl+T"), linux).unwrap(),
        ["newTab"]
    );
    assert_eq!(keys_for(&keys, "newTab", linux), None);
    for reserved in ["Ctrl+1", "Alt+Ctrl+9", "Ctrl+O"] {
        assert_eq!(
            set_binding(&mut keys, "reload", BindingEdit::Set(reserved), linux),
            Err(BindingError::Reserved)
        );
        assert_eq!(keys_for(&keys, "reload", linux), Some("Ctrl+T"));
    }
    assert_eq!(
        set_binding(&mut keys, "newTab", BindingEdit::Reset, linux).unwrap(),
        ["reload"]
    );
    assert_eq!(keys_for(&keys, "reload", linux), None);
    assert_eq!(keys_for(&keys, "newTab", linux), Some("CmdOrCtrl+T"));
}

#[test]
fn saved_shortcuts_resolve_again_on_a_different_platform() {
    let mut keys = Keybindings::from([
        ("reload".into(), Some("Ctrl+T".into())),
        ("newTab".into(), Some("Ctrl+Alt+1".into())),
    ]);
    assert_eq!(keys_for(&keys, "newTab", Platform::Linux), None);
    assert_eq!(keys_for(&keys, "reload", Platform::Linux), Some("Ctrl+T"));
    keys.remove("newTab");
    assert_eq!(keys_for(&keys, "newTab", Platform::Linux), None);
    assert_eq!(
        keys_for(&keys, "newTab", Platform::Mac),
        Some("CmdOrCtrl+T")
    );
}

#[test]
fn sleep_distinguishes_active_use_and_wall_clock() {
    use SleepState::{Alive, Discarded, Frozen};
    let now = 10 * DISCARD_AFTER_WALL_CLOCK;
    let minute = 60_000;
    let tab = SleepTab {
        last_active: Some(now),
        used_at: Some(0),
        ..SleepTab::default()
    };
    let frozen = SleepTab {
        state: Frozen,
        ..tab
    };
    assert_eq!(sleep_state(tab, now, FREEZE_AFTER - minute, false), Alive);
    assert_eq!(sleep_state(tab, now, FREEZE_AFTER, false), Frozen);
    assert_eq!(
        sleep_state(frozen, now, DISCARD_AFTER - minute, false),
        Frozen
    );
    assert_eq!(sleep_state(frozen, now, DISCARD_AFTER, false), Discarded);
    let day_idle = SleepTab {
        last_active: Some(now - DISCARD_AFTER_WALL_CLOCK),
        ..tab
    };
    assert_eq!(sleep_state(day_idle, now, 0, false), Discarded);
    assert_eq!(
        sleep_state(
            SleepTab {
                last_active: day_idle.last_active.map(|time| time + minute),
                ..tab
            },
            now,
            0,
            false
        ),
        Alive
    );

    let tabs: Vec<_> = (0..12)
        .map(|i| (TabId::new(format!("t{i}")), now - i * minute))
        .collect();
    let kept = protected_recent(tabs.iter().map(|(id, last)| (id, *last)));
    assert_eq!(
        kept,
        tabs.iter()
            .take(PROTECTED_RECENT)
            .map(|(id, _)| id)
            .collect()
    );
    assert_eq!(sleep_state(tab, now, DISCARD_AFTER, true), Frozen);
    assert_eq!(sleep_state(day_idle, now, 0, true), Frozen);
}

#[test]
fn sleep_uses_the_known_clock_if_the_other_is_missing() {
    use SleepState::{Alive, Discarded, Frozen};
    let now = DISCARD_AFTER_WALL_CLOCK;
    let only_wall_clock = SleepTab {
        last_active: Some(0),
        ..SleepTab::default()
    };
    assert_eq!(sleep_state(only_wall_clock, now - 1, 0, false), Alive);
    assert_eq!(sleep_state(only_wall_clock, now, 0, false), Discarded);

    let only_active_use = SleepTab {
        used_at: Some(0),
        ..SleepTab::default()
    };
    assert_eq!(
        sleep_state(only_active_use, now, FREEZE_AFTER - 1, false),
        Alive
    );
    assert_eq!(
        sleep_state(only_active_use, now, FREEZE_AFTER, false),
        Frozen
    );
    assert_eq!(
        sleep_state(only_active_use, now, DISCARD_AFTER, false),
        Discarded
    );
}

#[test]
fn sleep_prioritizes_visibility_media_and_debounce() {
    use SleepState::{Alive, Discarded, Frozen};
    let now = 10 * DISCARD_AFTER_WALL_CLOCK;
    let tab = SleepTab {
        last_active: Some(now),
        used_at: Some(0),
        ..SleepTab::default()
    };
    for url in [
        "https://www.notion.so/a",
        "https://dev.notion.so/",
        "https://stg.notion.so/x",
        "https://app.notion.com/p",
    ] {
        assert_eq!(
            sleep_state(
                SleepTab {
                    url: Some(url),
                    ..tab
                },
                now,
                DISCARD_AFTER,
                false
            ),
            Alive,
            "{url}"
        );
    }
    for url in [
        "https://notion.so/a",
        "https://www.notion.so.example.com/",
        "https://example.com/?u=https://www.notion.so/",
    ] {
        assert_eq!(
            sleep_state(
                SleepTab {
                    url: Some(url),
                    ..tab
                },
                now,
                DISCARD_AFTER,
                false
            ),
            Discarded,
            "{url}"
        );
    }
    let hidden = SleepTab {
        in_hidden_space: true,
        ..tab
    };
    assert_eq!(sleep_state(hidden, now, 0, false), Frozen);
    assert_eq!(sleep_state(hidden, now, DISCARD_AFTER, false), Discarded);
    assert_eq!(sleep_state(hidden, now, DISCARD_AFTER, true), Frozen);
    for shown in [
        SleepTab {
            shown: true,
            state: Frozen,
            ..tab
        },
        SleepTab { media: true, ..tab },
        SleepTab {
            media: true,
            ..hidden
        },
    ] {
        assert_eq!(sleep_state(shown, now, DISCARD_AFTER, false), Alive);
    }
    let recent_change = SleepTab {
        changed_at: Some(now - STATE_DEBOUNCE + 1),
        ..tab
    };
    assert_eq!(sleep_state(recent_change, now, DISCARD_AFTER, false), Alive);
    let settled = SleepTab {
        changed_at: Some(now - STATE_DEBOUNCE),
        ..tab
    };
    assert_eq!(sleep_state(settled, now, DISCARD_AFTER, false), Discarded);
    assert_eq!(
        sleep_state(
            SleepTab {
                state: Discarded,
                ..tab
            },
            now,
            0,
            false
        ),
        Discarded
    );
}

#[test]
fn site_ad_block_rules_cover_subdomains_and_survive_restart() {
    use flecha_core::{
        BrowserState, SpaceId, WindowId, is_site_host,
        storage::{decode, encode},
    };
    let mut state = BrowserState::new(SpaceId::new("s"), WindowId::new("w"));
    let root = "example.org";
    let subdomain = "www.example.org";
    for host in [root, "xn--bcher-kva.example", "127.0.0.1", "localhost"] {
        assert!(is_site_host(host));
    }
    for invalid in [
        "",
        "Example.org",
        ".foo",
        "foo..bar",
        "foo.",
        "[::1]",
        "foo:8080",
        "https://foo",
    ] {
        assert!(!is_site_host(invalid));
        assert!(!state.settings.set_ad_blocking_on(invalid, false));
    }
    assert!(state.settings.blocks_ads_on(root));
    assert!(state.settings.set_ad_blocking_on(root, false));
    assert!(state.settings.set_ad_blocking_on(root, false));
    assert!(state.settings.set_ad_blocking_on(subdomain, false));
    assert!(!state.settings.blocks_ads_on(subdomain));
    assert!(state.settings.blocks_ads_on("notexample.org"));
    assert_eq!(state.settings.ad_block_allowed.as_deref().unwrap(), [root]);
    let mut state = decode(&encode(&state).unwrap()).unwrap();
    assert!(!state.settings.blocks_ads_on(root));
    assert!(state.settings.set_ad_blocking_on(subdomain, true));
    assert!(state.settings.blocks_ads_on(root));
    assert_eq!(state.settings.ad_block_allowed, Some(Vec::new()));
    let malformed = encode(&state).unwrap().replace(
        "\"adBlockAllowed\": []",
        "\"adBlockAllowed\": [\"good.org\",\"[::1]\"]",
    );
    assert_eq!(decode(&malformed).unwrap().settings.ad_block_allowed, None);
}
