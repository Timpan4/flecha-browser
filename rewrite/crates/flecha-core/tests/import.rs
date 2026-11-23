use flecha_core::import::{
    ImportBatch, ImportError, ImportMode, ImportNode, ImportSpace, ImportTab,
};
use flecha_core::storage::{Slot, set_aside_before_import, state_before_import};
use flecha_core::{
    ARCHIVE_LIMIT, ArchivedTab, BrowserState, FolderId, PinnedNode, Placement, SpaceId, Tab, TabId,
    WindowId,
};

fn incoming_space(id: &str, name: &str) -> ImportSpace {
    ImportSpace {
        id: SpaceId::new(id),
        name: name.into(),
        emoji: None,
        color: None,
        isolated: false,
        pinned: Vec::new(),
        today: Vec::new(),
        splits: Vec::new(),
    }
}

fn incoming_tab(id: &str, url: &str) -> ImportTab {
    ImportTab {
        id: TabId::new(id),
        url: url.into(),
        title: id.into(),
    }
}

#[test]
fn import_rejects_non_web_urls_and_counts_dropped_entries() {
    let window = WindowId::new("w");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    let mut space = incoming_space("work", "Work");
    space.pinned = vec![ImportNode::Tab(incoming_tab(
        "script",
        "javascript:void(0)",
    ))];
    space.today = vec![
        incoming_tab("local", "file:///example.txt"),
        incoming_tab("web", "HTTPS://example.org"),
    ];
    let batch = ImportBatch {
        spaces: vec![space],
        favorites: vec![incoming_tab("empty", "https://")],
        archive: vec![ArchivedTab {
            url: "file:///archive.txt".into(),
            title: "Local".into(),
            closed_at: 1,
        }],
    };
    let result = state
        .import_spaces(&batch, ImportMode::Append, &window, 2)
        .unwrap();
    assert_eq!(result.tabs, 1);
    assert_eq!(result.tabs_dropped, 3);
    assert_eq!(result.archived, 0);
    assert_eq!(result.archive_dropped, 1);
    assert_eq!(
        state.open_tabs().keys().copied().collect::<Vec<_>>(),
        [&TabId::new("web")]
    );
    assert!(state.archive().is_empty());
}

#[test]
fn import_replaces_only_a_pristine_space_and_keeps_a_before_copy() {
    let home = SpaceId::new("home");
    let window = WindowId::new("w");
    let other = WindowId::new("other");
    let init = BrowserState::new(home.clone(), window.clone());
    let mut incoming = incoming_space("work", "Work");
    incoming.emoji = Some("🧪".into());
    incoming.color = Some("#aabbcc".into());
    incoming.isolated = true;
    let id = incoming.id.clone();
    let batch = ImportBatch {
        spaces: vec![incoming],
        ..ImportBatch::default()
    };

    let before = set_aside_before_import(&init).unwrap();
    assert_eq!(before.slot, Slot::BeforeImport);
    let mut state = init.clone();
    assert!(state.open_window(other.clone(), None));
    let result = state
        .import_spaces(&batch, ImportMode::Merge, &window, 0)
        .unwrap();
    assert_eq!(result.spaces, 1);
    let [space] = state.spaces() else {
        panic!("expected Personal to be replaced")
    };
    assert_eq!(space.id, id);
    assert!(space.isolated);
    assert_eq!(space.emoji.as_deref(), Some("🧪"));
    assert_eq!(space.color, "#aabbcc");
    assert_eq!(state.window(&other).unwrap().active_space_id, id);
    assert_eq!(
        state_before_import(Some(&before.value)).unwrap(),
        Some(init.clone())
    );
    assert!(state_before_import(None).unwrap().is_none());

    for kind in [
        "folded",
        "favorites",
        "closed window",
        "renamed",
        "recolored",
        "folder",
    ] {
        let mut state = init.clone();
        match kind {
            "folded" => {
                state.update_space(&home, None, None, None, Some(true));
            }
            "favorites" => {
                state.create_tab(
                    TabId::new("fav"),
                    Tab::new("https://example.org/", 0),
                    Placement::Favorite,
                    None,
                );
            }
            "closed window" => {
                state.open_window(other.clone(), None);
                state.open_today(
                    TabId::new("today"),
                    Tab::new("https://example.org/", 0),
                    &other,
                    None,
                    false,
                );
                assert!(state.close_window(&other, 1).result);
            }
            "renamed" => {
                state.update_space(&home, Some("Home"), None, None, None);
            }
            "recolored" => {
                state.update_space(&home, None, None, Some("#000000"), None);
            }
            "folder" => {
                state.add_folder(FolderId::new("folder"), "Folder", &home, None);
            }
            _ => unreachable!(),
        }
        state
            .import_spaces(&batch, ImportMode::Merge, &window, 2)
            .unwrap();
        assert_eq!(state.spaces().len(), 2, "{kind}");
        assert!(state.space(&home).is_some());
    }
}

