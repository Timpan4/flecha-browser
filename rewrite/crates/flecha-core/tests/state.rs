use flecha_core::storage::{decode, encode};
use flecha_core::{
    ArchivePolicy, BrowserState, CLOSED_WINDOW_LIMIT, CloseTab, FolderId, Placement, Reopened,
    SpaceId, Tab, TabId, WindowId,
};

#[test]
fn today_belongs_to_one_window_and_shared_tabs_have_one_visible_owner() {
    let home = SpaceId::new("home");
    let work = SpaceId::new("work");
    let first = WindowId::new("first");
    let second = WindowId::new("second");
    let favorite = TabId::new("favorite");
    let pin = TabId::new("pin");
    let today = TabId::new("today");
    let folder = FolderId::new("folder");
    let mut state = BrowserState::new(home.clone(), first.clone());
    assert!(state.add_space(work.clone()));
    assert!(state.update_space(
        &work,
        Some(" Work "),
        Some("💼"),
        Some("#AA1122"),
        Some(true)
    ));
    assert_eq!(state.space(&work).unwrap().color, "#aa1122");
    assert!(state.open_window(second.clone(), Some(&work)));
    assert!(state.add_folder(folder.clone(), "Project", &work, None));
    assert!(state.create_tab(
        favorite.clone(),
        Tab::new("https://example.org/", 0),
        Placement::Favorite,
        None
    ));
    assert!(state.create_tab(
        pin.clone(),
        Tab::new("https://example.org/pin", 0),
        Placement::Pinned {
            space: &work,
            folder: Some(&folder)
        },
        None
    ));
    assert!(state.open_today(
        today.clone(),
        Tab::new("https://example.org/today", 1),
        &second,
        None,
        true
    ));
    assert_eq!(state.tab_ids(&first, &work).unwrap(), [&favorite, &pin]);
    assert_eq!(
        state.tab_ids(&second, &work).unwrap(),
        [&favorite, &pin, &today]
    );
    assert_eq!(
        state.activate(&today, &first, Some(2)).result,
        Some(second.clone())
    );
    assert_eq!(
        state.activate(&pin, &first, Some(3)).result,
        Some(first.clone())
    );
    assert_eq!(state.window(&first).unwrap().active_space_id, work);
    assert_eq!(
        state.window(&second).unwrap().visible_tabs(),
        std::slice::from_ref(&today)
    );
    assert_eq!(
        state.activate(&pin, &second, Some(4)).result,
        Some(second.clone())
    );
    assert!(!state.window(&first).unwrap().visible_tabs().contains(&pin));
    assert_eq!(
        state.window(&second).unwrap().visible_tabs(),
        std::slice::from_ref(&pin)
    );
    assert_eq!(
        state.set_isolated(&work, true),
        [pin.clone(), today.clone()]
    );
    assert!(state.space(&work).unwrap().isolated);
    let missing_folder = FolderId::new("missing");
    assert!(
        !state
            .place_tab(
                &today,
                Placement::Pinned {
                    space: &work,
                    folder: Some(&missing_folder)
                },
                None
            )
            .result
    );
    assert_eq!(
        state.locate(&today),
        Some(Placement::Today {
            space: &work,
            window: &second
        })
    );
    assert!(
        !state
            .move_folder(&folder, &work, Some(&folder), None)
            .result
    );
    assert!(state.move_folder(&folder, &home, None, None).result);
    assert_eq!(state.space(&home).unwrap().pinned_tabs(), [&pin]);
    assert!(state.delete_folder(&folder, true, 5).result);
    assert_eq!(state.space(&home).unwrap().pinned_tabs(), [&pin]);
    assert!(state.set_sidebar_visible(&second, false));
    assert!(state.set_sidebar_visible(&first, true));
    assert!(!state.sidebar_visible(&second));
    assert!(state.sidebar_visible(&first));
}

