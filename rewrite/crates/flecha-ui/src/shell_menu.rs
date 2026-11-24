//! The menus the shipping shell draws in the page (`showPageMenu`, chromium/shell/settingsUi.ts):
//! the window menu behind the sidebar's menu button (`windowMenu`, ui/app.ts) and the Space menu
//! (`spaceMenu`, chromium/shell/background.ts).

use flecha_cef::PageZoom;
use flecha_core::shortcuts::{self, Keybindings};
use gpui::{
    Anchor, Bounds, ClickEvent, Context, Div, InteractiveElement as _, IntoElement, MouseDownEvent,
    ParentElement as _, Pixels, Point, StatefulInteractiveElement as _, Styled as _, Window,
    anchored, deferred, div, point, prelude::FluentBuilder as _, px, relative,
};

use crate::command_bar::Command;
use crate::settings_view::SettingsTab;
use crate::shortcuts::{Shortcut, platform};
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, menu_item};
use crate::{Action, dispatch};

/// What a menu item does: a shortcut's command, so the menu and the keys share one path, or an
/// action, or a surface that has no shortcut.
#[derive(Clone)]
pub(crate) enum Run {
    Shortcut(Shortcut),
    Action(Action),
    Downloads,
    Archive,
}

pub(crate) enum Entry {
    Item {
        label: &'static str,
        accel: String,
        checked: Option<bool>,
        run: Run,
    },
    Submenu {
        label: &'static str,
        entries: Vec<Entry>,
    },
    Separator,
}

impl Entry {
    fn item(label: &'static str, run: Run, accel: String) -> Self {
        Self::Item {
            label,
            accel,
            checked: None,
            run,
        }
    }
}

/// Where a menu opens: below the button that opens it, or at the pointer (settingsUi.ts:201-205).
#[derive(Clone, Copy)]
pub(crate) enum MenuAnchor {
    Below(Bounds<Pixels>),
    At(Point<Pixels>),
}

pub(crate) struct ShellMenu {
    entries: Vec<Entry>,
    anchor: MenuAnchor,
    /// The item whose submenu is open.
    open: Option<usize>,
}

/// `windowMenu()` (ui/app.ts): what the macOS menu bar holds, for a window without one. The rows
/// whose surface the rewrite does not have yet are left out (see `OMITTED` in the tests).
pub(crate) fn window_entries(incognito: bool, bindings: &Keybindings) -> Vec<Entry> {
    let platform = platform();
    let keys = |id| shortcuts::display_keys(shortcuts::keys_for(bindings, id, platform), platform);
    // Chromium's own accelerators, which the shipping menu names as text.
    let fixed = |keys| shortcuts::display_keys(Some(keys), platform);
    let shortcut = |label, shortcut, accel| Entry::item(label, Run::Shortcut(shortcut), accel);
    let mut entries = Vec::new();
    // An incognito window can't open a regular one from here; Ctrl+N still does.
    if !incognito {
        entries.push(shortcut(
            "New window",
            Shortcut::NewWindow,
            fixed("CmdOrCtrl+N"),
        ));
    }
    entries.push(shortcut(
        "New incognito window",
        Shortcut::NewIncognitoWindow,
        fixed("CmdOrCtrl+Shift+N"),
    ));
    // A Flechinha takes a regular window's Space (`incognito::refusal`).
    if !incognito {
        entries.push(shortcut(
            "New Flechinha window",
            Shortcut::Command(Command::Flechinha),
            keys("newFlechinha"),
        ));
    }
    entries.push(Entry::Separator);
    // An incognito window keeps no history and no archive.
    if !incognito {
        entries.push(shortcut("History", Shortcut::History, keys("history")));
        entries.push(Entry::item("Archive", Run::Archive, String::new()));
    }
    entries.extend([
        Entry::item("Downloads", Run::Downloads, String::new()),
        shortcut(
            "Boosts",
            Shortcut::SettingsTab(SettingsTab::Boosts),
            String::new(),
        ),
        Entry::Separator,
        shortcut("Find in page…", Shortcut::Find, keys("find")),
        Entry::Submenu {
            label: "Zoom",
            entries: vec![
                shortcut("Zoom in", Shortcut::Zoom(PageZoom::In), keys("zoomIn")),
                shortcut("Zoom out", Shortcut::Zoom(PageZoom::Out), keys("zoomOut")),
                shortcut(
                    "Actual size",
                    Shortcut::Zoom(PageZoom::Reset),
                    keys("actualSize"),
                ),
            ],
        },
        shortcut("Developer tools", Shortcut::DevTools, keys("devTools")),
        Entry::Separator,
        shortcut(
            "Import from another browser…",
            Shortcut::SettingsTab(SettingsTab::Import),
            String::new(),
        ),
        shortcut(
            "Settings",
            Shortcut::Command(Command::OpenSettings),
            keys("settings"),
        ),
        Entry::Separator,
        shortcut("Quit Flecha", Shortcut::Quit, keys("quit")),
    ]);
    entries
}

