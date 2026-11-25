use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{IncognitoSession, Owner, Partition};
use flecha_core::{BrowserState, EpochMillis, Placement, SpaceId, Tab, TabId, WindowId};
use uuid::Uuid;

use crate::incognito::Incognito;

pub struct Model {
    /// The regular windows, Spaces and tabs: the state storage saves.
    pub core: BrowserState,
    /// The incognito windows' own state while any is open. Storage never sees it.
    pub(crate) incognito: Option<Incognito>,
    incognito_sessions: u64,
    /// The last regular window closed while incognito windows stayed open, as Chromium allows.
    /// The regular state keeps that window, as a quit would, and shows nothing until a new
    /// regular window or the next start brings it back.
    regular_closed: bool,
    pub peeks: BTreeMap<WindowId, Transient>,
    pub flechinhas: BTreeMap<WindowId, Transient>,
    pub popups: BTreeMap<WindowId, Popup>,
}

/// What closing an incognito window took down.
pub(crate) struct ClosedIncognito {
    /// Pages whose browsers must close.
    pub(crate) removed: Vec<TabId>,
    /// The session that ended with its last window.
    pub(crate) ended: Option<IncognitoSession>,
}

/// A scripted popup (`window.open` with features) in a window of its own, as Chromium
/// keeps it for the TypeScript shell. It is never saved.
pub struct Popup {
    pub page: Transient,
    /// The Flecha window focus returns to when the popup closes.
    pub opener: WindowId,
    pub size: Option<(i32, i32)>,
}

pub struct Transient {
    pub id: TabId,
    pub tab: Tab,
    pub space: SpaceId,
    pub partition: Partition,
}

impl Transient {
    /// Whether the page came from the regular Space `space`. An incognito page's Space is its
    /// window's own, which a regular Space can share an ID with after an import.
    fn came_from_regular_space(&self, space: &SpaceId) -> bool {
        &self.space == space && !self.partition.is_incognito()
    }
}

pub struct Detachment {
    pub window: WindowId,
    pub tab: TabId,
    pub removed: Vec<TabId>,
}

impl Model {
    pub fn new(core: Option<BrowserState>) -> Self {
        Self {
            core: core.unwrap_or_else(|| BrowserState::new(space_id(), window_id())),
            incognito: None,
            incognito_sessions: 0,
            regular_closed: false,
            peeks: BTreeMap::new(),
            flechinhas: BTreeMap::new(),
            popups: BTreeMap::new(),
        }
    }

    /// The state that has this Flecha window: the regular one, or the incognito windows'.
    /// Whether `window` is a regular window on screen. After the last one closed with incognito
    /// windows open, the regular state still has it but nothing addresses it.
    fn shows_regular(&self, window: &WindowId) -> bool {
        !self.regular_closed && self.core.window(window).is_some()
    }

    pub(crate) fn state(&self, window: &WindowId) -> Option<&BrowserState> {
        if self.shows_regular(window) {
            return Some(&self.core);
        }
        let incognito = &self.incognito.as_ref()?.state;
        incognito.window(window).map(|_| incognito)
    }

    pub(crate) fn state_mut(&mut self, window: &WindowId) -> Option<&mut BrowserState> {
        if self.shows_regular(window) {
            return Some(&mut self.core);
        }
        let incognito = &mut self.incognito.as_mut()?.state;
        incognito.window(window).is_some().then_some(incognito)
    }

    /// The state that has this tab, and whether storage keeps it.
    fn tab_state_mut(&mut self, id: &TabId) -> Option<(&mut BrowserState, bool)> {
        if self.core.tab(id).is_some() {
            return Some((&mut self.core, true));
        }
        let incognito = &mut self.incognito.as_mut()?.state;
        incognito.tab(id).is_some().then_some((incognito, false))
    }

    /// A tab of a Flecha window, regular or incognito.
    pub(crate) fn tab(&self, id: &TabId) -> Option<&Tab> {
        self.core
            .tab(id)
            .or_else(|| self.incognito.as_ref()?.state.tab(id))
    }

    /// Every open tab and page: regular, incognito, and the transient pages of Peeks,
    /// Flechinhas and popups.
    pub(crate) fn all_tabs(&self) -> impl Iterator<Item = (&TabId, &Tab)> {
        let transient = self
            .flechinhas
            .values()
            .chain(self.peeks.values())
            .chain(self.popups.values().map(|popup| &popup.page))
            .map(|page| (&page.id, &page.tab));
        self.core
            .open_tabs()
            .into_iter()
            .chain(
                self.incognito
                    .iter()
                    .flat_map(|incognito| incognito.state.open_tabs()),
            )
            .chain(transient)
    }