#[test]
fn recency_skips_unseen_tabs_until_control_tab_commits() {
    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let oldest = TabId::new("oldest");
    let newest = TabId::new("newest");
    let background = TabId::new("background");
    let mut state = BrowserState::new(space, window.clone());
    for (time, id, active) in [
        (1, &oldest, true),
        (2, &newest, true),
        (3, &background, false),
    ] {
        assert!(state.open_today(
            id.clone(),
            Tab::new("https://example.org/", time),
            &window,
            None,
            active
        ));
    }
    assert_eq!(state.recent_tabs(&window).unwrap(), [&oldest, &background]);
    assert_eq!(
        state.activate(&background, &window, None).result,
        Some(window.clone())
    );
    let mut state = decode(&encode(&state).unwrap()).unwrap();
    assert!(state.tab(&background).unwrap().unseen);
    assert_eq!(
        state.by_recency([&oldest, &newest, &background]),
        [&newest, &oldest, &background]
    );
    assert!(state.touch(&background, 4));
    assert_eq!(
        state.by_recency([&oldest, &newest, &background]),
        [&background, &newest, &oldest]
    );
}

#[test]
fn undo_reopens_whichever_closed_most_recently() {
    let space = SpaceId::new("s");
    let first = WindowId::new("first");
    let second = WindowId::new("second");
    let today = TabId::new("today");
    let tab = TabId::new("tab");
    let favorite = TabId::new("favorite");
    let mut state = BrowserState::new(space.clone(), first.clone());
    assert!(!state.close_window(&first, 0).result);
    assert!(state.open_window(second.clone(), None));
    assert!(state.open_today(
        today.clone(),
        Tab::new("https://example.org/", 1),
        &second,
        None,
        true
    ));
    assert!(state.open_today(
        tab.clone(),
        Tab::new("https://example.org/closed", 2),
        &first,
        None,
        true
    ));
    assert!(state.create_tab(
        favorite.clone(),
        Tab::new("https://example.org/home", 3),
        Placement::Favorite,
        None
    ));
    state.tab_mut(&favorite).unwrap().url = "https://example.org/somewhere".into();
    assert_eq!(state.close_tab(&favorite, 4).result, CloseTab::Unloaded);
    assert_eq!(
        state.tab(&favorite).unwrap().url,
        "https://example.org/home"
    );
    assert_eq!(
        state.activate(&favorite, &second, Some(5)).result,
        Some(second.clone())
    );
    assert!(state.close_window(&second, 6).result);
    assert!(!state.open_tabs().contains_key(&today));
    assert_eq!(state.close_tab(&tab, 7).result, CloseTab::Archived);
    let mut state = decode(&encode(&state).unwrap()).unwrap();
    let reopen = TabId::new("reopened");
    assert_eq!(
        state.reopen_last(reopen.clone(), 8).result,
        Some(Reopened::Tab(reopen.clone()))
    );
    assert_eq!(
        state.tab(&reopen).unwrap().url,
        "https://example.org/closed"
    );
    assert_eq!(
        state.reopen_last(TabId::new("unused"), 9).result,
        Some(Reopened::Window(second.clone()))
    );
    assert_eq!(state.focused_window().id, second);
    assert_eq!(
        state.focused_window().visible_tabs(),
        std::slice::from_ref(&today)
    );
}

#[test]
fn closed_window_limit_archives_displaced_today_tabs() {
    let window = WindowId::new("main");
    let mut state = BrowserState::new(SpaceId::new("s"), window);
    for n in 0..=CLOSED_WINDOW_LIMIT {
        let window = WindowId::new(format!("window{n}"));
        let tab = TabId::new(format!("tab{n}"));
        assert!(state.open_window(window.clone(), None));
        assert!(state.open_today(
            tab,
            Tab::new(format!("https://example.org/{n}"), 0),
            &window,
            None,
            false
        ));
        let closed = state.close_window(&window, i64::try_from(n).unwrap());
        assert!(closed.result);
        if n == CLOSED_WINDOW_LIMIT {
            assert_eq!(closed.removed, [TabId::new("tab0")]);
        } else {
            assert!(closed.removed.is_empty());
        }
    }
    assert_eq!(state.closed_windows().len(), CLOSED_WINDOW_LIMIT);
    assert!(state.tab(&TabId::new("tab0")).is_none());
    assert_eq!(state.archive()[0].url, "https://example.org/0");
    assert_eq!(
        state.reopen_last(TabId::new("reopened"), 100).result,
        Some(Reopened::Window(WindowId::new(format!(
            "window{CLOSED_WINDOW_LIMIT}"
        ))))
    );
}

