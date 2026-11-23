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
                state.close_window(&other, 1);
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
    occupied.activate(&pin, &other, Some(1));
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
fn import_folder_depth_is_bounded_by_the_chromium_bookmark_import_limit() {
    let window = WindowId::new("w");
    let mut state = BrowserState::new(SpaceId::new("s"), window.clone());
    let before = state.clone();
    let mut incoming = incoming_space("copy", "Work");
    let mut nodes = vec![ImportNode::Tab(incoming_tab("tab", "https://example.org/"))];
    // browserImport.ts admits eight nested folders.
    for level in 0..8 {
        nodes = vec![ImportNode::Folder {
            id: FolderId::new(format!("folder-{level}")),
            name: "Folder".into(),
            children: nodes,
        }];
    }
    incoming.pinned = nodes;
    let mut batch = ImportBatch {
        spaces: vec![incoming],
        ..ImportBatch::default()
    };
    assert!(
        state
            .clone()
            .import_spaces(&batch, ImportMode::Append, &window, 0)
            .is_ok()
    );
    let children = std::mem::take(&mut batch.spaces[0].pinned);
    batch.spaces[0].pinned.push(ImportNode::Folder {
        id: FolderId::new("extra"),
        name: "Extra".into(),
        children,
    });
    assert_eq!(
        state.import_spaces(&batch, ImportMode::Append, &window, 0),
        Err(ImportError::FolderDepth)
    );
    assert_eq!(state, before);
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
    let batch = ImportBatch {
        spaces: vec![incoming_space("copy", "Work")],
        ..ImportBatch::default()
    };
    state
        .import_spaces(&batch, ImportMode::Append, &WindowId::new("open"), 2)
        .unwrap();
    assert!(state.space(&SpaceId::new("personal")).is_none());
    assert!(state.reopen_window().is_none());
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
    state.activate(&used, &window, Some(1));
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
