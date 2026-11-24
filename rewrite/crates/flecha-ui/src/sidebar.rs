use flecha_core::{BrowserState, FolderId, PinnedNode, Placement, TabId, WindowId};
use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    rgb,
};
use uuid::Uuid;

use crate::command_bar::Command;
use crate::shortcuts::Navigation;
use crate::text_field::TextField;
use crate::view::{Overlay, Shell, button, overlay_panel};
use crate::{Action, Controller, dispatch, now};

#[derive(Clone)]
pub(crate) enum SidebarAction {
    Toggle,
    Pin(TabId, Option<FolderId>),
    Favorite(TabId),
    ToToday(TabId),
    FolderName(Option<FolderId>, String),
    ToggleFolder(FolderId, bool),
    RemoveFolder(FolderId),
    TogglePinned(bool),
}

impl Controller {
    pub(crate) fn sidebar_action(&mut self, window: &WindowId, action: SidebarAction) {
        let Some(core) = self.model.state_mut(window) else {
            return;
        };
        let Some(current) = core.window(window) else {
            return;
        };
        let space = current.active_space_id.clone();
        match action {
            SidebarAction::Toggle => {
                core.set_sidebar_visible(window, !core.sidebar_visible(window));
            }
            SidebarAction::FolderName(Some(id), name) => {
                core.edit_folder(&id, Some(&name), None);
            }
            SidebarAction::FolderName(None, name) => {
                core.add_folder(
                    FolderId::new(Uuid::new_v4().to_string()),
                    &name,
                    &space,
                    None,
                );
            }
            SidebarAction::ToggleFolder(id, open) => {
                core.edit_folder(&id, None, Some(open));
            }
            SidebarAction::RemoveFolder(id) => {
                self.remove_pinned_folder(&id, true);
            }
            SidebarAction::TogglePinned(collapsed) => {
                core.update_space(&space, None, None, None, Some(collapsed));
            }
            SidebarAction::Pin(..) | SidebarAction::Favorite(_) | SidebarAction::ToToday(_) => {
                let (id, placement) = match &action {
                    SidebarAction::Pin(id, folder) => (
                        id,
                        Placement::Pinned {
                            space: &space,
                            folder: folder.as_ref(),
                        },
                    ),
                    SidebarAction::Favorite(id) => (id, Placement::Favorite),
                    SidebarAction::ToToday(id) => (
                        id,
                        Placement::Today {
                            space: &space,
                            window,
                        },
                    ),
                    _ => unreachable!(),
                };
                self.place_tab(window, id, placement);
            }
        }
    }

    pub(crate) fn place_tab(
        &mut self,
        window: &WindowId,
        id: &TabId,
        placement: Placement<'_>,
    ) -> bool {
        let active = self.model.active(window) == Some(id)
            && match placement {
                Placement::Favorite => true,
                Placement::Pinned { space, .. } | Placement::Today { space, .. } => self
                    .model
                    .state(window)
                    .and_then(|state| state.window(window))
                    .is_some_and(|current| &current.active_space_id == space),
            };
        let Some(core) = self.model.state_mut(window) else {
            return false;
        };
        let change = core.place_tab(id, placement, None);
        if change.result {
            for id in change.removed {
                self.engine.close(&id);
            }
            if active {
                for id in core.activate(id, window, Some(now())).removed {
                    self.engine.close(&id);
                }
            }
            self.reconcile_partition(id);
        }
        change.result
    }

    pub(crate) fn close_tab(&mut self, tab: &TabId) {
        self.engine.close(tab);
        for removed in self.model.close_tab(tab) {
            self.engine.close(&removed);
        }
    }

    pub(crate) fn remove_pinned_folder(&mut self, id: &FolderId, keep_tabs: bool) -> bool {
        let change = self.model.core.delete_folder(id, keep_tabs, now());
        for id in change.removed {
            self.engine.close(&id);
        }
        change.result
    }
}