#[test]
fn evicted_window_tabs_take_the_front_even_when_the_archive_is_full() {
    use flecha_core::ARCHIVE_LIMIT;
    let main = WindowId::new("main");
    let mut state = BrowserState::new(SpaceId::new("s"), main.clone());
    for n in 0..ARCHIVE_LIMIT {
        let tab = TabId::new(format!("archived{n}"));
        let time = i64::try_from(n).unwrap();
        assert!(state.open_today(
            tab.clone(),
            Tab::new(format!("https://example.org/archived{n}"), time),
            &main,
            None,
            true
        ));
        assert_eq!(state.close_tab(&tab, time).result, CloseTab::Archived);
    }
    let first_closed = i64::try_from(ARCHIVE_LIMIT).unwrap();
    for n in 0..=CLOSED_WINDOW_LIMIT {
        let window = WindowId::new(format!("window{n}"));
        assert!(state.open_window(window.clone(), None));
        assert!(state.open_today(
            TabId::new(format!("x{n}")),
            Tab::new(format!("https://example.org/x{n}"), 0),
            &window,
            None,
            true
        ));
        assert!(
            state
                .close_window(&window, first_closed + i64::try_from(n).unwrap())
                .result
        );
    }
    assert_eq!(state.archive().len(), ARCHIVE_LIMIT);
    assert_eq!(state.archive()[0].url, "https://example.org/x0");
    assert_eq!(state.archive()[0].closed_at, first_closed);
    assert_eq!(
        state.archive()[1].url,
        format!("https://example.org/archived{}", ARCHIVE_LIMIT - 1)
    );
    assert_eq!(
        state.archive().last().unwrap().url,
        "https://example.org/archived1"
    );
    let mut state = decode(&encode(&state).unwrap()).unwrap();
    assert_eq!(
        state
            .reopen_last(TabId::new("reopened"), first_closed)
            .result,
        Some(Reopened::Window(WindowId::new(format!(
            "window{CLOSED_WINDOW_LIMIT}"
        ))))
    );
    assert_eq!(state.archive()[0].url, "https://example.org/x0");
}

#[test]
fn reopen_orders_evicted_windows_and_tabs_by_close_time_even_after_restart() {
    let main = WindowId::new("main");
    let mut state = BrowserState::new(SpaceId::new("s"), main.clone());
    for n in 0..CLOSED_WINDOW_LIMIT {
        let window = WindowId::new(format!("window{n}"));
        assert!(state.open_window(window.clone(), None));
        assert!(state.open_today(
            TabId::new(format!("w{n}")),
            Tab::new(format!("https://example.org/w{n}"), 0),
            &window,
            None,
            true
        ));
        assert!(
            state
                .close_window(&window, i64::try_from(n + 1).unwrap())
                .result
        );
    }
    let a = TabId::new("a");
    assert!(state.open_today(
        a.clone(),
        Tab::new("https://example.org/a", 0),
        &main,
        None,
        true
    ));
    assert_eq!(state.close_tab(&a, 100).result, CloseTab::Archived);
    let last = WindowId::new("last");
    assert!(state.open_window(last.clone(), None));
    assert!(state.open_today(
        TabId::new("last-tab"),
        Tab::new("https://example.org/last", 0),
        &last,
        None,
        true
    ));
    assert!(state.close_window(&last, 200).result);
    assert_eq!(
        state
            .archive()
            .iter()
            .map(|entry| entry.closed_at)
            .collect::<Vec<_>>(),
        [1, 100]
    );

    let saved = encode(&state).unwrap();
    for mut state in [state, decode(&saved).unwrap()] {
        let reopened = TabId::new("reopened");
        assert_eq!(
            state.reopen_last(reopened.clone(), 201).result,
            Some(Reopened::Window(last.clone()))
        );
        assert_eq!(
            state.reopen_last(reopened.clone(), 202).result,
            Some(Reopened::Tab(reopened.clone()))
        );
        assert_eq!(state.tab(&reopened).unwrap().url, "https://example.org/a");
        assert_eq!(
            state.reopen_last(TabId::new("unused"), 203).result,
            Some(Reopened::Window(WindowId::new(format!(
                "window{}",
                CLOSED_WINDOW_LIMIT - 1
            ))))
        );
    }
}

