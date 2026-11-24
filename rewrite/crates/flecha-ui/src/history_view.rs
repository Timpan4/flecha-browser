use std::{collections::BTreeMap, path::Path};

use anyhow::Context as _;
use cef::{ImplBrowser as _, ImplBrowserHost as _, ImplNavigationEntry as _};
use chrono::{DateTime, Local, NaiveDate};
use flecha_cef::{BrowserKey, Partition};
use flecha_core::{EpochMillis, TabId, suggestions::display_url};
use flecha_history::{HistoryEntry, HistoryError, HistoryStore, PAGE, RecordedVisit};
use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, ParentElement as _, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};

use crate::text_field::TextField;
use crate::theme::palette;
use crate::view::{Overlay, Shell, button, overlay_panel};
use crate::{Action, Controller, dispatch, now};

pub(crate) struct History {
    store: Option<HistoryStore>,
    open_error: Option<HistoryError>,
    revision: u64,
    pending: BTreeMap<TabId, PendingVisit>,
    recorded: BTreeMap<TabId, (BrowserKey, RecordedVisit)>,
}

struct PendingVisit {
    key: BrowserKey,
    url: String,
    at: EpochMillis,
    loaded: bool,
}

impl History {
    pub(crate) fn open(profile: &Path) -> Self {
        Self::from_open(HistoryStore::open(profile.join("history.sqlite")))
    }

    fn from_open(result: Result<HistoryStore, HistoryError>) -> Self {
        let (store, open_error) = match result {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            store,
            open_error,
            revision: 0,
            pending: BTreeMap::new(),
            recorded: BTreeMap::new(),
        }
    }

    pub(crate) fn store(&self) -> Option<&HistoryStore> {
        self.store.as_ref()
    }

    fn writable_store(&mut self) -> anyhow::Result<&mut HistoryStore> {
        self.store
            .as_mut()
            .context("History storage is unavailable")
    }

    pub(crate) fn record(
        &mut self,
        durable: bool,
        partition: Option<&Partition>,
        url: &str,
        title: &str,
        visited_at: EpochMillis,
    ) -> Result<Option<RecordedVisit>, HistoryError> {
        // Shared and isolated Space partitions are both persistent. A transient
        // browser or a future non-persistent partition must never write history.
        if durable
            && matches!(partition, Some(Partition::Shared | Partition::Isolated(_)))
            && let Some(store) = self.store.as_mut()
            && let Some(visit) = store.record_navigation(url, title, visited_at)?
        {
            self.revision += 1;
            return Ok(Some(visit));
        }
        Ok(None)
    }

    pub(crate) fn begin(&mut self, key: &BrowserKey, address: &str) {
        if self.store.is_none() {
            return;
        }
        self.recorded.remove(&key.tab);
        self.pending.insert(
            key.tab.clone(),
            PendingVisit {
                key: key.clone(),
                url: address.into(),
                at: now(),
                loaded: false,
            },
        );
    }

    pub(crate) fn loaded(&mut self, id: &TabId, url: &str) {
        if let Some(visit) = self.pending.get_mut(id)
            && visit.url == url
        {
            visit.loaded = true;
        }
    }

    pub(crate) fn failed(&mut self, id: &TabId, url: &str) {
        if self.pending.get(id).is_some_and(|visit| visit.url == url) {
            self.pending.remove(id);
        }
    }

    pub(crate) fn forget(&mut self, id: &TabId) {
        self.pending.remove(id);
        self.recorded.remove(id);
    }