impl Shell {
    pub(crate) fn sidebar(
        &self,
        state: &Controller,
        current: &flecha_core::Window,
        cx: &Context<Self>,
    ) -> Div {
        let Some(core) = state.model.state(&self.id) else {
            return div();
        };
        let incognito = state.model.is_incognito(&self.id);
        let toggle = Action::Sidebar(SidebarAction::Toggle);
        if !core.sidebar_visible(&self.id) {
            return div()
                .absolute()
                .top_2()
                .left_2()
                .occlude()
                .rounded_md()
                .bg(rgb(0x202a30))
                .child(self.action_button("sidebar-toggle", "Show sidebar", toggle, cx));
        }
        let Some(space) = core.space(&current.active_space_id) else {
            return div();
        };
        let window_space = current.spaces.get(&space.id);
        let active = window_space.and_then(|space| space.active_tab_id.as_ref());
        let tab = active.and_then(|id| core.tab(id));
        let address = tab.map(|tab| flecha_core::suggestions::display_url(&tab.url));
        let tint = u32::from_str_radix(space.color.trim_start_matches('#'), 16).unwrap_or(0xcdd9f7);
        let mut favorites = div()
            .id("favorites-strip")
            .flex()
            .flex_shrink_0()
            .gap_1()
            .overflow_x_scroll();
        for id in core.favorites() {
            if let Some(tab) = core.tab(id) {
                let action = Action::Select(id.clone());
                let label = tab_label(tab);
                favorites = favorites.child(
                    button(format!("favorite-{}", id.as_str()), label)
                        .max_w(px(120.))
                        .truncate()
                        .flex_shrink_0()
                        .bg(rgb(if active == Some(id) {
                            0x37545e
                        } else {
                            0x26343d
                        }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            dispatch(&this.state, &this.id, action.clone(), cx);
                        })),
                );
            }
        }
        if let Some(id) = active.filter(|id| core.favorites().contains(id)) {
            favorites = favorites.child(self.tab_menu_button(id, cx));
        }
        let add_favorite = active.filter(|id| !core.favorites().contains(id));
        let favorites = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(
                div()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section("Favorites"))
                    .when_some(add_favorite, |row, id| {
                        row.child(
                            self.row_action(
                                "add-favorite",
                                id.as_str(),
                                "+ Favorite",
                                Action::Sidebar(SidebarAction::Favorite(id.clone())),
                                cx,
                            )
                            .text_xs(),
                        )
                    }),
            )
            .when(!core.favorites().is_empty(), |row| row.child(favorites));

        let mut tabs =
            div()
                .id("sidebar-tabs")
                .min_h_0()
                .flex_1()
                .overflow_y_scroll()
                .when(!incognito, |tabs| {
                    tabs.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_1()
                            .child(self.section_button(
                                "pinned-toggle",
                                if space.pinned_collapsed {
                                    "▸ Pinned"
                                } else {
                                    "▾ Pinned"
                                },
                                SidebarAction::TogglePinned(!space.pinned_collapsed),
                                cx,
                            ))
                            .child(button("new-folder", "+ Folder").text_xs().on_click(
                                cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.show_overlay(
                                        Overlay::Folder(None, TextField::default()),
                                        window,
                                        cx,
                                    );
                                }),
                            )),
                    )
                });
        if !space.pinned_collapsed && !incognito {
            tabs = tabs.children(self.pinned_rows(core, &space.pinned, active, cx));
        }
        tabs = tabs.child(section("Today"));
        if let Some(ws) = window_space {
            for id in &ws.today {
                tabs = tabs.child(self.tab_row(core, id, active, cx));
            }
        }

        let mut spaces = div()
            .id("spaces")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .overflow_x_scroll();
        for candidate in core.spaces() {
            let selected = candidate.id == space.id;
            let tint =
                u32::from_str_radix(candidate.color.trim_start_matches('#'), 16).unwrap_or(tint);
            let action = Action::Switch(candidate.id.clone());
            spaces = spaces.child(
                div()
                    .id(format!("space-{}", candidate.id.as_str()))
                    .flex()
                    .gap_1()
                    .items_center()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |row| row.bg(rgb(0x304855)))
                    .hover(|row| row.bg(rgb(0x37545e)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, action.clone(), cx);
                    }))
                    .child(tint_dot(tint))
                    .child(candidate.name.clone())
                    .when(candidate.isolated, |row| {
                        row.child(div().text_xs().child("Isolated"))
                    }),
            );
        }
        let toolbar = state.toolbars.get(&self.id).copied().unwrap_or_default();
        let downloads = state
            .downloads
            .count(state.model.incognito_session(&self.id));
        div()
            .flex()
            .flex_col()
            .w(px(state.model.core.settings.sidebar_width.pixels() as f32))
            .flex_shrink_0()
            .min_h_0()
            .p_2()
            .gap_2()
            .bg(rgb(if incognito {
                crate::incognito::SIDEBAR
            } else {
                0x202a30
            }))
            .text_sm()
            .when(incognito, |sidebar| {
                sidebar.child(
                    div()
                        .id("incognito-label")
                        .flex()
                        .gap_1()
                        .items_center()
                        .px_2()
                        .text_color(rgb(crate::incognito::LABEL))
                        .child(format!(
                            "{} {}",
                            space.emoji.as_deref().unwrap_or_default(),
                            space.name
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .child(self.action_button("sidebar-toggle", "Hide", toggle, cx))
                    .child(self.navigation_button("back", "←", Navigation::Back, toolbar.back, cx))
                    .child(self.navigation_button(
                        "forward",
                        "→",
                        Navigation::Forward,
                        toolbar.forward,
                        cx,
                    ))
                    .child(self.navigation_button(
                        "reload",
                        if toolbar.loading { "■" } else { "↻" },
                        if toolbar.loading {
                            Navigation::Stop
                        } else {
                            Navigation::Reload
                        },
                        toolbar.available,
                        cx,
                    )),
            )
            .when(toolbar.loading, |sidebar| {
                sidebar.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(0xa4b5c3))
                        .child("Loading…"),
                )
            })
            .child(
                button(
                    "sidebar-address",
                    address.unwrap_or_else(|| "Address or search".into()),
                )
                .truncate()
                .bg(rgb(0x26343d))
                .flex_shrink_0()
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_command(Command::EditLocation, window, cx);
                })),
            )
            .when(state.site_for_window(&self.id).is_some(), |sidebar| {
                sidebar.child(
                    button("site-controls", "Site Controls")
                        .self_start()
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.toggle_site_controls(window, cx)
                        })),
                )
            })
            .when(!incognito, |sidebar| sidebar.child(favorites))
            .child(tabs)
            .when(!incognito, |sidebar| {
                sidebar.child(
                    div()
                        .flex()
                        .min_w_0()
                        .gap_1()
                        .child(div().flex_1().min_w_0().child(spaces))
                        .child(self.action_button("new-space", "+ Space", Action::NewSpace, cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_shrink_0()
                    .gap_1()
                    .border_t_1()
                    .border_color(rgb(0x30404a))
                    .pt_1()
                    .when(!incognito, |row| {
                        row.child(button("show-history", "History").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| this.toggle_history(window, cx),
                        )))
                    })
                    .child(
                        button("show-downloads", "Downloads")
                            .when(downloads > 0, |button| {
                                button.flex().gap_1().items_center().child(
                                    div()
                                        .px_1()
                                        .text_xs()
                                        .rounded_md()
                                        .bg(rgb(0x37545e))
                                        .child(downloads.to_string()),
                                )
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.toggle_downloads(window, cx)
                            })),
                    )
                    .when(!incognito, |row| {
                        row.child(button("show-archive", "Archive").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| this.toggle_archive(window, cx),
                        )))
                    })
                    .child(self.command_button(
                        "open-settings",
                        "Settings",
                        Command::OpenSettings,
                        cx,
                    ))
                    .child(self.command_button("new-tab", "+ Tab", Command::NewTab, cx))
                    .when(!incognito, |row| {
                        row.child(self.action_button(
                            "isolation",
                            if space.isolated {
                                "Isolated cookies"
                            } else {
                                "Shared cookies"
                            },
                            Action::Isolate,
                            cx,
                        ))
                    })
                    .when(current.visible_tabs().len() > 1, |row| {
                        row.child(self.action_button(
                            "unsplit",
                            "Exit split",
                            Action::ExitSplit,
                            cx,
                        ))
                    })
                    // Last, so the regular controls keep the places the harness measured.
                    .child(self.action_button(
                        "new-incognito-window",
                        "+ Incognito",
                        Action::NewIncognitoWindow,
                        cx,
                    )),
            )
    }

    fn navigation_button(
        &self,
        id: &'static str,
        label: &'static str,
        navigation: Navigation,
        enabled: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        button(id, label)
            .px_1()
            .aria_label(match navigation {
                Navigation::Back => "Back",
                Navigation::Forward => "Forward",
                Navigation::Reload => "Reload",
                Navigation::HardReload => "Reload without cache",
                Navigation::Stop => "Stop loading",
            })
            .when_else(
                enabled,
                |button| {
                    button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, Action::Browser(navigation), cx)
                    }))
                },
                |button| button.opacity(0.5).cursor_default(),
            )
    }

    fn section_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: SidebarAction,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        button(id, label)
            .text_xs()
            .text_color(rgb(0xa4b5c3))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                dispatch(&this.state, &this.id, Action::Sidebar(action.clone()), cx);
            }))
    }

    fn pinned_rows(
        &self,
        core: &BrowserState,
        nodes: &[PinnedNode],
        active: Option<&TabId>,
        cx: &Context<Self>,
    ) -> Vec<Div> {
        let mut rows = Vec::new();
        let mut levels = vec![nodes.iter()];
        while let Some(level) = levels.last_mut() {
            let Some(node) = level.next() else {
                levels.pop();
                continue;
            };
            let inset = px((levels.len() - 1) as f32 * 8.);
            let row = match node {
                PinnedNode::Tab { tab_id } => self.tab_row(core, tab_id, active, cx),
                PinnedNode::Folder {
                    id,
                    name,
                    open,
                    children,
                } => {
                    let toggle = Action::Sidebar(SidebarAction::ToggleFolder(id.clone(), !open));
                    let rename = id.clone();
                    let folder_name = name.clone();
                    let mut row = div()
                        .flex()
                        .min_w_0()
                        .child(
                            button(
                                format!("folder-{}", id.as_str()),
                                format!("{} {name}", if *open { "▾" } else { "▸" }),
                            )
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    dispatch(&this.state, &this.id, toggle.clone(), cx);
                                },
                            )),
                        )
                        .child(
                            button(format!("rename-{}", id.as_str()), "Rename")
                                .text_xs()
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.show_overlay(
                                        Overlay::Folder(
                                            Some(rename.clone()),
                                            TextField::new(folder_name.clone(), true),
                                        ),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(self.row_action(
                            "remove-folder",
                            id.as_str(),
                            "×",
                            Action::Sidebar(SidebarAction::RemoveFolder(id.clone())),
                            cx,
                        ));
                    if let Some(active) = active {
                        row = row.child(self.row_action(
                            "pin-in-folder",
                            id.as_str(),
                            "+",
                            Action::Sidebar(SidebarAction::Pin(active.clone(), Some(id.clone()))),
                            cx,
                        ));
                    }
                    rows.push(row.pl(inset));
                    if *open {
                        levels.push(children.iter());
                    }
                    continue;
                }
            };
            rows.push(row.pl(inset));
        }
        rows
    }

    fn tab_row(
        &self,
        core: &BrowserState,
        id: &TabId,
        active: Option<&TabId>,
        cx: &Context<Self>,
    ) -> Div {
        let Some(tab) = core.tab(id) else {
            return div();
        };
        let selected = active == Some(id);
        let select = Action::Select(id.clone());
        let address = flecha_core::suggestions::display_url(&tab.url);
        let mut row = div()
            .flex()
            .items_center()
            .min_w_0()
            .h(px(38.))
            .rounded_md()
            .when(selected, |row| row.bg(rgb(0x37545e)))
            .child(
                div()
                    .id(format!("tab-{}", id.as_str()))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .px_2()
                    .cursor_pointer()
                    .rounded_md()
                    .hover(|row| row.bg(rgb(0x304855)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, select.clone(), cx);
                    }))
                    .child(div().truncate().child(tab_label(tab)))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(rgb(0xa4b5c3))
                            .child(address),
                    ),
            );
        row = row.child(self.tab_menu_button(id, cx));
        row = row.child(self.row_action(
            "close-tab",
            id.as_str(),
            "×",
            Action::CloseTab(id.clone()),
            cx,
        ));
        if !selected && active.is_some() {
            row = row.child(
                self.row_action("split", id.as_str(), "Split", Action::Split(id.clone()), cx)
                    .text_xs(),
            );
        }
        row
    }

    fn row_action(
        &self,
        kind: &'static str,
        id: &str,
        label: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        button(format!("{kind}-{id}"), label)
            .flex_shrink_0()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                dispatch(&this.state, &this.id, action.clone(), cx);
            }))
    }

    fn tab_menu_button(&self, id: &TabId, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let tab = id.clone();
        button(format!("tab-actions-{}", id.as_str()), "⋯")
            .flex_shrink_0()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.show_overlay(Overlay::TabActions(tab.clone()), window, cx);
            }))
    }

    fn command_button(
        &self,
        id: &'static str,
        label: &'static str,
        mode: Command,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        button(id, label).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_command(mode, window, cx);
        }))
    }

    pub(crate) fn tab_actions_panel(&self, tab_id: &TabId, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let Some(core) = state.model.state(&self.id) else {
            return overlay_panel();
        };
        let Some(tab) = core.tab(tab_id) else {
            return overlay_panel();
        };
        let placement = core.locate(tab_id);
        let mut actions = Vec::new();
        let id = tab_id.clone();
        if matches!(placement, Some(Placement::Pinned { .. })) {
            actions.push(("Unpin", Action::Sidebar(SidebarAction::ToToday(id.clone()))));
        } else {
            actions.push((
                "Pin in Space",
                Action::Sidebar(SidebarAction::Pin(id.clone(), None)),
            ));
        }
        if matches!(placement, Some(Placement::Favorite)) {
            actions.push((
                "Remove favorite",
                Action::Sidebar(SidebarAction::ToToday(id.clone())),
            ));
        } else {
            actions.push((
                "Add to favorites",
                Action::Sidebar(SidebarAction::Favorite(id.clone())),
            ));
        }
        actions.extend([
            ("Open in Peek", Action::PeekTab(id.clone())),
            ("Move to Flechinha", Action::DetachTab(id.clone())),
            ("Close tab", Action::CloseTab(id)),
        ]);
        if state.model.is_incognito(&self.id) {
            actions.retain(|(_, action)| crate::incognito::refusal(action).is_none());
        }
        overlay_panel()
            .child(div().truncate().child(tab_label(tab)))
            .children(actions.into_iter().map(|(label, action)| {
                button(label, label).on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.dismiss_overlay(window, cx);
                        dispatch(&this.state, &this.id, action.clone(), cx);
                    },
                ))
            }))
            .child(self.command_button("split-new", "Split with a new tab", Command::Split, cx))
            .child(self.command_button("peek-url", "Peek at another address", Command::Peek, cx))
    }

    pub(crate) fn folder_key(
        &mut self,
        stroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stroke.key == "enter" && stroke.modifiers == gpui::Modifiers::none() {
            self.save_folder(window, cx);
        } else if let Overlay::Folder(_, input) = &mut self.overlay {
            input.key(stroke, cx);
            cx.notify();
        }
    }

    fn save_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Folder(folder, input) = &self.overlay else {
            return;
        };
        let action = SidebarAction::FolderName(folder.clone(), input.text().to_owned());
        self.dismiss_overlay(window, cx);
        dispatch(&self.state, &self.id, Action::Sidebar(action), cx);
    }

    pub(crate) fn folder_panel(
        &self,
        id: Option<&FolderId>,
        input: &TextField,
        cx: &Context<Self>,
    ) -> Div {
        overlay_panel()
            .child(if id.is_some() {
                "Rename folder"
            } else {
                "New folder"
            })
            .child(input.render("Folder name"))
            .child(button("save-folder", "Save").on_click(cx.listener(
                |this, _: &ClickEvent, window, cx| {
                    this.save_folder(window, cx);
                },
            )))
    }
}