#[test]
fn reopen_uses_close_time_for_windows_behind_the_front() {
    let main = WindowId::new("main");
    let earlier = WindowId::new("earlier");
    let later = WindowId::new("later");
    let mut state = BrowserState::new(SpaceId::new("s"), main);
    for (window, time) in [(&later, 10_000), (&earlier, 9_000)] {
        assert!(state.open_window(window.clone(), None));
        assert!(state.open_today(
            TabId::new(window.as_str()),
            Tab::new("https://example.org", 0),
            window,
            None,
            true
        ));
        assert!(state.close_window(window, time).result);
    }
    assert_eq!(state.closed_windows()[0].window.id, earlier);
    let mut state = decode(&encode(&state).unwrap()).unwrap();
    assert_eq!(
        state.reopen_last(TabId::new("unused"), 9_100).result,
        Some(Reopened::Window(later))
    );
}

#[test]
fn pinned_tab_and_folder_ids_do_not_collide_as_ordering_anchors() {
    let space = SpaceId::new("s");
    let mut state = BrowserState::new(space.clone(), WindowId::new("w"));
    let tab = TabId::new("same");
    let folder = FolderId::new("same");
    let root = Placement::Pinned {
        space: &space,
        folder: None,
    };
    assert!(state.create_tab(tab.clone(), Tab::new("https://example.org", 0), root, None));
    assert!(state.add_folder(folder.clone(), "Folder", &space, None));
    assert!(
        state
            .move_folder(
                &folder,
                &space,
                None,
                Some(flecha_core::PinnedKey::Tab(&tab))
            )
            .result
    );
    assert!(matches!(
        state.spaces()[0].pinned[0],
        flecha_core::PinnedNode::Folder { .. }
    ));
}

#[test]
fn archive_policy_expires_only_inactive_today_tabs_and_archive_is_searchable() {
    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let second = WindowId::new("second");
    let selected = TabId::new("selected");
    let pane = TabId::new("pane");
    let stale = TabId::new("stale");
    let edge = TabId::new("edge");
    let mut state = BrowserState::new(space, window.clone());
    assert_eq!(state.settings.archive_policy, ArchivePolicy::HALF_DAY);
    assert_eq!(
        [
            ArchivePolicy::HALF_DAY,
            ArchivePolicy::DAY,
            ArchivePolicy::WEEK,
            ArchivePolicy::THIRTY_DAYS,
            ArchivePolicy::NEVER
        ]
        .map(ArchivePolicy::hours),
        [12.0, 24.0, 168.0, 720.0, 0.0]
    );
    assert!(state.open_window(second.clone(), None));
    for (id, owner, time) in [
        (&pane, &window, 0),
        (&selected, &window, 0),
        (&stale, &second, 0),
        (&edge, &second, 1),
    ] {
        assert!(state.open_today(
            id.clone(),
            Tab::new("https://www.example.org/project/notes", time),
            owner,
            None,
            false
        ));
    }
    assert!(
        state
            .set_split(&window, [selected.clone(), pane.clone()])
            .result
    );
    let cutoff = 12 * 60 * 60 * 1000 + 1;
    state.settings.archive_policy = ArchivePolicy::NEVER;
    assert!(state.archive_stale(cutoff).is_empty());
    state.settings.archive_policy = ArchivePolicy::HALF_DAY;
    assert_eq!(state.archive_stale(cutoff), [stale]);
    assert!(state.tab(&edge).is_some());
    assert_eq!(state.search_archive("example project").len(), 1);
    assert!(state.search_archive("unrelated").is_empty());
    assert_eq!(
        state.window(&window).unwrap().visible_tabs(),
        [selected, pane]
    );
}

