use std::cmp::Reverse;
use std::collections::BTreeSet;

use flecha_core::{
    BrowserState, Tab, TabId, WindowId,
    suggestions::{self, InputResolution, MatchQuality, SuggestionAction},
};
use flecha_history::{HistoryStore, PAGE};
use gpui::{
    ClickEvent, ClipboardItem, Context, Div, InteractiveElement as _, ParentElement as _,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, rgb,
};

use crate::text_field::TextField;
use crate::view::{Overlay, Shell, button, overlay_panel};
use crate::{Action, now};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    NewTab,
    EditLocation,
    Split,
    Peek,
}

impl Command {
    fn label(self) -> &'static str {
        match self {
            Self::NewTab => "New tab",
            Self::EditLocation => "Edit address",
            Self::Split => "Split with a new tab",
            Self::Peek => "Open Peek",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Tab(TabId, String),
    Address(String),
    Search(String),
    Copy(String),
}

struct Suggestion {
    target: Target,
    label: String,
    detail: String,
    kind: &'static str,
}

pub(crate) struct CommandBar {
    mode: Command,
    input: TextField,
    rows: Vec<Suggestion>,
    selected: usize,
    scroll: ScrollHandle,
}

#[derive(Debug, Eq, PartialEq)]
enum Key {
    Up,
    Down,
    Submit,
    Dismiss,
    Edit,
}

fn command_key(stroke: &gpui::Keystroke) -> Key {
    if stroke.modifiers != gpui::Modifiers::none() {
        return Key::Edit;
    }
    match stroke.key.as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "enter" => Key::Submit,
        "escape" => Key::Dismiss,
        _ => Key::Edit,
    }
}

fn selection_after(index: usize, len: usize, key: Key) -> usize {
    if len == 0 {
        return 0;
    }
    match key {
        Key::Up if index == 0 => len - 1,
        Key::Up => index - 1,
        Key::Down => (index + 1) % len,
        _ => index,
    }
}

fn location<'a>(
    core: &'a BrowserState,
    window: &WindowId,
    mode: Command,
) -> Option<Option<&'a Tab>> {
    let window = core.window(window)?;
    let tab = window
        .spaces
        .get(&window.active_space_id)
        .and_then(|space| space.active_tab_id.as_ref())
        .and_then(|tab| core.tab(tab));
    Some(tab.filter(|_| mode == Command::EditLocation))
}

