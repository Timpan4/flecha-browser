use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use flecha_cef::{Discarded, Engine, Owner, Partition, Viewport};
use flecha_core::{
    EpochMillis, Placement, TabId, WindowId,
    sleep::{EVALUATE_EVERY, SleepState, SleepTab},
};

use crate::{Controller, model::now};

const INTERVAL: Duration = Duration::from_millis(EVALUATE_EVERY.unsigned_abs());

pub(crate) struct TabSleep {
    last_evaluation: Instant,
    sampled_at: Instant,
    active_use: Duration,
    active_window: Option<WindowId>,
    tabs: BTreeMap<TabId, Activity>,
}

#[derive(Default)]
struct Activity {
    shown: bool,
    used_at: EpochMillis,
    changed_at: Option<EpochMillis>,
}

impl TabSleep {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            last_evaluation: now,
            sampled_at: now,
            active_use: Duration::ZERO,
            active_window: None,
            tabs: BTreeMap::new(),
        }
    }

    fn due(&mut self, now: Instant, verify: bool) -> bool {
        if verify || now.duration_since(self.last_evaluation) < INTERVAL {
            return false;
        }
        self.last_evaluation = now;
        true
    }

    fn sample(&mut self, now: Instant) -> EpochMillis {
        if self.active_window.is_some() {
            self.active_use += now.duration_since(self.sampled_at);
        }
        self.sampled_at = now;
        EpochMillis::try_from(self.active_use.as_millis()).unwrap_or(EpochMillis::MAX)
    }

    pub(crate) fn window_activation(&mut self, window: &WindowId, active: bool) {
        self.sample(Instant::now());
        if active {
            self.active_window = Some(window.clone());
        } else if self.active_window.as_ref() == Some(window) {
            self.active_window = None;
        }
    }

    pub(crate) fn clear_pages(&mut self, engine: &mut Engine) {
        for (id, _) in std::mem::take(&mut self.tabs) {
            if engine.discarded(&id).is_some() {
                engine.close(&id);
            }
        }
    }
}

impl Controller {
    pub(crate) fn note_shown_tabs(&mut self, instant: Instant) -> EpochMillis {
        if self.verify {
            return 0;
        }
        let active_use = self.sleep.sample(instant);
        for activity in self.sleep.tabs.values_mut() {
            if std::mem::take(&mut activity.shown) {
                activity.used_at = active_use;
            }
        }
        for window in self.model.core.windows() {
            for tab in window.visible_tabs() {
                let activity = self.sleep.tabs.entry(tab.clone()).or_default();
                activity.shown = true;
                activity.used_at = active_use;
            }
        }
        active_use
    }

    pub(crate) fn poll_sleep(&mut self, instant: Instant) -> bool {
        if !self.sleep.due(instant, self.verify) {
            return false;
        }
        let active_use = self.note_shown_tabs(instant);
        let now = now();
        let core = &self.model.core;
        let mut tabs = Vec::new();
        for id in self.engine.ids() {
            let Some(tab) = core
                .tab(id)
                .filter(|_| matches!(self.engine.owner(id), Some(Owner::Tab(_))))
            else {
                continue;
            };
            let activity = self
                .sleep
                .tabs
                .entry(id.clone())
                .or_insert_with(|| Activity {
                    used_at: active_use,
                    ..Activity::default()
                });
            let space = match core.locate(id) {
                Some(Placement::Today { space, .. } | Placement::Pinned { space, .. }) => {
                    Some(space)
                }
                _ => None,
            };
            let in_hidden_space = space.is_some_and(|space| {
                core.windows()
                    .iter()
                    .all(|window| &window.active_space_id != space)
            });
            tabs.push((
                id.clone(),
                SleepTab {
                    url: Some(&tab.url),
                    last_active: tab.last_active.map(|time| time as EpochMillis),
                    used_at: Some(activity.used_at),
                    shown: activity.shown
                        || self.engine.is_loading(id)
                        || self.downloads.uses_tab(id)
                        || self.engine.pending_permission_prompt(id).is_some(),
                    in_hidden_space,
                    state: if self.engine.is_frozen(id) {
                        SleepState::Frozen
                    } else {
                        SleepState::Alive
                    },
                    changed_at: activity.changed_at,
                    ..SleepTab::default()
                },
            ));
        }
        let changes = self.engine.evaluate_sleep(now, active_use, &tabs);
        let mut changed = false;
        for change in changes {
            let applied = match change.to {
                SleepState::Frozen => self.engine.is_frozen(change.tab),
                SleepState::Discarded => self.engine.discarded(change.tab).is_some(),
                SleepState::Alive => !self.engine.is_frozen(change.tab),
            };
            if applied {
                if let Some(activity) = self.sleep.tabs.get_mut(change.tab) {
                    activity.changed_at = Some(now);
                }
                changed = true;
            }
        }
        self.sleep.tabs.retain(|id, _| core.tab(id).is_some());
        changed
    }
}

pub(crate) fn ensure_visible(
    engine: &mut Engine,
    tab: &TabId,
    owner: Owner,
    partition: Partition,
    url: &str,
    viewport: Viewport,
) -> Result<()> {
    if engine
        .discarded(tab)
        .is_some_and(|record| same_page(record, &owner, &partition, url))
    {
        engine.restore_tab(tab, viewport)?;
    } else {
        engine.ensure(tab.clone(), owner, partition, url, viewport)?;
    }
    Ok(())
}

fn same_page(record: &Discarded, owner: &Owner, partition: &Partition, url: &str) -> bool {
    &record.owner == owner && &record.partition == partition && record.url == url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_uses_policy_interval_and_skips_verification() {
        let start = Instant::now();
        let mut sleep = TabSleep::new(start);
        let due = start + INTERVAL;
        assert!(!sleep.due(start, false));
        assert!(!sleep.due(due, true));
        assert!(sleep.due(due, false));
        assert!(!sleep.due(due, false));
        assert!(sleep.due(due + INTERVAL, false));
    }

    #[test]
    fn restore_reuses_only_the_discarded_record_for_the_current_pane() {
        let owner = Owner::Tab(WindowId::new("window"));
        let url = "https://example.org/";
        let record = Discarded {
            url: url.into(),
            owner: owner.clone(),
            partition: Partition::Shared,
        };
        assert!(same_page(&record, &owner, &Partition::Shared, url));
        assert!(!same_page(
            &record,
            &owner,
            &Partition::Shared,
            "https://example.net/"
        ));
        assert!(!same_page(
            &record,
            &Owner::Tab(WindowId::new("other")),
            &Partition::Shared,
            url
        ));
    }
}