#[test]
fn responsive_split_round_trips_and_exit_removes_only_the_mirror() {
    use flecha_core::{Device, EndedSplit, SplitMode};
    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let source = TabId::new("desktop");
    let mirror = TabId::new("phone");
    let second_pane = TabId::new("second-pane");
    let mut state = BrowserState::new(space.clone(), window.clone());
    let mut tab = Tab::new("https://example.org", 0);
    tab.title = "Project".into();
    assert!(state.open_today(source.clone(), tab, &window, None, true));
    assert!(!state.set_split_ratio(&window, 0.5));
    assert_eq!(
        state.open_responsive(&window, mirror.clone(), 1),
        Some(mirror.clone())
    );
    assert_eq!(
        state.open_responsive(&window, TabId::new("another"), 2),
        None
    );
    assert_eq!(state.tab(&mirror).unwrap().title, "Project");
    assert_eq!(state.tab(&mirror).unwrap().device, Some(Device::Mobile));
    assert_eq!(
        state.focused_window().spaces[&space].today,
        [source.clone(), mirror.clone()]
    );
    assert_eq!(
        state.focused_window().visible_tabs(),
        [source.clone(), mirror.clone()]
    );
    assert!(state.set_split_ratio(&window, 0.1));
    assert_eq!(state.focused_window().spaces[&space].split_ratio, Some(0.2));
    assert!(state.set_split_ratio(&window, 0.9));
    assert_eq!(state.focused_window().spaces[&space].split_ratio, Some(0.8));
    assert_eq!(
        state.activate(&mirror, &window, None).result,
        Some(window.clone())
    );
    let mut wire: serde_json::Value = serde_json::from_str(&encode(&state).unwrap()).unwrap();
    wire["windows"][0]["spaces"]["s"]["splitRatio"] = serde_json::json!(0.1);
    let mut state = decode(&wire.to_string()).unwrap();
    let ws = &state.focused_window().spaces[&space];
    assert_eq!(ws.split_ratio, Some(0.1));
    assert_eq!(ws.split_mode, Some(SplitMode::Responsive));
    assert_eq!(
        state.unsplit(&window),
        Some(EndedSplit::Responsive {
            mirror: mirror.clone()
        })
    );
    assert!(state.tab(&mirror).is_none() && state.archive().is_empty());
    assert_eq!(
        state.focused_window().visible_tabs(),
        std::slice::from_ref(&source)
    );
    assert!(state.open_today(
        second_pane.clone(),
        Tab::new("https://example.org/docs", 2),
        &window,
        None,
        false
    ));
    assert!(
        state
            .set_split(&window, [source, second_pane.clone()])
            .result
    );
    assert_eq!(state.unsplit(&window), Some(EndedSplit::Standard));
    assert!(state.tab(&second_pane).is_some());
}

#[test]
fn today_bulk_close_respects_agent_folder_and_shared_split_rows() {
    use flecha_core::CloseToday;
    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let pin = TabId::new("pin");
    let first = TabId::new("first");
    let last = TabId::new("last");
    let agent_a = TabId::new("agent-a");
    let agent_b = TabId::new("agent-b");
    let mut state = BrowserState::new(space.clone(), window.clone());
    let mut previous = None;
    for (id, agent) in [
        (&first, false),
        (&agent_a, true),
        (&agent_b, true),
        (&last, false),
    ] {
        let mut tab = Tab::new(format!("https://example.org/{}", id.as_str()), 0);
        tab.agent = agent;
        assert!(state.open_today(id.clone(), tab, &window, previous, false));
        previous = Some(id);
    }
    assert!(state.in_agent_folder(&window, &agent_a));
    assert_eq!(state.close_today_tabs(&first, CloseToday::Below, 1), [last]);
    assert_eq!(
        state.close_today_tabs(&agent_a, CloseToday::Others, 2),
        [agent_b]
    );
    assert!(state.create_tab(
        pin.clone(),
        Tab::new("https://example.org/pin", 3),
        Placement::Pinned {
            space: &space,
            folder: None
        },
        None
    ));
    assert!(state.set_split(&window, [pin, agent_a.clone()]).result);
    assert!(state.in_agent_folder(&window, &agent_a));
    assert_eq!(state.clear_today(&window, 4), [first]);
    let ordinary = TabId::new("ordinary");
    assert!(state.open_today(
        ordinary.clone(),
        Tab::new("https://example.org/", 5),
        &window,
        None,
        false
    ));
    assert!(
        state
            .set_split(&window, [ordinary.clone(), agent_a.clone()])
            .result
    );
    assert!(!state.in_agent_folder(&window, &agent_a));
    assert_eq!(state.clear_today(&window, 6), [ordinary, agent_a]);
}