    /// Whether this window, or the page in a Flechinha or popup window, is incognito.
    pub(crate) fn is_incognito(&self, window: &WindowId) -> bool {
        self.incognito_session(window).is_some()
    }

    /// The incognito session a window browses in, or `None` for a regular window.
    pub(crate) fn incognito_session(&self, window: &WindowId) -> Option<IncognitoSession> {
        if let Some(page) = self.flechinhas.get(window) {
            return page.partition.incognito_session();
        }
        if let Some(popup) = self.popups.get(window) {
            return popup.page.partition.incognito_session();
        }
        self.incognito
            .as_ref()
            .filter(|incognito| incognito.state.window(window).is_some())
            .map(|incognito| incognito.session)
    }

    /// The regular windows on screen.
    pub(crate) fn regular_windows(&self) -> &[flecha_core::Window] {
        if self.regular_closed {
            &[]
        } else {
            self.core.windows()
        }
    }

    /// Whether the regular windows are closed while incognito windows stay open.
    pub(crate) fn regular_closed(&self) -> bool {
        self.regular_closed
    }

    /// The last regular window closed while incognito windows stay open.
    pub(crate) fn close_regular(&mut self) {
        self.regular_closed = true;
    }

    /// A regular window. After the regular windows closed while incognito windows stayed open, it
    /// is the window they left, with its tabs: shipping's `attachWindow` gives a new Chromium
    /// window the saved window no other one shows. Otherwise it is a new window on the focused
    /// window's Space.
    pub(crate) fn open_regular_window(&mut self) -> Result<WindowId> {
        if std::mem::take(&mut self.regular_closed) {
            return Ok(self.core.focused_window().id.clone());
        }
        let window = window_id();
        ensure!(
            self.core.open_window(window.clone(), None),
            "could not open window"
        );
        Ok(window)
    }

    /// A new regular window showing `space` ("Open in New Window", background.ts:1172).
    pub(crate) fn open_space_window(&mut self, space: &SpaceId) -> Result<WindowId> {
        let window = window_id();
        ensure!(
            self.core.open_window(window.clone(), Some(space)),
            "could not open window"
        );
        Ok(window)
    }

    /// The incognito windows, while a session lasts.
    pub(crate) fn incognito_windows(&self) -> &[flecha_core::Window] {
        self.incognito
            .as_ref()
            .map_or(&[], |incognito| incognito.state.windows())
    }

    /// Opens an incognito window, starting a session when none is open.
    pub(crate) fn open_incognito_window(&mut self) -> WindowId {
        let window = window_id();
        match &mut self.incognito {
            Some(incognito) => {
                incognito.state.open_window(window.clone(), None);
                incognito.state.focus_window(&window);
            }
            None => {
                self.incognito_sessions += 1;
                let session = IncognitoSession::new(self.incognito_sessions);
                self.incognito = Some(Incognito::new(session, window.clone()));
            }
        }
        window
    }

    /// Closes an incognito window. The last one ends the session, with every page it opened.
    pub(crate) fn close_incognito_window(&mut self, window: &WindowId) -> ClosedIncognito {
        let mut removed = Vec::new();
        removed.extend(self.peeks.remove(window).map(|page| page.id));
        let Some(incognito) = &mut self.incognito else {
            return ClosedIncognito {
                removed,
                ended: None,
            };
        };
        if incognito.state.windows().len() > 1 {
            removed.extend(incognito.state.close_window(window, now()).removed);
            return ClosedIncognito {
                removed,
                ended: None,
            };
        }
        let partition = incognito.partition();
        let session = incognito.session;
        self.incognito = None;
        removed.extend(
            self.peeks
                .extract_if(.., |_, page| page.partition == partition)
                .chain(
                    self.flechinhas
                        .extract_if(.., |_, page| page.partition == partition),
                )
                .map(|(_, page)| page.id)
                .chain(
                    self.popups
                        .extract_if(.., |_, popup| popup.page.partition == partition)
                        .map(|(_, popup)| popup.page.id),
                ),
        );
        ClosedIncognito {
            removed,
            ended: Some(session),
        }
    }

    pub(crate) fn focus_window(&mut self, window: &WindowId) {
        if let Some(state) = self.state_mut(window) {
            state.focus_window(window);
        }
    }

