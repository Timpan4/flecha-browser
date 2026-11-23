use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{Owner, Partition};
use flecha_core::{BrowserState, EpochMillis, Placement, SpaceId, Tab, TabId, WindowId};
use uuid::Uuid;

pub struct Model {
    pub core: BrowserState,
    pub peeks: BTreeMap<WindowId, Transient>,
    pub flechinhas: BTreeMap<WindowId, Transient>,
}

pub struct Transient {
    pub id: TabId,
    pub tab: Tab,
    pub space: SpaceId,
    pub partition: Partition,
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
            peeks: BTreeMap::new(),
            flechinhas: BTreeMap::new(),
        }
    }

    pub fn active(&self, window: &WindowId) -> Option<&TabId> {
        let window = self.core.window(window)?;
        window
            .spaces
            .get(&window.active_space_id)?
            .active_tab_id
            .as_ref()
    }

    pub fn keyboard_pane(&self, window: &WindowId) -> Option<(Owner, &TabId)> {
        if let Some(page) = self.flechinhas.get(window) {
            return Some((Owner::Flechinha(window.clone()), &page.id));
        }
        self.core.window(window)?;
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
        ensure!(
            self.core.open_today(
                id.clone(),
                Tab::new(url, now()),
                window,
                after.as_ref(),
                activate
            ),
            "could not open tab"
        );
        Ok(id)
    }

    pub fn peek(
        &mut self,
        window: WindowId,
        url: String,
        source: Option<&TabId>,
    ) -> Result<Option<TabId>> {
        let space = self.origin_space(&window, source)?.clone();
        let partition = source
            .and_then(|id| self.partition(id))
            .unwrap_or_else(|| self.space_partition(&space));
        let transient = Transient {
            id: tab_id(),
            tab: Tab::new(url, now()),
            space,
            partition,
        };
        Ok(self.peeks.insert(window, transient).map(|old| old.id))
    }

    pub fn promote(&mut self, window: &WindowId) -> Result<TabId> {
        let page = self.peeks.remove(window).context("no Peek to promote")?;
        let after = self.active(window).cloned();
        self.adopt(window, page, after.as_ref())
    }

    pub fn detach(&mut self, window: &WindowId, source: &TabId) -> Result<Detachment> {
        let mut tab = self.core.tab(source).context("no tab to move")?.clone();
        let space = self.origin_space(window, Some(source))?.clone();
        let partition = self.partition(source).context("tab has no Space")?;
        let id = window_id();
        let change = self.core.close_tab(source, now());
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

    pub fn return_page(&mut self, window: &WindowId) -> Result<(WindowId, TabId)> {
        let destination = self.core.focused_window().id.clone();
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
        ensure!(
            self.core
                .open_today(page.id.clone(), page.tab, window, after, true),
            "could not adopt page"
        );
        Ok(page.id)
    }

    pub fn partition(&self, id: &TabId) -> Option<Partition> {
        if let Some(transient) = self.transient(id) {
            return Some(transient.partition.clone());
        }
        Some(match self.core.locate(id)? {
            Placement::Favorite => Partition::Shared,
            Placement::Pinned { space, .. } | Placement::Today { space, .. } => {
                self.space_partition(space)
            }
        })
    }

    fn origin_space(&self, window: &WindowId, source: Option<&TabId>) -> Result<&SpaceId> {
        match source.and_then(|tab| self.core.locate(tab)) {
            Some(Placement::Pinned { space, .. } | Placement::Today { space, .. }) => Ok(space),
            Some(Placement::Favorite) | None => self
                .core
                .window(window)
                .map(|window| &window.active_space_id)
                .context("missing Flecha window"),
        }
    }

    pub fn close_space_transients(&mut self, space: &SpaceId) -> Vec<TabId> {
        self.peeks
            .extract_if(.., |_, page| &page.space == space)
            .chain(
                self.flechinhas
                    .extract_if(.., |_, page| &page.space == space),
            )
            .map(|(_, page)| page.id)
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
            .find(|page| &page.id == id)
    }

    pub fn tab_mut(&mut self, id: &TabId) -> Option<(&mut Tab, bool)> {
        self.flechinhas
            .values_mut()
            .chain(self.peeks.values_mut())
            .find(|page| &page.id == id)
            .map(|page| (&mut page.tab, false))
            .or_else(|| self.core.tab_mut(id).map(|tab| (tab, true)))
    }

    pub fn page(&self, owner: &Owner, id: &TabId) -> Option<&Tab> {
        match owner {
            Owner::Tab(window) => self
                .core
                .window(window)
                .filter(|window| window.visible_tabs().contains(id) && self.transient(id).is_none())
                .and_then(|_| self.core.tab(id)),
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