#[test]
fn archive_display_order_survives_clock_rollback_while_undo_uses_close_time() {
    let window = WindowId::new("w");
    let first = TabId::new("first");
    let last = TabId::new("last");
    let reopened = TabId::new("reopened");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    for (id, url) in [
        (&first, "https://example.org/a"),
        (&last, "https://example.org/b"),
    ] {
        state.open_today(id.clone(), Tab::new(url, 0), &window, None, true);
    }
    assert_eq!(state.close_tab(&first, 10_000).result, CloseTab::Archived);
    assert_eq!(state.close_tab(&last, 9_000).result, CloseTab::Archived);
    let saved = encode(&state).unwrap();
    for mut version in [state, decode(&saved).unwrap()] {
        assert_eq!(version.archive()[0].url, "https://example.org/b");
        assert_eq!(
            version.reopen_last(reopened.clone(), 9_100).result,
            Some(Reopened::Tab(reopened.clone()))
        );
        assert_eq!(version.tab(&reopened).unwrap().url, "https://example.org/a");
        assert_eq!(version.archive()[0].url, "https://example.org/b");
    }
}

#[test]
fn closing_other_today_tabs_keeps_the_selected_responsive_pair() {
    let window = WindowId::new("w");
    let desktop = TabId::new("desktop");
    let mirror = TabId::new("mirror");
    let other = TabId::new("other");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    state.open_today(
        other.clone(),
        Tab::new("https://example.org/other", 0),
        &window,
        None,
        false,
    );
    state.open_today(
        desktop.clone(),
        Tab::new("https://example.org", 0),
        &window,
        None,
        true,
    );
    state.open_responsive(&window, mirror.clone(), 1).unwrap();
    for kept in [&desktop, &mirror] {
        let mut state = state.clone();
        assert_eq!(
            state.activate(kept, &window, None).result,
            Some(window.clone())
        );
        assert_eq!(
            state
                .close_today_tabs(kept, flecha_core::CloseToday::Others, 2)
                .as_slice(),
            std::slice::from_ref(&other)
        );
        assert_eq!(
            state.focused_window().visible_tabs(),
            [desktop.clone(), mirror.clone()]
        );
        assert_eq!(state.archive().len(), 1);
    }
}