fn tint_dot(tint: u32) -> Div {
    div()
        .size(px(8.))
        .rounded_full()
        .flex_shrink_0()
        .bg(rgb(tint))
}

fn section(label: &'static str) -> Div {
    div()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(0xa4b5c3))
        .child(label)
}

pub(crate) fn tab_label(tab: &flecha_core::Tab) -> String {
    let title = tab.custom_title.as_ref().unwrap_or(&tab.title);
    if title.is_empty() || *title == tab.url {
        flecha_core::suggestions::display_url(&tab.url)
    } else {
        title.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_actions_persist_space_and_tab_placement_changes() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let first = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        let act = |controller: &mut Controller, action| controller.act(&window, action).unwrap();
        act(&mut controller, Action::NewSpace);
        let second = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        assert_ne!(first, second);
        act(
            &mut controller,
            Action::Open("https://example.test/".into()),
        );
        let tab = controller.model.active(&window).unwrap().clone();
        let sidebar =
            |controller: &mut Controller, action| act(controller, Action::Sidebar(action));
        sidebar(
            &mut controller,
            SidebarAction::FolderName(None, "Work".into()),
        );
        let PinnedNode::Folder { id, .. } =
            &controller.model.core.space(&second).unwrap().pinned[0]
        else {
            panic!("the Space needs a pinned folder")
        };
        let folder = id.clone();
        sidebar(
            &mut controller,
            SidebarAction::Pin(tab.clone(), Some(folder.clone())),
        );
        sidebar(
            &mut controller,
            SidebarAction::ToggleFolder(folder.clone(), false),
        );
        assert!(
            matches!(&controller.model.core.space(&second).unwrap().pinned[0],
            PinnedNode::Folder { open: false, children, .. }
                if matches!(&children[0], PinnedNode::Tab { tab_id } if tab_id == &tab))
        );
        sidebar(&mut controller, SidebarAction::ToToday(tab.clone()));
        assert!(
            matches!(controller.model.core.locate(&tab), Some(Placement::Today { space, .. }) if space == &second)
        );
        sidebar(&mut controller, SidebarAction::Favorite(tab.clone()));
        act(&mut controller, Action::Switch(first.clone()));
        assert_eq!(
            controller.model.core.focused_window().active_space_id,
            first
        );
        assert_eq!(
            controller.model.core.favorites(),
            std::slice::from_ref(&tab)
        );
        sidebar(&mut controller, SidebarAction::ToToday(tab.clone()));
        act(&mut controller, Action::CloseTab(tab));
        let entry = controller.model.core.archive()[0].clone();
        act(&mut controller, Action::RestoreArchive(entry));
        let restored = controller.model.active(&window).unwrap();
        assert_eq!(
            controller.model.core.tab(restored).unwrap().url,
            "https://example.test/"
        );
        assert!(controller.model.core.archive().is_empty());
        let visible = controller.model.core.sidebar_visible(&window);
        sidebar(&mut controller, SidebarAction::Toggle);
        assert_ne!(controller.model.core.sidebar_visible(&window), visible);
    }
}