/// `spaceMenu()` (chromium/shell/background.ts), as far as the rewrite's Spaces go: whether the
/// shown Space keeps its own cookies and logins. An incognito window has no Space menu.
pub(crate) fn space_entries(isolated: bool) -> Vec<Entry> {
    vec![Entry::Item {
        label: "Separate Cookies and Logins",
        accel: String::new(),
        checked: Some(isolated),
        run: Run::Action(Action::Isolate),
    }]
}

impl Shell {
    pub(crate) fn open_menu(
        &mut self,
        entries: Vec<Entry>,
        anchor: MenuAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = ShellMenu {
            entries,
            anchor,
            open: None,
        };
        self.show_overlay(Overlay::Menu(menu), window, cx);
    }

    fn run_menu(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        // The menu goes first: Find, Zoom and DevTools act only while no overlay is open.
        self.dismiss_overlay(window, cx);
        match run {
            Run::Shortcut(shortcut) => {
                self.invoke_shortcut(shortcut, window, cx);
            }
            Run::Action(action) => dispatch(&self.state, &self.id, action, cx),
            Run::Downloads => self.toggle_downloads(window, cx),
            Run::Archive => self.toggle_archive(window, cx),
        }
        cx.notify();
    }

    fn open_submenu(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if let Overlay::Menu(menu) = &mut self.overlay
            && menu.open != index
        {
            menu.open = index;
            cx.notify();
        }
    }

    /// `.shell-menu` (chromium/shell/shell.css:72-135) over a layer that closes it on any click.
    pub(crate) fn menu_panel(&self, menu: &ShellMenu, cx: &Context<Self>) -> impl IntoElement {
        let list = self
            .menu_list("shell-menu", &menu.entries, Some(menu.open), cx)
            .occlude();
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
                .child(
                    match menu.anchor {
                        MenuAnchor::Below(button) => anchored()
                            .attach(button, Anchor::BottomLeft)
                            .offset(point(px(0.), px(tokens::MENU_ANCHOR_GAP))),
                        MenuAnchor::At(pointer) => anchored().position(pointer),
                    }
                    .snap_to_window_with_margin(px(tokens::MENU_WINDOW_MARGIN))
                    .child(list),
                ),
        )
    }

    /// One level of a menu. `open` is the top level's open submenu, `None` for a submenu.
    fn menu_list(
        &self,
        id: &'static str,
        entries: &[Entry],
        open: Option<Option<usize>>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let checkable = entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::Item {
                    checked: Some(_),
                    ..
                }
            )
        });
        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| match entry {
                Entry::Separator => div()
                    .h(px(tokens::MENU_SEPARATOR))
                    .mx(px(tokens::MENU_SEPARATOR_MARGIN_X))
                    .my(px(tokens::MENU_SEPARATOR_MARGIN_Y))
                    .bg(palette().surface_line)
                    .into_any_element(),
                Entry::Item {
                    label,
                    accel,
                    checked,
                    run,
                } => {
                    let run = run.clone();
                    menu_row(label, checkable, *checked == Some(true))
                        .when(!accel.is_empty(), |row| {
                            row.child(
                                div()
                                    .ml(px(tokens::MENU_ACCEL_GAP))
                                    .opacity(tokens::MENU_ACCEL_OPACITY)
                                    .child(accel.clone()),
                            )
                        })
                        .when(open.is_some(), |row| {
                            row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered {
                                    this.open_submenu(None, cx);
                                }
                            }))
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.run_menu(run.clone(), window, cx);
                        }))
                        .into_any_element()
                }
                Entry::Submenu { label, entries } => {
                    let shown = open == Some(Some(index));
                    menu_row(label, checkable, false)
                        .relative()
                        .when(shown, |row| {
                            row.bg(palette().selection)
                                .text_color(palette().selection_text)
                        })
                        .child(
                            div()
                                .ml(px(tokens::MENU_SUBMENU_ARROW_GAP))
                                .opacity(tokens::MENU_ACCEL_OPACITY)
                                .child("›"),
                        )
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.open_submenu(Some(index), cx);
                            }
                        }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.open_submenu(Some(index), cx);
                        }))
                        .when(shown, |row| {
                            // Beside its item, its first item level with it (settingsUi.ts:90-94).
                            row.child(
                                div().absolute().top_0().left(relative(1.)).child(
                                    anchored()
                                        .offset(point(
                                            px(tokens::SUBMENU_GAP + tokens::MENU_PAD),
                                            px(-tokens::SUBMENU_RAISE),
                                        ))
                                        .snap_to_window_with_margin(px(tokens::MENU_WINDOW_MARGIN))
                                        .child(self.menu_list("shell-submenu", entries, None, cx)),
                                ),
                            )
                        })
                        .into_any_element()
                }
            });
        div()
            .id(id)
            .flex()
            .flex_col()
            .min_w(px(tokens::MENU_MIN_WIDTH))
            .p(px(tokens::MENU_PAD))
            .rounded(px(tokens::RADIUS_MD))
            .bg(palette().surface)
            .text_color(palette().text)
            .text_size(px(tokens::TEXT_MD))
            .shadow(palette().shadow_pop())
            .occlude()
            .children(rows)
    }
}

