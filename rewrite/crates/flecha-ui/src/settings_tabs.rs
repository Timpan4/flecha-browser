use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use flecha_core::{
    BrowserState, FolderId, PinnedNode, Placement, SpaceId, Tab, TabId, WindowId, walk_pins,
};
use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, IntoElement as _, Keystroke, Modifiers,
    ParentElement as _, ScrollHandle, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px, rgb,
};

use super::{SettingsPanel, muted, row};
use crate::sidebar::tab_label;
use crate::text_field::TextField;
use crate::view::{Overlay, Shell, button};
use crate::{Controller, now};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Item {
    Tab(TabId),
    Folder(FolderId),
}

enum Row {
    Heading(String),
    Tab(TabId, usize),
    Folder(FolderId, String, usize),
}

impl Row {
    fn item(&self) -> Option<Item> {
        match self {
            Self::Heading(_) => None,
            Self::Tab(id, _) => Some(Item::Tab(id.clone())),
            Self::Folder(id, ..) => Some(Item::Folder(id.clone())),
        }
    }
}

fn tab_matches(tab: &Tab, query: &str) -> bool {
    query.is_empty()
        || [
            Some(&tab.title),
            Some(&tab.url),
            tab.custom_title.as_ref(),
            tab.home_url.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(query))
}

fn tabs(core: &BrowserState, ids: &[TabId], query: &str) -> Vec<Row> {
    ids.iter()
        .filter(|id| core.tab(id).is_some_and(|tab| tab_matches(tab, query)))
        .map(|id| Row::Tab(id.clone(), 0))
        .collect()
}

fn pinned_rows(core: &BrowserState, nodes: &[PinnedNode], query: &str) -> Vec<Row> {
    let mut rows: Vec<(Row, bool)> = Vec::new();
    let mut parents: Vec<usize> = Vec::new();
    let mut matching_folder = None;
    for (node, depth, _) in walk_pins(nodes) {
        parents.truncate(depth);
        if matching_folder.is_some_and(|parent| depth <= parent) {
            matching_folder = None;
        }
        let (row, matches) = match node {
            PinnedNode::Tab { tab_id } => (
                Row::Tab(tab_id.clone(), depth),
                core.tab(tab_id).is_some_and(|tab| tab_matches(tab, query)),
            ),
            PinnedNode::Folder { id, name, .. } => {
                let matches = query.is_empty() || name.to_lowercase().contains(query);
                if matches && matching_folder.is_none() {
                    matching_folder = Some(depth);
                }
                (Row::Folder(id.clone(), name.clone(), depth), matches)
            }
        };
        let visible = matches || matching_folder.is_some();
        if visible {
            for &parent in parents.iter().rev() {
                let (_, shown) = &mut rows[parent];
                if *shown {
                    break;
                }
                *shown = true;
            }
        }
        if matches!(node, PinnedNode::Folder { .. }) {
            parents.push(rows.len());
        }
        rows.push((row, visible));
    }
    rows.into_iter()
        .filter_map(|(row, visible)| visible.then_some(row))
        .collect()
}

fn rows(core: &BrowserState, window: &WindowId, query: &str) -> Vec<Row> {
    let Some(window) = core.window(window) else {
        return Vec::new();
    };
    let query = query.trim().to_lowercase();
    let favorites = tabs(core, core.favorites(), &query);
    let mut rows = Vec::new();
    if !favorites.is_empty() {
        rows.push(Row::Heading("Favorites".into()));
        rows.extend(favorites);
    }
    for space in core.spaces() {
        let pins = pinned_rows(core, &space.pinned, &query);
        let today = window
            .spaces
            .get(&space.id)
            .map_or_else(Vec::new, |view| tabs(core, &view.today, &query));
        if !query.is_empty() && pins.is_empty() && today.is_empty() {
            continue;
        }
        rows.push(Row::Heading(space.name.clone()));
        for (label, items) in [("Pinned", pins), ("Today", today)] {
            if !items.is_empty() {
                rows.push(Row::Heading(label.into()));
                rows.extend(items);
            }
        }
    }
    rows
}