    pub(crate) fn title(&mut self, key: &BrowserKey, title: &str) -> Result<(), HistoryError> {
        if let Some((browser, visit)) = self.recorded.get(&key.tab)
            && browser == key
            && let Some(store) = self.store.as_mut()
            && store.update_visit_title(visit, title)?
        {
            self.revision += 1;
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, url: &str) -> anyhow::Result<()> {
        if self.writable_store()?.remove_url(url)? {
            self.recorded.clear();
            self.revision += 1;
        }
        Ok(())
    }

    pub(crate) fn clear(&mut self, range: ClearRange, at: EpochMillis) -> anyhow::Result<()> {
        let store = self.writable_store()?;
        match range.hours() {
            Some(hours) => store.clear_since(at.saturating_sub(hours * 3_600_000))?,
            None => store.clear_all()?,
        }
        self.pending.clear();
        self.recorded.clear();
        self.revision += 1;
        Ok(())
    }
}

impl Controller {
    pub(crate) fn finish_history(&mut self) -> bool {
        self.history
            .pending
            .retain(|id, visit| self.engine.key(id).as_ref() == Some(&visit.key));
        let finished: Vec<_> = self
            .history
            .pending
            .iter()
            .filter(|(id, visit)| {
                visit.loaded
                    || self
                        .engine
                        .host(id)
                        .and_then(|host| host.browser())
                        .is_some_and(|browser| browser.is_loading() == 0)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut changed = false;
        for id in finished {
            let Some(visit) = self.history.pending.remove(&id) else {
                continue;
            };
            let Some(tab) = self.model.core.tab(&id) else {
                continue;
            };
            let title = self
                .engine
                .host(&id)
                .and_then(|host| host.visible_navigation_entry())
                .filter(|entry| cef::CefString::from(&entry.url()).to_string() == visit.url)
                .map(|entry| cef::CefString::from(&entry.title()).to_string())
                .filter(|title| !title.is_empty());
            let title = title.as_deref().unwrap_or(if tab.url == visit.url {
                &tab.title
            } else {
                &visit.url
            });
            match self.history.record(
                true,
                self.engine.partition(&id),
                &visit.url,
                title,
                visit.at,
            ) {
                Ok(Some(saved)) => {
                    self.history.recorded.insert(id, (visit.key, saved));
                    changed = true;
                }
                Ok(None) => {}
                Err(error) => {
                    self.status.set(None, error.to_string());
                    changed = true;
                }
            }
        }
        changed
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ClearRange {
    Hour,
    Day,
    Week,
    FourWeeks,
    All,
}

impl ClearRange {
    fn hours(self) -> Option<i64> {
        Some(match self {
            Self::Hour => 1,
            Self::Day => 24,
            Self::Week => 7 * 24,
            Self::FourWeeks => 4 * 7 * 24,
            Self::All => return None,
        })
    }
}

#[derive(Default)]
pub(crate) struct HistoryPanel {
    input: TextField,
    entries: Vec<HistoryEntry>,
    revision: Option<u64>,
    exhausted: bool,
    error: Option<String>,
    scroll: ScrollHandle,
}

impl HistoryPanel {
    pub(crate) fn refresh(&mut self, history: &History) {
        if self.revision != Some(history.revision) {
            self.entries.clear();
            self.load(history);
            self.revision = Some(history.revision);
        }
    }

    fn load(&mut self, history: &History) {
        let Some(store) = history.store() else {
            self.exhausted = true;
            return;
        };
        match store.query(self.input.text().trim(), self.entries.len()) {
            Ok(page) => {
                self.entries.extend(page.entries);
                self.exhausted = page.exhausted;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.exhausted = true;
            }
        }
    }
}

fn local_date(timestamp: EpochMillis) -> Option<NaiveDate> {
    Some(
        DateTime::from_timestamp_millis(timestamp)?
            .with_timezone(&Local)
            .date_naive(),
    )
}

fn day_label(day: Option<NaiveDate>, today: NaiveDate) -> String {
    match day {
        Some(day) if day == today => "Today".into(),
        Some(day) if Some(day) == today.pred_opt() => "Yesterday".into(),
        Some(day) => day.format("%b %-d, %Y").to_string(),
        None => "Unknown date".into(),
    }
}

impl Shell {
    pub(crate) fn toggle_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::History(_)) {
            self.dismiss_overlay(window, cx);
        } else if self.state.borrow().model.core.window(&self.id).is_some() {
            self.show_overlay(Overlay::History(HistoryPanel::default()), window, cx);
        }
    }

    pub(crate) fn toggle_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Archive(_)) {
            self.dismiss_overlay(window, cx);
        } else {
            self.show_overlay(Overlay::Archive(TextField::default()), window, cx);
        }
    }