#[test]
fn close_operations_report_every_removed_today_tab_for_each_pair_position() {
    use flecha_core::{CloseToday, PinnedKey};
    use std::collections::BTreeSet;

    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let desktop = TabId::new("d");
    let mirror = TabId::new("m");
    let anchor = TabId::new("t");
    let last = TabId::new("x");
    let mut baseline = BrowserState::new(space.clone(), window.clone());
    for (id, after) in [
        (&desktop, None),
        (&anchor, Some(&mirror)),
        (&last, Some(&anchor)),
    ] {
        let active = id == &desktop;
        let tab = Tab::new(format!("https://example.org/{}", id.as_str()), 0);
        assert!(baseline.open_today(id.clone(), tab, &window, after, active));
        if active {
            baseline
                .open_responsive(&window, mirror.clone(), 1)
                .unwrap();
        }
    }

    let all_tabs = |state: &BrowserState| {
        state
            .open_tabs()
            .keys()
            .map(|id| (*id).clone())
            .collect::<BTreeSet<_>>()
    };
    let check = |state: &BrowserState, before: BTreeSet<TabId>, reported: Vec<TabId>| {
        let actual: BTreeSet<_> = before
            .into_iter()
            .filter(|id| state.tab(id).is_none())
            .collect();
        assert_eq!(
            reported.len(),
            actual.len(),
            "reported {reported:?}, removed {actual:?}"
        );
        assert_eq!(reported.into_iter().collect::<BTreeSet<_>>(), actual);
    };
    let moved = |before: Option<&TabId>| {
        let mut state = baseline.clone();
        assert!(
            state
                .place_tab(
                    &desktop,
                    Placement::Today {
                        space: &space,
                        window: &window
                    },
                    before.map(PinnedKey::Tab)
                )
                .result
        );
        state
    };
    let mut example = moved(Some(&last));
    assert_eq!(
        example.focused_window().spaces[&space].today,
        [
            mirror.clone(),
            anchor.clone(),
            desktop.clone(),
            last.clone()
        ]
    );
    let before = all_tabs(&example);
    let removed = example.close_today_tabs(&anchor, CloseToday::Below, 2);
    assert!(removed.contains(&mirror));
    check(&example, before, removed);

    for position in [Some(&mirror), Some(&anchor), Some(&last), None] {
        for selected in [&desktop, &mirror, &anchor, &last] {
            for which in [CloseToday::Below, CloseToday::Others] {
                let mut state = moved(position);
                assert!(state.activate(selected, &window, None).result.is_some());
                let before = all_tabs(&state);
                let reported = state.close_today_tabs(selected, which, 2);
                check(&state, before, reported);
            }
        }
        let mut state = moved(position);
        let before = all_tabs(&state);
        let reported = state.clear_today(&window, 2);
        check(&state, before, reported);
        let mut state = moved(position);
        let before = all_tabs(&state);
        let reported = state.archive_stale(12 * 60 * 60 * 1000 + 2);
        check(&state, before, reported);
    }
}