#[test]
fn merge_deduplicates_existing_urls_and_append_keeps_the_import_intact() {
    let work = SpaceId::new("work");
    let window = WindowId::new("w");
    let folder = FolderId::new("folder");
    let favorite = TabId::new("favorite");
    let kept = TabId::new("kept");
    let mut state = BrowserState::new(work.clone(), window.clone());
    assert!(state.update_space(&work, Some("Work"), None, None, None));
    assert!(state.add_folder(folder.clone(), "Docs", &work, None));
    let home_url = "https://example.org/origin";
    assert!(state.create_tab(
        favorite.clone(),
        Tab::new(home_url, 0),
        Placement::Favorite,
        None
    ));
    state.tab_mut(&favorite).unwrap().url = "https://example.org/other".into();
    let existing = "https://example.org/kept";
    let shared = "https://example.org/shared";
    let repeat = "https://example.org/repeat";
    assert!(state.create_tab(
        kept.clone(),
        Tab::new(existing, 0),
        Placement::Pinned {
            space: &work,
            folder: Some(&folder)
        },
        None
    ));
    let mut incoming = incoming_space("work-copy", " work ");
    incoming.isolated = true;
    incoming.pinned = vec![
        ImportNode::Folder {
            id: FolderId::new("imported-folder"),
            name: " docs ".into(),
            children: vec![
                ImportNode::Tab(incoming_tab("duplicate-pin", existing)),
                ImportNode::Tab(incoming_tab("first-repeat", repeat)),
            ],
        },
        ImportNode::Folder {
            id: FolderId::new("all-duplicates"),
            name: "All duplicates".into(),
            children: vec![ImportNode::Tab(incoming_tab("duplicate-home", home_url))],
        },
    ];
    incoming.today = vec![
        incoming_tab("duplicate-favorite", shared),
        incoming_tab("second-repeat", repeat),
    ];
    incoming.splits = vec![
        vec![repeat.into(), repeat.into()],
        vec![existing.into(), repeat.into(), home_url.into()],
    ];
    let batch = ImportBatch {
        spaces: vec![incoming],
        favorites: vec![
            incoming_tab("new-favorite", shared),
            incoming_tab("old-favorite", home_url),
        ],
        ..ImportBatch::default()
    };
    let mut appended = state.clone();

    let out = state
        .import_spaces(&batch, ImportMode::Merge, &window, 3)
        .unwrap();
    assert_eq!(
        (
            out.spaces,
            out.merged,
            out.folders,
            out.tabs,
            out.splits,
            out.pairs
        ),
        (0, 1, 0, 3, 1, 1)
    );
    let space = state.space(&work).unwrap();
    assert!(!space.isolated);
    assert!(
        matches!(&space.pinned[0], PinnedNode::Folder { id, children, .. } if id == &folder && children.len() == 2)
    );
    assert_eq!(space.pinned.len(), 1);
    let ws = &state.focused_window().spaces[&work];
    assert_eq!(
        ws.split,
        Some([TabId::new("first-repeat"), TabId::new("second-repeat")])
    );
    assert_eq!(state.favorites(), [favorite, TabId::new("new-favorite")]);

    let out = appended
        .import_spaces(&batch, ImportMode::Append, &window, 3)
        .unwrap();
    assert_eq!(
        (out.spaces, out.merged, out.folders, out.tabs),
        (1, 0, 2, 6)
    );
    let new = appended.space(&SpaceId::new("work-copy")).unwrap();
    assert!(new.isolated);
    assert_eq!(new.pinned_tabs().len(), 3);
}