    pub(crate) fn drawer_key(&mut self, stroke: &gpui::Keystroke, cx: &mut Context<Self>) {
        match &mut self.overlay {
            Overlay::History(panel) if self.state.borrow().history.store().is_some() => {
                let previous = panel.input.text().to_owned();
                panel.input.key(stroke, cx);
                if panel.input.text() != previous {
                    panel.revision = None;
                    panel.scroll.scroll_to_item(0);
                }
            }
            Overlay::Archive(input) => input.key(stroke, cx),
            _ => return,
        }
        cx.notify();
    }

    fn drawer_heading(&self, title: &'static str, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(title)
            .child(button("close-overlay", "Esc").on_click(cx.listener(
                |this, _: &ClickEvent, window, cx| {
                    this.dismiss_overlay(window, cx);
                },
            )))
    }

    pub(crate) fn history_panel(&self, panel: &HistoryPanel, cx: &Context<Self>) -> Div {
        if let Some(error) = &self.state.borrow().history.open_error {
            return overlay_panel()
                .child(self.drawer_heading("History", cx))
                .child(
                    div()
                        .text_sm()
                        .child(format!("History is unavailable. {error}")),
                );
        }
        let today = Local::now().date_naive();
        let mut previous = None;
        let mut list = div()
            .id("history-list")
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&panel.scroll);
        for entry in &panel.entries {
            let day = local_date(entry.last_visit_ms);
            if previous != Some(day) {
                list = list.child(
                    div()
                        .mt_2()
                        .px_2()
                        .text_xs()
                        .text_color(palette().text_2)
                        .child(day_label(day, today)),
                );
                previous = Some(day);
            }
            let url = entry.url.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        entry_link(format!("history-{url}"), &entry.title, &url).on_click(
                            cx.listener({
                                let url = url.clone();
                                move |this, _: &ClickEvent, window, cx| {
                                    this.navigate_from_overlay(
                                        Action::Open(url.clone()),
                                        window,
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )
                    .child(
                        button(format!("delete-{url}"), "Delete")
                            .text_xs()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                dispatch(
                                    &this.state,
                                    &this.id,
                                    Action::DeleteHistory(url.clone()),
                                    cx,
                                );
                            })),
                    ),
            );
        }
        if !panel.exhausted {
            list = list.child(button("load-history", "Load more").on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if let Overlay::History(panel) = &mut this.overlay {
                        let state = this.state.borrow();
                        if panel.revision == Some(state.history.revision)
                            && panel.entries.len().is_multiple_of(PAGE)
                        {
                            panel.load(&state.history);
                        }
                        cx.notify();
                    }
                },
            )));
        } else if panel.entries.is_empty() && panel.error.is_none() {
            list = list.child(div().p_2().text_sm().child("No matching history"));
        }

        let mut clear = div().flex().flex_wrap().gap_1().text_xs();
        for (label, range) in [
            ("Last hour", ClearRange::Hour),
            ("24 hours", ClearRange::Day),
            ("7 days", ClearRange::Week),
            ("4 weeks", ClearRange::FourWeeks),
            ("All time", ClearRange::All),
        ] {
            clear = clear.child(self.action_button(label, label, Action::ClearHistory(range), cx));
        }
        overlay_panel()
            .child(self.drawer_heading("History", cx))
            .child(panel.input.render("Search history"))
            .when_some(panel.error.clone(), |panel, error| panel.child(error))
            .child(list)
            .child(
                div()
                    .border_t_1()
                    .border_color(palette().surface_2)
                    .pt_2()
                    .child(div().text_sm().child("Clear browsing data"))
                    .child(clear),
            )
    }

    pub(crate) fn archive_panel(&self, input: &TextField, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let entries = state.model.core.search_archive(input.text());
        let mut list = div().id("archive-list").min_h_0().overflow_y_scroll();
        if entries.is_empty() {
            list = list.child(div().p_2().text_sm().child("No matching archived tabs"));
        }
        for (index, entry) in entries.into_iter().enumerate() {
            let archived = entry.clone();
            list = list.child(
                entry_link(("archive", index), &entry.title, &entry.url)
                    .child(div().text_xs().child("Restore"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.navigate_from_overlay(
                            Action::RestoreArchive(archived.clone()),
                            window,
                            cx,
                        );
                    })),
            );
        }
        overlay_panel()
            .child(self.drawer_heading("Archive", cx))
            .child(input.render("Search archived tabs"))
            .child(list)
            .when(!state.model.core.archive().is_empty(), |panel| {
                panel.child(self.action_button(
                    "clear-archive",
                    "Clear archive",
                    Action::ClearArchive,
                    cx,
                ))
            })
    }
}