#[derive(Clone)]
enum Destination {
    Favorites,
    Pinned(SpaceId, Option<FolderId>),
    Today(SpaceId),
}

#[derive(Clone)]
enum Edit {
    Move(Destination),
    Close,
    Archive,
    DeleteFolder { keep_tabs: bool },
}

impl Controller {
    fn edit_managed_tabs(&mut self, window: &WindowId, items: &[Item], edit: Edit) -> Result<()> {
        let core = &self.model.core;
        ensure!(
            core.window(window).is_some(),
            "This Flecha window has closed"
        );
        if let Edit::Move(destination) = &edit {
            match destination {
                Destination::Pinned(space, _) | Destination::Today(space) => {
                    ensure!(
                        core.space(space).is_some(),
                        "The destination Space has been removed"
                    );
                }
                Destination::Favorites => {}
            }
            ensure!(
                items.iter().all(|item| match item {
                    Item::Tab(id) => core.tab(id).is_some_and(|tab| tab.device.is_none()),
                    Item::Folder(_) => matches!(destination, Destination::Pinned(..)),
                }),
                "Some selected items can no longer be moved"
            );
        }
        for item in items {
            match (item, &edit) {
                (Item::Tab(id), Edit::Move(to)) => {
                    let placement = match to {
                        Destination::Favorites => Placement::Favorite,
                        Destination::Pinned(space, folder) => Placement::Pinned {
                            space,
                            folder: folder.as_ref(),
                        },
                        Destination::Today(space) => Placement::Today { space, window },
                    };
                    ensure!(
                        self.place_tab(window, id, placement),
                        "The tab or destination has changed"
                    );
                }
                (Item::Folder(id), Edit::Move(Destination::Pinned(space, folder))) => {
                    let change = self
                        .model
                        .core
                        .move_folder(id, space, folder.as_ref(), None);
                    ensure!(change.result, "The folder or destination has changed");
                    for removed in change.removed {
                        self.engine.close(&removed);
                    }
                    let tabs: Vec<_> = self
                        .model
                        .core
                        .folder_tab_ids(id)
                        .into_iter()
                        .flatten()
                        .cloned()
                        .collect();
                    for tab in tabs {
                        self.reconcile_partition(&tab);
                    }
                }
                (Item::Tab(id), Edit::Close) => self.close_tab(id),
                (Item::Tab(id), Edit::Archive) => {
                    for removed in self.model.core.delete_tab(id, now()).removed {
                        self.engine.close(&removed);
                    }
                }
                (Item::Folder(id), Edit::DeleteFolder { keep_tabs }) => {
                    self.remove_pinned_folder(id, *keep_tabs);
                }
                _ => unreachable!("the manager only offers edits for the selected item type"),
            }
        }
        self.update_visibility();
        self.save()
            .map_err(|error| error.context("Save Tabs & Spaces changes"))
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Menu {
    #[default]
    None,
    Move,
    DeleteFolder,
}

pub(super) struct Manager {
    search: TextField,
    search_focused: bool,
    selected: BTreeSet<Item>,
    anchor: Option<Item>,
    cursor: Option<Item>,
    menu: Menu,
    scroll: ScrollHandle,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            search: TextField::default(),
            search_focused: true,
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
            menu: Menu::None,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Manager {
    pub(super) fn focus_search(&mut self) {
        self.search_focused = true;
        self.menu = Menu::None;
    }

    pub(super) fn before_shortcuts(&mut self, stroke: &Keystroke) -> bool {
        match stroke.key.as_str() {
            "f" if primary(stroke.modifiers)
                && !stroke.modifiers.alt
                && !stroke.modifiers.shift =>
            {
                self.focus_search();
                self.search = TextField::new(self.search.text().into(), true);
            }
            "escape" if stroke.modifiers == Modifiers::none() && self.menu != Menu::None => {
                self.menu = Menu::None;
            }
            "escape"
                if stroke.modifiers == Modifiers::none()
                    && self.search_focused
                    && !self.search.text().is_empty() =>
            {
                self.search = TextField::default();
                self.clear_selection();
                self.scroll.scroll_to_item(0);
            }
            "escape" if stroke.modifiers == Modifiers::none() && !self.selected.is_empty() => {
                self.clear_selection();
            }
            _ => return false,
        }
        true
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.menu = Menu::None;
    }

    fn selected(&self, rows: &[Row]) -> Vec<Item> {
        rows.iter()
            .filter_map(Row::item)
            .filter(|item| self.selected.contains(item))
            .collect()
    }

    fn select(&mut self, item: Item, rows: &[Row], modifiers: Modifiers) {
        self.menu = Menu::None;
        self.search_focused = false;
        self.cursor = Some(item.clone());
        if matches!(item, Item::Tab(_)) {
            let visible: Vec<_> = rows.iter().filter_map(Row::item).collect();
            if modifiers.shift
                && let Some(anchor @ Item::Tab(_)) = &self.anchor
                && let Some(start) = visible.iter().position(|candidate| candidate == anchor)
                && let Some(end) = visible.iter().position(|candidate| candidate == &item)
            {
                self.selected = visible[start.min(end)..=start.max(end)]
                    .iter()
                    .filter(|item| matches!(item, Item::Tab(_)))
                    .cloned()
                    .collect();
                return;
            }
            if primary(modifiers) {
                self.selected.retain(|item| matches!(item, Item::Tab(_)));
                if !self.selected.remove(&item) {
                    self.selected.insert(item.clone());
                }
                self.anchor = Some(item);
                return;
            }
        }
        self.anchor = Some(item.clone());
        self.selected = BTreeSet::from([item]);
    }
}

fn primary(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.control
    }
}

impl Shell {
    pub(super) fn tabs_settings_key(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) {
        let Overlay::Settings(panel) = &mut self.overlay else {
            return;
        };
        let manager = &mut panel.manager;
        if manager.menu != Menu::None {
            return;
        }
        let key = stroke.key.as_str();
        let editing = manager.search_focused && key != "tab" && key != "down";
        if editing {
            let previous = manager.search.text().to_owned();
            manager.search.key(stroke, cx);
            if manager.search.text() != previous {
                manager.clear_selection();
                manager.scroll.scroll_to_item(0);
                panel.message = None;
            }
            cx.notify();
            return;
        }
        let rows = rows(
            &self.state.borrow().model.core,
            &self.id,
            manager.search.text(),
        );
        let items: Vec<_> = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| Some((index, row.item()?)))
            .collect();
        match key {
            "tab" if !manager.search_focused => manager.focus_search(),
            "tab" | "up" | "down" => {
                let current = if manager.search_focused {
                    None
                } else {
                    items
                        .iter()
                        .position(|(_, item)| manager.cursor.as_ref() == Some(item))
                };
                let next = current.map_or(0, |index| match key {
                    "up" => index.saturating_sub(1),
                    _ => (index + 1).min(items.len() - 1),
                });
                if let Some((row, item)) = items.get(next) {
                    manager.select(item.clone(), &rows, stroke.modifiers);
                    manager.scroll.scroll_to_item(*row);
                }
            }
            "a" if primary(stroke.modifiers) => {
                manager.selected = items
                    .into_iter()
                    .map(|(_, item)| item)
                    .filter(|item| matches!(item, Item::Tab(_)))
                    .collect();
            }
            "backspace" | "delete" if stroke.modifiers == Modifiers::none() => {
                if matches!(manager.selected(&rows).as_slice(), [Item::Folder(_)]) {
                    self.tabs_menu(Menu::DeleteFolder, cx);
                } else {
                    self.edit_tabs(Edit::Archive, cx);
                }
                return;
            }
            _ => return,
        }
        cx.notify();
    }

    fn edit_tabs(&mut self, edit: Edit, cx: &mut Context<Self>) {
        let Overlay::Settings(panel) = &mut self.overlay else {
            return;
        };
        let mut state = self.state.borrow_mut();
        let manager = &mut panel.manager;
        let items = manager.selected(&rows(&state.model.core, &self.id, manager.search.text()));
        if items.is_empty() {
            return;
        }
        let result = state.edit_managed_tabs(&self.id, &items, edit);
        manager.clear_selection();
        panel.result(result.map(|()| String::new()));
        cx.refresh_windows();
    }

    fn tabs_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        if let Some(panel) = self.settings_mut() {
            panel.manager.menu = if panel.manager.menu == menu {
                Menu::None
            } else {
                menu
            };
            panel.manager.search_focused = false;
            cx.notify();
        }
    }