#[test]
fn merge_splits_use_existing_tabs_unless_another_window_has_claimed_them() {
    let work = SpaceId::new("work");
    let window = WindowId::new("w");
    let other = WindowId::new("other");
    let pin = TabId::new("pin");
    let pinned = "https://example.org/pinned";
    let today = "https://example.org/today";
    let mut base = BrowserState::new(work.clone(), window.clone());
    base.update_space(&work, Some("Work"), None, None, None);
    base.open_window(other.clone(), None);
    base.create_tab(
        pin.clone(),
        Tab::new(pinned, 0),
        Placement::Pinned {
            space: &work,
            folder: None,
        },
        None,
    );
    let mut incoming = incoming_space("copy", "Work");
    incoming.today = vec![incoming_tab("today", today)];
    incoming.splits = vec![vec![pinned.into(), today.into()]];
    let batch = ImportBatch {
        spaces: vec![incoming],
        ..ImportBatch::default()
    };
    let mut occupied = base.clone();
    assert_eq!(
        occupied.activate(&pin, &other, Some(1)).result,
        Some(other.clone())
    );
    let out = occupied
        .import_spaces(&batch, ImportMode::Merge, &window, 2)
        .unwrap();
    assert_eq!((out.splits, out.pairs), (0, 1));
    assert_eq!(
        occupied.window(&other).unwrap().visible_tabs(),
        std::slice::from_ref(&pin)
    );
    let out = base
        .import_spaces(&batch, ImportMode::Merge, &window, 2)
        .unwrap();
    assert_eq!((out.splits, out.pairs), (1, 0));
    assert_eq!(
        base.focused_window().spaces[&work].split,
        Some([pin, TabId::new("today")])
    );
}

#[test]
fn archive_import_is_ordered_deduplicated_and_bounded() {
    let window = WindowId::new("w");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    let archive: Vec<_> = (0..=ARCHIVE_LIMIT)
        .map(|time| ArchivedTab {
            url: "https://example.org/".into(),
            title: format!("Entry {time}"),
            closed_at: i64::try_from(time).unwrap(),
        })
        .collect();
    let mut batch = ImportBatch {
        archive,
        ..ImportBatch::default()
    };
    batch.archive.push(batch.archive[ARCHIVE_LIMIT].clone());
    let out = state
        .import_spaces(&batch, ImportMode::Merge, &window, 1)
        .unwrap();
    assert_eq!((out.archived, out.archive_dropped), (ARCHIVE_LIMIT, 1));
    assert_eq!(
        state.archive().first().unwrap().title,
        format!("Entry {ARCHIVE_LIMIT}")
    );
    assert_eq!(state.archive().last().unwrap().closed_at, 1);
    let out = state
        .import_spaces(&batch, ImportMode::Append, &window, 2)
        .unwrap();
    assert_eq!(out.archived, 0);
}

#[test]
fn a_duplicate_id_rolls_back_the_entire_import() {
    let window = WindowId::new("w");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    let before = state.clone();
    let mut incoming = incoming_space("copy", "Work");
    incoming.today = vec![
        incoming_tab("duplicate", "https://example.org/a"),
        incoming_tab("duplicate", "https://example.org/b"),
    ];
    let batch = ImportBatch {
        spaces: vec![incoming],
        ..ImportBatch::default()
    };
    assert_eq!(
        state.import_spaces(&batch, ImportMode::Append, &window, 1),
        Err(ImportError::DuplicateTab(TabId::new("duplicate")))
    );
    assert_eq!(state, before);
}

#[test]
fn deep_imports_keep_their_folders_across_merge_and_restore() {
    // Use the same nesting as the damaged-state fixture to exercise the complete tree lifecycle.
    let depth = 20_000;
    let window = WindowId::new("w");
    let space = SpaceId::new("copy");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    let leaf = incoming_tab("leaf", "https://example.org/leaf");
    let new_leaf = incoming_tab("new-leaf", "https://example.org/new-leaf");
    let deep_space = |tab, offset| {
        let mut incoming = incoming_space(space.as_str(), "Work");
        let mut nodes = vec![ImportNode::Tab(tab)];
        for level in 0..depth {
            nodes = vec![ImportNode::Folder {
                id: FolderId::new(format!("folder-{}", level + offset)),
                name: format!("Folder {level}"),
                children: nodes,
            }];
        }
        incoming.pinned = nodes;
        ImportBatch {
            spaces: vec![incoming],
            ..ImportBatch::default()
        }
    };

    let imported = state
        .import_spaces(&deep_space(leaf.clone(), 0), ImportMode::Append, &window, 0)
        .unwrap();
    assert_eq!(imported.folders, depth);
    let merged = state
        .import_spaces(
            &deep_space(new_leaf.clone(), depth),
            ImportMode::Merge,
            &window,
            1,
        )
        .unwrap();
    assert_eq!((merged.folders, merged.tabs), (0, 1));

    let saved = flecha_core::storage::encode(&state).unwrap();
    let restored = flecha_core::storage::decode(&saved).unwrap();
    assert!(state == restored);
    let mut state = restored;
    assert_eq!(
        state.space(&space).unwrap().pinned_tabs(),
        [&leaf.id, &new_leaf.id]
    );
    let mut nodes = state.space(&space).unwrap().pinned.as_slice();
    for level in (0..depth).rev() {
        let [PinnedNode::Folder { name, children, .. }] = nodes else {
            panic!("expected the imported folder")
        };
        assert_eq!(name, &format!("Folder {level}"));
        nodes = children;
    }
    assert_eq!(nodes.len(), 2);
    assert!(
        state
            .delete_folder(&FolderId::new("folder-0"), true, 2)
            .result
    );
    assert!(state.delete_tab(&new_leaf.id, 3).result);
    assert_eq!(state.space(&space).unwrap().pinned_tabs(), [&leaf.id]);
}