    /// The window a page with this partition joins when it leaves a Flechinha or popup, if one of
    /// its kind is open.
    pub(crate) fn focused_window_for(&self, partition: &Partition) -> Option<&WindowId> {
        match &self.incognito {
            Some(incognito) if partition.is_incognito() => {
                (incognito.partition() == *partition).then(|| &incognito.state.focused_window().id)
            }
            _ if partition.is_incognito() || self.regular_closed => None,
            _ => Some(&self.core.focused_window().id),
        }
    }

    pub fn active(&self, window: &WindowId) -> Option<&TabId> {
        let window = self.state(window)?.window(window)?;
        window
            .spaces
            .get(&window.active_space_id)?
            .active_tab_id
            .as_ref()
    }

    pub fn keyboard_pane(&self, window: &WindowId) -> Option<(Owner, &TabId)> {
        if let Some(popup) = self.popups.get(window) {
            return Some((Owner::Popup(window.clone()), &popup.page.id));
        }
        if let Some(page) = self.flechinhas.get(window) {
            return Some((Owner::Flechinha(window.clone()), &page.id));
        }
        self.state(window)?;
        if let Some(page) = self.peeks.get(window) {
            return Some((Owner::Peek(window.clone()), &page.id));
        }
        self.active(window)
            .map(|tab| (Owner::Tab(window.clone()), tab))
    }

    pub fn open(&mut self, window: &WindowId, url: String, activate: bool) -> Result<TabId> {
        let id = tab_id();
        let after = if activate {
            None
        } else {
            self.active(window).cloned()
        };
        self.open_page(id.clone(), window, url, after.as_ref(), activate)?;
        Ok(id)
    }

    /// Adds a Today tab to the window's current Space, after `after` when it is there
    /// and first otherwise.
    pub fn open_page(
        &mut self,
        id: TabId,
        window: &WindowId,
        url: String,
        after: Option<&TabId>,
        activate: bool,
    ) -> Result<()> {
        let state = self.state_mut(window).context("missing Flecha window")?;
        ensure!(
            state.open_today(id, Tab::new(url, now()), window, after, activate),
            "could not open tab"
        );
        Ok(())
    }

    /// A new Flecha window showing one new tab, in the focused window's Space. A page from an
    /// incognito window opens in a new incognito window.
    pub fn open_window_page(
        &mut self,
        id: TabId,
        url: String,
        opener: &Partition,
    ) -> Result<WindowId> {
        let window = if opener.is_incognito() {
            ensure!(
                self.incognito
                    .as_ref()
                    .is_some_and(|incognito| incognito.partition() == *opener),
                "the incognito session that opened this page has ended"
            );
            self.open_incognito_window()
        } else {
            self.open_regular_window()?
        };
        self.open_page(id, &window, url, None, true)?;
        Ok(window)
    }

    pub fn open_popup(
        &mut self,
        id: TabId,
        url: String,
        opener: (&WindowId, &TabId),
        partition: Partition,
        size: Option<(i32, i32)>,
    ) -> Result<WindowId> {
        let (window, source) = opener;
        // A Peek's, Flechinha's or popup's page keeps the Space it came from.
        let space = match self.transient(source) {
            Some(page) => page.space.clone(),
            None => self.origin_space(window, Some(source))?.clone(),
        };
        let popup = window_id();
        self.popups.insert(
            popup.clone(),
            Popup {
                page: Transient {
                    id,
                    tab: Tab::new(url, now()),
                    space,
                    partition,
                },
                opener: window.clone(),
                size,
            },
        );
        Ok(popup)
    }

    pub fn peek(
        &mut self,
        window: WindowId,
        url: String,
        source: Option<&TabId>,
    ) -> Result<Option<TabId>> {
        self.peek_page(window, tab_id(), url, source)
    }

    /// Opens `id` in the window's Peek and returns the page it replaces.
    pub fn peek_page(
        &mut self,
        window: WindowId,
        id: TabId,
        url: String,
        source: Option<&TabId>,
    ) -> Result<Option<TabId>> {
        let space = self.origin_space(&window, source)?.clone();
        let partition = source
            .and_then(|id| self.partition(id))
            .unwrap_or_else(|| self.window_partition(&window, &space));
        let transient = Transient {
            id,
            tab: Tab::new(url, now()),
            space,
            partition,
        };
        Ok(self.peeks.insert(window, transient).map(|old| old.id))
    }