#[test]
fn single_closes_and_deletions_report_durable_ids_removed_from_state() {
    use std::collections::BTreeSet;

    let space = SpaceId::new("s");
    let other = SpaceId::new("other");
    let main = WindowId::new("main");
    let window = WindowId::new("w");
    let desktop = TabId::new("d");
    let mirror = TabId::new("m");
    let keep = TabId::new("keep");
    let folder = FolderId::new("f");
    let known = [&desktop, &mirror, &keep];
    let present = |state: &BrowserState| {
        known
            .into_iter()
            .filter(|id| state.tab(id).is_some())
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let check = |label: &str, before: &BrowserState, after: &BrowserState, removed: Vec<TabId>| {
        let actual: BTreeSet<_> = present(before)
            .difference(&present(after))
            .cloned()
            .collect();
        assert_eq!(
            removed.len(),
            actual.len(),
            "{label}: reported {removed:?}, removed {actual:?}"
        );
        assert_eq!(
            removed.into_iter().collect::<BTreeSet<_>>(),
            actual,
            "{label}"
        );
    };

    for pinned in [false, true] {
        let mut original = BrowserState::new(space.clone(), main.clone());
        assert!(original.add_space(other.clone()));
        assert!(original.open_window(window.clone(), Some(&space)));
        assert!(original.open_today(
            keep.clone(),
            Tab::new("https://example.org/keep", 0),
            &window,
            None,
            false
        ));
        let tab = Tab::new("https://example.org/source", 0);
        if pinned {
            assert!(original.add_folder(folder.clone(), "Folder", &space, None));
            assert!(original.create_tab(
                desktop.clone(),
                tab,
                Placement::Pinned {
                    space: &space,
                    folder: Some(&folder)
                },
                None
            ));
            assert_eq!(
                original.activate(&desktop, &window, Some(1)).result,
                Some(window.clone())
            );
        } else {
            assert!(original.open_today(desktop.clone(), tab, &window, None, true));
        }
        assert_eq!(
            original.open_responsive(&window, mirror.clone(), 2),
            Some(mirror.clone())
        );

        let mut state = original.clone();
        let result = state.close_tab(&desktop, 3);
        assert_eq!(
            result.result,
            if pinned {
                CloseTab::Unloaded
            } else {
                CloseTab::Archived
            }
        );
        check("close tab", &original, &state, result.removed);

        let mut state = original.clone();
        let result = state.close_window(&window, 3);
        assert!(result.result);
        assert!(state.tab(&keep).is_some() && state.tab(&desktop).is_some());
        check("close window", &original, &state, result.removed);

        let mut state = original.clone();
        let result = state.delete_tab(&desktop, 3);
        assert!(result.result);
        check("delete tab", &original, &state, result.removed);

        let mut state = original.clone();
        let result = state.set_split(&window, [desktop.clone(), keep.clone()]);
        assert!(result.result);
        check(
            "replace responsive split",
            &original,
            &state,
            result.removed,
        );

        if pinned {
            for keep_children in [false, true] {
                let mut state = original.clone();
                let result = state.delete_folder(&folder, keep_children, 3);
                assert!(result.result);
                check("delete folder", &original, &state, result.removed);
            }
            let mut state = original.clone();
            let result = state.activate(&desktop, &main, Some(3));
            assert_eq!(result.result, Some(main.clone()));
            check(
                "take a shared responsive source",
                &original,
                &state,
                result.removed,
            );

            let mut state = original.clone();
            let result = state.move_folder(&folder, &other, None, None);
            assert!(result.result);
            check(
                "move folder to another Space",
                &original,
                &state,
                result.removed,
            );

            let mut state = original.clone();
            let result = state.place_tab(
                &desktop,
                Placement::Pinned {
                    space: &other,
                    folder: None,
                },
                None,
            );
            assert!(result.result);
            check(
                "move tab to another Space",
                &original,
                &state,
                result.removed,
            );
        }
    }
}

#[test]
fn moving_a_tab_before_the_folder_with_the_same_id_uses_the_folder_anchor() {
    use flecha_core::{PinnedKey, PinnedNode};
    let space = SpaceId::new("s");
    let folder = FolderId::new("same");
    let tab = TabId::new("same");
    let next = TabId::new("next");
    let mut state = BrowserState::new(space.clone(), WindowId::new("w"));
    let root = Placement::Pinned {
        space: &space,
        folder: None,
    };
    assert!(state.add_folder(folder.clone(), "Folder", &space, None));
    assert!(state.create_tab(tab.clone(), Tab::new("https://example.org", 0), root, None));
    assert!(
        state
            .place_tab(&tab, root, Some(PinnedKey::Folder(&folder)))
            .result
    );
    assert!(state.create_tab(
        next.clone(),
        Tab::new("https://example.org/next", 1),
        root,
        Some(PinnedKey::Folder(&folder))
    ));
    let [first, second, third] = state.spaces()[0].pinned.as_slice() else {
        panic!("expected two tabs and a folder")
    };
    assert!(matches!(first, PinnedNode::Tab { tab_id } if *tab_id == tab));
    assert!(matches!(second, PinnedNode::Tab { tab_id } if *tab_id == next));
    assert!(matches!(third, PinnedNode::Folder { id, .. } if *id == folder));
}

#[test]
fn set_split_rejects_a_phone_mirror_and_replaces_responsive_with_regular() {
    use flecha_core::SplitMode;
    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let desktop = TabId::new("desktop");
    let mirror = TabId::new("mirror");
    let regular = TabId::new("regular");
    let mut state = BrowserState::new(space.clone(), window.clone());
    assert!(state.open_today(
        regular.clone(),
        Tab::new("https://example.org/regular", 0),
        &window,
        None,
        true
    ));
    assert!(state.open_today(
        desktop.clone(),
        Tab::new("https://example.org/desktop", 0),
        &window,
        None,
        true
    ));
    assert_eq!(
        state.open_responsive(&window, mirror.clone(), 1),
        Some(mirror.clone())
    );
    let prior = state.clone();
    assert!(
        !state
            .set_split(&window, [desktop.clone(), mirror.clone()])
            .result
    );
    assert_eq!(state, prior);
    assert!(
        state
            .set_split(&window, [regular.clone(), desktop.clone()])
            .result
    );
    let ws = &state.focused_window().spaces[&space];
    assert_eq!(ws.split_mode, None);
    assert!(!ws.today.contains(&mirror));
    assert!(state.tab(&mirror).is_none() && state.archive().is_empty());
    assert_eq!(state.focused_window().visible_tabs(), [regular, desktop]);
    assert_eq!(
        prior.focused_window().spaces[&space].split_mode,
        Some(SplitMode::Responsive)
    );
}
