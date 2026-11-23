use super::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn pick<T: Clone>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.below(items.len())].clone())
    }
}

fn keys(state: &BrowserState) -> BTreeSet<TabId> {
    state.tabs.keys().cloned().collect()
}

fn referenced(state: &BrowserState) -> BTreeSet<TabId> {
    let mut ids: BTreeSet<TabId> = state.favorites.iter().cloned().collect();
    for space in &state.spaces {
        ids.extend(space.pinned_tabs().into_iter().cloned());
    }
    for win in state
        .windows
        .iter()
        .chain(state.closed_windows.iter().map(|c| &c.window))
    {
        for ws in win.spaces.values() {
            ids.extend(ws.today.iter().cloned());
        }
    }
    ids
}

#[test]
fn random_transitions_preserve_tab_and_selection_invariants() {
    for seed in 1..=128u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut state = BrowserState::new(SpaceId::new("s0"), WindowId::new("w0"));
        let mut next_tab = 0;
        let mut next_space = 1;
        let mut next_window = 1;
        let mut next_folder = 0;
        let mut now: EpochMillis = 1_000_000;
        for step in 0..120 {
            now += 1 + rng.below(4 * 3_600_000) as i64;
            let tabs: Vec<TabId> = state.tabs.keys().cloned().collect();
            let mut any_tabs = tabs.clone();
            any_tabs.push(TabId::new("missing"));
            let windows: Vec<WindowId> = state.windows.iter().map(|w| w.id.clone()).collect();
            let spaces: Vec<SpaceId> = state.spaces.iter().map(|s| s.id.clone()).collect();
            let folders: Vec<FolderId> = (0..next_folder)
                .map(|i| FolderId::new(format!("f{i}")))
                .collect();
            let before = keys(&state);
            let snapshot = state.clone();
            let op = rng.below(26);
            let mut reported: Option<Vec<TabId>> = None;
            let mut anchor_check = None;
            let mut selection_owner = None;
            let mut targeted_tab = None;
            let name: &str;
            match op {
                0..=2 => {
                    name = "open_today";
                    let id = TabId::new(format!("t{next_tab}"));
                    next_tab += 1;
                    let mut tab = Tab::new(format!("https://e.org/{}", id.as_str()), now);
                    tab.agent = rng.below(5) == 0;
                    let w = rng.pick(&windows).unwrap();
                    let after = rng.pick(&tabs);
                    let _ = state.open_today(id, tab, &w, after.as_ref(), rng.below(2) == 0);
                    selection_owner = Some(w);
                }
                3 => {
                    name = "create_shared";
                    let id = TabId::new(format!("t{next_tab}"));
                    next_tab += 1;
                    let tab = Tab::new(format!("https://p.org/{}", id.as_str()), now);
                    let s = rng.pick(&spaces).unwrap();
                    let f = if rng.below(2) == 0 {
                        rng.pick(&folders)
                    } else {
                        None
                    };
                    let to = if rng.below(4) == 0 {
                        Placement::Favorite
                    } else {
                        Placement::Pinned {
                            space: &s,
                            folder: f.as_ref(),
                        }
                    };
                    let _ = state.create_tab(id, tab, to, None);
                }
                4 => {
                    name = "add_space";
                    let _ = state.add_space(SpaceId::new(format!("s{next_space}")));
                    next_space += 1;
                }
                5 => {
                    name = "open_window";
                    let s = rng.pick(&spaces);
                    let _ = state.open_window(WindowId::new(format!("w{next_window}")), s.as_ref());
                    next_window += 1;
                }
                6 => {
                    name = "close_window";
                    let w = rng.pick(&windows).unwrap();
                    reported = Some(state.close_window(&w, now).removed);
                }
                7 => {
                    name = "reopen_window";
                    reported = Some(state.reopen_window().removed);
                }
                8 => {
                    name = "reopen_last";
                    let id = TabId::new(format!("t{next_tab}"));
                    next_tab += 1;
                    let focused = state.focused_window().id.clone();
                    let change = state.reopen_last(id, now);
                    if matches!(change.result, Some(Reopened::Tab(_))) {
                        selection_owner = Some(focused);
                    }
                    reported = Some(change.removed);
                }
                9 => {
                    name = "switch_space";
                    let w = rng.pick(&windows).unwrap();
                    let s = rng.pick(&spaces).unwrap();
                    let change = state.switch_space(&w, &s);
                    selection_owner = Some(w);
                    reported = Some(change.removed);
                }
                10 | 11 => {
                    name = "activate";
                    let w = rng.pick(&windows).unwrap();
                    let t = rng.pick(&any_tabs).unwrap();
                    let change = state.activate(&t, &w, Some(now));
                    selection_owner = change.result;
                    reported = Some(change.removed);
                }
                12 => {
                    name = "place_tab";
                    let t = rng.pick(&any_tabs).unwrap();
                    let s = rng.pick(&spaces).unwrap();
                    let w = rng.pick(&windows).unwrap();
                    let f = rng.pick(&folders);
                    let before_tab = rng.pick(&any_tabs);
                    let to = match rng.below(3) {
                        0 => Placement::Favorite,
                        1 => Placement::Pinned {
                            space: &s,
                            folder: f.as_ref(),
                        },
                        _ => Placement::Today {
                            space: &s,
                            window: &w,
                        },
                    };
                    let before = if rng.below(2) == 0 {
                        before_tab.as_ref().map(PinnedKey::Tab)
                    } else {
                        None
                    };
                    reported = Some(state.place_tab(&t, to, before).removed);
                    targeted_tab = Some(t);
                }
                13 | 14 => {
                    name = "close_tab";
                    let t = rng.pick(&any_tabs).unwrap();
                    reported = Some(state.close_tab(&t, now).removed);
                    targeted_tab = Some(t);
                }
                15 => {
                    name = "delete_tab";
                    let t = rng.pick(&any_tabs).unwrap();
                    reported = Some(state.delete_tab(&t, now).removed);
                }
                16 => {
                    name = "clear_today";
                    let w = rng.pick(&windows).unwrap();
                    reported = Some(state.clear_today(&w, now));
                }
                17 => {
                    let t = rng.pick(&any_tabs).unwrap();
                    let which = if rng.below(2) == 0 {
                        CloseToday::Below
                    } else {
                        CloseToday::Others
                    };
                    name = if which == CloseToday::Below {
                        "close_below"
                    } else {
                        "close_others"
                    };
                    anchor_check = Some(t.clone());
                    reported = Some(state.close_today_tabs(&t, which, now));
                }
                18 => {
                    name = "archive_stale";
                    // Age some tabs first.
                    for id in tabs.iter() {
                        if rng.below(3) == 0
                            && let Some(tab) = state.tabs.get_mut(id)
                        {
                            tab.last_active = Some((now - 13 * 3_600_000) as f64);
                        }
                    }
                    reported = Some(state.archive_stale(now));
                }
                19 => {
                    name = "set_split";
                    let w = rng.pick(&windows).unwrap();
                    let a = rng.pick(&any_tabs).unwrap();
                    let b = rng.pick(&any_tabs).unwrap();
                    reported = Some(state.set_split(&w, [a, b]).removed);
                    selection_owner = Some(w);
                }
                20 | 21 => {
                    name = "open_responsive";
                    let w = rng.pick(&windows).unwrap();
                    let _ = state.open_responsive(&w, TabId::new(format!("t{next_tab}")), now);
                    next_tab += 1;
                }
                22 => {
                    name = "unsplit";
                    let w = rng.pick(&windows).unwrap();
                    reported = Some(match state.unsplit(&w) {
                        Some(EndedSplit::Responsive { mirror }) => vec![mirror],
                        _ => vec![],
                    });
                }
                23 => {
                    let s = rng.pick(&spaces).unwrap();
                    match rng.below(4) {
                        0 => {
                            name = "add_folder";
                            let parent = rng.pick(&folders);
                            let _ = state.add_folder(
                                FolderId::new(format!("f{next_folder}")),
                                "x",
                                &s,
                                parent.as_ref(),
                            );
                            next_folder += 1;
                        }
                        1 => {
                            name = "move_folder";
                            if let Some(f) = rng.pick(&folders) {
                                let parent = rng.pick(&folders);
                                reported =
                                    Some(state.move_folder(&f, &s, parent.as_ref(), None).removed);
                            } else {
                                reported = Some(vec![]);
                            }
                        }
                        _ => {
                            name = "delete_folder";
                            if let Some(f) = rng.pick(&folders) {
                                reported =
                                    Some(state.delete_folder(&f, rng.below(2) == 0, now).removed);
                            } else {
                                reported = Some(vec![]);
                            }
                        }
                    }
                }
                24 => {
                    name = "delete_space";
                    let s = rng.pick(&spaces).unwrap();
                    reported = Some(state.delete_space(&s, now));
                }
                _ => match rng.below(3) {
                    0 => {
                        name = "focus_window";
                        let w = rng.pick(&windows).unwrap();
                        let _ = state.focus_window(&w);
                    }
                    1 => {
                        name = "set_isolated";
                        let s = rng.pick(&spaces).unwrap();
                        let _ = state.set_isolated(&s, rng.below(2) == 0);
                    }
                    _ => {
                        name = "touch";
                        let t = rng.pick(&any_tabs).unwrap();
                        let _ = state.touch(&t, now);
                    }
                },
            }
            let after = keys(&state);
            let actual: BTreeSet<_> = before.difference(&after).cloned().collect();
            let reported = reported.unwrap_or_default();
            let report_set: BTreeSet<_> = reported.iter().cloned().collect();
            let context = format!("seed {seed}, step {step}, {name}");
            assert_eq!(
                reported.len(),
                report_set.len(),
                "duplicate removal: {context}"
            );
            assert_eq!(report_set, actual, "removed IDs: {context}");
            assert_eq!(after, referenced(&state), "tab references: {context}");
            if let Some(anchor) = anchor_check
                && before.contains(&anchor)
            {
                assert!(
                    after.contains(&anchor),
                    "anchor {anchor:?} removed: {context}"
                );
            }
            for win in &state.windows {
                assert!(state.space(&win.active_space_id).is_some(), "{context}");
                for (space, ws) in &win.spaces {
                    let members = state.tab_ids(&win.id, space).unwrap();
                    for id in ws.active_tab_id.iter().chain(ws.split.iter().flatten()) {
                        assert!(
                            members.contains(&id),
                            "selected/split tab {id:?} is outside window {:?}, Space {space:?}: {context}",
                            win.id,
                        );
                    }
                }
            }
            for win in &snapshot.windows {
                let Some(current) = state.window(&win.id) else {
                    continue;
                };
                for (space, ws) in &win.spaces {
                    let Some(active) = &ws.active_tab_id else {
                        continue;
                    };
                    let selected = current
                        .spaces
                        .get(space)
                        .and_then(|ws| ws.active_tab_id.as_ref());
                    if name == "archive_stale" {
                        assert_eq!(selected, Some(active), "selection in {space:?}: {context}");
                    }
                    if selection_owner.as_ref() != Some(&win.id)
                        && targeted_tab.as_ref() != Some(active)
                        && state
                            .tab_ids(&win.id, space)
                            .is_some_and(|ids| ids.contains(&active))
                    {
                        let claimed_elsewhere = state.windows.iter().any(|owner| {
                            owner.id != win.id
                                && owner.visible_tabs().contains(active)
                                && snapshot
                                    .window(&owner.id)
                                    .is_none_or(|old| !old.visible_tabs().contains(active))
                        });
                        if !claimed_elsewhere {
                            assert_eq!(selected, Some(active), "selection in {space:?}: {context}");
                        }
                    }
                }
            }
        }
    }
}
