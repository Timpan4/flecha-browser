//! Tab sleep: flecha-core's policy decides, the engine freezes and discards.
//!
//! A frozen tab stays loaded but hidden, muted and in the frozen page lifecycle state. A
//! discarded tab has no CEF browser; the engine keeps its page so the shell can load it again
//! when the tab is shown.

use flecha_core::sleep::{SleepState, SleepTab, protected_recent, sleep_state};
use flecha_core::{EpochMillis, TabId};

use cef::ImplBrowserHost as _;

use crate::{Owner, Partition};

/// One tab's sleep transition from [`crate::Engine::evaluate_sleep`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SleepChange<'a> {
    pub tab: &'a TabId,
    pub from: SleepState,
    pub to: SleepState,
}

/// What a discarded tab needs to load again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discarded {
    pub url: String,
    pub owner: Owner,
    pub partition: Partition,
}

impl crate::Engine {
    /// Runs the sleep policy over `tabs` and applies its transitions: freezing, discarding,
    /// and thawing a frozen tab. A discarded tab the policy wakes is reported but stays
    /// discarded until the shell calls [`Self::restore_tab`] with its viewport.
    pub fn evaluate_sleep<'a>(
        &mut self,
        now: EpochMillis,
        active_use: EpochMillis,
        tabs: &'a [(TabId, SleepTab<'_>)],
    ) -> Vec<SleepChange<'a>> {
        let changes = plan(now, active_use, tabs);
        for change in &changes {
            match (change.from, change.to) {
                (_, SleepState::Frozen) => {
                    self.freeze_tab(change.tab);
                }
                (_, SleepState::Discarded) => {
                    self.discard_tab(change.tab);
                }
                (SleepState::Frozen, SleepState::Alive) => {
                    self.thaw_tab(change.tab);
                }
                (_, SleepState::Alive) => {}
            }
        }
        changes
    }

    /// Hides, mutes and freezes the page's lifecycle. Returns whether the tab has a browser.
    pub fn freeze_tab(&mut self, tab: &TabId) -> bool {
        let Some(session) = self.browsers.get(tab) else {
            return false;
        };
        if session.frozen {
            return true;
        }
        self.set_visible(tab, false);
        if let Some(host) = &self.host(tab) {
            host.set_audio_muted(1);
            let slot = self.browsers[tab].slot.clone();
            let (method, params) = lifecycle(true);
            // A page that refuses the lifecycle change stays hidden and muted.
            let _ = crate::devtools::send(
                &slot,
                host,
                method,
                Some(&params),
                Some(crate::devtools::Internal::Ignore),
            );
        }
        if let Some(session) = self.browsers.get_mut(tab) {
            session.frozen = true;
        }
        true
    }

    /// Undoes [`Self::freeze_tab`]. The tab stays hidden until it is shown, and its audio
    /// returns to the tab's own mute, which may have changed while it slept.
    pub fn thaw_tab(&mut self, tab: &TabId) -> bool {
        let Some(session) = self.browsers.get_mut(tab) else {
            return false;
        };
        if !std::mem::take(&mut session.frozen) {
            return false;
        }
        let slot = session.slot.clone();
        let muted = session.muted;
        if let Some(host) = self.host(tab) {
            let (method, params) = lifecycle(false);
            let _ = crate::devtools::send(
                &slot,
                &host,
                method,
                Some(&params),
                Some(crate::devtools::Internal::Ignore),
            );
            host.set_audio_muted(i32::from(muted));
        }
        true
    }

    pub fn is_frozen(&self, tab: &TabId) -> bool {
        self.browsers.get(tab).is_some_and(|session| session.frozen)
    }

    /// Mutes or unmutes the tab's audio. A frozen browser stays silent and takes the setting when
    /// it thaws. Returns whether the tab has a browser.
    pub fn set_audio_muted(&mut self, tab: &TabId, muted: bool) -> bool {
        let Some(session) = self.browsers.get_mut(tab) else {
            return false;
        };
        session.muted = muted;
        if !session.frozen
            && let Some(host) = self.host(tab)
        {
            host.set_audio_muted(i32::from(muted));
        }
        true
    }

    /// Closes the tab's browser and keeps its page, owner and partition for
    /// [`Self::restore_tab`]. `None` when the tab has no loaded page.
    pub fn discard_tab(&mut self, tab: &TabId) -> Option<Discarded> {
        let url = self.url(tab).filter(|url| !url.is_empty())?;
        let session = self.browsers.get(tab)?;
        let discarded = Discarded {
            url,
            owner: session.owner.clone(),
            partition: session.partition.clone(),
        };
        self.close(tab);
        self.discarded.insert(tab.clone(), discarded.clone());
        Some(discarded)
    }

    pub fn discarded(&self, tab: &TabId) -> Option<&Discarded> {
        self.discarded.get(tab)
    }

    /// Loads a discarded tab's page again. Returns `false` for a tab that wasn't discarded.
    /// The record stays until the new browser exists, so a request context that isn't ready
    /// yet can be retried.
    pub fn restore_tab(&mut self, tab: &TabId, viewport: crate::Viewport) -> anyhow::Result<bool> {
        let Some(discarded) = self.discarded.get(tab).cloned() else {
            return Ok(false);
        };
        self.ensure(
            tab.clone(),
            discarded.owner,
            discarded.partition,
            &discarded.url,
            viewport,
        )?;
        Ok(true)
    }
}

