//! The sidebar's menus: a tab's (`tabMenu`, chromium/shell/background.ts:1111-1162) and a Space's
//! (`spaceMenu`, background.ts:1164-1179), which `shell_menu` draws, what they do to a tab, and
//! the field that renames a tab.

use anyhow::{Context as _, Result, ensure};
use flecha_core::{CloseToday, Placement, SpaceId, TabId, WindowId, suggestions::pinnable_address};
use gpui::{
    ClickEvent, ClipboardItem, Context, Div, FontWeight, ParentElement as _, Pixels, Point,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::shell_menu::{Entry, MenuAnchor, Run};
use crate::sidebar::{SidebarAction, tab_label};
use crate::space_editor::SpaceAction;
use crate::text_field::TextField;
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

fn action(label: impl Into<SharedString>, action: Action) -> Entry {
    Entry::item(label, Run::Action(action), String::new())
}

fn item(label: impl Into<SharedString>, run: Run) -> Entry {
    Entry::item(label, run, String::new())
}

/// `spaceLabel`: a Space's emoji, two spaces, and its name.
pub(crate) fn space_label(space: &flecha_core::Space) -> String {
    match space.emoji.as_deref().filter(|emoji| !emoji.is_empty()) {
        Some(emoji) => format!("{emoji}  {}", space.name),
        None => space.name.clone(),
    }
}

/// `tabMenu` (chromium/shell/background.ts:1111-1162): the entries for `id` as `window` shows it.
pub(crate) fn tab_menu(state: &Controller, window: &WindowId, id: &TabId) -> Option<Vec<Entry>> {
    let core = state.model.state(window)?;
    let tab = core.tab(id)?;
    let zone = core.locate(id)?;
    let today = matches!(zone, Placement::Today { .. });
    let incognito = state.model.is_incognito(window);
    let current = core.window(window)?;
    let shown = current.spaces.get(&current.active_space_id);
    let mut entries = Vec::new();
    if current.visible_tabs().len() > 1 && current.visible_tabs().contains(id) {
        entries.push(action("Exit Split View", Action::ExitSplit));
    } else if shown
        .and_then(|space| space.active_tab_id.as_ref())
        .is_some_and(|active| active != id)
    {
        entries.push(action("Open in Split View", Action::Split(id.clone())));
    }
    if !entries.is_empty() {
        entries.push(Entry::Separator);
    }
    let name = tab.custom_title.as_ref().unwrap_or(&tab.title);
    let home = tab.home_url.as_deref().filter(|_| !today);
    entries.extend([
        item("Copy Link", Run::Copy(tab.url.clone(), "Link copied")),
        item(
            "Copy as Markdown Link",
            Run::Copy(format!("[{name}]({})", tab.url), "Markdown link copied"),
        ),
        item("Rename…", Run::Rename(id.clone())),
        action("Duplicate", Action::Tab(id.clone(), TabAction::Duplicate)),
        action("Open in Peek", Action::PeekTab(id.clone())),
        action("Move to Flechinha", Action::DetachTab(id.clone())),
        Entry::Separator,
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
        entries.extend([
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
            Entry::Submenu {
                label: "Move to Space".into(),
                entries: others,
            },
            Entry::Separator,
        ]);
    }
    entries.push(
        action("Reload", Action::Tab(id.clone(), TabAction::Reload))
            .enabled(state.engine.key(id).is_some()),
    );
    entries.push(action(
        if tab.muted == Some(true) {
            "Unmute Tab"
        } else {
            "Mute Tab"
        },
        Action::Tab(id.clone(), TabAction::ToggleMute),
    ));
    if home.is_some_and(|home| !same_address(home, &tab.url)) {
        entries.push(action(
            "Go Back to Pinned URL",
            Action::Tab(id.clone(), TabAction::GoHome),
        ));
        if pinnable_address(&tab.url).is_some() {
            entries.push(action(
                "Set Pinned URL to Current Page",
                Action::Tab(id.clone(), TabAction::SetHome),
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(action(
        if today { "Close Tab" } else { "Unload Tab" },
        Action::CloseTab(id.clone()),
    ));
    if today {
        entries.extend([
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
    Some(entries)
}

/// `spaceMenu` (chromium/shell/background.ts:1164-1179). An incognito window has no Spaces to
/// manage, so it has none.
pub(crate) fn space_menu(
    state: &Controller,
    window: &WindowId,
    id: &SpaceId,
) -> Option<Vec<Entry>> {
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
        Entry::Separator,
        action(
            "Separate Cookies and Logins",
            Action::Space(id.clone(), SpaceAction::Isolate(!space.isolated)),
        )
        .checked(space.isolated),
        Entry::Separator,
        item("Delete Space…", Run::DeleteSpace(id.clone()))
            .enabled(state.model.core.spaces().len() > 1),
    ])
}

impl Shell {
    pub(crate) fn open_tab_menu(
        &mut self,
        id: &TabId,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = tab_menu(&self.state.borrow(), &self.id, id);
        if let Some(entries) = entries {
            self.open_menu(entries, MenuAnchor::At(anchor), window, cx);
        }
    }

    pub(crate) fn open_space_menu(
        &mut self,
        id: &SpaceId,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = space_menu(&self.state.borrow(), &self.id, id);
        if let Some(entries) = entries {
            self.open_menu(entries, MenuAnchor::At(anchor), window, cx);
        }
    }

    /// Copies `text` and confirms it on the status line, as the shipping shell's flash does.
    pub(crate) fn copy_text(&self, text: String, message: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let mut state = self.state.borrow_mut();
        let session = state.model.incognito_session(&self.id);
        state.status.set(session, message);
        cx.notify();
    }

    /// The Space menu's New Folder: the folder editor, in that Space. It saves into the shown
    /// Space, so it opens only once the chosen one is shown.
    pub(crate) fn new_folder_in(
        &mut self,
        id: &SpaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        dispatch(&self.state, &self.id, Action::Switch(id.clone()), cx);
        let shown = self
            .state
            .borrow()
            .model
            .state(&self.id)
            .and_then(|core| core.window(&self.id))
            .is_some_and(|current| &current.active_space_id == id);
        if shown {
            self.show_overlay(Overlay::Folder(None, TextField::default()), window, cx);
        }
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

    fn labels(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Item { label, .. } | Entry::Submenu { label, .. } => label.as_ref(),
                Entry::Separator => "-",
            })
            .collect()
    }

    fn entry<'a>(entries: &'a [Entry], label: &str) -> &'a Entry {
        entries
            .iter()
            .find(|entry| {
                matches!(entry, Entry::Item { label: name, .. } | Entry::Submenu { label: name, .. }
                    if name.as_ref() == label)
            })
            .unwrap_or_else(|| panic!("no {label} row"))
    }

    fn run<'a>(entries: &'a [Entry], label: &str) -> &'a Run {
        match entry(entries, label) {
            Entry::Item { run, .. } => run,
            _ => panic!("{label} is not an item"),
        }
    }

    fn submenu<'a>(entries: &'a [Entry], label: &str) -> &'a [Entry] {
        match entry(entries, label) {
            Entry::Submenu { entries, .. } => entries,
            _ => panic!("{label} is not a submenu"),
        }
    }

    fn act(controller: &mut Controller, window: &WindowId, rows: &[Entry], label: &str) {
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
            matches!(run(&rows, "Copy as Markdown Link"), Run::Copy(text, _) if text == "[https://two.example/](https://two.example/)")
        );
        // One Space: nowhere to move to, which shows the submenu disabled.
        assert!(submenu(&rows, "Move to Space").is_empty());

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
        let spaces = submenu(&rows, "Move to Space");
        assert_eq!(spaces.len(), 1);
        let Entry::Item {
            run: Run::Action(action),
            ..
        } = &spaces[0]
        else {
            panic!("the Space row is not an action");
        };
        controller.act(&window, action.clone()).unwrap();
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

    #[test]
    fn the_space_menu_follows_shipping_and_checks_separate_cookies() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let space = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        let entries = space_menu(&controller, &window, &space).unwrap();
        assert_eq!(
            labels(&entries),
            [
                "Edit Space…",
                "New Folder",
                "Open in New Window",
                "-",
                "Separate Cookies and Logins",
                "-",
                "Delete Space…",
            ]
        );
        // The last Space can't go.
        assert!(matches!(
            entry(&entries, "Delete Space…"),
            Entry::Item { enabled: false, .. }
        ));
        for isolated in [true, false] {
            let entries = entries_for(&controller, &window, &space);
            act(
                &mut controller,
                &window,
                &entries,
                "Separate Cookies and Logins",
            );
            assert_eq!(
                controller.model.core.space(&space).unwrap().isolated,
                isolated
            );
            assert!(matches!(
                entry(
                    &entries_for(&controller, &window, &space),
                    "Separate Cookies and Logins"
                ),
                Entry::Item { checked: Some(checked), .. } if *checked == isolated
            ));
        }
    }

    fn entries_for(controller: &Controller, window: &WindowId, space: &SpaceId) -> Vec<Entry> {
        space_menu(controller, window, space).unwrap()
    }
}
