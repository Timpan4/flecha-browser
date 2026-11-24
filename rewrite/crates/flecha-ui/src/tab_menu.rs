//! The sidebar's menus: a tab's (`tabMenu`, chromium/shell/background.ts:1111-1162) and a Space's
//! (`spaceMenu`, background.ts:1164-1179), drawn as the shipping shell's `.shell-menu`
//! (chromium/shell/shell.css:72-135), and the field that renames a tab.

use anyhow::{Context as _, Result, ensure};
use flecha_core::{CloseToday, Placement, SpaceId, TabId, WindowId, suggestions::pinnable_address};
use gpui::{
    ClickEvent, ClipboardItem, Context, Div, FontWeight, InteractiveElement as _, IntoElement,
    MouseDownEvent, ParentElement as _, Pixels, Point, StatefulInteractiveElement as _,
    Styled as _, TextRun, Window, deferred, div, prelude::FluentBuilder as _, px,
};

use crate::sidebar::{SidebarAction, tab_label};
use crate::space_editor::SpaceAction;
use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, overlay_panel, primary_button};
use crate::{Action, Controller, dispatch, now};

/// What a tab's menu does to it, by its id, from the window that asked.
#[derive(Clone, Debug)]
pub(crate) enum TabAction {
    Duplicate,
    Reload,
    ToggleMute,
    MoveToSpace(SpaceId),
    Rename(String),
    GoHome,
    SetHome,
    CloseTabs(CloseToday),
    ViewSource,
}

/// `sameAddress` (chromium/shell/lib/url.ts:141-145): two addresses that load the same page.
fn same_address(a: &str, b: &str) -> bool {
    let comparable = |url: &str| pinnable_address(url).unwrap_or_else(|| url.to_owned());
    comparable(a) == comparable(b)
}