    /// A Flechinha showing `url`, which arrived from outside Flecha, in the current Space of
    /// the regular window `window` and that Space's request context. It reads the regular state
    /// directly, so it works while the regular windows are closed beside incognito ones.
    pub fn open_flechinha(&mut self, window: &WindowId, url: String) -> Result<WindowId> {
        let space = self
            .core
            .window(window)
            .context("missing Flecha window")?
            .active_space_id
            .clone();
        let partition = self.space_partition(&space);
        let id = window_id();
        self.flechinhas.insert(
            id.clone(),
            Transient {
                id: tab_id(),
                tab: Tab::new(url, now()),
                space,
                partition,
            },
        );
        Ok(id)
    }

    pub fn promote(&mut self, window: &WindowId) -> Result<TabId> {
        let page = self.peeks.remove(window).context("no Peek to promote")?;
        let after = self.active(window).cloned();
        self.adopt(window, page, after.as_ref())
    }

    pub fn detach(&mut self, window: &WindowId, source: &TabId) -> Result<Detachment> {
        let mut tab = self.tab(source).context("no tab to move")?.clone();
        let space = self.origin_space(window, Some(source))?.clone();
        let partition = self.partition(source).context("tab has no Space")?;
        let id = window_id();
        let (state, _) = self.tab_state_mut(source).context("no tab to move")?;
        let change = state.close_tab(source, now());
        tab.agent = false;
        let page = Transient {
            id: tab_id(),
            tab,
            space,
            partition,
        };
        let tab = page.id.clone();
        self.flechinhas.insert(id.clone(), page);
        Ok(Detachment {
            window: id,
            tab,
            removed: change
                .removed
                .into_iter()
                .filter(|id| id != source)
                .collect(),
        })
    }

    /// A Flechinha's page goes back to the focused window of its own kind: an incognito page
    /// to an incognito window (patch 0032), any other to a regular one. A regular page brings
    /// back the regular window that closed while incognito windows stayed open.
    pub fn return_page(&mut self, window: &WindowId) -> Result<(WindowId, TabId)> {
        let partition = self
            .flechinhas
            .get(window)
            .context("no Flechinha to return")?
            .partition
            .clone();
        let destination = if !partition.is_incognito() && self.regular_closed {
            self.open_regular_window()?
        } else {
            self.focused_window_for(&partition)
                .context("no window to return the page to")?
                .clone()
        };
        let page = self
            .flechinhas
            .remove(window)
            .context("no Flechinha to return")?;
        let tab = self.adopt(&destination, page, None)?;
        Ok((destination, tab))
    }

    fn adopt(
        &mut self,
        window: &WindowId,
        mut page: Transient,
        after: Option<&TabId>,
    ) -> Result<TabId> {
        page.tab.home_url = None;
        page.tab.last_active = Some(now() as f64);
        let state = self.state_mut(window).context("missing Flecha window")?;
        ensure!(
            state.open_today(page.id.clone(), page.tab, window, after, true),
            "could not adopt page"
        );
        Ok(page.id)
    }

    pub fn partition(&self, id: &TabId) -> Option<Partition> {
        if let Some(transient) = self.transient(id) {
            return Some(transient.partition.clone());
        }
        // Before Spaces and placement: an incognito tab is never in a persistent context.
        if let Some(incognito) = &self.incognito
            && incognito.state.tab(id).is_some()
        {
            return Some(incognito.partition());
        }
        Some(match self.core.locate(id)? {
            Placement::Favorite => Partition::Shared,
            Placement::Pinned { space, .. } | Placement::Today { space, .. } => {
                self.space_partition(space)
            }
        })
    }

    fn origin_space(&self, window: &WindowId, source: Option<&TabId>) -> Result<&SpaceId> {
        let state = self.state(window).context("missing Flecha window")?;
        match source.and_then(|tab| state.locate(tab)) {
            Some(Placement::Pinned { space, .. } | Placement::Today { space, .. }) => Ok(space),
            Some(Placement::Favorite) | None => state
                .window(window)
                .map(|window| &window.active_space_id)
                .context("missing Flecha window"),
        }
    }

    /// The request context a new page in this window's Space browses in.
    pub(crate) fn window_partition(&self, window: &WindowId, space: &SpaceId) -> Partition {
        match &self.incognito {
            Some(incognito) if incognito.state.window(window).is_some() => incognito.partition(),
            _ => self.space_partition(space),
        }
    }

