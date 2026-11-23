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
    pub partition: Partition,
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

    pub fn open(&mut self, window: &WindowId, url: String, activate: bool) -> Result<TabId> {
        let id = tab_id();
        let after = self.active(window).cloned();
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
        ensure!(
            self.core.window(&window).is_some(),
            "Peek requires a Flecha window"
        );
        let transient = Transient {
            id: tab_id(),
            tab: Tab::new(url, now()),
            partition: source
                .and_then(|id| self.partition(id))
                .unwrap_or(Partition::Shared),
        };
        Ok(self.peeks.insert(window, transient).map(|old| old.id))
    }

    pub fn promote(&mut self, window: &WindowId) -> Result<()> {
        let page = self.peeks.remove(window).context("no Peek to promote")?;
        self.adopt(window, page)
    }

    pub fn to_flechinha(&mut self, source: &TabId) -> Result<Vec<TabId>> {
        let mut tab = self.core.tab(source).context("no tab to move")?.clone();
        let partition = self.partition(source).context("tab has no Space")?;
        let id = window_id();
        let change = self.core.close_tab(source, now());
        tab.agent = false;
        let page = Transient {
            id: source.clone(),
            tab,
            partition,
        };
        self.flechinhas.insert(id.clone(), page);
        Ok(change
            .removed
            .into_iter()
            .filter(|id| id != source)
            .collect())
    }

    pub fn from_flechinha(&mut self, window: &WindowId) -> Result<WindowId> {
        let destination = self.core.focused_window().id.clone();
        let page = self
            .flechinhas
            .remove(window)
            .context("no Flechinha to return")?;
        self.adopt(&destination, page)?;
        Ok(destination)
    }

    fn adopt(&mut self, window: &WindowId, mut page: Transient) -> Result<()> {
        let after = self.active(window).cloned();
        // A kept pin can still own the original durable ID.
        if self.core.tab(&page.id).is_some() {
            page.id = tab_id();
        }
        page.tab.home_url = None;
        page.tab.last_active = Some(now() as f64);
        ensure!(
            self.core
                .open_today(page.id, page.tab, window, after.as_ref(), true),
            "could not adopt page"
        );
        Ok(())
    }

    pub fn partition(&self, id: &TabId) -> Option<Partition> {
        if let Some(transient) = self.transient(id) {
            return Some(transient.partition.clone());
        }
        let space = match self.core.locate(id)? {
            Placement::Favorite => None,
            Placement::Pinned { space, .. } | Placement::Today { space, .. } => Some(space),
        };
        Some(
            match space.filter(|space| self.core.space(space).is_some_and(|space| space.isolated)) {
                Some(space) => Partition::Isolated(space.clone()),
                None => Partition::Shared,
            },
        )
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