/// `isWebPage`: what Chromium shows the source of.
fn is_web_page(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl Controller {
    pub(crate) fn tab_action(
        &mut self,
        window: &WindowId,
        id: &TabId,
        action: TabAction,
    ) -> Result<()> {
        let tab = self
            .model
            .state(window)
            .and_then(|state| state.tab(id))
            .context("The tab has closed")?
            .clone();
        match action {
            TabAction::Duplicate => {
                self.model.open(window, tab.url, true)?;
            }
            TabAction::ViewSource => {
                ensure!(is_web_page(&tab.url), "This page has no source to show");
                self.model
                    .open(window, format!("view-source:{}", tab.url), true)?;
            }
            TabAction::Reload => self.engine.reload(id),
            TabAction::ToggleMute => {
                let muted = tab.muted != Some(true);
                if let Some((tab, _)) = self.model.tab_mut(id) {
                    tab.muted = muted.then_some(true);
                }
                self.engine.set_audio_muted(id, muted);
            }
            TabAction::MoveToSpace(space) => {
                let state = self.model.state(window).context("missing Flecha window")?;
                let placement = match state.locate(id) {
                    Some(Placement::Pinned { .. }) => Placement::Pinned {
                        space: &space,
                        folder: None,
                    },
                    Some(Placement::Today { window: owner, .. }) => {
                        let owner = owner.clone();
                        return self
                            .place_tab(
                                window,
                                id,
                                Placement::Today {
                                    space: &space,
                                    window: &owner,
                                },
                                None,
                            )
                            .then_some(())
                            .context("could not move the tab");
                    }
                    _ => return Ok(()),
                };
                ensure!(
                    self.place_tab(window, id, placement, None),
                    "could not move the tab"
                );
            }
            TabAction::Rename(name) => {
                let name = flecha_core::input_text::trim_input(&name);
                if let Some((tab, _)) = self.model.tab_mut(id) {
                    tab.custom_title = (!name.is_empty()).then(|| name.to_owned());
                }
            }
            TabAction::GoHome => {
                let home = tab.home_url.context("This tab has no pinned URL")?;
                if !self.engine.navigate(id, &home) {
                    // An unloaded tab opens its URL later, where a refusal would go unseen.
                    ensure!(
                        pinnable_address(&home).is_some() || home.starts_with("about:"),
                        "Flecha can’t open the pinned URL; edit it to a web address."
                    );
                    if let Some((tab, _)) = self.model.tab_mut(id) {
                        tab.url = home;
                    }
                }
            }
            TabAction::SetHome => {
                let home = tab
                    .home_url
                    .as_deref()
                    .context("This tab has no pinned URL")?;
                let url =
                    pinnable_address(&tab.url).context("Enter a web address, like example.com.")?;
                if !same_address(&url, home)
                    && let Some((tab, _)) = self.model.tab_mut(id)
                {
                    tab.home_url = Some(url);
                }
            }
            TabAction::CloseTabs(which) => {
                let state = self
                    .model
                    .state_mut(window)
                    .context("missing Flecha window")?;
                for removed in state.close_today_tabs(id, which, now()) {
                    self.engine.close(&removed);
                }
            }
        }
        Ok(())
    }
}

/// One line of a menu.
#[derive(Clone)]
pub(crate) enum Row {
    Item {
        label: String,
        enabled: bool,
        /// Whether a checkable item is on; `None` for the rest.
        checked: Option<bool>,
        run: Run,
    },
    Separator,
}

/// What choosing a row does.
#[derive(Clone)]
pub(crate) enum Run {
    Action(Action),
    Copy(String),
    Rename(TabId),
    EditSpace(SpaceId),
    NewFolder(SpaceId),
    DeleteSpace(SpaceId),
    Submenu(Vec<Row>),
}

fn item(label: impl Into<String>, run: Run) -> Row {
    Row::Item {
        label: label.into(),
        enabled: true,
        checked: None,
        run,
    }
}

fn action(label: impl Into<String>, action: Action) -> Row {
    item(label, Run::Action(action))
}

/// A menu open at the pointer, with the submenu of its row `submenu` beside it.
pub(crate) struct ShellMenu {
    pub(crate) anchor: Point<Pixels>,
    pub(crate) rows: Vec<Row>,
    pub(crate) submenu: Option<usize>,
}

impl ShellMenu {
    /// What the enabled row labelled `label` runs, in the menu or its open submenu.
    fn find(&self, label: &str) -> Option<Run> {
        let submenu = self.submenu.and_then(|index| match self.rows.get(index)? {
            Row::Item {
                run: Run::Submenu(rows),
                ..
            } => Some(rows.as_slice()),
            _ => None,
        });
        self.rows
            .iter()
            .chain(submenu.into_iter().flatten())
            .find_map(|row| match row {
                Row::Item {
                    label: name,
                    enabled: true,
                    run,
                    ..
                } if name == label => Some(run.clone()),
                _ => None,
            })
    }
}

/// `spaceLabel`: a Space's emoji, two spaces, and its name.
pub(crate) fn space_label(space: &flecha_core::Space) -> String {
    match space.emoji.as_deref().filter(|emoji| !emoji.is_empty()) {
        Some(emoji) => format!("{emoji}  {}", space.name),
        None => space.name.clone(),
    }
}

/// `tabMenu` (chromium/shell/background.ts:1111-1162): the rows for `id` as `window` shows it.
pub(crate) fn tab_menu(state: &Controller, window: &WindowId, id: &TabId) -> Option<Vec<Row>> {
    let core = state.model.state(window)?;
    let tab = core.tab(id)?;
    let zone = core.locate(id)?;
    let today = matches!(zone, Placement::Today { .. });
    let incognito = state.model.is_incognito(window);
    let current = core.window(window)?;
    let shown = current.spaces.get(&current.active_space_id);
    let mut rows = Vec::new();
    if current.visible_tabs().len() > 1 && current.visible_tabs().contains(id) {
        rows.push(action("Exit Split View", Action::ExitSplit));
    } else if shown
        .and_then(|space| space.active_tab_id.as_ref())
        .is_some_and(|active| active != id)
    {
        rows.push(action("Open in Split View", Action::Split(id.clone())));
    }
    if !rows.is_empty() {
        rows.push(Row::Separator);
    }
    let name = tab.custom_title.as_ref().unwrap_or(&tab.title);
    let home = tab.home_url.as_deref().filter(|_| !today);
    rows.extend([
        item("Copy Link", Run::Copy(tab.url.clone())),
        item(
            "Copy as Markdown Link",
            Run::Copy(format!("[{name}]({})", tab.url)),
        ),
        item("Rename…", Run::Rename(id.clone())),
        action("Duplicate", Action::Tab(id.clone(), TabAction::Duplicate)),
        action("Open in Peek", Action::PeekTab(id.clone())),
        action("Move to Flechinha", Action::DetachTab(id.clone())),
        Row::Separator,
    ]);
    let mobile = tab.device.is_some();
    if !incognito && !mobile {
        let others: Vec<_> =
            core.spaces()
                .iter()
                .filter(|space| match zone {
                    Placement::Pinned { space: from, .. }
                    | Placement::Today { space: from, .. } => &space.id != from,
                    Placement::Favorite => false,
                })
                .map(|space| {
                    action(
                        space_label(space),
                        Action::Tab(id.clone(), TabAction::MoveToSpace(space.id.clone())),
                    )
                })
                .collect();
        rows.extend([
            if today {
                action(
                    "Pin Tab",
                    Action::Sidebar(SidebarAction::Pin(id.clone(), None)),
                )
            } else {
                action(
                    "Unpin Tab",
                    Action::Sidebar(SidebarAction::ToToday(id.clone())),
                )
            },
            if matches!(zone, Placement::Favorite) {
                action(
                    "Remove from Favorites",
                    Action::Sidebar(SidebarAction::ToToday(id.clone())),
                )
            } else {
                action(
                    "Add to Favorites",
                    Action::Sidebar(SidebarAction::Favorite(id.clone())),
                )
            },
            Row::Item {
                label: "Move to Space".into(),
                enabled: !others.is_empty(),
                checked: None,
                run: Run::Submenu(others),
            },
            Row::Separator,
        ]);
    }
    rows.push(Row::Item {
        label: "Reload".into(),
        enabled: state.engine.key(id).is_some(),
        checked: None,
        run: Run::Action(Action::Tab(id.clone(), TabAction::Reload)),
    });
    rows.push(action(
        if tab.muted == Some(true) {
            "Unmute Tab"
        } else {
            "Mute Tab"
        },
        Action::Tab(id.clone(), TabAction::ToggleMute),
    ));
    if home.is_some_and(|home| !same_address(home, &tab.url)) {
        rows.push(action(
            "Go Back to Pinned URL",
            Action::Tab(id.clone(), TabAction::GoHome),
        ));
        if pinnable_address(&tab.url).is_some() {
            rows.push(action(
                "Set Pinned URL to Current Page",
                Action::Tab(id.clone(), TabAction::SetHome),
            ));
        }
    }
    rows.push(Row::Separator);
    rows.push(action(
        if today { "Close Tab" } else { "Unload Tab" },
        Action::CloseTab(id.clone()),
    ));
    if today {
        rows.extend([
            action(
                "Close Tabs Below",
                Action::Tab(id.clone(), TabAction::CloseTabs(CloseToday::Below)),
            ),
            action(
                "Close Other Tabs",
                Action::Tab(id.clone(), TabAction::CloseTabs(CloseToday::Others)),
            ),
        ]);
    }
    Some(rows)
}

/// `spaceMenu` (chromium/shell/background.ts:1164-1179). An incognito window has no Spaces to
/// manage, so it has none.
pub(crate) fn space_menu(state: &Controller, window: &WindowId, id: &SpaceId) -> Option<Vec<Row>> {
    if state.model.is_incognito(window) {
        return None;
    }
    let space = state.model.core.space(id)?;
    Some(vec![
        item("Edit Space…", Run::EditSpace(id.clone())),
        item("New Folder", Run::NewFolder(id.clone())),
        action(
            "Open in New Window",
            Action::Space(id.clone(), SpaceAction::OpenInNewWindow),
        ),
        Row::Separator,
        Row::Item {
            label: "Separate Cookies and Logins".into(),
            enabled: true,
            checked: Some(space.isolated),
            run: Run::Action(Action::Space(
                id.clone(),
                SpaceAction::Isolate(!space.isolated),
            )),
        },
        Row::Separator,
        Row::Item {
            label: "Delete Space…".into(),
            enabled: state.model.core.spaces().len() > 1,
            checked: None,
            run: Run::DeleteSpace(id.clone()),
        },
    ])
}

/// `.shell-menu-item` is one line of body text with 4px above and below.
const ROW: f32 = tokens::MENU_ITEM_PAD_Y * 2. + tokens::TEXT_MD * tokens::LINE_HEIGHT;
const SEPARATOR: f32 = tokens::MENU_SEPARATOR + tokens::MENU_SEPARATOR_MARGIN_Y * 2.;

impl Shell {
    pub(crate) fn open_tab_menu(
        &mut self,
        id: &TabId,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = tab_menu(&self.state.borrow(), &self.id, id);
        if let Some(rows) = rows {
            self.open_menu(rows, anchor, window, cx);
        }
    }

    pub(crate) fn open_space_menu(
        &mut self,
        id: &SpaceId,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = space_menu(&self.state.borrow(), &self.id, id);
        if let Some(rows) = rows {
            self.open_menu(rows, anchor, window, cx);
        }
    }

    fn open_menu(
        &mut self,
        rows: Vec<Row>,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = ShellMenu {
            anchor,
            rows,
            submenu: None,
        };
        self.show_overlay(Overlay::Menu(menu), window, cx);
    }

    /// Copies `text` and confirms it on the status line, as the shipping shell's flash does.
    pub(crate) fn copy_text(&self, text: String, message: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let mut state = self.state.borrow_mut();
        let session = state.model.incognito_session(&self.id);
        state.status.set(session, message);
        cx.notify();
    }

    /// Chooses the open tab or Space menu's enabled row labelled `label`, as clicking it does.
    pub(crate) fn use_shell_menu_row(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let Overlay::Menu(menu) = &self.overlay else {
            anyhow::bail!("no tab or Space menu is open");
        };
        let run = menu
            .find(label)
            .ok_or_else(|| anyhow::anyhow!("the menu has no enabled {label} row"))?;
        self.run_menu(run, window, cx);
        Ok(())
    }

    pub(crate) fn run_menu(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        if let Run::Submenu(_) = run {
            return;
        }
        self.dismiss_overlay(window, cx);
        match run {
            Run::Action(action) => dispatch(&self.state, &self.id, action, cx),
            Run::Copy(text) => {
                let message = if text.starts_with('[') {
                    "Markdown link copied"
                } else {
                    "Link copied"
                };
                self.copy_text(text, message, cx);
            }
            Run::Rename(id) => self.open_rename(&id, window, cx),
            Run::EditSpace(id) => self.open_space_editor(&id, window, cx),
            Run::NewFolder(id) => {
                dispatch(&self.state, &self.id, Action::Switch(id), cx);
                self.show_overlay(Overlay::Folder(None, TextField::default()), window, cx);
            }
            Run::DeleteSpace(id) => self.confirm_delete_space(&id, window, cx),
            Run::Submenu(_) => unreachable!("a submenu row opens its submenu"),
        }
    }

    /// The width a menu's rows need: its longest label in the body text, its padding and
    /// `.shell-menu`'s 180px at least.
    fn menu_width(rows: &[Row], window: &Window, cx: &Context<Self>) -> Pixels {
        let style = window.text_style();
        let font = crate::view::ui_font(cx).map_or(style.font(), gpui::font);
        let checkable = rows.iter().any(|row| {
            matches!(
                row,
                Row::Item {
                    checked: Some(_),
                    ..
                }
            )
        });
        let widest = rows
            .iter()
            .filter_map(|row| match row {
                Row::Item { label, run, .. } => {
                    let run_style = TextRun {
                        len: label.len(),
                        font: font.clone(),
                        color: style.color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let width = window
                        .text_system()
                        .shape_line(
                            label.clone().into(),
                            px(tokens::TEXT_MD),
                            &[run_style],
                            None,
                        )
                        .width;
                    // `.has-submenu::after`: a `›` 16px past the label.
                    let arrow = if matches!(run, Run::Submenu(_)) {
                        px(tokens::MENU_SUBMENU_ARROW_GAP + tokens::TEXT_MD)
                    } else {
                        px(0.)
                    };
                    Some(width + arrow)
                }
                Row::Separator => None,
            })
            .fold(px(0.), Pixels::max);
        let left = if checkable {
            tokens::MENU_CHECKABLE_PAD
        } else {
            tokens::MENU_ITEM_PAD_X
        };
        (widest + px(left + tokens::MENU_ITEM_PAD_X + tokens::MENU_PAD * 2.))
            .max(px(tokens::MENU_MIN_WIDTH))
            .ceil()
    }

    fn menu_height(rows: &[Row]) -> Pixels {
        let height: f32 = rows
            .iter()
            .map(|row| match row {
                Row::Item { .. } => ROW,
                Row::Separator => SEPARATOR,
            })
            .sum();
        px(height + tokens::MENU_PAD * 2.)
    }

    /// `.shell-menu-item`s: the pointer's row, and the row whose submenu is open, take the
    /// selection colour. `submenu` is `Some` for the main menu, with its open row.
    fn menu_rows(&self, rows: &[Row], submenu: Option<Option<usize>>, cx: &Context<Self>) -> Div {
        let checkable = rows.iter().any(|row| {
            matches!(
                row,
                Row::Item {
                    checked: Some(_),
                    ..
                }
            )
        });
        let palette = palette();
        div()
            .flex()
            .flex_col()
            .children(rows.iter().enumerate().map(|(index, row)| {
                let Row::Item {
                    label,
                    enabled,
                    checked,
                    run,
                } = row
                else {
                    return div()
                        .flex_none()
                        .h(px(tokens::MENU_SEPARATOR))
                        .mx(px(tokens::MENU_SEPARATOR_MARGIN_X))
                        .my(px(tokens::MENU_SEPARATOR_MARGIN_Y))
                        .bg(palette.surface_line)
                        .into_any_element();
                };
                let open = submenu.is_some_and(|open| open == Some(index));
                let has_submenu = matches!(run, Run::Submenu(_));
                let id = (
                    if submenu.is_some() {
                        "menu-row"
                    } else {
                        "submenu-row"
                    },
                    index,
                );
                let row = div()
                    .id(id)
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(ROW))
                    .pl(px(if checkable {
                        tokens::MENU_CHECKABLE_PAD
                    } else {
                        tokens::MENU_ITEM_PAD_X
                    }))
                    .pr(px(tokens::MENU_ITEM_PAD_X))
                    .rounded(px(tokens::MENU_ITEM_RADIUS))
                    .cursor_default()
                    .when(*checked == Some(true), |row| {
                        row.child(
                            div()
                                .absolute()
                                .left(px(tokens::MENU_CHECKABLE_PAD - tokens::MENU_CHECK_OFFSET))
                                .child("✓"),
                        )
                    })
                    .child(div().flex_1().child(label.clone()))
                    .when(has_submenu, |row| {
                        row.child(
                            div()
                                .ml(px(tokens::MENU_SUBMENU_ARROW_GAP))
                                .opacity(tokens::MENU_ACCENT_OPACITY)
                                .child("›"),
                        )
                    })
                    .when(open, |row| {
                        row.bg(palette.selection).text_color(palette.selection_text)
                    });
                if !*enabled {
                    return row
                        .opacity(tokens::MENU_DISABLED_OPACITY)
                        .into_any_element();
                }
                let run = run.clone();
                row.hover(|row| row.bg(palette.selection).text_color(palette.selection_text))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if has_submenu {
                            this.open_submenu(Some(index), cx);
                        } else {
                            this.run_menu(run.clone(), window, cx);
                        }
                    }))
                    .when(submenu.is_some(), |row| {
                        row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.open_submenu(has_submenu.then_some(index), cx);
                            }
                        }))
                    })
                    .into_any_element()
            }))
    }

    fn open_submenu(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if let Overlay::Menu(menu) = &mut self.overlay
            && menu.submenu != index
        {
            menu.submenu = index;
            cx.notify();
        }
    }

    /// `.shell-menu` at the pointer, kept inside the window, and its open submenu beside it.
    pub(crate) fn shell_menu_panel(
        &self,
        menu: &ShellMenu,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let viewport = window.viewport_size();
        let limit = viewport.height - px(tokens::MENU_VIEWPORT_INSET);
        let frame = |left: Pixels, top: Pixels, width: Pixels, height: Pixels| {
            let palette = palette();
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(width)
                .h(height)
                .occlude()
                .overflow_hidden()
                .p(px(tokens::MENU_PAD))
                .rounded(px(tokens::RADIUS_MD))
                .bg(palette.surface)
                .text_color(palette.text)
                .shadow(palette.shadow_pop())
                .whitespace_nowrap()
                .font_weight(FontWeight::NORMAL)
        };
        let width = Self::menu_width(&menu.rows, window, cx).min(viewport.width);
        let height = Self::menu_height(&menu.rows).min(limit);
        let left = menu.anchor.x.clamp(px(0.), viewport.width - width);
        let top = menu.anchor.y.clamp(px(0.), viewport.height - height);
        let main = frame(left, top, width, height)
            .id("tab-space-menu")
            .child(self.menu_rows(&menu.rows, Some(menu.submenu), cx));
        let submenu = menu.submenu.and_then(|index| {
            let Row::Item {
                run: Run::Submenu(rows),
                ..
            } = menu.rows.get(index)?
            else {
                return None;
            };
            let sub_width = Self::menu_width(rows, window, cx).min(viewport.width);
            let sub_height = Self::menu_height(rows).min(limit);
            let row_top = top + Self::menu_height(&menu.rows[..index]) - px(tokens::MENU_PAD);
            let right = left + width;
            let sub_left = if right + sub_width <= viewport.width {
                right
            } else {
                (left - sub_width).max(px(0.))
            };
            let sub_top = row_top.clamp(px(0.), viewport.height - sub_height);
            Some(
                frame(sub_left, sub_top, sub_width, sub_height)
                    .id("tab-space-submenu")
                    .child(self.menu_rows(rows, None, cx)),
            )
        });
        deferred(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .child(div().absolute().inset_0().on_any_mouse_down(cx.listener(
                    |this, _: &MouseDownEvent, window, cx| {
                        this.dismiss_overlay(window, cx);
                        cx.stop_propagation();
                    },
                )))
                .child(main)
                .children(submenu),
        )
    }

    pub(crate) fn open_rename(&mut self, id: &TabId, window: &mut Window, cx: &mut Context<Self>) {
        let name = {
            let state = self.state.borrow();
            let Some(tab) = state.model.state(&self.id).and_then(|core| core.tab(id)) else {
                return;
            };
            tab_label(tab)
        };
        self.show_overlay(
            Overlay::Rename(id.clone(), TextField::new(name, true)),
            window,
            cx,
        );
    }

    pub(crate) fn rename_key(
        &mut self,
        stroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stroke.key == "enter" && stroke.modifiers == gpui::Modifiers::none() {
            self.save_rename(window, cx);
        } else if let Overlay::Rename(_, input) = &mut self.overlay {
            input.key(stroke, cx);
            cx.notify();
        }
    }

    fn save_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Rename(id, input) = &self.overlay else {
            return;
        };
        let action = Action::Tab(id.clone(), TabAction::Rename(input.text().to_owned()));
        self.dismiss_overlay(window, cx);
        dispatch(&self.state, &self.id, action, cx);
    }

    pub(crate) fn rename_panel(&self, input: &TextField, cx: &Context<Self>) -> Div {
        overlay_panel()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Rename tab"))
            .child(input.render("Tab name"))
            .child(div().flex().justify_end().child(
                primary_button("save-rename", "Save").on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| this.save_rename(window, cx)),
                ),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_today(controller: &mut Controller, window: &WindowId, url: &str) -> TabId {
        controller.model.open(window, url.into(), true).unwrap()
    }

    fn labels(rows: &[Row]) -> Vec<&str> {
        rows.iter()
            .map(|row| match row {
                Row::Item { label, .. } => label.as_str(),
                Row::Separator => "-",
            })
            .collect()
    }

    fn run<'a>(rows: &'a [Row], label: &str) -> &'a Run {
        rows.iter()
            .find_map(|row| match row {
                Row::Item {
                    label: name, run, ..
                } if name == label => Some(run),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} row"))
    }

    fn act(controller: &mut Controller, window: &WindowId, rows: &[Row], label: &str) {
        let Run::Action(action) = run(rows, label).clone() else {
            panic!("{label} is not an action");
        };
        controller.act(window, action).unwrap();
    }

    #[test]
    fn tab_menu_rows_follow_the_tab_zone_and_act_on_the_tab() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let first = open_today(&mut controller, &window, "https://one.example/");
        let second = open_today(&mut controller, &window, "https://two.example/");
        let third = open_today(&mut controller, &window, "https://three.example/");
        let today = &controller.model.core.focused_window().spaces
            [&controller.model.core.focused_window().active_space_id]
            .today;
        assert_eq!(today, &[third.clone(), second.clone(), first.clone()]);

        let rows = tab_menu(&controller, &window, &second).unwrap();
        assert_eq!(
            labels(&rows),
            [
                "Open in Split View",
                "-",
                "Copy Link",
                "Copy as Markdown Link",
                "Rename…",
                "Duplicate",
                "Open in Peek",
                "Move to Flechinha",
                "-",
                "Pin Tab",
                "Add to Favorites",
                "Move to Space",
                "-",
                "Reload",
                "Mute Tab",
                "-",
                "Close Tab",
                "Close Tabs Below",
                "Close Other Tabs",
            ]
        );
        assert!(
            matches!(run(&rows, "Copy as Markdown Link"), Run::Copy(text) if text == "[https://two.example/](https://two.example/)")
        );
        // One Space: nowhere to move to.
        assert!(rows.iter().any(|row| matches!(row,
            Row::Item { label, enabled: false, .. } if label == "Move to Space")));

        act(&mut controller, &window, &rows, "Mute Tab");
        assert_eq!(
            controller.model.core.tab(&second).unwrap().muted,
            Some(true)
        );
        let rows = tab_menu(&controller, &window, &second).unwrap();
        act(&mut controller, &window, &rows, "Unmute Tab");
        assert_eq!(controller.model.core.tab(&second).unwrap().muted, None);

        controller
            .act(
                &window,
                Action::Tab(second.clone(), TabAction::Rename("  Named  ".into())),
            )
            .unwrap();
        assert_eq!(
            controller
                .model
                .core
                .tab(&second)
                .unwrap()
                .custom_title
                .as_deref(),
            Some("Named")
        );
        controller
            .act(
                &window,
                Action::Tab(second.clone(), TabAction::Rename(" ".into())),
            )
            .unwrap();
        assert_eq!(
            controller.model.core.tab(&second).unwrap().custom_title,
            None
        );

        act(&mut controller, &window, &rows, "Close Tabs Below");
        assert!(controller.model.core.tab(&first).is_none());
        assert!(controller.model.core.tab(&third).is_some());

        let rows = tab_menu(&controller, &window, &second).unwrap();
        act(&mut controller, &window, &rows, "Duplicate");
        let open = controller.model.core.open_tabs();
        assert_eq!(
            open.values()
                .filter(|tab| tab.url == "https://two.example/")
                .count(),
            2
        );
        drop(open);
        act(&mut controller, &window, &rows, "Close Other Tabs");
        assert_eq!(
            controller
                .model
                .core
                .open_tabs()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [&second]
        );
    }

    #[test]
    fn pinned_tabs_unload_and_go_back_to_their_pinned_url() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let tab = open_today(&mut controller, &window, "https://home.example/");
        let rows = tab_menu(&controller, &window, &tab).unwrap();
        act(&mut controller, &window, &rows, "Pin Tab");
        assert!(matches!(
            controller.model.core.locate(&tab),
            Some(Placement::Pinned { .. })
        ));
        controller.model.core.tab_mut(&tab).unwrap().url = "https://home.example/away".into();
        let rows = tab_menu(&controller, &window, &tab).unwrap();
        assert!(labels(&rows).contains(&"Unload Tab"));
        assert!(!labels(&rows).contains(&"Close Tabs Below"));
        act(
            &mut controller,
            &window,
            &rows,
            "Set Pinned URL to Current Page",
        );
        assert_eq!(
            controller.model.core.tab(&tab).unwrap().home_url.as_deref(),
            Some("https://home.example/away")
        );
        controller.model.core.tab_mut(&tab).unwrap().url = "https://elsewhere.example/".into();
        let rows = tab_menu(&controller, &window, &tab).unwrap();
        act(&mut controller, &window, &rows, "Go Back to Pinned URL");
        assert_eq!(
            controller.model.core.tab(&tab).unwrap().url,
            "https://home.example/away"
        );
        let rows = tab_menu(&controller, &window, &tab).unwrap();
        assert!(!labels(&rows).contains(&"Go Back to Pinned URL"));
        act(&mut controller, &window, &rows, "Unpin Tab");
        assert!(matches!(
            controller.model.core.locate(&tab),
            Some(Placement::Today { .. })
        ));
    }

    #[test]
    fn move_to_space_keeps_the_zone_and_incognito_menus_offer_no_spaces() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let first = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        controller.act(&window, Action::NewSpace).unwrap();
        let second = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        controller
            .act(&window, Action::Switch(first.clone()))
            .unwrap();
        let tab = open_today(&mut controller, &window, "https://move.example/");
        let rows = tab_menu(&controller, &window, &tab).unwrap();
        let Run::Submenu(spaces) = run(&rows, "Move to Space") else {
            panic!("Move to Space has no submenu");
        };
        assert_eq!(spaces.len(), 1);
        let Row::Item {
            run: Run::Action(action),
            ..
        } = spaces[0].clone()
        else {
            panic!("the Space row is not an action");
        };
        controller.act(&window, action).unwrap();
        assert!(matches!(
            controller.model.core.locate(&tab),
            Some(Placement::Today { space, .. }) if space == &second
        ));
        assert_ne!(controller.model.active(&window), Some(&tab));

        let private = controller.model.open_incognito_window();
        let hidden = controller
            .model
            .open(&private, "https://private.example/".into(), true)
            .unwrap();
        let rows = tab_menu(&controller, &private, &hidden).unwrap();
        for absent in ["Pin Tab", "Add to Favorites", "Move to Space"] {
            assert!(!labels(&rows).contains(&absent), "{absent}");
        }
        assert!(space_menu(&controller, &private, &first).is_none());
    }
}
