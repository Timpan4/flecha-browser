//! Apply parsed Spaces and archive entries to a browser state.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use crate::state::{DEFAULT_SPACE_COLOR, walk_pins};
use crate::{
    ARCHIVE_LIMIT, ArchivedTab, BrowserState, Device, EpochMillis, FolderId, PinnedNode, SpaceId,
    Tab, TabId, WindowId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMode {
    Merge,
    Append,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportBatch {
    pub spaces: Vec<ImportSpace>,
    pub favorites: Vec<ImportTab>,
    pub archive: Vec<ArchivedTab>,
}

/// IDs for new entries are allocated by the caller, independently of the imported browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSpace {
    pub id: SpaceId,
    pub name: String,
    pub emoji: Option<String>,
    pub color: Option<String>,
    pub isolated: bool,
    pub pinned: Vec<ImportNode>,
    pub today: Vec<ImportTab>,
    /// Splits by tab URL in their saved order.
    pub splits: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportNode {
    Tab(ImportTab),
    Folder {
        id: FolderId,
        name: String,
        children: Vec<ImportNode>,
    },
}

impl Drop for ImportNode {
    fn drop(&mut self) {
        if let Self::Folder { children, .. } = self {
            let mut stack = std::mem::take(children);
            while let Some(mut node) = stack.pop() {
                if let Self::Folder { children, .. } = &mut node {
                    stack.append(children);
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportTab {
    pub id: TabId,
    pub url: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub folders: usize,
    pub spaces: usize,
    pub merged: usize,
    pub tabs: usize,
    pub splits: usize,
    pub pairs: usize,
    pub archived: usize,
    pub archive_dropped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    UnknownWindow,
    DuplicateSpace(SpaceId),
    DuplicateFolder(FolderId),
    DuplicateTab(TabId),
}

impl BrowserState {
    /// Merge or append a parsed import as one transaction.
    /// IDs are checked only for inserted entries. IDs on URL-filtered tabs and
    /// name-merged Spaces or folders are ignored.
    pub fn import_spaces(
        &mut self,
        batch: &ImportBatch,
        mode: ImportMode,
        window: &WindowId,
        now: EpochMillis,
    ) -> Result<ImportOutcome, ImportError> {
        if self.window(window).is_none() {
            return Err(ImportError::UnknownWindow);
        }
        let mut next = self.clone();
        let out = next.apply_import(batch, mode, window, now)?;
        *self = next;
        Ok(out)
    }

    fn apply_import(
        &mut self,
        batch: &ImportBatch,
        mode: ImportMode,
        window: &WindowId,
        now: EpochMillis,
    ) -> Result<ImportOutcome, ImportError> {
        let pristine = !batch.spaces.is_empty() && self.is_pristine();
        if pristine {
            self.spaces.clear();
            for win in &mut self.windows {
                win.spaces.clear();
            }
        }
        let mut outcome = ImportOutcome::default();
        let mut favorite_urls = BTreeSet::new();
        for id in &self.favorites {
            if let Some(tab) = self.tabs.get(id) {
                favorite_urls.insert(tab.url.clone());
                favorite_urls.extend(tab.home_url.iter().cloned());
            }
        }
        for tab in &batch.favorites {
            if favorite_urls.insert(tab.url.clone()) {
                insert_tab(&mut self.tabs, tab, true, now)?;
                self.favorites.push(tab.id.clone());
                outcome.tabs += 1;
            }
        }
        let mut existing = BTreeMap::new();
        if mode == ImportMode::Merge {
            for space in &self.spaces {
                existing.entry(name_key(&space.name)).or_insert_with(|| {
                    let ids = space.pinned_tabs().into_iter().chain(
                        self.windows
                            .iter()
                            .chain(self.closed_windows.iter().map(|closed| &closed.window))
                            .filter_map(|win| win.spaces.get(&space.id))
                            .flat_map(|ws| &ws.today),
                    );
                    let urls: BTreeSet<_> = ids
                        .filter_map(|id| self.tabs.get(id))
                        .filter(|tab| tab.device != Some(Device::Mobile))
                        .flat_map(|tab| std::iter::once(&tab.url).chain(&tab.home_url))
                        .cloned()
                        .collect();
                    (space.id.clone(), urls)
                });
            }
        }
        let mut folder_ids = BTreeSet::new();
        for space in &self.spaces {
            collect_folders(&space.pinned, &mut folder_ids);
        }
        let mut first = None;
        for incoming in &batch.spaces {
            let previous = existing.get(&name_key(&incoming.name));
            let space = if let Some((id, _)) = previous {
                outcome.merged += 1;
                id
            } else {
                if !self.add_space(incoming.id.clone()) {
                    return Err(ImportError::DuplicateSpace(incoming.id.clone()));
                }
                let space = self
                    .spaces
                    .last_mut()
                    .expect("add_space appended the imported Space");
                space.name.clone_from(&incoming.name);
                if let Some(emoji) = &incoming.emoji {
                    space.emoji = Some(emoji.clone());
                }
                if let Some(color) = &incoming.color {
                    space.color.clone_from(color);
                }
                space.isolated = incoming.isolated;
                outcome.spaces += 1;
                &incoming.id
            };
            first.get_or_insert_with(|| space.clone());
            let had: BTreeSet<_> = self
                .tab_ids(window, space)
                .expect("import destination exists")
                .into_iter()
                .cloned()
                .collect();
            let skip = |tab: &ImportTab| {
                mode == ImportMode::Merge
                    && (favorite_urls.contains(&tab.url)
                        || previous.is_some_and(|(_, urls)| urls.contains(&tab.url)))
            };
            let pinned = &mut self
                .spaces
                .iter_mut()
                .find(|candidate| candidate.id == *space)
                .expect("import destination Space exists")
                .pinned;
            let mut apply = PinnedImport {
                tabs: &mut self.tabs,
                folder_ids: &mut folder_ids,
                outcome: &mut outcome,
                now,
            };
            apply.add_nodes(pinned, &incoming.pinned, previous.is_some(), &skip)?;
            let win = self
                .windows
                .iter_mut()
                .find(|win| win.id == *window)
                .expect("import destination window exists");
            let ws = win.spaces.entry(space.clone()).or_default();
            for tab in incoming.today.iter().filter(|tab| !skip(tab)) {
                insert_tab(&mut self.tabs, tab, false, now)?;
                ws.today.push(tab.id.clone());
                outcome.tabs += 1;
            }
            self.import_splits(window, space, &incoming.splits, &had, &mut outcome);
        }
        if pristine && let Some(first) = first {
            for win in &mut self.windows {
                win.active_space_id = first.clone();
            }
        }
        if !batch.archive.is_empty() {
            merge_archive(&mut self.archive, &batch.archive, &mut outcome);
        }
        Ok(outcome)
    }

    fn is_pristine(&self) -> bool {
        let [space] = self.spaces.as_slice() else {
            return false;
        };
        space.name == "Personal"
            && space.emoji.as_deref() == Some("🏠")
            && space.color == DEFAULT_SPACE_COLOR
            && !space.isolated
            && !space.pinned_collapsed
            && space.pinned.is_empty()
            && self.favorites.is_empty()
            && self
                .windows
                .iter()
                .chain(self.closed_windows.iter().map(|closed| &closed.window))
                .all(|win| {
                    win.spaces
                        .get(&space.id)
                        .is_none_or(|ws| ws.today.is_empty())
                })
    }

    fn import_splits(
        &mut self,
        window: &WindowId,
        space: &SpaceId,
        pairs: &[Vec<String>],
        had: &BTreeSet<TabId>,
        outcome: &mut ImportOutcome,
    ) {
        if pairs.is_empty() {
            return;
        }
        let candidates: Vec<_> = self
            .tab_ids(window, space)
            .expect("import destination exists")
            .into_iter()
            .cloned()
            .collect();
        let unavailable: BTreeSet<_> = self
            .windows
            .iter()
            .filter(|win| win.id != *window)
            .flat_map(|win| win.spaces.values())
            .flat_map(|ws| ws.active_tab_id.iter().chain(ws.split.iter().flatten()))
            .cloned()
            .collect();
        let ws = self
            .windows
            .iter_mut()
            .find(|win| win.id == *window)
            .and_then(|win| win.spaces.get_mut(space))
            .expect("import prepared the window Space");
        let tab_by_url = |url: &str, exclude: Option<&TabId>| {
            candidates
                .iter()
                .filter(|id| {
                    Some(*id) != exclude
                        && !unavailable.contains(*id)
                        && self
                            .tabs
                            .get(*id)
                            .is_some_and(|tab| tab.url == url && tab.device != Some(Device::Mobile))
                })
                .min_by_key(|id| had.contains(*id))
                .cloned()
        };
        for pair in pairs {
            let [left, right] = pair.as_slice() else {
                outcome.pairs += 1;
                continue;
            };
            if let Some(split) = &ws.split {
                let equal = ws.split_mode.is_none()
                    && split
                        .iter()
                        .zip(pair)
                        .all(|(id, url)| self.tabs.get(id).is_some_and(|tab| tab.url == *url));
                if !equal {
                    outcome.pairs += 1;
                }
            } else if let Some(left) = tab_by_url(left, None)
                && let Some(right) = tab_by_url(right, Some(&left))
            {
                ws.split = Some([left, right]);
                outcome.splits += 1;
            } else {
                outcome.pairs += 1;
            }
        }
    }
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn insert_tab(
    tabs: &mut BTreeMap<TabId, Tab>,
    input: &ImportTab,
    pinned: bool,
    now: EpochMillis,
) -> Result<(), ImportError> {
    if tabs.contains_key(&input.id) {
        return Err(ImportError::DuplicateTab(input.id.clone()));
    }
    let mut tab = Tab::new(input.url.clone(), now);
    if !input.title.is_empty() {
        tab.title.clone_from(&input.title);
    }
    if pinned {
        tab.home_url = Some(input.url.clone());
    }
    tabs.insert(input.id.clone(), tab);
    Ok(())
}

fn collect_folders(nodes: &[PinnedNode], ids: &mut BTreeSet<FolderId>) {
    ids.extend(walk_pins(nodes).filter_map(|(node, _, _)| match node {
        PinnedNode::Folder { id, .. } => Some(id.clone()),
        PinnedNode::Tab { .. } => None,
    }));
}

struct PinnedImport<'a> {
    tabs: &'a mut BTreeMap<TabId, Tab>,
    folder_ids: &'a mut BTreeSet<FolderId>,
    outcome: &'a mut ImportOutcome,
    now: EpochMillis,
}

struct ImportFrame<'a> {
    incoming: std::slice::Iter<'a, ImportNode>,
    output: Vec<PinnedNode>,
    folder: Option<ImportFolder>,
}

struct ImportFolder {
    node: PinnedNode,
    previous_index: Option<usize>,
    keep_empty: bool,
}

impl PinnedImport<'_> {
    fn add_nodes(
        &mut self,
        dest: &mut Vec<PinnedNode>,
        incoming: &[ImportNode],
        merge: bool,
        skip: &impl Fn(&ImportTab) -> bool,
    ) -> Result<(), ImportError> {
        let mut frames = vec![ImportFrame {
            incoming: incoming.iter(),
            output: std::mem::take(dest),
            folder: None,
        }];

        while let Some(frame) = frames.last_mut() {
            match frame.incoming.next() {
                Some(ImportNode::Tab(tab)) if !skip(tab) => {
                    insert_tab(self.tabs, tab, true, self.now)?;
                    frame.output.push(PinnedNode::Tab {
                        tab_id: tab.id.clone(),
                    });
                    self.outcome.tabs += 1;
                }
                Some(ImportNode::Tab(_)) => {}
                Some(ImportNode::Folder { id, name, children }) => {
                    let previous_index = if merge {
                        let key = name_key(name);
                        frame.output.iter().position(|node| {
                            matches!(node, PinnedNode::Folder { name, .. } if name_key(name) == key)
                        })
                    } else {
                        None
                    };
                    let mut node = match previous_index {
                        Some(index) => frame.output.remove(index),
                        None => PinnedNode::Folder {
                            id: id.clone(),
                            name: name.clone(),
                            open: false,
                            children: Vec::new(),
                        },
                    };
                    let PinnedNode::Folder {
                        children: existing, ..
                    } = &mut node
                    else {
                        unreachable!("the name match only selects folders");
                    };
                    let output = std::mem::take(existing);
                    frames.push(ImportFrame {
                        incoming: children.iter(),
                        output,
                        folder: Some(ImportFolder {
                            node,
                            previous_index,
                            keep_empty: children.is_empty(),
                        }),
                    });
                }
                None => {
                    let done = frames.pop().expect("an active frame finished");
                    let Some(mut folder) = done.folder else {
                        *dest = done.output;
                        return Ok(());
                    };
                    let parent = frames.last_mut().expect("a folder has a parent list");
                    let PinnedNode::Folder { id, children, .. } = &mut folder.node else {
                        unreachable!("a folder frame contains a folder");
                    };
                    *children = done.output;
                    if let Some(index) = folder.previous_index {
                        parent.output.insert(index, folder.node);
                    } else if folder.keep_empty || !children.is_empty() {
                        if !self.folder_ids.insert(id.clone()) {
                            return Err(ImportError::DuplicateFolder(id.clone()));
                        }
                        parent.output.push(folder.node);
                        self.outcome.folders += 1;
                    }
                }
            }
        }
        Ok(())
    }
}

fn merge_archive(
    archive: &mut Vec<ArchivedTab>,
    incoming: &[ArchivedTab],
    outcome: &mut ImportOutcome,
) {
    let mut seen: BTreeSet<_> = archive
        .iter()
        .map(|tab| (tab.closed_at, tab.url.as_str()))
        .collect();
    let mut combined: Vec<_> = archive.iter().map(|tab| (tab, false)).collect();
    combined.extend(incoming.iter().filter_map(|tab| {
        seen.insert((tab.closed_at, tab.url.as_str()))
            .then_some((tab, true))
    }));
    combined.sort_by_key(|(tab, _)| Reverse(tab.closed_at));
    outcome.archive_dropped = combined.len().saturating_sub(ARCHIVE_LIMIT);
    let kept = combined.into_iter().take(ARCHIVE_LIMIT);
    let mut next = Vec::new();
    for (tab, imported) in kept {
        outcome.archived += usize::from(imported);
        next.push(tab.clone());
    }
    *archive = next;
}