fn suggestions_for(
    core: &BrowserState,
    history: Option<&HistoryStore>,
    current_url: Option<&str>,
    text: &str,
) -> Vec<Suggestion> {
    let mut rows = Vec::new();
    let private = suggestions::contains_private_jwt(text);
    for row in
        suggestions::developer_suggestions(text, current_url, &core.settings.environments, now())
    {
        let target = match row.action {
            SuggestionAction::Copy(text) => Target::Copy(text),
            SuggestionAction::Open(url) if !private => Target::Address(url),
            SuggestionAction::Open(_) => continue,
        };
        rows.push(Suggestion {
            target,
            label: row.label,
            detail: row.detail,
            kind: "Local",
        });
    }
    if private {
        return rows;
    }
    let resolved = suggestions::resolve_input(text, core.settings.search_engine);
    if let InputResolution::Navigate(url) = &resolved {
        rows.push(Suggestion {
            target: Target::Address(url.clone()),
            label: suggestions::display_url(url),
            detail: String::new(),
            kind: "Go to",
        });
    }

    let mut ranked = Vec::new();
    let mut open_urls = BTreeSet::new();
    let empty = text.trim().is_empty();
    let favorites = core
        .favorites()
        .iter()
        .map(|id| (id, None, ("Favorite", 3)));
    let pinned = core.spaces().iter().flat_map(|space| {
        space
            .pinned_tabs()
            .into_iter()
            .map(move |id| (id, Some(space), ("Pinned", 2)))
    });
    let today = core
        .windows()
        .iter()
        .flat_map(|window| &window.spaces)
        .flat_map(|(space, view)| {
            let space = core.space(space);
            view.today
                .iter()
                .map(move |id| (id, space, ("Open tab", 1)))
        });
    for (id, space, (kind, rank)) in favorites.chain(pinned).chain(today) {
        let Some(tab) = core.tab(id) else { continue };
        let title = tab.custom_title.as_ref().unwrap_or(&tab.title);
        let address = suggestions::display_url(&tab.url);
        let quality = if empty {
            Some(MatchQuality::Contains)
        } else {
            suggestions::match_quality(text, &[title, &address])
        };
        let Some(quality) = quality else { continue };
        open_urls.insert(&tab.url);
        let detail = match space {
            Some(space) => format!("{} · {address}", space.name),
            None => address,
        };
        ranked.push((
            (quality, rank, 0, tab.last_active.unwrap_or_default() as i64),
            Suggestion {
                target: Target::Tab(id.clone(), tab.url.clone()),
                label: title.clone(),
                detail,
                kind,
            },
        ));
    }
    if !empty
        && let Some(history) = history
        && let Ok(entries) = history.suggest(text.trim(), PAGE)
    {
        for entry in entries {
            if open_urls.contains(&entry.url) {
                continue;
            }
            let address = suggestions::display_url(&entry.url);
            let quality = suggestions::match_quality(text, &[&entry.title, &address])
                .unwrap_or(MatchQuality::Contains);
            ranked.push((
                (quality, 0, entry.visit_count, entry.last_visit_ms),
                Suggestion {
                    target: Target::Address(entry.url),
                    label: entry.title,
                    detail: address,
                    kind: "History",
                },
            ));
        }
    }
    ranked.sort_by_key(|(rank, _)| Reverse(*rank));
    rows.extend(ranked.into_iter().map(|(_, row)| row));
    if let InputResolution::Search(url) = resolved {
        rows.push(Suggestion {
            target: Target::Search(url),
            label: text.trim().to_owned(),
            detail: core.settings.search_engine.name().into(),
            kind: "Search",
        });
    }
    rows
}

impl Shell {
    pub(crate) fn open_command(
        &mut self,
        mode: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let next = {
            let state = self.state.borrow();
            let Some(tab) = location(&state.model.core, &self.id, mode) else {
                return false;
            };
            let input = TextField::new(
                tab.map_or_else(String::new, |tab| tab.url.clone()),
                tab.is_some(),
            );
            let current = state
                .model
                .active(&self.id)
                .and_then(|id| state.model.core.tab(id));
            let rows = suggestions_for(
                &state.model.core,
                state.history.store(),
                current.map(|tab| tab.url.as_str()),
                input.text(),
            );
            CommandBar {
                mode,
                input,
                rows,
                selected: 0,
                scroll: ScrollHandle::new(),
            }
        };
        self.show_overlay(Overlay::Command(next), window, cx);
        true
    }

    pub(crate) fn command_key(
        &mut self,
        stroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Overlay::Command(bar) = &mut self.overlay else {
            return;
        };
        match command_key(stroke) {
            key @ (Key::Up | Key::Down) => {
                bar.selected = selection_after(bar.selected, bar.rows.len(), key);
                if !bar.rows.is_empty() {
                    bar.scroll.scroll_to_item(bar.selected);
                }
            }
            Key::Submit => {
                let selected = bar.selected;
                self.choose_suggestion(selected, window, cx);
                return;
            }
            Key::Dismiss => {
                self.dismiss_overlay(window, cx);
                return;
            }
            Key::Edit => {
                let previous = bar.input.text().to_owned();
                bar.input.key(stroke, cx);
                if bar.input.text() != previous {
                    let state = self.state.borrow();
                    let current = state
                        .model
                        .active(&self.id)
                        .and_then(|id| state.model.core.tab(id));
                    bar.rows = suggestions_for(
                        &state.model.core,
                        state.history.store(),
                        current.map(|tab| tab.url.as_str()),
                        bar.input.text(),
                    );
                    bar.selected = 0;
                    bar.scroll.scroll_to_item(0);
                }
            }
        }
        cx.notify();
    }

