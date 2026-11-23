use std::collections::HashSet;

use flecha_core::import::{
    ImportBatch, ImportError, ImportMode, ImportNode, ImportOutcome, ImportSpace, ImportTab,
};
use flecha_core::input_text::trim_input;
use flecha_core::{ARCHIVE_LIMIT, BrowserState, EpochMillis, FolderId, SpaceId, TabId, WindowId};
use serde::Serialize;

use crate::ArcImport;

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Bookmark {
    Tab(SavedTab),
    Folder {
        name: String,
        children: Vec<Bookmark>,
    },
}

impl Drop for Bookmark {
    fn drop(&mut self) {
        if let Self::Folder { children, .. } = self {
            let mut nodes = std::mem::take(children);
            while let Some(mut node) = nodes.pop() {
                if let Self::Folder { children, .. } = &mut node {
                    nodes.append(children);
                }
            }
        }
    }
}

#[derive(PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryVisit {
    pub url: String,
    pub title: String,
    pub visits: f64,
    pub last_visit: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arc: Option<ArcImport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarks: Option<Vec<Bookmark>>,
    pub bookmark_count: usize,
    pub history: Vec<HistoryVisit>,
    pub warnings: Vec<String>,
}

pub struct ImportOptions<'a> {
    pub mode: ImportMode,
    pub isolate: &'a [usize],
    pub spaces_key: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub name: String,
    pub spaces: Vec<PreviewSpace>,
    pub favorites: usize,
    pub archived: usize,
    pub history: usize,
    pub skipped: Vec<String>,
    pub can_isolate: bool,
    pub warnings: Vec<String>,
    pub spaces_key: String,
}

#[derive(Serialize)]
pub struct PreviewSpace {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub pinned: usize,
    pub today: usize,
    pub splits: usize,
    pub matches: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Serialize)]
pub struct AppliedImport {
    pub changed: bool,
    pub spaces: usize,
    pub merged: usize,
    pub bookmarks: usize,
    pub folders: usize,
    pub splits: usize,
    pub history: usize,
    pub archived: usize,
    pub warnings: Vec<String>,
    pub details: Vec<String>,
}

struct Space<'a> {
    id: &'a str,
    name: &'a str,
    emoji: Option<&'a str>,
    color: Option<&'a str>,
    profile: Option<&'a crate::ArcProfile>,
    pinned: &'a [Bookmark],
    today: &'a [SavedTab],
    splits: &'a [Vec<String>],
}