    pub(super) fn tabs_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let core = &state.model.core;
        let manager = &panel.manager;
        let rows = rows(core, &self.id, manager.search.text());
        let selected = manager.selected(&rows);
        let folder = match selected.as_slice() {
            [Item::Folder(id)] => Some(id),
            _ => None,
        };
        let can_move = selected.iter().all(|item| match item {
            Item::Tab(id) => core.tab(id).is_some_and(|tab| tab.device.is_none()),
            Item::Folder(_) => true,
        });
        let search = div()
            .id("manager-search")
            .flex_1()
            .min_w_0()
            .when(manager.search_focused, |field| {
                field.border_b_1().border_color(rgb(0x73b4e4))
            })
            .child(if manager.search_focused {
                manager.search.render("Search tabs and folders")
            } else if manager.search.text().is_empty() {
                muted("Search tabs and folders").px_3().py_2()
            } else {
                div()
                    .px_3()
                    .py_2()
                    .truncate()
                    .child(manager.search.text().to_owned())
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(panel) = this.settings_mut() {
                    panel.manager.focus_search();
                    cx.notify();
                }
            }));
        let mut toolbar = row().child(search);
        if !selected.is_empty() {
            let count = if folder.is_some() {
                "1 folder".into()
            } else {
                format!("{} selected", selected.len())
            };
            toolbar = toolbar
                .child(muted(count))
                .when(can_move, |bar| {
                    bar.child(button("manager-move", "Move to…").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.tabs_menu(Menu::Move, cx)),
                    ))
                })
                .when(folder.is_some(), |bar| {
                    bar.child(button("manager-delete-folder", "Delete folder…").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.tabs_menu(Menu::DeleteFolder, cx)
                        }),
                    ))
                })
                .when(folder.is_none(), |bar| {
                    bar.child(button("manager-close-tabs", "Close").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.edit_tabs(Edit::Close, cx)),
                    ))
                    .child(
                        button("manager-archive-tabs", "Archive").on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| this.edit_tabs(Edit::Archive, cx),
                        )),
                    )
                });
        }
        let content = match manager.menu {
            Menu::Move if !selected.is_empty() && can_move => {
                self.move_destinations(core, folder, cx)
            }
            Menu::DeleteFolder if folder.is_some() => div()
                .flex()
                .flex_col()
                .gap_2()
                .child("Remove this folder and keep its contents, or archive its tabs?")
                .child(
                    row()
                        .child(
                            button("manager-keep-folder-tabs", "Keep contents").on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.edit_tabs(Edit::DeleteFolder { keep_tabs: true }, cx)
                                }),
                            ),
                        )
                        .child(
                            button("manager-archive-folder-tabs", "Archive tabs").on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.edit_tabs(Edit::DeleteFolder { keep_tabs: false }, cx)
                                }),
                            ),
                        )
                        .child(
                            button("manager-cancel-folder", "Cancel").on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| this.tabs_menu(Menu::None, cx),
                            )),
                        ),
                )
                .into_any_element(),
            _ => {
                let empty = !rows.iter().any(|row| !matches!(row, Row::Heading(_)));
                div()
                    .id("manager-results")
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .flex_1()
                    .gap_1()
                    .overflow_y_scroll()
                    .track_scroll(&manager.scroll)
                    .when(empty, |results| {
                        results.child(muted(if manager.search.text().trim().is_empty() {
                            "No tabs or folders"
                        } else {
                            "No matching tabs or folders"
                        }))
                    })
                    .children(
                        rows.iter()
                            .map(|row| self.manager_row(core, row, manager, cx)),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .gap_2()
            .child(toolbar.flex_shrink_0())
            .child(content)
    }

    fn manager_row(
        &self,
        core: &BrowserState,
        row: &Row,
        manager: &Manager,
        cx: &Context<Self>,
    ) -> Div {
        let (id, title, detail, depth) = match row {
            Row::Heading(name) => {
                return div().pt_2().text_color(rgb(0xa4b5c3)).child(name.clone());
            }
            Row::Tab(id, depth) => {
                let Some(tab) = core.tab(id) else {
                    return div();
                };
                (
                    format!("manager-tab-{}", id.as_str()),
                    tab_label(tab),
                    flecha_core::suggestions::display_url(
                        tab.home_url.as_deref().unwrap_or(&tab.url),
                    ),
                    *depth,
                )
            }
            Row::Folder(id, name, depth) => (
                format!("manager-folder-{}", id.as_str()),
                name.clone(),
                "Folder".into(),
                *depth,
            ),
        };
        let Some(item) = row.item() else { return div() };
        div().pl(px(12. * depth as f32)).child(
            button(id, "")
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .when(manager.selected.contains(&item), |row| {
                    row.bg(rgb(0x27547b))
                })
                .child(div().min_w_0().truncate().child(title))
                .child(muted(detail).min_w_0().truncate().text_xs())
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    let Overlay::Settings(panel) = &mut this.overlay else {
                        return;
                    };
                    let state = this.state.borrow();
                    let manager = &mut panel.manager;
                    let rows = rows(&state.model.core, &this.id, manager.search.text());
                    if rows.iter().filter_map(Row::item).any(|row| row == item) {
                        manager.select(item.clone(), &rows, event.modifiers());
                        cx.notify();
                    }
                })),
        )
    }

    fn move_destinations(
        &self,
        core: &BrowserState,
        folder: Option<&FolderId>,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let mut menu =
            div()
                .id("manager-destinations")
                .flex()
                .flex_col()
                .gap_2()
                .min_h_0()
                .overflow_y_scroll()
                .child(row().child("Move to").child(
                    button("manager-cancel-move", "Cancel").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.tabs_menu(Menu::None, cx)),
                    ),
                ));
        if folder.is_none() {
            menu = menu.child(self.move_button(
                "manager-move-favorite",
                "Favorites".into(),
                Destination::Favorites,
                cx,
            ));
        }
        for space in core.spaces() {
            let mut actions = row().child(self.move_button(
                format!("manager-move-pin-{}", space.id.as_str()),
                "Pinned".into(),
                Destination::Pinned(space.id.clone(), None),
                cx,
            ));
            if folder.is_none() {
                actions = actions.child(self.move_button(
                    format!("manager-move-today-{}", space.id.as_str()),
                    "Today".into(),
                    Destination::Today(space.id.clone()),
                    cx,
                ));
            }
            let mut skip = None;
            for (node, depth, _) in walk_pins(&space.pinned) {
                if skip.is_some_and(|parent| depth > parent) {
                    continue;
                }
                skip = None;
                if let PinnedNode::Folder { id, name, .. } = node {
                    if folder == Some(id) {
                        skip = Some(depth);
                    } else {
                        actions = actions.child(self.move_button(
                            format!("manager-move-folder-{}", id.as_str()),
                            name.clone(),
                            Destination::Pinned(space.id.clone(), Some(id.clone())),
                            cx,
                        ));
                    }
                }
            }
            menu = menu.child(muted(space.name.clone())).child(actions);
        }
        menu.into_any_element()
    }

    fn move_button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        destination: Destination,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        button(id, label).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.edit_tabs(Edit::Move(destination.clone()), cx);
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, model::tab_id};

    #[test]
    fn search_matches_titles_addresses_and_folder_contents() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut controller = Controller::open_test_profile(profile.path())?;
        let core = &mut controller.model.core;
        let current = core.focused_window();
        let (window, space) = (current.id.clone(), current.active_space_id.clone());
        let folder = FolderId::new("outer");
        let nested = FolderId::new("nested");
        assert!(core.add_folder(folder.clone(), "Reading", &space, None));
        assert!(core.add_folder(nested.clone(), "Notes", &space, Some(&folder)));
        let child = tab_id();
        let mut tab = Tab::new("https://example.test/live", now());
        tab.custom_title = Some("Release Candidate".into());
        tab.home_url = Some("https://example.test/start".into());
        assert!(core.create_tab(
            child.clone(),
            tab,
            Placement::Pinned {
                space: &space,
                folder: Some(&nested),
            },
            None
        ));
        let today = tab_id();
        assert!(core.open_today(
            today.clone(),
            Tab::new("https://example.test/today", now()),
            &window,
            None,
            false
        ));
        let expected = [Item::Folder(folder), Item::Folder(nested), Item::Tab(child)];
        let shown = |query| {
            rows(core, &window, query)
                .iter()
                .filter_map(Row::item)
                .collect::<Vec<_>>()
        };
        for query in [" RELEASE candidate ", "/live", "/start", "reaDing"] {
            assert_eq!(shown(query), expected, "{query}");
        }
        assert_eq!(shown("/today"), [Item::Tab(today)]);
        assert!(shown("missing").is_empty());
        Ok(())
    }

    #[test]
    fn bulk_move_and_folder_move_survive_restart() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut controller = Controller::open_test_profile(profile.path())?;
        let current = controller.model.core.focused_window();
        let (window, first) = (current.id.clone(), current.active_space_id.clone());
        controller.act(&window, Action::NewSpace)?;
        let second = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        let third = SpaceId::new("destination");
        let core = &mut controller.model.core;
        assert!(core.add_space(third.clone()));
        assert!(core.set_isolated(&third, true).is_empty());
        let folder = FolderId::new("project");
        assert!(core.add_folder(folder.clone(), "Project", &first, None));
        let pinned = tab_id();
        assert!(core.create_tab(
            pinned.clone(),
            Tab::new("https://example.test/pinned", now()),
            Placement::Pinned {
                space: &first,
                folder: None
            },
            None
        ));
        let today = controller
            .model
            .open(&window, "https://example.test/today".into(), true)?;

        let visible = rows(&controller.model.core, &window, "example.test");
        let mut manager = Manager::default();
        manager.select(Item::Tab(pinned.clone()), &visible, Modifiers::none());
        manager.select(
            Item::Tab(today.clone()),
            &visible,
            Modifiers {
                shift: true,
                ..Modifiers::none()
            },
        );
        controller.edit_managed_tabs(
            &window,
            &manager.selected(&visible),
            Edit::Move(Destination::Pinned(first.clone(), Some(folder.clone()))),
        )?;
        controller.edit_managed_tabs(
            &window,
            &[Item::Folder(folder.clone())],
            Edit::Move(Destination::Pinned(third.clone(), None)),
        )?;

        let reopened = Controller::open_test_profile(profile.path())?;
        let core = &reopened.model.core;
        assert_eq!(core, &controller.model.core);
        assert_eq!(core.focused_window().active_space_id, second);
        assert_eq!(core.folder_tab_ids(&folder).unwrap(), [&pinned, &today]);
        for tab in [pinned, today] {
            assert!(
                matches!(core.locate(&tab), Some(Placement::Pinned { space, folder: Some(parent) })
                if space == &third && parent == &folder)
            );
            assert_eq!(
                reopened.model.partition(&tab),
                Some(flecha_cef::Partition::Isolated(third.clone()))
            );
        }
        Ok(())
    }
}
