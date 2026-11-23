use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::Settings;
use crate::suggestions::{display_url, match_quality};

macro_rules! durable_id {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    )+};
}

durable_id!(TabId, SpaceId, WindowId, FolderId);

pub type EpochMillis = i64;

pub const ARCHIVE_LIMIT: usize = 500;
/// Chromium's TabRestoreServiceHelper::kMaxEntries, as used in store.ts.
pub const CLOSED_WINDOW_LIMIT: usize = 25;
pub(crate) const DEFAULT_SPACE_COLOR: &str = "#cdd9f7";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub url: String,
    pub title: String,
    pub last_active: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<TabHistory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<Device>,
    pub unseen: bool,
    pub agent: bool,
}

impl Tab {
    pub fn new(url: impl Into<String>, now: EpochMillis) -> Self {
        let url = url.into();
        Self {
            title: url.clone(),
            url,
            last_active: now as f64,
            favicon: None,
            custom_title: None,
            muted: None,
            home_url: None,
            history: None,
            device: None,
            unseen: true,
            agent: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Mobile,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabHistory {
    pub entries: Vec<HistoryEntry>,
    pub index: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PinnedNode {
    Tab {
        #[serde(rename = "tabId")]
        tab_id: TabId,
    },
    Folder {
        id: FolderId,
        name: String,
        open: bool,
        children: Vec<PinnedNode>,
    },
}

impl PinnedNode {
    fn key(&self) -> PinnedKey<'_> {
        match self {
            Self::Tab { tab_id } => PinnedKey::Tab(tab_id),
            Self::Folder { id, .. } => PinnedKey::Folder(id),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinnedKey<'a> {
    Tab(&'a TabId),
    Folder(&'a FolderId),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    pub color: String,
    pub isolated: bool,
    pub pinned_collapsed: bool,
    pub pinned: Vec<PinnedNode>,
}

impl Space {
    pub fn pinned_tabs(&self) -> Vec<&TabId> {
        let mut tabs = Vec::new();
        collect_pins(&self.pinned, &mut tabs);
        tabs
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSpace {
    pub today: Vec<TabId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tab_id: Option<TabId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<[TabId; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split_mode: Option<SplitMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split_ratio: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitMode {
    Responsive,
}

impl WindowSpace {
    fn visible_tabs(&self) -> &[TabId] {
        let Some(active) = &self.active_tab_id else {
            return &[];
        };
        self.split
            .as_ref()
            .filter(|pair| pair.contains(active))
            .map_or_else(|| std::slice::from_ref(active), |pair| pair.as_slice())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub id: WindowId,
    pub active_space_id: SpaceId,
    pub spaces: BTreeMap<SpaceId, WindowSpace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_visible: Option<bool>,
}

impl Window {
    pub fn visible_tabs(&self) -> &[TabId] {
        self.spaces
            .get(&self.active_space_id)
            .map_or(&[], WindowSpace::visible_tabs)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedWindow {
    #[serde(flatten)]
    pub window: Window,
    pub closed_at: EpochMillis,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedTab {
    pub url: String,
    pub title: String,
    pub closed_at: EpochMillis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement<'a> {
    Favorite,
    Pinned {
        space: &'a SpaceId,
        folder: Option<&'a FolderId>,
    },
    Today {
        space: &'a SpaceId,
        window: &'a WindowId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTab {
    Missing,
    Unloaded,
    Archived,
    Removed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseToday {
    Below,
    Others,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndedSplit {
    Standard,
    Responsive { mirror: TabId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reopened {
    Window(WindowId),
    Tab(TabId),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub(crate) spaces: Vec<Space>,
    pub(crate) windows: Vec<Window>,
    pub(crate) closed_windows: Vec<ClosedWindow>,
    pub(crate) favorites: Vec<TabId>,
    pub(crate) tabs: BTreeMap<TabId, Tab>,
    pub(crate) archive: Vec<ArchivedTab>,
    pub(crate) sidebar_visible: bool,
    #[serde(flatten)]
    pub settings: Settings,
}

impl BrowserState {
    /// IDs are supplied by the application and persist across native browser recreation.
    pub fn new(space: SpaceId, window: WindowId) -> Self {
        Self {
            spaces: vec![Space {
                id: space.clone(),
                name: "Personal".into(),
                emoji: Some("🏠".into()),
                color: DEFAULT_SPACE_COLOR.into(),
                isolated: false,
                pinned_collapsed: false,
                pinned: Vec::new(),
            }],
            windows: vec![Window {
                id: window,
                active_space_id: space,
                spaces: BTreeMap::new(),
                sidebar_visible: None,
            }],
            closed_windows: Vec::new(),
            favorites: Vec::new(),
            tabs: BTreeMap::new(),
            archive: Vec::new(),
            sidebar_visible: true,
            settings: Settings::default(),
        }
    }

    pub fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    pub fn space(&self, id: &SpaceId) -> Option<&Space> {
        self.spaces.iter().find(|space| space.id == *id)
    }

    fn space_mut(&mut self, id: &SpaceId) -> Option<&mut Space> {
        self.spaces.iter_mut().find(|space| space.id == *id)
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn window(&self, id: &WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == *id)
    }

    pub fn focused_window(&self) -> &Window {
        &self.windows[0]
    }

    pub fn closed_windows(&self) -> &[ClosedWindow] {
        &self.closed_windows
    }

    pub fn favorites(&self) -> &[TabId] {
        &self.favorites
    }

    pub fn tab(&self, id: &TabId) -> Option<&Tab> {
        self.tabs.get(id)
    }

    pub fn tab_mut(&mut self, id: &TabId) -> Option<&mut Tab> {
        self.tabs.get_mut(id)
    }

    pub fn archive(&self) -> &[ArchivedTab] {
        &self.archive
    }

    pub fn search_archive(&self, query: &str) -> Vec<&ArchivedTab> {
        if query.trim().is_empty() {
            return self.archive.iter().collect();
        }
        let mut matches: Vec<_> = self
            .archive
            .iter()
            .filter_map(|tab| {
                Some((
                    match_quality(query, &[&tab.title, &display_url(&tab.url)])?,
                    tab,
                ))
            })
            .collect();
        matches.sort_by_key(|(quality, tab)| (Reverse(*quality), Reverse(tab.closed_at)));
        matches.into_iter().map(|(_, tab)| tab).collect()
    }

    pub fn add_space(&mut self, id: SpaceId) -> bool {
        if self.space(&id).is_some() {
            return false;
        }
        const COLORS: [&str; 6] = [
            DEFAULT_SPACE_COLOR,
            "#f6d6cc",
            "#d3ecd5",
            "#e8d6f3",
            "#f5e7c4",
            "#cfe8ec",
        ];
        const EMOJI: [&str; 6] = ["🏠", "💼", "🎨", "📚", "🧪", "🎮"];
        let index = self.spaces.len();
        self.spaces.push(Space {
            id,
            name: format!("Space {}", index + 1),
            emoji: Some(EMOJI[index % EMOJI.len()].into()),
            color: COLORS[index % COLORS.len()].into(),
            isolated: false,
            pinned_collapsed: false,
            pinned: Vec::new(),
        });
        true
    }

    /// Moves the Space before the given Space, or to the end.
    pub fn move_space(&mut self, id: &SpaceId, before: Option<&SpaceId>) -> bool {
        if before == Some(id) {
            return false;
        }
        let Some(index) = self.spaces.iter().position(|space| space.id == *id) else {
            return false;
        };
        let space = self.spaces.remove(index);
        let index = insertion_index(&self.spaces, |space| Some(&space.id) == before);
        self.spaces.insert(index, space);
        true
    }

    pub fn update_space(
        &mut self,
        id: &SpaceId,
        name: Option<&str>,
        emoji: Option<&str>,
        color: Option<&str>,
        pinned_collapsed: Option<bool>,
    ) -> bool {
        let Some(space) = self.space_mut(id) else {
            return false;
        };
        if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
            // store.ts limits the name to 40 UTF-16 units.
            space.name = truncate_utf16(name, 40).into();
        }
        if let Some(emoji) = emoji {
            let emoji: String = emoji.trim().chars().take(2).collect();
            space.emoji = (!emoji.is_empty()).then_some(emoji);
        }
        if let Some(color) = color
            && color.len() == 7
            && color.starts_with('#')
            && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        {
            space.color = color.to_ascii_lowercase();
        }
        if let Some(collapsed) = pinned_collapsed {
            space.pinned_collapsed = collapsed;
        }
        true
    }

    /// Returns open tabs whose engine context must be recreated.
    pub fn set_isolated(&mut self, id: &SpaceId, isolated: bool) -> Vec<TabId> {
        let Some(space) = self.space_mut(id) else {
            return Vec::new();
        };
        if space.isolated == isolated {
            return Vec::new();
        }
        space.isolated = isolated;
        let mut affected: Vec<_> = space.pinned_tabs().into_iter().cloned().collect();
        for ws in self
            .windows
            .iter()
            .filter_map(|window| window.spaces.get(id))
        {
            affected.extend(ws.today.iter().cloned());
        }
        affected
    }

    pub fn delete_space(&mut self, id: &SpaceId, now: EpochMillis) -> Vec<TabId> {
        if self.spaces.len() == 1 {
            return Vec::new();
        }
        let Some(index) = self.spaces.iter().position(|space| space.id == *id) else {
            return Vec::new();
        };
        let space = self.spaces.remove(index);
        let mut removed: Vec<_> = space.pinned_tabs().into_iter().cloned().collect();
        let first = &self.spaces[0].id;
        for window in self.windows.iter_mut().chain(
            self.closed_windows
                .iter_mut()
                .map(|closed| &mut closed.window),
        ) {
            if let Some(ws) = window.spaces.remove(id) {
                removed.extend(ws.today);
            }
            if window.active_space_id == *id {
                window.active_space_id = first.clone();
            }
        }
        self.closed_windows
            .retain(|closed| !closed.window.spaces.is_empty());
        for id in &removed {
            self.archive_removed(id, now);
        }
        self.repair_selections();
        self.repair_visible_tabs();
        removed
    }

    pub fn open_window(&mut self, id: WindowId, space: Option<&SpaceId>) -> bool {
        if self.window(&id).is_some()
            || self
                .closed_windows
                .iter()
                .any(|closed| closed.window.id == id)
        {
            return false;
        }
        let space = space
            .and_then(|id| self.space(id).map(|space| &space.id))
            .unwrap_or(&self.focused_window().active_space_id);
        self.windows.push(Window {
            id,
            active_space_id: space.clone(),
            spaces: BTreeMap::new(),
            sidebar_visible: None,
        });
        true
    }

    pub fn focus_window(&mut self, id: &WindowId) -> bool {
        let Some(index) = self.windows.iter().position(|window| window.id == *id) else {
            return false;
        };
        self.windows[..=index].rotate_right(1);
        true
    }

    pub fn switch_space(&mut self, window: &WindowId, space: &SpaceId) -> bool {
        if self.space(space).is_none() {
            return false;
        }
        let Some(win) = self.windows.iter_mut().find(|win| win.id == *window) else {
            return false;
        };
        win.active_space_id = space.clone();
        for tab in win.visible_tabs().to_vec() {
            self.claim_visible(&tab, window);
        }
        true
    }

    pub fn set_sidebar_visible(&mut self, window: &WindowId, visible: bool) -> bool {
        let Some(win) = self.windows.iter_mut().find(|win| win.id == *window) else {
            return false;
        };
        win.sidebar_visible = Some(visible);
        self.sidebar_visible = visible;
        true
    }

    pub fn sidebar_visible(&self, window: &WindowId) -> bool {
        self.window(window)
            .and_then(|win| win.sidebar_visible)
            .unwrap_or(self.sidebar_visible)
    }

    pub fn close_window(&mut self, id: &WindowId, now: EpochMillis) -> bool {
        if self.windows.len() == 1 {
            return false;
        }
        let Some(index) = self.windows.iter().position(|window| window.id == *id) else {
            return false;
        };
        let mut window = self.windows.remove(index);
        window.spaces.retain(|_, ws| {
            if let Some([desktop, phone]) = ws.split.take()
                && ws.split_mode == Some(SplitMode::Responsive)
                && ws.today.contains(&phone)
                && is_mobile(&self.tabs, &phone)
            {
                ws.today.retain(|id| *id != phone);
                self.tabs.remove(&phone);
                if ws.active_tab_id.as_ref() == Some(&phone) {
                    ws.active_tab_id = Some(desktop);
                }
            }
            ws.split_mode = None;
            ws.split_ratio = None;
            if !ws
                .active_tab_id
                .as_ref()
                .is_some_and(|id| ws.today.contains(id))
            {
                ws.active_tab_id = most_recent(&self.tabs, &ws.today).cloned();
            }
            !ws.today.is_empty()
        });
        if !window.spaces.is_empty() {
            self.closed_windows.insert(
                0,
                ClosedWindow {
                    window,
                    closed_at: now,
                },
            );
            if self.closed_windows.len() > CLOSED_WINDOW_LIMIT {
                let dropped: Vec<_> = self.closed_windows.drain(CLOSED_WINDOW_LIMIT..).collect();
                for old in dropped {
                    for id in old.window.spaces.into_values().flat_map(|ws| ws.today) {
                        if let Some(entry) = self.take_archive_entry(&id, old.closed_at) {
                            self.archive.push(entry);
                        }
                    }
                }
                self.archive.truncate(ARCHIVE_LIMIT);
            }
        }
        true
    }

    pub fn reopen_window(&mut self) -> Option<WindowId> {
        if self.closed_windows.is_empty() {
            return None;
        }
        let mut closed = self.closed_windows.remove(0);
        if !closed
            .window
            .spaces
            .contains_key(&closed.window.active_space_id)
            && let Some(space) = self
                .spaces
                .iter()
                .find(|space| closed.window.spaces.contains_key(&space.id))
        {
            closed.window.active_space_id = space.id.clone();
        }
        let id = closed.window.id.clone();
        self.windows.insert(0, closed.window);
        self.repair_visible_tabs();
        Some(id)
    }

    /// Reopen the most recent closed window or archive entry. Archive entries reopen as new Today tabs.
    pub fn reopen_last(&mut self, new_tab_id: TabId, now: EpochMillis) -> Option<Reopened> {
        if let Some(closed) = self.closed_windows.first()
            && self
                .archive
                .first()
                .is_none_or(|entry| closed.closed_at > entry.closed_at)
        {
            return self.reopen_window().map(Reopened::Window);
        }
        let entry = self.archive.first()?;
        let mut tab = Tab::new(entry.url.clone(), now);
        tab.title.clone_from(&entry.title);
        let window = self.focused_window().id.clone();
        if !self.open_today(new_tab_id.clone(), tab, &window, None, true) {
            return None;
        }
        self.archive.remove(0);
        Some(Reopened::Tab(new_tab_id))
    }

    pub fn tab_ids(&self, window: &WindowId, space: &SpaceId) -> Option<Vec<&TabId>> {
        let space = self.space(space)?;
        let ws = self.window(window)?.spaces.get(&space.id);
        Some(
            self.favorites
                .iter()
                .chain(space.pinned_tabs())
                .chain(ws.into_iter().flat_map(|ws| &ws.today))
                .collect(),
        )
    }

    pub fn open_tabs(&self) -> BTreeMap<&TabId, &Tab> {
        self.favorites
            .iter()
            .chain(self.spaces.iter().flat_map(Space::pinned_tabs))
            .chain(
                self.windows
                    .iter()
                    .flat_map(|win| win.spaces.values())
                    .flat_map(|ws| &ws.today),
            )
            .filter_map(|id| Some((id, self.tabs.get(id)?)))
            .collect()
    }

    pub fn by_recency<'a>(&self, ids: impl IntoIterator<Item = &'a TabId>) -> Vec<&'a TabId> {
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort_by(|left, right| compare_recency(&self.tabs, left, right));
        ids
    }

    pub fn recent_tabs(&self, window: &WindowId) -> Option<Vec<&TabId>> {
        let win = self.window(window)?;
        let active = win
            .spaces
            .get(&win.active_space_id)
            .and_then(|ws| ws.active_tab_id.as_ref());
        Some(
            self.by_recency(
                self.tab_ids(window, &win.active_space_id)?
                    .into_iter()
                    .filter(|id| Some(*id) != active),
            ),
        )
    }

    pub fn open_today(
        &mut self,
        id: TabId,
        mut tab: Tab,
        window: &WindowId,
        after: Option<&TabId>,
        activate: bool,
    ) -> bool {
        if self.tabs.contains_key(&id) {
            return false;
        }
        let Some(win) = self.windows.iter_mut().find(|win| win.id == *window) else {
            return false;
        };
        let ws = win.spaces.entry(win.active_space_id.clone()).or_default();
        let at = after
            .and_then(|after| ws.today.iter().position(|id| id == after))
            .map_or(0, |index| index + 1);
        ws.today.insert(at, id.clone());
        tab.unseen = !activate;
        if activate {
            ws.active_tab_id = Some(id.clone());
        }
        self.tabs.insert(id, tab);
        true
    }

    pub fn locate(&self, id: &TabId) -> Option<Placement<'_>> {
        if self.favorites.contains(id) {
            return Some(Placement::Favorite);
        }
        for space in &self.spaces {
            if let Some(folder) = pin_parent(&space.pinned, id, None) {
                return Some(Placement::Pinned {
                    space: &space.id,
                    folder,
                });
            }
        }
        for window in &self.windows {
            for (space, ws) in &window.spaces {
                if ws.today.contains(id) {
                    return Some(Placement::Today {
                        space,
                        window: &window.id,
                    });
                }
            }
        }
        None
    }

    /// Shows a Today tab in its owner or a shared tab in the requested window.
    /// Pass no timestamp when moving through Control-Tab without committing the choice.
    pub fn activate(
        &mut self,
        tab: &TabId,
        requested: &WindowId,
        used_at: Option<EpochMillis>,
    ) -> Option<WindowId> {
        let (window, space) = match self.locate(tab)? {
            Placement::Today { window, space } => (window.clone(), space.clone()),
            Placement::Pinned { space, .. } => (requested.clone(), space.clone()),
            Placement::Favorite => (
                requested.clone(),
                self.window(requested)?.active_space_id.clone(),
            ),
        };
        let win = self.windows.iter_mut().find(|win| win.id == window)?;
        win.active_space_id = space.clone();
        let ws = win.spaces.entry(space).or_default();
        ws.active_tab_id = Some(tab.clone());
        for id in win.visible_tabs().to_vec() {
            self.claim_visible(&id, &window);
        }
        if let Some(now) = used_at {
            self.touch(tab, now);
        }
        Some(window)
    }

    pub fn touch(&mut self, id: &TabId, now: EpochMillis) -> bool {
        let Some(tab) = self.tabs.get_mut(id) else {
            return false;
        };
        tab.last_active = now as f64;
        tab.unseen = false;
        true
    }

    pub fn create_tab(
        &mut self,
        id: TabId,
        mut tab: Tab,
        to: Placement<'_>,
        before: Option<PinnedKey<'_>>,
    ) -> bool {
        if self.tabs.contains_key(&id) || !self.placement_exists(to) {
            return false;
        }
        if !matches!(to, Placement::Today { .. }) && tab.home_url.is_none() {
            tab.home_url = Some(tab.url.clone());
        }
        self.insert(&id, to, before);
        self.tabs.insert(id, tab);
        true
    }

    /// A stale destination leaves the tab in its original list.
    pub fn place_tab(
        &mut self,
        id: &TabId,
        to: Placement<'_>,
        before: Option<PinnedKey<'_>>,
    ) -> bool {
        if before == Some(PinnedKey::Tab(id))
            || self.locate(id).is_none()
            || is_mobile(&self.tabs, id)
            || !self.placement_exists(to)
        {
            return false;
        }
        self.detach(id);
        self.insert(id, to, before);
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.agent = false;
            if matches!(to, Placement::Today { .. }) {
                tab.home_url = None;
            } else if tab.home_url.is_none() {
                tab.home_url = Some(tab.url.clone());
            }
        }
        self.repair_selections();
        self.repair_visible_tabs();
        true
    }

    pub fn close_tab(&mut self, id: &TabId, now: EpochMillis) -> CloseTab {
        let Some(location) = self.locate(id) else {
            return CloseTab::Missing;
        };
        if let Placement::Today { space, window } = location {
            let space = space.clone();
            let window = window.clone();
            let ws = self
                .windows
                .iter_mut()
                .find(|win| win.id == window)
                .and_then(|win| win.spaces.get_mut(&space))
                .expect("Today placement has a window and Space");
            let index = ws
                .today
                .iter()
                .position(|tab| tab == id)
                .expect("Today placement contains the tab");
            ws.today.remove(index);
            if let Some([desktop, phone]) = &ws.split
                && ws.split_mode == Some(SplitMode::Responsive)
            {
                if id == phone && ws.active_tab_id.as_ref() == Some(phone) {
                    ws.active_tab_id = Some(desktop.clone());
                } else if id == desktop && ws.today.contains(phone) && is_mobile(&self.tabs, phone)
                {
                    ws.today.retain(|tab| tab != phone);
                    self.tabs.remove(phone);
                    if ws.active_tab_id.as_ref() == Some(phone) {
                        ws.active_tab_id = Some(id.clone());
                    }
                }
            }
            if ws.active_tab_id.as_ref() == Some(id) {
                ws.active_tab_id = ws
                    .today
                    .get(index.min(ws.today.len().saturating_sub(1)))
                    .cloned();
            }
            let archived = self.archive_removed(id, now);
            self.repair_selections();
            return if archived {
                CloseTab::Archived
            } else {
                CloseTab::Removed
            };
        }
        if let Some(tab) = self.tabs.get_mut(id) {
            if let Some(home) = &tab.home_url {
                tab.url.clone_from(home);
            }
            tab.history = None;
        }
        for ws in self
            .windows
            .iter_mut()
            .flat_map(|win| win.spaces.values_mut())
        {
            if ws.active_tab_id.as_ref() == Some(id) {
                ws.active_tab_id = None;
            }
            if let Some([desktop, phone]) = &ws.split
                && desktop == id
                && ws.split_mode == Some(SplitMode::Responsive)
            {
                if ws.active_tab_id.as_ref() == Some(phone) || ws.active_tab_id.is_none() {
                    ws.active_tab_id = ws.today.iter().find(|tab| *tab != phone).cloned();
                }
                ws.today.retain(|tab| tab != phone);
                self.tabs.remove(phone);
            }
        }
        self.repair_selections();
        CloseTab::Unloaded
    }

    pub fn delete_tab(&mut self, id: &TabId, now: EpochMillis) -> bool {
        if self.locate(id).is_none() {
            return false;
        }
        self.detach(id);
        self.archive_removed(id, now);
        self.repair_selections();
        true
    }

    pub fn in_agent_folder(&self, window: &WindowId, tab: &TabId) -> bool {
        self.window(window).is_some_and(|win| {
            win.spaces
                .values()
                .any(|ws| ws.today.contains(tab) && in_agent_folder(&self.tabs, ws, tab))
        })
    }

    /// Clear the active Space's Today list while leaving tabs in its agent folder.
    pub fn clear_today(&mut self, window: &WindowId, now: EpochMillis) -> Vec<TabId> {
        let Some(ws) = self
            .windows
            .iter_mut()
            .find(|win| win.id == *window)
            .and_then(|win| win.spaces.get_mut(&win.active_space_id))
        else {
            return Vec::new();
        };
        let mut removed: Vec<_> = ws
            .today
            .iter()
            .filter(|id| !in_agent_folder(&self.tabs, ws, id))
            .cloned()
            .collect();
        let removed_ids: BTreeSet<_> = removed.iter().collect();
        ws.today.retain(|id| !removed_ids.contains(id));
        for id in &removed {
            self.archive_removed(id, now);
        }
        removed.extend(self.repair_selections());
        removed
    }

    /// Close rows in the same Today list or agent folder as the given tab.
    pub fn close_today_tabs(
        &mut self,
        tab: &TabId,
        which: CloseToday,
        now: EpochMillis,
    ) -> Vec<TabId> {
        let Some(Placement::Today { space, window }) = self.locate(tab) else {
            return Vec::new();
        };
        let ws = &self
            .window(window)
            .expect("Today placement has an open window")
            .spaces[space];
        let agent = in_agent_folder(&self.tabs, ws, tab);
        let kept_source = match &ws.split {
            Some([source, mirror])
                if ws.split_mode == Some(SplitMode::Responsive) && mirror == tab =>
            {
                Some(source)
            }
            _ => None,
        };
        let siblings = ws
            .today
            .iter()
            .filter(|id| in_agent_folder(&self.tabs, ws, id) == agent);
        let closed: Vec<_> = match which {
            CloseToday::Below => siblings
                .skip_while(|id| *id != tab)
                .skip(1)
                .cloned()
                .collect(),
            CloseToday::Others => siblings
                .filter(|id| *id != tab && Some(*id) != kept_source)
                .cloned()
                .collect(),
        };
        for id in &closed {
            self.close_tab(id, now);
        }
        closed
    }

    pub fn archive_stale(&mut self, now: EpochMillis) -> Vec<TabId> {
        let hours = self.settings.archive_policy.hours();
        if hours == 0.0 {
            return Vec::new();
        }
        let cutoff = now as f64 - hours * 3_600_000.0;
        let mut removed = Vec::new();
        for win in &mut self.windows {
            for (space, ws) in &mut win.spaces {
                let visible_split = ws.split.as_ref().filter(|pair| {
                    space == &win.active_space_id
                        && ws
                            .active_tab_id
                            .as_ref()
                            .is_some_and(|id| pair.contains(id))
                });
                ws.today.retain(|id| {
                    let keep = ws.active_tab_id.as_ref() == Some(id)
                        || visible_split.is_some_and(|pair| pair.contains(id))
                        || self
                            .tabs
                            .get(id)
                            .is_none_or(|tab| tab.last_active >= cutoff);
                    if !keep {
                        removed.push(id.clone());
                    }
                    keep
                });
            }
        }
        for id in &removed {
            self.archive_removed(id, now);
        }
        removed.extend(self.repair_selections());
        removed
    }

    pub fn set_split(&mut self, window: &WindowId, tabs: [TabId; 2]) -> bool {
        let Some(win) = self.window(window) else {
            return false;
        };
        let space = win.active_space_id.clone();
        let members = self
            .tab_ids(window, &space)
            .expect("an open window has a valid active Space");
        if tabs[0] == tabs[1]
            || !tabs
                .iter()
                .all(|id| members.contains(&id) && !is_mobile(&self.tabs, id))
        {
            return false;
        }
        let Some(win) = self.windows.iter_mut().find(|win| win.id == *window) else {
            return false;
        };
        let ws = win.spaces.entry(space).or_default();
        end_split(ws, &mut self.tabs);
        ws.active_tab_id = Some(tabs[0].clone());
        ws.split = Some(tabs.clone());
        for id in &tabs {
            self.claim_visible(id, window);
        }
        true
    }

    /// Opens a phone-sized Today mirror against the window's selected desktop tab.
    pub fn open_responsive(
        &mut self,
        window: &WindowId,
        mirror: TabId,
        now: EpochMillis,
    ) -> Option<TabId> {
        let win = self.window(window)?;
        let space = win.active_space_id.clone();
        let ws = win.spaces.get(&space)?;
        let source = ws.active_tab_id.clone()?;
        let tab = self.tabs.get(&source)?;
        if ws.split_mode == Some(SplitMode::Responsive) || tab.device.is_some() {
            return None;
        }
        let mut new_tab = Tab::new(tab.url.clone(), now);
        new_tab.title.clone_from(&tab.title);
        new_tab.device = Some(Device::Mobile);
        if !self.open_today(mirror.clone(), new_tab, window, Some(&source), false) {
            return None;
        }
        let ws = self
            .active_window_space_mut(window)
            .expect("open_today created the window Space");
        ws.split = Some([source, mirror.clone()]);
        ws.split_mode = Some(SplitMode::Responsive);
        Some(mirror)
    }

    pub fn set_split_ratio(&mut self, window: &WindowId, ratio: f64) -> bool {
        let Some(ws) = self
            .active_window_space_mut(window)
            .filter(|ws| ws.split.is_some())
        else {
            return false;
        };
        if ratio.is_nan() {
            return false;
        }
        // The shell divider range in store.ts.
        ws.split_ratio = Some(ratio.clamp(0.2, 0.8));
        true
    }

    /// Ends the split and removes its responsive mirror from Today without archiving it.
    pub fn unsplit(&mut self, window: &WindowId) -> Option<EndedSplit> {
        let win = self.windows.iter_mut().find(|win| win.id == *window)?;
        let ws = win.spaces.get_mut(&win.active_space_id)?;
        end_split(ws, &mut self.tabs)
    }

    fn active_window_space_mut(&mut self, window: &WindowId) -> Option<&mut WindowSpace> {
        let win = self.windows.iter_mut().find(|win| win.id == *window)?;
        win.spaces.get_mut(&win.active_space_id)
    }

    pub fn add_folder(
        &mut self,
        id: FolderId,
        name: &str,
        space: &SpaceId,
        parent: Option<&FolderId>,
    ) -> bool {
        if self
            .spaces
            .iter()
            .any(|space| find_folder(&space.pinned, &id).is_some())
        {
            return false;
        }
        let Some(list) = self.pinned_list_mut(space, parent) else {
            return false;
        };
        let name = truncate_utf16(name.trim(), 80);
        list.push(PinnedNode::Folder {
            id,
            name: if name.is_empty() { "New Folder" } else { name }.into(),
            open: true,
            children: Vec::new(),
        });
        true
    }

    pub fn edit_folder(&mut self, id: &FolderId, name: Option<&str>, open: Option<bool>) -> bool {
        let Some(PinnedNode::Folder {
            name: saved_name,
            open: saved_open,
            ..
        }) = self
            .spaces
            .iter_mut()
            .find_map(|space| find_folder_mut(&mut space.pinned, id))
        else {
            return false;
        };
        if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
            *saved_name = name.into();
        }
        if let Some(open) = open {
            *saved_open = open;
        }
        true
    }

    pub fn move_folder(
        &mut self,
        id: &FolderId,
        space: &SpaceId,
        parent: Option<&FolderId>,
        before: Option<PinnedKey<'_>>,
    ) -> bool {
        let Some(PinnedNode::Folder { children, .. }) = self
            .spaces
            .iter()
            .find_map(|space| find_folder(&space.pinned, id))
        else {
            return false;
        };
        if before == Some(PinnedKey::Folder(id))
            || parent.is_some_and(|parent| parent == id || find_folder(children, parent).is_some())
            || !self.placement_exists(Placement::Pinned {
                space,
                folder: parent,
            })
        {
            return false;
        }
        let Some(folder) = self
            .spaces
            .iter_mut()
            .find_map(|space| take_folder(&mut space.pinned, id, false))
        else {
            return false;
        };
        let list = self
            .pinned_list_mut(space, parent)
            .expect("destination survives moving a different folder");
        let index = insertion_index(list, |node| Some(node.key()) == before);
        list.insert(index, folder);
        self.repair_selections();
        true
    }

    pub fn delete_folder(&mut self, id: &FolderId, keep_children: bool, now: EpochMillis) -> bool {
        let Some(folder) = self
            .spaces
            .iter_mut()
            .find_map(|space| take_folder(&mut space.pinned, id, keep_children))
        else {
            return false;
        };
        if let PinnedNode::Folder { children, .. } = folder {
            let mut ids = Vec::new();
            collect_pins(&children, &mut ids);
            for id in ids {
                self.archive_removed(id, now);
            }
            self.repair_selections();
        }
        true
    }

    fn pinned_list_mut(
        &mut self,
        space: &SpaceId,
        folder: Option<&FolderId>,
    ) -> Option<&mut Vec<PinnedNode>> {
        let list = &mut self.space_mut(space)?.pinned;
        match folder {
            None => Some(list),
            Some(id) => match find_folder_mut(list, id)? {
                PinnedNode::Folder { children, .. } => Some(children),
                PinnedNode::Tab { .. } => None,
            },
        }
    }

    fn placement_exists(&self, placement: Placement<'_>) -> bool {
        match placement {
            Placement::Favorite => true,
            Placement::Pinned { space, folder } => self.space(space).is_some_and(|space| {
                folder.is_none_or(|id| find_folder(&space.pinned, id).is_some())
            }),
            Placement::Today { space, window } => {
                self.space(space).is_some() && self.window(window).is_some()
            }
        }
    }

    fn insert(&mut self, id: &TabId, to: Placement<'_>, before: Option<PinnedKey<'_>>) {
        let list = match to {
            Placement::Favorite => &mut self.favorites,
            Placement::Pinned { space, folder } => {
                let nodes = self
                    .pinned_list_mut(space, folder)
                    .expect("validated pinned list");
                let index = insertion_index(nodes, |node| Some(node.key()) == before);
                nodes.insert(index, PinnedNode::Tab { tab_id: id.clone() });
                return;
            }
            Placement::Today { space, window } => {
                let win = self
                    .windows
                    .iter_mut()
                    .find(|win| win.id == *window)
                    .expect("validated Today window");
                &mut win.spaces.entry(space.clone()).or_default().today
            }
        };
        let index = insertion_index(list, |id| Some(PinnedKey::Tab(id)) == before);
        list.insert(index, id.clone());
    }

    fn detach(&mut self, id: &TabId) {
        self.favorites.retain(|tab| tab != id);
        for space in &mut self.spaces {
            remove_pin(&mut space.pinned, id);
        }
        for ws in self
            .windows
            .iter_mut()
            .flat_map(|win| win.spaces.values_mut())
        {
            ws.today.retain(|tab| tab != id);
        }
    }

    fn claim_visible(&mut self, id: &TabId, owner: &WindowId) {
        for win in self.windows.iter_mut().filter(|win| win.id != *owner) {
            for ws in win.spaces.values_mut() {
                if ws.active_tab_id.as_ref() == Some(id)
                    || ws.split.as_ref().is_some_and(|split| split.contains(id))
                {
                    release_visible(ws, id, &mut self.tabs);
                }
            }
        }
    }

    pub(crate) fn repair_visible_tabs(&mut self) {
        let mut visible = BTreeSet::new();
        for win in &mut self.windows {
            for id in win.visible_tabs().to_vec() {
                if !visible.contains(&id) {
                    continue;
                }
                let ws = win
                    .spaces
                    .get_mut(&win.active_space_id)
                    .expect("visible tabs have a Space");
                release_visible(ws, &id, &mut self.tabs);
            }
            visible.extend(win.visible_tabs().iter().cloned());
        }
    }

    pub(crate) fn repair_selections(&mut self) -> Vec<TabId> {
        let pinned: BTreeMap<_, _> = self
            .spaces
            .iter()
            .map(|space| (&space.id, space.pinned_tabs()))
            .collect();
        let mut removed = Vec::new();
        let windows = self.windows.iter_mut().chain(
            self.closed_windows
                .iter_mut()
                .map(|closed| &mut closed.window),
        );
        for win in windows {
            for (space, ws) in &mut win.spaces {
                let shared = |id: &TabId| {
                    self.favorites.contains(id)
                        || pinned.get(space).is_some_and(|ids| ids.contains(&id))
                };
                if let Some([desktop, phone]) = &ws.split
                    && ws.split_mode == Some(SplitMode::Responsive)
                    && ws.today.contains(phone)
                    && is_mobile(&self.tabs, phone)
                    && ((!shared(desktop) && !ws.today.contains(desktop))
                        || is_mobile(&self.tabs, desktop))
                {
                    ws.today.retain(|id| id != phone);
                    self.tabs.remove(phone);
                    removed.push(phone.clone());
                }
                let belongs = |id: &TabId| shared(id) || ws.today.contains(id);
                if ws.active_tab_id.as_ref().is_some_and(|id| !belongs(id)) {
                    ws.active_tab_id = None;
                }
                if ws
                    .split
                    .as_ref()
                    .is_none_or(|[left, right]| left == right || !belongs(left) || !belongs(right))
                {
                    ws.split = None;
                    ws.split_mode = None;
                }
            }
        }
        removed
    }

    fn archive_removed(&mut self, id: &TabId, now: EpochMillis) -> bool {
        let Some(entry) = self.take_archive_entry(id, now) else {
            return false;
        };
        self.archive.insert(0, entry);
        self.archive.truncate(ARCHIVE_LIMIT);
        true
    }

    fn take_archive_entry(&mut self, id: &TabId, closed_at: EpochMillis) -> Option<ArchivedTab> {
        let tab = self.tabs.remove(id)?;
        (tab.url != "about:blank" && tab.device != Some(Device::Mobile)).then_some(ArchivedTab {
            url: tab.url,
            title: tab.title,
            closed_at,
        })
    }
}

fn insertion_index<T>(list: &[T], matches: impl FnMut(&T) -> bool) -> usize {
    list.iter().position(matches).unwrap_or(list.len())
}

fn most_recent<'a>(tabs: &BTreeMap<TabId, Tab>, ids: &'a [TabId]) -> Option<&'a TabId> {
    ids.iter()
        .min_by(|left, right| compare_activity(tabs, left, right))
}

fn compare_activity(
    tabs: &BTreeMap<TabId, Tab>,
    left: &TabId,
    right: &TabId,
) -> std::cmp::Ordering {
    let active_at = |id| tabs.get(id).map_or(0.0, |tab| tab.last_active);
    active_at(right).total_cmp(&active_at(left))
}

pub(crate) fn compare_recency(
    tabs: &BTreeMap<TabId, Tab>,
    left: &TabId,
    right: &TabId,
) -> std::cmp::Ordering {
    let unseen = |id| tabs.get(id).is_some_and(|tab| tab.unseen);
    unseen(left)
        .cmp(&unseen(right))
        .then_with(|| compare_activity(tabs, left, right))
}

fn collect_pins<'a>(nodes: &'a [PinnedNode], tabs: &mut Vec<&'a TabId>) {
    for node in nodes {
        match node {
            PinnedNode::Tab { tab_id } => tabs.push(tab_id),
            PinnedNode::Folder { children, .. } => collect_pins(children, tabs),
        }
    }
}

fn pin_parent<'a>(
    nodes: &'a [PinnedNode],
    tab: &TabId,
    parent: Option<&'a FolderId>,
) -> Option<Option<&'a FolderId>> {
    nodes.iter().find_map(|node| match node {
        PinnedNode::Tab { tab_id } if tab_id == tab => Some(parent),
        PinnedNode::Folder { id, children, .. } => pin_parent(children, tab, Some(id)),
        PinnedNode::Tab { .. } => None,
    })
}

fn find_folder<'a>(nodes: &'a [PinnedNode], id: &FolderId) -> Option<&'a PinnedNode> {
    nodes.iter().find_map(|node| match node {
        PinnedNode::Folder { id: candidate, .. } if candidate == id => Some(node),
        PinnedNode::Folder { children, .. } => find_folder(children, id),
        PinnedNode::Tab { .. } => None,
    })
}

fn find_folder_mut<'a>(nodes: &'a mut [PinnedNode], id: &FolderId) -> Option<&'a mut PinnedNode> {
    for node in nodes {
        if matches!(node, PinnedNode::Folder { id: candidate, .. } if candidate == id) {
            return Some(node);
        }
        if let PinnedNode::Folder { children, .. } = node
            && let Some(found) = find_folder_mut(children, id)
        {
            return Some(found);
        }
    }
    None
}

fn remove_pin(nodes: &mut Vec<PinnedNode>, id: &TabId) {
    nodes.retain_mut(|node| match node {
        PinnedNode::Tab { tab_id } => tab_id != id,
        PinnedNode::Folder { children, .. } => {
            remove_pin(children, id);
            true
        }
    });
}

fn take_folder(
    nodes: &mut Vec<PinnedNode>,
    id: &FolderId,
    promote_children: bool,
) -> Option<PinnedNode> {
    for (index, node) in nodes.iter_mut().enumerate() {
        if let PinnedNode::Folder {
            id: candidate,
            children,
            ..
        } = node
        {
            if candidate == id {
                let promoted = if promote_children {
                    std::mem::take(children)
                } else {
                    Vec::new()
                };
                let removed = nodes.remove(index);
                nodes.splice(index..index, promoted);
                return Some(removed);
            }
            if let Some(removed) = take_folder(children, id, promote_children) {
                return Some(removed);
            }
        }
    }
    None
}

fn release_visible(ws: &mut WindowSpace, id: &TabId, tabs: &mut BTreeMap<TabId, Tab>) {
    if let Some(pair) = &ws.split
        && let Some(other) = pair.iter().find(|tab| *tab != id)
        && pair.contains(id)
    {
        if ws.split_mode == Some(SplitMode::Responsive)
            && is_mobile(tabs, other)
            && ws.today.contains(other)
        {
            ws.today.retain(|tab| tab != other);
            tabs.remove(other);
            ws.active_tab_id = most_recent(tabs, &ws.today).cloned();
        } else if ws.active_tab_id.as_ref() == Some(id) {
            ws.active_tab_id = ws
                .today
                .contains(other)
                .then(|| other.clone())
                .or_else(|| most_recent(tabs, &ws.today).cloned());
        }
        ws.split = None;
        ws.split_mode = None;
    } else if ws.active_tab_id.as_ref() == Some(id) {
        ws.active_tab_id = most_recent(tabs, &ws.today).cloned();
    }
}

fn is_mobile(tabs: &BTreeMap<TabId, Tab>, id: &TabId) -> bool {
    tabs.get(id)
        .is_some_and(|tab| tab.device == Some(Device::Mobile))
}

fn end_split(ws: &mut WindowSpace, tabs: &mut BTreeMap<TabId, Tab>) -> Option<EndedSplit> {
    let [source, mirror] = ws.split.take()?;
    if ws.split_mode.take() == Some(SplitMode::Responsive)
        && ws.today.contains(&mirror)
        && is_mobile(tabs, &mirror)
    {
        ws.today.retain(|id| *id != mirror);
        tabs.remove(&mirror);
        if ws.active_tab_id.as_ref() == Some(&mirror) {
            ws.active_tab_id = Some(source);
        }
        return Some(EndedSplit::Responsive { mirror });
    }
    Some(EndedSplit::Standard)
}

fn in_agent_folder(tabs: &BTreeMap<TabId, Tab>, ws: &WindowSpace, id: &TabId) -> bool {
    tabs.get(id).is_some_and(|tab| tab.agent)
        && !ws.split.as_ref().is_some_and(|pair| {
            pair.contains(id)
                && pair
                    .iter()
                    .all(|id| ws.today.contains(id) && tabs.contains_key(id))
        })
}

fn truncate_utf16(text: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > limit {
            return &text[..index];
        }
    }
    text
}