impl ImportPlan {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            arc: None,
            bookmarks: None,
            bookmark_count: 0,
            history: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn spaces(&self) -> Vec<Space<'_>> {
        if let Some(arc) = &self.arc {
            arc.spaces
                .iter()
                .map(|space| Space {
                    id: space.id.as_deref().unwrap_or_default(),
                    name: &space.name,
                    emoji: space.emoji.as_deref(),
                    color: space.color.as_deref(),
                    profile: space.profile.as_ref(),
                    pinned: &space.pinned,
                    today: &space.today,
                    splits: &space.splits,
                })
                .collect()
        } else if let Some(bookmarks) = &self.bookmarks
            && !bookmarks.is_empty()
        {
            vec![Space {
                id: "",
                name: &self.name,
                emoji: None,
                color: None,
                profile: None,
                pinned: bookmarks,
                today: &[],
                splits: &[],
            }]
        } else {
            Vec::new()
        }
    }

    pub fn preview(&self, state: &BrowserState, can_isolate: bool) -> ImportPreview {
        let spaces = self.spaces();
        let pristine = !spaces.is_empty() && state.has_pristine_import_space();
        let names: HashSet<_> = state
            .spaces()
            .iter()
            .filter(|_| !pristine)
            .map(|space| trim_input(&space.name).to_lowercase())
            .collect();
        let archive = self
            .arc
            .as_ref()
            .map_or(&[][..], |arc| arc.archive.as_slice());
        let pairs = spaces
            .iter()
            .map(|space| {
                space.splits.len() - usize::from(space.splits.iter().any(|pair| pair.len() == 2))
            })
            .sum();
        ImportPreview {
            name: self.name.clone(),
            spaces: spaces
                .iter()
                .map(|space| PreviewSpace {
                    name: space.name.into(),
                    emoji: space.emoji.map(str::to_owned),
                    color: space.color.map(str::to_owned),
                    pinned: tabs_in(space.pinned).count(),
                    today: space.today.len(),
                    splits: space.splits.len(),
                    matches: names.contains(&trim_input(space.name).to_lowercase()),
                    profile: space.profile.map(|profile| profile.name.clone()),
                })
                .collect(),
            favorites: self.arc.as_ref().map_or(0, |arc| arc.favorites.len()),
            archived: archive.len(),
            history: self.history.len(),
            skipped: self.skipped_lines(
                pairs,
                if archive.is_empty() {
                    0
                } else {
                    state.preview_archive_drops(archive)
                },
            ),
            can_isolate,
            warnings: self.warnings.clone(),
            spaces_key: spaces_key(&spaces),
        }
    }

    /// Apply to the given window. The history owner receives visits independently of Spaces,
    /// so its failure can be reported without undoing a successful Space import.
    pub fn apply<E>(
        &self,
        state: &mut BrowserState,
        window: &WindowId,
        now: EpochMillis,
        options: ImportOptions<'_>,
        mut new_id: impl FnMut() -> String,
        merge_history: impl FnOnce(&[HistoryVisit]) -> Result<(usize, usize), E>,
    ) -> Result<AppliedImport, ImportError> {
        let spaces = self.spaces();
        let mut warnings = self.warnings.clone();
        let mut outcome = ImportOutcome::default();
        let mut history = 0;
        let mut details = Vec::new();
        if options
            .spaces_key
            .is_some_and(|key| key != spaces_key(&spaces))
        {
            warnings.push(format!("{}’s Spaces changed since the preview, so nothing was imported. Look at the preview again.", self.name));
        } else {
            if !spaces.is_empty() || self.arc.is_some() {
                let batch = ImportBatch {
                    spaces: spaces
                        .into_iter()
                        .enumerate()
                        .map(|(index, space)| ImportSpace {
                            id: SpaceId::new(new_id()),
                            name: space.name.into(),
                            emoji: space.emoji.map(str::to_owned),
                            color: space.color.map(str::to_owned),
                            isolated: options.isolate.contains(&index),
                            pinned: allocate_pins(space.pinned, &mut new_id),
                            today: space
                                .today
                                .iter()
                                .map(|tab| allocate_tab(tab, &mut new_id))
                                .collect(),
                            splits: space.splits.to_vec(),
                        })
                        .collect(),
                    favorites: self
                        .arc
                        .iter()
                        .flat_map(|arc| &arc.favorites)
                        .map(|tab| allocate_tab(tab, &mut new_id))
                        .collect(),
                    archive: self
                        .arc
                        .as_ref()
                        .map_or_else(Vec::new, |arc| arc.archive.clone()),
                };
                outcome = state.import_spaces(&batch, options.mode, window, now)?;
                details = self.skipped_lines(outcome.pairs, outcome.archive_dropped);
            }
            if !self.history.is_empty() {
                match merge_history(&self.history) {
                    Ok((added, failed)) => {
                        history = added;
                        if failed > 0 {
                            warnings.push(format!(
                                "Couldn’t import {} from history.",
                                plural(failed, "page")
                            ));
                        }
                    }
                    Err(_) => warnings.push("Couldn’t finish importing browser history.".into()),
                }
            }
        }
        Ok(AppliedImport {
            changed: outcome.spaces
                + outcome.tabs
                + outcome.folders
                + outcome.splits
                + outcome.archived
                + history
                > 0,
            spaces: outcome.spaces,
            merged: outcome.merged,
            bookmarks: outcome.tabs,
            folders: outcome.folders,
            splits: outcome.splits,
            archived: outcome.archived,
            history,
            warnings,
            details,
        })
    }

    fn skipped_lines(&self, pairs: usize, archive_dropped: usize) -> Vec<String> {
        let Some(arc) = &self.arc else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        let count = |key| arc.skipped.get(key).copied().unwrap_or(0);
        let non_web = count("non-web tab");
        let no_address = count("tab without address");
        let partial = count("partial split");
        let overlapping = count("overlapping split");
        if non_web > 0 {
            lines.push(format!(
                "{} with an address other than a web page (such as arc:)",
                plural(non_web, "tab")
            ));
        }
        if no_address > 0 {
            lines.push(format!(
                "{} with no saved address",
                plural(no_address, "tab")
            ));
        }
        for (kind, count) in &arc.skipped {
            if !matches!(
                kind.as_str(),
                "non-web tab" | "tab without address" | "partial split" | "overlapping split"
            ) {
                lines.push(format!(
                    "{} of a kind Flecha doesn’t have (Arc calls it “{kind}”)",
                    plural(*count, "sidebar item")
                ));
            }
        }
        if pairs > 0 {
            lines.push(format!(
                "{} kept as tabs side by side: Flecha shows one split of two tabs per Space",
                plural(pairs, "split view")
            ));
        }
        if partial > 0 {
            lines.push(format!(
                "{} that lost a tab, kept as the tabs that came over",
                plural(partial, "split view")
            ));
        }
        if overlapping > 0 {
            lines.push(format!(
                "{} kept as tabs because they overlap another split",
                plural(overlapping, "split view")
            ));
        }
        if archive_dropped > 0 {
            lines.push(format!("{}, Arc’s or Flecha’s own, past the {ARCHIVE_LIMIT} most recent, which is all Flecha’s archive keeps", plural(archive_dropped, "archived tab")));
        }
        let mut seen = HashSet::new();
        let profiles: Vec<_> = arc
            .spaces
            .iter()
            .filter_map(|space| space.profile.as_ref())
            .filter(|profile| seen.insert(profile.name.as_str()))
            .map(|profile| format!("“{}”", profile.name))
            .collect();
        if !profiles.is_empty() {
            lines.push(format!("Arc Profiles ({}): Flecha has no Profiles yet. Their Spaces can have Separate Cookies and Logins instead", profiles.join(", ")));
        }
        lines.push(
            "Little Arc windows don’t come over: no Arc importer has found where Arc keeps them."
                .into(),
        );
        lines.push("Arc Notes aren’t in any file Flecha can read, so they don’t come over. If your Arc still has Help › Export Arc Notes, export them there before Arc stops working on 22 December 2026.".into());
        lines
    }
}