    /// Closes the Peeks, Flechinhas and popups that came from the regular Space `space`.
    pub fn close_space_transients(&mut self, space: &SpaceId) -> Vec<TabId> {
        self.peeks
            .extract_if(.., |_, page| page.came_from_regular_space(space))
            .chain(
                self.flechinhas
                    .extract_if(.., |_, page| page.came_from_regular_space(space)),
            )
            .map(|(_, page)| page.id)
            .chain(
                self.popups
                    .extract_if(.., |_, popup| popup.page.came_from_regular_space(space))
                    .map(|(_, popup)| popup.page.id),
            )
            .collect()
    }

    fn space_partition(&self, id: &SpaceId) -> Partition {
        if self.core.space(id).is_some_and(|space| space.isolated) {
            Partition::Isolated(id.clone())
        } else {
            Partition::Shared
        }
    }

    fn transient(&self, id: &TabId) -> Option<&Transient> {
        self.flechinhas
            .values()
            .chain(self.peeks.values())
            .chain(self.popups.values().map(|popup| &popup.page))
            .find(|page| &page.id == id)
    }

    /// The tab or page, and whether it is durable: kept in the saved state and recorded in
    /// history. Transient pages and incognito tabs are not.
    pub fn tab_mut(&mut self, id: &TabId) -> Option<(&mut Tab, bool)> {
        if self.transient(id).is_none() {
            let (state, durable) = self.tab_state_mut(id)?;
            return Some((state.tab_mut(id)?, durable));
        }
        self.flechinhas
            .values_mut()
            .chain(self.peeks.values_mut())
            .chain(self.popups.values_mut().map(|popup| &mut popup.page))
            .find(|page| &page.id == id)
            .map(|page| (&mut page.tab, false))
    }

    /// Closes a tab of a Flecha window, regular or incognito, and returns the tabs it took
    /// down.
    pub(crate) fn close_tab(&mut self, id: &TabId) -> Vec<TabId> {
        self.tab_state_mut(id)
            .map(|(state, _)| state.close_tab(id, now()).removed)
            .unwrap_or_default()
    }

    pub fn page(&self, owner: &Owner, id: &TabId) -> Option<&Tab> {
        match owner {
            Owner::Tab(window) => self
                .state(window)
                .and_then(|state| state.window(window))
                .filter(|window| window.visible_tabs().contains(id) && self.transient(id).is_none())
                .and_then(|_| self.tab(id)),
            Owner::Peek(window) => self
                .peeks
                .get(window)
                .filter(|page| &page.id == id)
                .map(|page| &page.tab),
            Owner::Flechinha(window) => self
                .flechinhas
                .get(window)
                .filter(|page| &page.id == id)
                .map(|page| &page.tab),
            Owner::Popup(window) => self
                .popups
                .get(window)
                .filter(|popup| &popup.page.id == id)
                .map(|popup| &popup.page.tab),
        }
    }
}

pub fn tab_id() -> TabId {
    TabId::new(Uuid::new_v4().to_string())
}
pub fn window_id() -> WindowId {
    WindowId::new(Uuid::new_v4().to_string())
}
pub fn space_id() -> SpaceId {
    SpaceId::new(Uuid::new_v4().to_string())
}

pub fn now() -> EpochMillis {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| EpochMillis::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_is_transient_and_detached_tabs_return_to_the_focused_space() {
        let mut model = Model::new(None);
        let main = model.core.focused_window().id.clone();
        let first_space = model.core.focused_window().active_space_id.clone();
        let page = model
            .open(&main, "https://example.test/first".into(), true)
            .unwrap();
        model.core.set_isolated(&first_space, true);
        let isolated = Partition::Isolated(first_space);
        model
            .peek(
                main.clone(),
                "https://example.test/peek".into(),
                Some(&page),
            )
            .unwrap();
        let peek = model.peeks[&main].id.clone();
        assert!(model.core.tab(&peek).is_none());
        assert_eq!(model.partition(&peek), Some(isolated.clone()));
        assert_eq!(model.promote(&main).unwrap(), peek);
        let detached = model.detach(&main, &peek).unwrap();
        assert!(model.core.tab(&peek).is_none());
        assert_eq!(model.partition(&detached.tab), Some(isolated));

        let second_space = space_id();
        assert!(model.core.add_space(second_space.clone()));
        let second = window_id();
        assert!(model.core.open_window(second.clone(), Some(&second_space)));
        assert!(model.core.focus_window(&second));
        let (window, returned) = model.return_page(&detached.window).unwrap();
        assert_eq!(window, second);
        assert_eq!(model.active(&window), Some(&returned));
        assert_eq!(model.partition(&returned), Some(Partition::Shared));
        assert!(model.flechinhas.is_empty());
    }
}
