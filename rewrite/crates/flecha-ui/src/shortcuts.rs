use anyhow::{Context as _, Result};
use flecha_core::{
    BrowserState, TabId, WindowId,
    shortcuts::{self, Keybindings, Platform},
};
use gpui::{Context, KeyDownEvent, Keystroke, Window};

use crate::command_bar::Command;
use crate::settings_view::SettingsTab;
use crate::sidebar::SidebarAction;
use crate::view::{Overlay, Shell};
use crate::{Action, Controller, dispatch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Navigation {
    Back,
    Forward,
    Reload,
    HardReload,
    Stop,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct Toolbar {
    pub(crate) back: bool,
    pub(crate) forward: bool,
    pub(crate) loading: bool,
    pub(crate) available: bool,
    blocked: u64,
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
    SettingsTab(SettingsTab),
    Sidebar,
    Return,
    NewIncognitoWindow,
    Quit,
    Escape,
}

pub(crate) struct Bindings {
    settings: Keybindings,
    bindings: Vec<(Keystroke, Shortcut)>,
}

pub(crate) const fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

impl Bindings {
    pub(crate) fn new(settings: &Keybindings) -> Self {
        let platform = platform();
        let os = match platform {
            Platform::Linux => gpui::OperatingSystem::Linux,
            Platform::Mac => gpui::OperatingSystem::Mac,
            Platform::Windows => gpui::OperatingSystem::Windows,
        };
        let map = shortcuts::shortcut_map(settings, platform);
        let mut bindings: Vec<_> = map
            .into_iter()
            .filter_map(|(keys, id)| {
                Some((
                    Keystroke::parse_with_os(&gpui_keys(&keys), os).ok()?,
                    shortcut_for_id(&id)?,
                ))
            })
            .collect();
        for (keys, action) in [
            ("alt-left", Shortcut::Browser(Navigation::Back)),
            ("alt-right", Shortcut::Browser(Navigation::Forward)),
            ("f5", Shortcut::Browser(Navigation::Reload)),
            ("secondary-q", Shortcut::Quit),
            ("escape", Shortcut::Escape),
            // Chromium's IDC_NEW_INCOGNITO_WINDOW, which the shipping menu shows (`ui/app.ts`).
            ("secondary-shift-n", Shortcut::NewIncognitoWindow),
        ] {
            bindings.push((
                Keystroke::parse_with_os(keys, os).expect("valid shell shortcut"),
                action,
            ));
        }
        Self {
            settings: settings.clone(),
            bindings,
        }
    }

    pub(crate) fn refresh(&mut self, settings: &Keybindings) {
        if &self.settings != settings {
            *self = Self::new(settings);
        }
    }

    fn action(&self, stroke: &Keystroke) -> Option<Shortcut> {
        self.bindings.iter().find_map(|(key, action)| {
            (stroke.key == key.key && stroke.modifiers == key.modifiers).then_some(*action)
        })
    }
}

pub(crate) fn has_action(id: &str) -> bool {
    shortcut_for_id(id).is_some()
}

fn shortcut_for_id(id: &str) -> Option<Shortcut> {
    Some(match id {
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
        "settings" => Shortcut::Command(Command::OpenSettings),
        "shortcuts" => Shortcut::SettingsTab(SettingsTab::Shortcuts),
        "editBoost" => Shortcut::SettingsTab(SettingsTab::Boosts),
        "toggleSidebar" => Shortcut::Sidebar,
        "openInFlecha" => Shortcut::Return,
        "quit" => Shortcut::Quit,
        id => {
            let number = id.strip_prefix("selectTab:")?.parse::<usize>().ok()?;
            Shortcut::Select(number - 1)
        }
    })
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
    pub(crate) fn poll_toolbars(&mut self) -> bool {
        let mut changed = false;
        while let Some(message) = self.engine.next_devtools_event() {
            if let flecha_cef::DevToolsEvent::Result { id, success, .. } = message.event
                && self.pending_reloads.remove(&id).is_some()
                && !success
            {
                self.status = "Could not reload the page without its cache.".into();
                changed = true;
            }
        }
        self.pending_reloads
            .retain(|_, key| self.engine.key(&key.tab).as_ref() == Some(key));
        self.toolbars.retain(|id, _| self.model.state(id).is_some());
        for window in self
            .model
            .core
            .windows()
            .iter()
            .chain(self.model.incognito_windows())
        {
            let next =
                self.model
                    .keyboard_pane(&window.id)
                    .map_or_else(Toolbar::default, |(_, tab)| Toolbar {
                        back: self.engine.can_go_back(tab),
                        forward: self.engine.can_go_forward(tab),
                        loading: self.engine.is_loading(tab),
                        available: self.engine.host(tab).is_some(),
                        blocked: self.engine.blocked_count(tab),
                    });
            if let Some(current) = self.toolbars.get_mut(&window.id) {
                changed |= *current != next;
                *current = next;
            } else {
                self.toolbars.insert(window.id.clone(), next);
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn browser_navigation(
        &mut self,
        window: &WindowId,
        action: Navigation,
    ) -> Result<()> {
        let Some((_, id)) = self.model.keyboard_pane(window) else {
            return Ok(());
        };
        match action {
            Navigation::Back if self.engine.can_go_back(id) => self.engine.go_back(id),
            Navigation::Forward if self.engine.can_go_forward(id) => self.engine.go_forward(id),
            Navigation::Reload => self.engine.reload(id),
            Navigation::Stop if self.engine.is_loading(id) => self.engine.stop_load(id),
            Navigation::HardReload if self.engine.host(id).is_some() => {
                let key = self
                    .engine
                    .key(id)
                    .context("No current browser for this tab")?;
                let request = self
                    .engine
                    .send_devtools_message(id, "Page.reload", Some(r#"{"ignoreCache":true}"#))
                    .context("Reload without cache")?;
                self.pending_reloads.insert(request, key);
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn navigate_tab(&mut self, window: &WindowId, url: String) -> anyhow::Result<()> {
        let Some(id) = self.model.active(window).cloned() else {
            self.model.open(window, url, true)?;
            return Ok(());
        };
        if let Some((tab, _)) = self.model.tab_mut(&id)
            && tab.url != url
            && !self.engine.navigate(&id, &url)
        {
            tab.title.clone_from(&url);
            tab.url = url;
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
        self.bindings
            .refresh(&self.state.borrow().model.core.settings.keybindings);
        let stroke = &event.keystroke;
        let altgr =
            event.prefer_character_input && stroke.modifiers.control && stroke.modifiers.alt;
        let settings_open = matches!(self.overlay, Overlay::Settings(_));
        if settings_open && self.settings_before_shortcuts(stroke, cx) {
            self.consumed_keys.insert(stroke.key.clone());
            cx.stop_propagation();
            return;
        }
        if !altgr
            && let Some(shortcut) = self.bindings.action(stroke)
            && (!settings_open
                || matches!(
                    shortcut,
                    Shortcut::Escape
                        | Shortcut::Command(Command::OpenSettings)
                        | Shortcut::SettingsTab(_)
                ))
            && (if event.is_held
                && matches!(
                    shortcut,
                    Shortcut::Command(Command::OpenSettings) | Shortcut::SettingsTab(_)
                ) {
                true
            } else {
                self.invoke_shortcut(shortcut, window, cx)
            })
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
                Overlay::Settings(_) => self.settings_key(stroke, cx),
                Overlay::Downloads(_) => self.downloads_key(stroke, cx),
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
            Shortcut::SettingsTab(tab) => return self.toggle_settings(tab, window, cx),
            Shortcut::History if self.state.borrow().model.core.window(&self.id).is_some() => {
                self.toggle_history(window, cx);
                return true;
            }
            Shortcut::Sidebar if self.state.borrow().model.state(&self.id).is_some() => {
                Action::Sidebar(SidebarAction::Toggle)
            }
            Shortcut::Escape if self.overlay.is_open() => {
                self.dismiss_overlay(window, cx);
                return true;
            }
            Shortcut::Escape if self.state.borrow().model.peeks.contains_key(&self.id) => {
                Action::ClosePeek
            }
            Shortcut::Escape
                if self
                    .state
                    .borrow()
                    .model
                    .keyboard_pane(&self.id)
                    .is_some_and(|(_, id)| self.state.borrow().engine.is_loading(id)) =>
            {
                Action::Browser(Navigation::Stop)
            }
            Shortcut::Close => {
                self.dismiss_overlay(window, cx);
                Action::Close
            }
            Shortcut::Reopen if self.state.borrow().model.state(&self.id).is_some() => {
                Action::Reopen
            }
            Shortcut::Browser(nav) if !self.overlay.is_open() => Action::Browser(nav),
            Shortcut::Select(_) | Shortcut::Cycle(_) if !self.overlay.is_open() => {
                let state = self.state.borrow();
                let Some(id) = state.model.state(&self.id).and_then(|core| {
                    tab_for_shortcut(core, &self.id, state.model.active(&self.id), shortcut)
                }) else {
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
            Shortcut::NewIncognitoWindow => Action::NewIncognitoWindow,
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
            (
                format!("{ctrl}-,"),
                Shortcut::Command(Command::OpenSettings),
            ),
            (format!("{ctrl}-r"), Shortcut::Browser(Navigation::Reload)),
            ("f5".into(), Shortcut::Browser(Navigation::Reload)),
            ("alt-left".into(), Shortcut::Browser(Navigation::Back)),
            ("alt-right".into(), Shortcut::Browser(Navigation::Forward)),
            ("ctrl-tab".into(), Shortcut::Cycle(false)),
            ("ctrl-shift-tab".into(), Shortcut::Cycle(true)),
            (format!("{ctrl}-9"), Shortcut::Select(8)),
            (format!("{ctrl}-shift-n"), Shortcut::NewIncognitoWindow),
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

    #[test]
    fn settings_changes_refresh_each_window_and_displace_conflicts() {
        use shortcuts::BindingEdit;

        let mut settings = Keybindings::new();
        let mut first = Bindings::new(&settings);
        let mut second = Bindings::new(&settings);
        let os = match platform() {
            Platform::Linux => gpui::OperatingSystem::Linux,
            Platform::Mac => gpui::OperatingSystem::Mac,
            Platform::Windows => gpui::OperatingSystem::Windows,
        };
        let new_tab = Keystroke::parse_with_os("secondary-t", os).unwrap();
        let settings_key = Keystroke::parse_with_os("secondary-,", os).unwrap();
        let open_settings = Some(Shortcut::Command(Command::OpenSettings));

        assert_eq!(
            shortcuts::set_binding(
                &mut settings,
                "settings",
                BindingEdit::Set("CmdOrCtrl+T"),
                platform()
            )
            .unwrap(),
            ["newTab"]
        );
        for window in [&mut first, &mut second] {
            window.refresh(&settings);
            assert_eq!(window.action(&new_tab), open_settings);
            assert_eq!(window.action(&settings_key), None);
        }
        shortcuts::set_binding(&mut settings, "settings", BindingEdit::Reset, platform()).unwrap();
        first.refresh(&settings);
        assert_eq!(first.action(&settings_key), open_settings);
        assert_eq!(first.action(&new_tab), None);
        settings.clear();
        for window in [&mut first, &mut second] {
            window.refresh(&settings);
            assert_eq!(window.action(&settings_key), open_settings);
            assert_eq!(
                window.action(&new_tab),
                Some(Shortcut::Command(Command::NewTab))
            );
        }
    }
}