#[test]
fn empty_recovered_windows_leave_no_stale_space_after_import() {
    let saved = r##"{
        "version":2,
        "tabs":{},
        "spaces":[{"id":"personal","name":"Personal","emoji":"🏠","color":"#cdd9f7"}],
        "windows":[{"id":"open","activeSpaceId":"personal"}],
        "closedWindows":[{"id":"closed","activeSpaceId":"personal","closedAt":1,"spaces":{"personal":{"today":[]}}}]
    }"##;
    let mut state = flecha_core::storage::decode(saved).unwrap();
    assert!(state.closed_windows().is_empty());
    let batch = ImportBatch {
        spaces: vec![incoming_space("copy", "Work")],
        ..ImportBatch::default()
    };
    state
        .import_spaces(&batch, ImportMode::Append, &WindowId::new("open"), 2)
        .unwrap();
    assert!(state.space(&SpaceId::new("personal")).is_none());
    assert!(state.reopen_window().result.is_none());
}

#[test]
fn merge_ignores_ids_for_matched_spaces_folders_and_tabs() {
    let existing = SpaceId::new("work");
    let duplicate = SpaceId::new("another");
    let window = WindowId::new("w");
    let docs = FolderId::new("work-docs");
    let duplicate_folder = FolderId::new("another-docs");
    let known = TabId::new("known");
    let url = "https://example.org/existing";
    let mut state = BrowserState::new(existing.clone(), window.clone());
    state.update_space(&existing, Some("Work"), None, None, None);
    state.add_space(duplicate.clone());
    state.add_folder(docs.clone(), "Docs", &existing, None);
    state.add_folder(duplicate_folder.clone(), "Docs", &duplicate, None);
    state.create_tab(
        known.clone(),
        Tab::new(url, 0),
        Placement::Pinned {
            space: &existing,
            folder: Some(&docs),
        },
        None,
    );
    let new = incoming_tab("new", "https://example.org/new");
    let mut draft = incoming_space(duplicate.as_str(), "Work");
    draft.pinned = vec![ImportNode::Folder {
        id: duplicate_folder,
        name: "Docs".into(),
        children: vec![
            ImportNode::Tab(incoming_tab(known.as_str(), url)),
            ImportNode::Tab(new.clone()),
        ],
    }];
    let batch = ImportBatch {
        spaces: vec![draft],
        ..ImportBatch::default()
    };
    let out = state
        .import_spaces(&batch, ImportMode::Merge, &window, 1)
        .unwrap();
    assert_eq!(
        (out.merged, out.spaces, out.folders, out.tabs),
        (1, 0, 0, 1)
    );
    assert_eq!(
        state.space(&existing).unwrap().pinned_tabs(),
        [&known, &new.id]
    );
}

#[test]
fn imported_split_prefers_new_unseen_tabs_over_an_existing_favorite() {
    let window = WindowId::new("w");
    let used = TabId::new("used");
    let url = "https://example.org/shared";
    let mut state = BrowserState::new(SpaceId::new("home"), window.clone());
    state.create_tab(used.clone(), Tab::new(url, 0), Placement::Favorite, None);
    assert_eq!(
        state.activate(&used, &window, Some(1)).result,
        Some(window.clone())
    );
    let left = incoming_tab("left", url);
    let right = incoming_tab("right", "https://example.org/right");
    let fav = incoming_tab("favorite", "https://example.org/favorite");
    let mut incoming = incoming_space("copy", "Work");
    incoming.today = vec![left.clone()];
    incoming.pinned = vec![ImportNode::Tab(right.clone())];
    incoming.splits = vec![vec![left.url.clone(), right.url.clone()]];
    let space = incoming.id.clone();
    let batch = ImportBatch {
        spaces: vec![incoming],
        favorites: vec![fav.clone()],
        ..ImportBatch::default()
    };
    assert_eq!(
        state
            .import_spaces(&batch, ImportMode::Append, &window, 2)
            .unwrap()
            .splits,
        1
    );
    let ws = &state.focused_window().spaces[&space];
    assert_eq!(ws.split, Some([left.id.clone(), right.id.clone()]));
    assert_eq!(ws.active_tab_id, None);
    for id in [&left.id, &right.id, &fav.id] {
        assert!(state.tab(id).unwrap().unseen);
    }
    assert_eq!(
        state.by_recency([&left.id, &right.id, &fav.id, &used]),
        [&used, &left.id, &right.id, &fav.id]
    );
}

