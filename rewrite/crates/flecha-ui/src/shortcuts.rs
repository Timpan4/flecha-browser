use cef::{ImplBrowser as _, ImplBrowserHost as _, ImplFrame as _};
use flecha_core::{
    BrowserState, TabId, WindowId,
    shortcuts::{self, Keybindings, Platform},
};
use gpui::{Context, KeyDownEvent, Keystroke, Window};

use crate::command_bar::Command;
use crate::sidebar::SidebarAction;
use crate::view::{Overlay, Shell};
use crate::{Action, Controller, dispatch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Navigation {
    Back,
    Forward,
    Reload,
    HardReload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shortcut {
    Command(Command),
    Close,
    Reopen,
    Browser(Navigation),
    Cycle(bool),
    Select(usize),
    History,
    Sidebar,
    Return,
    Quit,
    Escape,
}

pub(crate) struct Bindings(Vec<(Keystroke, Shortcut)>);

impl Bindings {
    pub(crate) fn new(settings: &Keybindings) -> Self {
        let platform = if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        };
        let os = match platform {
            Platform::Linux => gpui::OperatingSystem::Linux,
            Platform::Mac => gpui::OperatingSystem::Mac,
            Platform::Windows => gpui::OperatingSystem::Windows,
        };
        let map = shortcuts::shortcut_map(settings, platform);
        let mut bindings: Vec<_> = map
            .into_iter()
            .filter_map(|(keys, id)| {
                let shortcut = match id.as_str() {
                    "newTab" => Shortcut::Command(Command::NewTab),
                    "openLocation" => Shortcut::Command(Command::EditLocation),
                    "splitNew" => Shortcut::Command(Command::Split),
                    "closeTab" => Shortcut::Close,
                    "reopenTab" => Shortcut::Reopen,
                    "back" => Shortcut::Browser(Navigation::Back),
                    "forward" => Shortcut::Browser(Navigation::Forward),
                    "reload" => Shortcut::Browser(Navigation::Reload),
                    "hardReload" => Shortcut::Browser(Navigation::HardReload),
                    "nextTab" => Shortcut::Cycle(false),
                    "previousTab" => Shortcut::Cycle(true),
                    "history" => Shortcut::History,
                    "toggleSidebar" => Shortcut::Sidebar,
                    "openInFlecha" => Shortcut::Return,
                    "quit" => Shortcut::Quit,
                    id => {
                        let number = id.strip_prefix("selectTab:")?.parse::<usize>().ok()?;
                        Shortcut::Select(number - 1)
                    }
                };
                Some((
                    Keystroke::parse_with_os(&gpui_keys(&keys), os).ok()?,
                    shortcut,
                ))
            })
            .collect();
        for (keys, action) in [
            ("alt-left", Shortcut::Browser(Navigation::Back)),
            ("alt-right", Shortcut::Browser(Navigation::Forward)),
            ("f5", Shortcut::Browser(Navigation::Reload)),
            ("secondary-q", Shortcut::Quit),
            ("escape", Shortcut::Escape),
        ] {
            bindings.push((
                Keystroke::parse_with_os(keys, os).expect("valid shell shortcut"),
                action,
            ));
        }
        Self(bindings)
    }

    fn action(&self, stroke: &Keystroke) -> Option<Shortcut> {
        self.0.iter().find_map(|(key, action)| {
            (stroke.key == key.key && stroke.modifiers == key.modifiers).then_some(*action)
        })
    }
}

fn gpui_keys(keys: &str) -> String {
    let keys = keys
        .strip_suffix("++")
        .map_or_else(|| keys.to_owned(), |prefix| format!("{prefix}+="));
    keys.split('+')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "cmdorctrl" | "commandorcontrol" => "secondary".into(),
            "ctrl" | "control" => "ctrl".into(),
            "cmd" | "command" | "super" | "meta" => "cmd".into(),
            "alt" | "option" => "alt".into(),
            "plus" => "=".into(),
            "return" => "enter".into(),
            "esc" => "escape".into(),
            key => key.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn tab_for_shortcut(
    core: &BrowserState,
    window: &WindowId,
    active: Option<&TabId>,
    shortcut: Shortcut,
) -> Option<TabId> {
    let tabs = core.tab_ids(window, &core.window(window)?.active_space_id)?;
    let index = match shortcut {
        Shortcut::Select(index) => index,
        Shortcut::Cycle(back) => match tabs.iter().position(|tab| Some(*tab) == active) {
            Some(index) if back => index.checked_sub(1).unwrap_or(tabs.len() - 1),
            Some(index) => (index + 1) % tabs.len(),
            None if back => tabs.len().checked_sub(1)?,
            None => 0,
        },
        _ => return None,
    };
    tabs.get(index).map(|id| (*id).clone())
}

impl Controller {
    pub(crate) fn browser_navigation(&self, window: &WindowId, action: Navigation) {
        let Some((_, id)) = self.model.keyboard_pane(window) else {
            return;
        };
        let Some(browser) = self.engine.host(id).and_then(|host| host.browser()) else {
            return;
        };
        match action {
            Navigation::Back if browser.can_go_back() != 0 => browser.go_back(),
            Navigation::Forward if browser.can_go_forward() != 0 => browser.go_forward(),
            Navigation::Reload => browser.reload(),
            Navigation::HardReload => browser.reload_ignore_cache(),
            _ => {}
        }
    }

    pub(crate) fn navigate_tab(&mut self, window: &WindowId, url: String) -> anyhow::Result<()> {
        let Some(id) = self.model.active(window).cloned() else {
            self.model.open(window, url, true)?;
            return Ok(());
        };
        if let Some((tab, _)) = self.model.tab_mut(&id)
            && tab.url != url
        {
            if let Some(frame) = self
                .engine
                .host(&id)
                .and_then(|host| host.browser())
                .and_then(|browser| browser.main_frame())
            {
                frame.load_url(Some(&cef::CefString::from(url.as_str())));
            } else {
                tab.title.clone_from(&url);
                tab.url = url;
            }
        }
        Ok(())
    }
}

impl Shell {
    pub(crate) fn key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self.sync_page_focus(window, cx);
        let stroke = &event.keystroke;
        let altgr =
            event.prefer_character_input && stroke.modifiers.control && stroke.modifiers.alt;
        if !altgr
            && let Some(shortcut) = self.bindings.action(stroke)
            && self.invoke_shortcut(shortcut, window, cx)
        {
            self.consumed_keys.insert(stroke.key.clone());
            cx.stop_propagation();
            return;
        }
        if self.overlay.is_open() {
            self.consumed_keys.insert(stroke.key.clone());
            match &self.overlay {
                Overlay::Command(_) => self.command_key(stroke, window, cx),
                Overlay::History(_) | Overlay::Archive(_) => self.drawer_key(stroke, cx),
                Overlay::Folder(..) => self.folder_key(stroke, window, cx),
                _ => {}
            }
            cx.stop_propagation();
        } else {
            self.page_key(
                focused,
                stroke,
                false,
                event.is_held,
                event.prefer_character_input,
            );
        }
    }

    fn invoke_shortcut(
        &mut self,
        shortcut: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let action = match shortcut {
            Shortcut::Command(command) => return self.open_command(command, window, cx),
            Shortcut::History if self.state.borrow().model.core.window(&self.id).is_some() => {
                self.toggle_history(window, cx);
                return true;
            }
            Shortcut::Sidebar if self.state.borrow().model.core.window(&self.id).is_some() => {
                Action::Sidebar(SidebarAction::Toggle)
            }
            Shortcut::Escape if self.overlay.is_open() => {
                self.dismiss_overlay(window, cx);
                return true;
            }
            Shortcut::Escape if self.state.borrow().model.peeks.contains_key(&self.id) => {
                Action::ClosePeek
            }
            Shortcut::Close => {
                self.dismiss_overlay(window, cx);
                Action::Close
            }
            Shortcut::Reopen if self.state.borrow().model.core.window(&self.id).is_some() => {
                Action::Reopen
            }
            Shortcut::Browser(nav) if !self.overlay.is_open() => Action::Browser(nav),
            Shortcut::Select(_) | Shortcut::Cycle(_) if !self.overlay.is_open() => {
                let state = self.state.borrow();
                let Some(id) = tab_for_shortcut(
                    &state.model.core,
                    &self.id,
                    state.model.active(&self.id),
                    shortcut,
                ) else {
                    return false;
                };
                Action::Select(id)
            }
            Shortcut::Return if self.state.borrow().model.flechinhas.contains_key(&self.id) => {
                Action::Return
            }
            Shortcut::Return if self.state.borrow().model.peeks.contains_key(&self.id) => {
                Action::Promote
            }
            Shortcut::Quit => Action::Quit,
            _ => return false,
        };
        dispatch(&self.state, &self.id, action, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    #[test]
    fn daily_driver_accelerators_dispatch_to_the_expected_actions() {
        let bindings = Bindings::new(&Keybindings::new());
        let ctrl = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let os = if cfg!(target_os = "macos") {
            gpui::OperatingSystem::Mac
        } else {
            gpui::OperatingSystem::Linux
        };
        for (key, action) in [
            (format!("{ctrl}-t"), Shortcut::Command(Command::NewTab)),
            (
                format!("{ctrl}-l"),
                Shortcut::Command(Command::EditLocation),
            ),
            (format!("{ctrl}-w"), Shortcut::Close),
            (format!("{ctrl}-shift-t"), Shortcut::Reopen),
            (format!("{ctrl}-s"), Shortcut::Sidebar),
            (format!("{ctrl}-r"), Shortcut::Browser(Navigation::Reload)),
            ("f5".into(), Shortcut::Browser(Navigation::Reload)),
            ("alt-left".into(), Shortcut::Browser(Navigation::Back)),
            ("alt-right".into(), Shortcut::Browser(Navigation::Forward)),
            ("ctrl-tab".into(), Shortcut::Cycle(false)),
            ("ctrl-shift-tab".into(), Shortcut::Cycle(true)),
            (format!("{ctrl}-9"), Shortcut::Select(8)),
            ("escape".into(), Shortcut::Escape),
        ] {
            assert_eq!(
                bindings.action(&Keystroke::parse_with_os(&key, os).unwrap()),
                Some(action)
            );
        }
        let history = if cfg!(target_os = "macos") {
            "cmd-y"
        } else {
            "ctrl-h"
        };
        assert_eq!(
            bindings.action(&Keystroke::parse_with_os(history, os).unwrap()),
            Some(Shortcut::History)
        );
        assert_eq!(
            bindings.action(&Keystroke {
                key: "t".into(),
                modifiers: Modifiers::none(),
                key_char: Some("t".into())
            }),
            None
        );
    }
}