    fn choose_suggestion(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Command(bar) = &self.overlay else {
            return;
        };
        let Some(row) = bar.rows.get(index) else {
            return;
        };
        let url = match &row.target {
            Target::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.dismiss_overlay(window, cx);
                return;
            }
            Target::Tab(tab, _) if matches!(bar.mode, Command::NewTab | Command::EditLocation) => {
                let action = Action::Select(tab.clone());
                self.navigate_from_overlay(action, window, cx);
                return;
            }
            Target::Tab(_, url) | Target::Address(url) | Target::Search(url) => url.clone(),
        };
        let action = match bar.mode {
            Command::NewTab => Action::Open(url),
            Command::EditLocation => Action::Navigate(url),
            Command::Split => Action::SplitNew(url),
            Command::Peek => Action::Peek(url),
        };
        self.navigate_from_overlay(action, window, cx);
    }

    pub(crate) fn command_panel(&self, bar: &CommandBar, cx: &Context<Self>) -> Div {
        let rows = bar.rows.iter().enumerate().map(|(index, row)| {
            div()
                .id(("suggestion", index))
                .flex()
                .gap_3()
                .items_center()
                .min_w_0()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .when(index == bar.selected, |row| row.bg(rgb(0x27547b)))
                .hover(|style| style.bg(rgb(0x304855)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.choose_suggestion(index, window, cx);
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(div().truncate().child(row.label.clone()))
                        .when(!row.detail.is_empty(), |label| {
                            label.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(0xa4b5c3))
                                    .child(row.detail.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(0xc0d1dc))
                        .child(row.kind),
                )
        });
        overlay_panel()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(bar.mode.label())
                    .child(button("close-overlay", "Esc").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            this.dismiss_overlay(window, cx);
                        },
                    ))),
            )
            .child(bar.input.render("Enter an address or search"))
            .child(
                div()
                    .id("suggestions")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&bar.scroll)
                    .children(rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flecha_core::{Placement, SpaceId};
    use gpui::Keystroke;

    #[test]
    fn candidates_rank_local_sources_and_deduplicate_history() {
        let window = WindowId::new("main");
        let start = SpaceId::new("start");
        let other = SpaceId::new("other");
        let mut core = BrowserState::new(start.clone(), window.clone());
        core.add_space(other.clone());
        let favorite = TabId::new("favorite");
        let pinned = TabId::new("pinned");
        let today = TabId::new("today");
        for (id, url, title, placement) in [
            (
                &favorite,
                "https://favorite.example/",
                "Project favorite",
                Placement::Favorite,
            ),
            (
                &pinned,
                "https://pinned.example/",
                "Project pinned",
                Placement::Pinned {
                    space: &other,
                    folder: None,
                },
            ),
            (
                &today,
                "https://today.example/",
                "Project today",
                Placement::Today {
                    window: &window,
                    space: &start,
                },
            ),
        ] {
            let mut tab = Tab::new(url, 0);
            tab.title = title.into();
            assert!(core.create_tab(id.clone(), tab, placement, None));
        }
        let mut history = HistoryStore::in_memory().unwrap();
        for (url, title, at) in [
            ("https://today.example/", "Project today", 3),
            ("https://old.example/", "Project history", 1),
            ("https://old.example/", "Project history", 2),
            ("https://recent.example/", "Project recent", 4),
        ] {
            history.record_visit(url, title, at).unwrap();
        }
        let rows = suggestions_for(&core, Some(&history), None, "project");
        assert_eq!(
            rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            [
                "Favorite", "Pinned", "Open tab", "History", "History", "Search"
            ]
        );
        assert_eq!(rows[3].label, "Project history");
        assert_eq!(
            suggestions_for(&core, Some(&history), None, "example.com")[0].target,
            Target::Address("https://example.com".into())
        );
    }

    #[test]
    fn padded_credential_addresses_never_offer_a_search() {
        let core = BrowserState::new(SpaceId::new("space"), WindowId::new("main"));
        let history = HistoryStore::in_memory().unwrap();
        for userinfo in ["user", "user:pass"] {
            for separator in [
                "@\u{FEFF}",
                "@\u{FEFF}\t",
                "@\t\u{FEFF}",
                "@\u{FEFF}\u{200A}",
                "@\u{FEFF}\u{00A0}",
                "@\u{FEFF}\u{3000}",
                "@\u{200A}",
                "@\u{2009}",
                "@\u{00A0}",
                "@\u{3000}",
                "@\u{202F}",
                "@\u{2028}",
                "@\t",
                "@ ",
                "@\u{85}\u{200B}",
                "\u{FEFF}\u{2009}@\u{2060}\u{202F}",
            ] {
                for host in ["host.example", "localhost", "127.0.0.1", "[::1]"] {
                    for suffix in ["", ":8080", "/path", ":8080/path"] {
                        let input = format!("{userinfo}{separator}{host}{suffix}");
                        let rows = suggestions_for(&core, Some(&history), None, &input);
                        assert!(
                            rows.iter().all(|row| matches!(row.target, Target::Copy(_))),
                            "{input:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ordinary_at_sign_phrases_offer_a_search() {
        let core = BrowserState::new(SpaceId::new("space"), WindowId::new("main"));
        for query in [
            "rust @ 2026",
            "dinner @ 7pm",
            "lunch @ 12",
            "price @ 5%",
            "tweet @ someone",
            "meet @ noon tomorrow",
            "contact@ example",
            "weather @ stockholm",
            "lunch @ noon?",
            "anyone free @ noon?",
            "dinner @ 7pm?",
            "is it raining @ stockholm?",
            "meet @ 7/8",
            "apples @ $2/lb",
            "rate @ 5%/yr",
            "who is free @ 3pm/4pm",
        ] {
            let rows = suggestions_for(&core, None, None, query);
            assert_eq!(rows.len(), 1, "{query:?}");
            assert_eq!(
                rows[0].target,
                Target::Search(core.settings.search_engine.search_url(query))
            );
        }
    }

    #[test]
    fn developer_aliases_with_credentials_offer_no_external_targets() {
        let core = BrowserState::new(SpaceId::new("space"), WindowId::new("main"));
        for query in [
            "mdn user:pass@\u{feff}host.example",
            "mdn user:pass@host.example",
            "mdn user@ \u{feff}localhost",
        ] {
            let rows = suggestions_for(&core, None, None, query);
            assert!(
                rows.iter().all(|row| matches!(row.target, Target::Copy(_))),
                "{query:?}"
            );
        }
    }

    #[test]
    fn command_overlay_selects_and_wraps_and_rejects_non_main_windows() {
        let main = WindowId::new("main");
        let mut core = BrowserState::new(SpaceId::new("space"), main.clone());
        core.open_today(
            TabId::new("tab"),
            Tab::new("https://example.com", 0),
            &main,
            None,
            true,
        );
        assert!(location(&core, &WindowId::new("popup"), Command::NewTab).is_none());
        assert!(location(&core, &WindowId::new("popup"), Command::EditLocation).is_none());
        assert!(matches!(
            location(&core, &main, Command::NewTab),
            Some(None)
        ));
        assert_eq!(
            location(&core, &main, Command::EditLocation)
                .flatten()
                .unwrap()
                .url,
            "https://example.com"
        );
        let up = || command_key(&Keystroke::parse("up").unwrap());
        let down = || command_key(&Keystroke::parse("down").unwrap());
        assert_eq!(selection_after(0, 3, up()), 2);
        assert_eq!(selection_after(2, 3, down()), 0);
        assert_eq!(selection_after(0, 3, down()), 1);
        assert_eq!(selection_after(0, 0, up()), 0);
        assert_eq!(
            command_key(&Keystroke::parse("enter").unwrap()),
            Key::Submit
        );
        assert_eq!(
            command_key(&Keystroke::parse("escape").unwrap()),
            Key::Dismiss
        );
    }
}