/// `.shell-menu-item` (chromium/shell/shell.css:85-131): its label, then what goes on its right.
fn menu_row(label: &'static str, checkable: bool, checked: bool) -> gpui::Stateful<Div> {
    menu_item(label, "")
        .flex()
        .items_center()
        .when(checkable, |row| {
            row.relative().pl(px(tokens::MENU_CHECKABLE_PAD_LEFT))
        })
        .when(checked, |row| {
            row.child(
                div()
                    .absolute()
                    .left(px(
                        tokens::MENU_CHECKABLE_PAD_LEFT - tokens::MENU_CHECK_OFFSET
                    ))
                    .child("✓"),
            )
        })
        .child(div().flex_1().child(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipping rows the rewrite has no surface for yet.
    const OMITTED: &[&str] = &[
        // No extensions in the rewrite.
        "Extensions",
        // CEF prints from an off-screen browser only through a CefPrintHandler, which the
        // rewrite does not have on Linux.
        "Print…",
        // No help pages and no About page in the rewrite.
        "Help",
        "Keyboard shortcuts",
        "Flecha help",
        "About Flecha",
    ];
    /// Rows the rewrite adds: the archive, which the shipping shell reaches from its sidebar.
    const ADDED: &[&str] = &["Archive"];

    fn labels(entries: &[Entry]) -> Vec<&'static str> {
        entries
            .iter()
            .flat_map(|entry| match entry {
                Entry::Item { label, .. } => vec![*label],
                Entry::Submenu { label, entries } => {
                    std::iter::once(*label).chain(labels(entries)).collect()
                }
                Entry::Separator => vec!["-"],
            })
            .collect()
    }

    /// `windowMenu()`'s labels in order, with `-` for a separator.
    fn shipping() -> Vec<String> {
        let source = crate::tokens::tests::source("ui/app.ts");
        let body = source
            .split_once("function windowMenu()")
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .expect("app.ts keeps windowMenu")
            .0;
        let mut found: Vec<(usize, String)> = ["item('", "label: '"]
            .into_iter()
            .flat_map(|prefix| {
                body.match_indices(prefix).map(move |(index, _)| {
                    let rest = &body[index + prefix.len()..];
                    (index, rest.split_once('\'').unwrap().0.to_owned())
                })
            })
            .collect();
        found.extend(
            body.match_indices("{ separator: true }")
                .map(|(index, _)| (index, "-".to_owned())),
        );
        found.sort();
        found.into_iter().map(|(_, label)| label).collect()
    }

    #[test]
    fn the_window_menu_lists_the_shipping_rows_in_order() {
        let shipping = shipping();
        assert!(shipping.iter().any(|label| label == "Quit Flecha"));
        let mut expected: Vec<String> = shipping
            .into_iter()
            .filter(|label| !OMITTED.contains(&label.as_str()))
            .collect();
        // A separator left without rows after it (the omitted Help and About) goes too.
        expected.dedup_by(|a, b| a == "-" && b == "-");
        let history = expected
            .iter()
            .position(|label| label == "History")
            .unwrap();
        expected.insert(history + 1, ADDED[0].to_owned());
        let rows = labels(&window_entries(false, &Keybindings::new()));
        assert_eq!(rows, expected);
    }

    #[test]
    fn the_window_menu_shows_the_configured_keys() {
        let mut bindings = Keybindings::new();
        bindings.insert("find".into(), Some("CmdOrCtrl+Shift+F".into()));
        let entries = window_entries(false, &bindings);
        let accel = |wanted: &str| {
            entries.iter().find_map(|entry| match entry {
                Entry::Item { label, accel, .. } if *label == wanted => Some(accel.clone()),
                _ => None,
            })
        };
        let display = |keys| shortcuts::display_keys(Some(keys), platform());
        assert_eq!(
            accel("Find in page…").unwrap(),
            display("CmdOrCtrl+Shift+F")
        );
        assert_eq!(accel("New window").unwrap(), display("CmdOrCtrl+N"));
        assert_eq!(
            accel("History").unwrap(),
            display(shortcuts::default_keys("history", platform()).unwrap())
        );
        assert!(accel("Downloads").unwrap().is_empty());
    }

    #[test]
    fn an_incognito_window_menu_leaves_out_what_it_refuses() {
        let rows = labels(&window_entries(true, &Keybindings::new()));
        for refused in ["New window", "New Flechinha window", "History", "Archive"] {
            assert!(!rows.contains(&refused), "{refused}");
        }
        assert!(rows.contains(&"New incognito window"));
        assert!(rows.contains(&"Settings"));
        for entry in window_entries(true, &Keybindings::new()) {
            if let Entry::Item {
                run: Run::Action(action),
                ..
            } = entry
            {
                assert!(crate::incognito::refusal(&action).is_none());
            }
        }
    }

    #[test]
    fn the_space_menu_checks_separate_cookies_for_an_isolated_space() {
        for isolated in [false, true] {
            let entries = space_entries(isolated);
            assert!(matches!(
                &entries[..],
                [Entry::Item {
                    label: "Separate Cookies and Logins",
                    checked: Some(checked),
                    run: Run::Action(Action::Isolate),
                    ..
                }] if *checked == isolated
            ));
        }
    }
}