fn deep_trait_batch() -> ImportBatch {
    let mut space = incoming_space("copy", "Work");
    let mut nodes = vec![ImportNode::Tab(incoming_tab(
        "leaf",
        "https://example.org/",
    ))];
    // Match the depth from the verifier's stack-overflow reproducer.
    for level in 0..60_000 {
        nodes = vec![ImportNode::Folder {
            id: FolderId::new(format!("folder-{level}")),
            name: format!("Folder {level}"),
            children: nodes,
        }];
    }
    space.pinned = nodes;
    ImportBatch {
        spaces: vec![space],
        ..ImportBatch::default()
    }
}

#[test]
fn import_batch_traits_support_the_verifiers_deep_tree() {
    let batch = deep_trait_batch();
    let mut copy = batch.clone();
    assert!(copy == batch);
    assert!(format!("{copy:?}").contains("\"Folder 0\""));
    let mut node = &mut copy.spaces[0].pinned[0];
    loop {
        match node {
            ImportNode::Folder { children, .. } => node = &mut children[0],
            ImportNode::Tab(leaf) => {
                leaf.title.push('!');
                break;
            }
        }
    }
    assert!(copy != batch);
}

#[test]
fn pinned_tree_traits_support_the_verifiers_deep_tree() {
    let window = WindowId::new("w");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    state
        .import_spaces(&deep_trait_batch(), ImportMode::Append, &window, 0)
        .unwrap();
    let copy = state.clone();
    assert!(copy == state);
    assert!(format!("{copy:?}").contains("\"Folder 0\""));
    let pin = &copy.spaces()[0].pinned[0];
    let value = serde_json::to_value(pin).unwrap();
    let records = value
        .as_array()
        .expect("the standalone pin uses flat records");
    assert_eq!(records.first().unwrap()["name"], "Folder 59999");
    assert_eq!(records.last().unwrap()["tabId"], "leaf");
}

#[test]
fn merged_folder_keeps_its_slot_id_and_open_state() {
    let space = SpaceId::new("s");
    let window = WindowId::new("w");
    let a = TabId::new("a");
    let b = TabId::new("b");
    let f = FolderId::new("f");
    let g = FolderId::new("g");
    let mut state = BrowserState::new(space.clone(), window.clone());
    let pinned = Placement::Pinned {
        space: &space,
        folder: None,
    };
    assert!(state.create_tab(
        a.clone(),
        Tab::new("https://example.org/a", 0),
        pinned,
        None
    ));
    assert!(state.add_folder(f.clone(), "F", &space, None));
    assert!(state.edit_folder(&f, None, Some(false)));
    assert!(state.create_tab(
        b.clone(),
        Tab::new("https://example.org/b", 0),
        pinned,
        None
    ));
    assert!(state.add_folder(g.clone(), "G", &space, None));
    let mut incoming = incoming_space("import", "Personal");
    let leaf = incoming_tab("new-child", "https://example.org/new");
    incoming.pinned.push(ImportNode::Folder {
        id: FolderId::new("import-f"),
        name: "F".into(),
        children: vec![ImportNode::Tab(leaf.clone())],
    });
    let batch = ImportBatch {
        spaces: vec![incoming],
        ..ImportBatch::default()
    };
    let out = state
        .import_spaces(&batch, ImportMode::Merge, &window, 0)
        .unwrap();
    assert_eq!((out.merged, out.folders, out.tabs), (1, 0, 1));
    let [first, second, third, fourth] = state.space(&space).unwrap().pinned.as_slice() else {
        panic!("expected two tabs and two folders")
    };
    assert!(matches!(first, PinnedNode::Tab { tab_id } if tab_id == &a));
    assert!(
        matches!(second, PinnedNode::Folder { id, open: false, children, .. }
        if id == &f && matches!(children.as_slice(), [PinnedNode::Tab { tab_id }] if tab_id == &leaf.id))
    );
    assert!(matches!(third, PinnedNode::Tab { tab_id } if tab_id == &b));
    assert!(matches!(fourth, PinnedNode::Folder { id, .. } if id == &g));
}