pub(crate) fn tabs_in(nodes: &[Bookmark]) -> impl Iterator<Item = &SavedTab> {
    let mut frames = vec![nodes.iter()];
    std::iter::from_fn(move || {
        while let Some(frame) = frames.last_mut() {
            match frame.next() {
                Some(Bookmark::Folder { children, .. }) => frames.push(children.iter()),
                Some(Bookmark::Tab(tab)) => return Some(tab),
                None => {
                    frames.pop();
                }
            }
        }
        None
    })
}

fn allocate_pins(nodes: &[Bookmark], new_id: &mut impl FnMut() -> String) -> Vec<ImportNode> {
    let mut frames = vec![(nodes.iter(), Vec::new(), None)];
    while let Some((nodes, output, _)) = frames.last_mut() {
        match nodes.next() {
            Some(Bookmark::Folder { name, children }) => {
                let info = (FolderId::new(new_id()), name.clone());
                frames.push((children.iter(), Vec::new(), Some(info)));
            }
            Some(Bookmark::Tab(tab)) => output.push(ImportNode::Tab(allocate_tab(tab, new_id))),
            None => {
                let (_, children, folder) = frames.pop().expect("an active pin list");
                let Some((id, name)) = folder else {
                    return children;
                };
                let (_, parent, _) = frames.last_mut().expect("a folder has a parent list");
                parent.push(ImportNode::Folder { id, name, children });
            }
        }
    }
    unreachable!("the root pin list returns its children")
}

fn allocate_tab(tab: &SavedTab, new_id: &mut impl FnMut() -> String) -> ImportTab {
    ImportTab {
        id: TabId::new(new_id()),
        url: tab.url.clone(),
        title: tab.title.clone(),
    }
}

fn spaces_key(spaces: &[Space<'_>]) -> String {
    let key: Vec<_> = spaces
        .iter()
        .map(|space| {
            [
                space.id,
                space.name,
                space.profile.map_or("", |profile| &profile.dir),
            ]
        })
        .collect();
    serde_json::to_string(&key).expect("Space identity contains only strings")
}

fn plural(count: usize, noun: &str) -> String {
    let mut grouped = String::new();
    let digits = count.to_string();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} {noun}{}", if count == 1 { "" } else { "s" })
}