/// The policy's transitions for `tabs`. The ten most recently active tabs are protected from
/// discarding; a tab with no activity time is never among them.
pub fn plan<'a>(
    now: EpochMillis,
    active_use: EpochMillis,
    tabs: &'a [(TabId, SleepTab<'_>)],
) -> Vec<SleepChange<'a>> {
    let protected = protected_recent(
        tabs.iter()
            .filter_map(|(tab, sleep)| Some((tab, sleep.last_active?))),
    );
    tabs.iter()
        .filter_map(|(tab, sleep)| {
            let to = sleep_state(*sleep, now, active_use, protected.contains(tab));
            (to != sleep.state).then_some(SleepChange {
                tab,
                from: sleep.state,
                to,
            })
        })
        .collect()
}

/// The CDP call that moves a page in or out of the frozen lifecycle state.
pub(crate) fn lifecycle(frozen: bool) -> (&'static str, String) {
    let state = if frozen { "frozen" } else { "active" };
    (
        "Page.setWebLifecycleState",
        serde_json::json!({ "state": state }).to_string(),
    )
}

#[cfg(test)]
mod tests {
    use flecha_core::sleep::{DISCARD_AFTER, FREEZE_AFTER, PROTECTED_RECENT, STATE_DEBOUNCE};

    use super::*;

    const NOW: EpochMillis = 10 * DISCARD_AFTER;

    fn idle(minutes: EpochMillis) -> SleepTab<'static> {
        SleepTab {
            url: Some("https://news.example/"),
            last_active: Some(NOW - minutes * 60_000),
            used_at: Some(NOW - minutes * 60_000),
            ..SleepTab::default()
        }
    }

    #[test]
    fn idle_tabs_freeze_then_discard_and_shown_tabs_wake() {
        let tabs = vec![
            (TabId::new("fresh"), idle(1)),
            (TabId::new("idle"), idle(FREEZE_AFTER / 60_000)),
            (
                TabId::new("shown"),
                SleepTab {
                    shown: true,
                    state: SleepState::Frozen,
                    ..idle(FREEZE_AFTER / 60_000)
                },
            ),
            (
                TabId::new("playing"),
                SleepTab {
                    media: true,
                    ..idle(DISCARD_AFTER / 60_000)
                },
            ),
            (
                TabId::new("just frozen"),
                SleepTab {
                    state: SleepState::Frozen,
                    changed_at: Some(NOW - STATE_DEBOUNCE / 2),
                    ..idle(DISCARD_AFTER / 60_000)
                },
            ),
        ];
        let changes = plan(NOW, NOW, &tabs);
        let summary: Vec<_> = changes
            .iter()
            .map(|change| (change.tab.as_str(), change.from, change.to))
            .collect();
        assert_eq!(
            summary,
            [
                ("idle", SleepState::Alive, SleepState::Frozen),
                ("shown", SleepState::Frozen, SleepState::Alive),
            ]
        );
    }

    #[test]
    fn the_most_recent_tabs_freeze_instead_of_discarding() {
        let mut tabs: Vec<_> = (0..=PROTECTED_RECENT)
            .map(|index| {
                let minutes = DISCARD_AFTER / 60_000 + index as EpochMillis;
                (TabId::new(format!("tab-{index}")), idle(minutes))
            })
            .collect();
        tabs.push((
            TabId::new("never active"),
            SleepTab {
                last_active: None,
                ..idle(DISCARD_AFTER / 60_000)
            },
        ));
        let changes = plan(NOW, NOW, &tabs);
        let discarded: Vec<_> = changes
            .iter()
            .filter(|change| change.to == SleepState::Discarded)
            .map(|change| change.tab.as_str())
            .collect();
        // The oldest tab and the one with no activity time are the only unprotected ones.
        assert_eq!(
            discarded,
            [format!("tab-{PROTECTED_RECENT}").as_str(), "never active"]
        );
        assert_eq!(changes.len(), tabs.len());
    }

    #[test]
    fn freezing_uses_the_page_lifecycle_cdp_call() {
        assert_eq!(
            lifecycle(true),
            ("Page.setWebLifecycleState", r#"{"state":"frozen"}"#.into())
        );
        assert_eq!(lifecycle(false).1, r#"{"state":"active"}"#);
    }
}