fn entry_link(id: impl Into<gpui::ElementId>, title: &str, url: &str) -> gpui::Stateful<Div> {
    let address = display_url(url);
    let title = if title.is_empty() || title == url {
        &address
    } else {
        title
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .flex_1()
        .min_w_0()
        .px_2()
        .py_1()
        .cursor_pointer()
        .rounded_md()
        .hover(|row| row.bg(palette().surface_2))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(title.to_owned()))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(palette().text_2)
                        .child(address),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_records_persistent_http_visits_and_supports_search_and_deletion() {
        let mut history = History::from_open(HistoryStore::in_memory());
        let shared = Some(&Partition::Shared);
        let current = 4 * 7 * 24 * 3_600_000;
        let recent = "https://recent.example/";
        let old = "http://old.example/";
        history
            .record(true, shared, recent, "Shared project", current)
            .unwrap();
        history
            .record(
                true,
                Some(&Partition::Isolated(flecha_core::SpaceId::new("work"))),
                old,
                "Separate project",
                0,
            )
            .unwrap();
        history
            .record(
                false,
                shared,
                "https://transient.example/",
                "project",
                current,
            )
            .unwrap();
        history
            .record(true, None, "https://ephemeral.example/", "project", current)
            .unwrap();
        history
            .record(true, shared, "about:blank", "project", current)
            .unwrap();
        let hits = history
            .store()
            .unwrap()
            .query("PROJECT", 0)
            .unwrap()
            .entries;
        assert_eq!(
            hits.iter().map(|hit| hit.url.as_str()).collect::<Vec<_>>(),
            [recent, old]
        );
        assert_eq!(hits[0].visit_count, 1);
        history.clear(ClearRange::Hour, current).unwrap();
        assert_eq!(
            history.store().unwrap().query("", 0).unwrap().entries[0].url,
            old
        );
        history.remove(old).unwrap();
        assert!(
            history
                .store()
                .unwrap()
                .query("", 0)
                .unwrap()
                .entries
                .is_empty()
        );
        history
            .record(true, shared, recent, "Shared project", current)
            .unwrap();
        history.clear(ClearRange::All, current).unwrap();
        assert!(
            history
                .store()
                .unwrap()
                .query("", 0)
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn unreadable_disk_history_exposes_its_error_and_rejects_mutations() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("history.sqlite");
        let original = b"Not a SQLite database";
        std::fs::write(&path, original).unwrap();
        let mut history = History::open(profile.path());
        assert!(history.store().is_none());
        assert!(history.open_error.is_some());
        let url = "https://example.test/";
        assert!(
            history
                .record(true, Some(&Partition::Shared), url, "Project", 0)
                .unwrap()
                .is_none()
        );
        assert!(history.remove(url).is_err());
        assert!(history.clear(ClearRange::All, 0).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}
