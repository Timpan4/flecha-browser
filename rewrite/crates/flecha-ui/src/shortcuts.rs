use anyhow::{Context as _, Result};
use flecha_cef::{PageZoom, zoom_percentage};
use flecha_core::{
    BrowserState, TabId, WindowId,
    shortcuts::{self, Keybindings, Platform},
};
use gpui::{
    ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::command_bar::Command;
use crate::settings_view::SettingsTab;
use crate::sidebar::SidebarAction;
use crate::theme::palette;
use crate::tokens;
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
    pub(crate) zoom: Option<u16>,
    blocked: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Shortcut {
    Command(Command),
    Close,
    Reopen,
    Browser(Navigation),
    Find,
    Zoom(PageZoom),
    DevTools,
    Print,
    ViewSource,
    Cycle(bool),
    Select(usize),
    History,
    SettingsTab(SettingsTab),
    Sidebar,
    Return,
    NewWindow,
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
            ("secondary-p", Shortcut::Print),
            ("secondary-u", Shortcut::ViewSource),
            // Chromium's IDC_NEW_WINDOW, which shipping leaves to Chromium (`ui/app.ts`).
            ("secondary-n", Shortcut::NewWindow),
            // Chromium's IDC_NEW_INCOGNITO_WINDOW, which the shipping menu shows (`ui/app.ts`).
            ("secondary-shift-n", Shortcut::NewIncognitoWindow),
        ] {
            bindings.push((
                Keystroke::parse_with_os(keys, os).expect("valid shell shortcut"),
                action,
            ));
        }
        if shortcuts::keys_for(settings, "devTools", platform)
            == shortcuts::default_keys("devTools", platform)
        {
            for keys in ["ctrl-shift-i", "f12"] {
                bindings.push((
                    Keystroke::parse_with_os(keys, os).expect("valid DevTools shortcut"),
                    Shortcut::DevTools,
                ));
            }
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
        let exact = |symbol: &str, modifiers| {
            self.bindings.iter().find_map(|(key, action)| {
                (symbol == key.key && modifiers == key.modifiers).then_some(*action)
            })
        };
        exact(&stroke.key, stroke.modifiers).or_else(|| {
            if !matches!(stroke.key.as_str(), "+" | "add") {
                return None;
            }
            // A keyboard layout may need Shift to produce '+'. Follow the configured '=' accelerator.
            let mut modifiers = stroke.modifiers;
            modifiers.shift = false;
            exact("=", modifiers)
                .filter(|action| *action == Shortcut::Zoom(PageZoom::In))
                .or_else(|| exact("=", stroke.modifiers))
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
        "newFlechinha" => Shortcut::Command(Command::Flechinha),
        "splitNew" => Shortcut::Command(Command::Split),
        "closeTab" => Shortcut::Close,
        "reopenTab" => Shortcut::Reopen,
        "back" => Shortcut::Browser(Navigation::Back),
        "forward" => Shortcut::Browser(Navigation::Forward),
        "reload" => Shortcut::Browser(Navigation::Reload),
        "hardReload" => Shortcut::Browser(Navigation::HardReload),
        "find" => Shortcut::Find,
        "zoomIn" => Shortcut::Zoom(PageZoom::In),
        "zoomOut" => Shortcut::Zoom(PageZoom::Out),
        "actualSize" => Shortcut::Zoom(PageZoom::Reset),
        "devTools" => Shortcut::DevTools,
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
            let Some(message) = self.agents.devtools(message) else {
                continue;
            };
            if let flecha_cef::DevToolsEvent::Result { id, success, .. } = message.event
                && let Some(key) = self.pending_reloads.remove(&id)
                && !success
            {
                let session = self
                    .model
                    .partition(&key.tab)
                    .and_then(|partition| partition.incognito_session());
                self.status
                    .set(session, "Could not reload the page without its cache.");
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
                        zoom: self
                            .engine
                            .zoom_level(tab)
                            .map(zoom_percentage)
                            .filter(|value| *value != 100),
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
    pub(crate) fn zoom_badge(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let percent = self.state.borrow().toolbars.get(&self.id)?.zoom?;
        // `.zoom-value` on the `.zoom-control` ground (ui/styles.css:2123-2150); the shipping
        // shell shows zoom in the site panel and Chromium's own bubble, not the toolbar.
        Some(
            div()
                .id("reset-page-zoom")
                .flex()
                .flex_none()
                .items_center()
                .h(px(tokens::ZOOM_VALUE))
                .px(px(tokens::MINI_SELECT_PAD))
                .rounded(px(tokens::RADIUS_SM))
                .bg(palette().surface_2)
                .text_size(px(tokens::TEXT_XS))
                .font_family(tokens::MONO)
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette().text_2)
                .cursor_pointer()
                .hover(|style| style.text_color(palette().text))
                .child(format!("{percent}%"))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.zoom(PageZoom::Reset, cx);
                })),
        )
    }

    fn zoom(&self, action: PageZoom, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.active_browser() else {
            return false;
        };
        let state = self.state.borrow();
        let Some(level) = state.engine.zoom_level(&key.tab) else {
            return false;
        };
        state.engine.set_zoom_level(&key.tab, action.level(level));
        cx.notify();
        true
    }

    pub(crate) fn key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" && self.cancel_tab_drag(window, cx) {
            self.consumed_keys.insert(event.keystroke.key.clone());
            cx.stop_propagation();
            return;
        }
        let focused = self.sync_page_focus(window, cx);
        self.bindings
            .refresh(&self.state.borrow().model.core.settings.keybindings);
        let stroke = &event.keystroke;
        if self.browser_dialog.is_some()
            && self.bindings.action(stroke) != Some(Shortcut::Close)
            && self.bindings.action(stroke) != Some(Shortcut::Quit)
        {
            self.browser_dialog_key(stroke, window, cx);
            self.consumed_keys.insert(stroke.key.clone());
            cx.stop_propagation();
            return;
        }
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
                    Shortcut::Command(Command::OpenSettings)
                        | Shortcut::SettingsTab(_)
                        | Shortcut::Find
                        | Shortcut::DevTools
                        | Shortcut::Print
                        | Shortcut::ViewSource
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
        if self.find_key(stroke, window, cx) {
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
        } else if !altgr
            && self.page_clipboard_key(focused.as_ref(), stroke, event.is_held, window, cx)
        {
            self.consumed_keys.insert(stroke.key.clone());
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

    pub(crate) fn invoke_shortcut(
        &mut self,
        shortcut: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let action = match shortcut {
            Shortcut::Find if !self.overlay.is_open() => return self.open_find(window, cx),
            Shortcut::Zoom(action) if !self.overlay.is_open() => return self.zoom(action, cx),
            Shortcut::Print if !self.overlay.is_open() => Action::Print,
            Shortcut::ViewSource if !self.overlay.is_open() => Action::ViewSource,
            Shortcut::DevTools if !self.overlay.is_open() => {
                let Some(key) = self.active_browser() else {
                    return false;
                };
                let state = self.state.borrow();
                if state.engine.has_dev_tools(&key.tab) {
                    state.engine.close_dev_tools(&key.tab);
                } else {
                    state.engine.show_dev_tools(&key.tab, None);
                }
                return true;
            }
            Shortcut::Escape if self.find.is_some() && !self.overlay.is_open() => return false,
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
            Shortcut::Escape if self.state.borrow().fullscreen_pane(&self.id).is_some() => {
                let state = self.state.borrow();
                if let Some((_, tab)) = state.fullscreen_pane(&self.id) {
                    state.engine.exit_fullscreen(&tab);
                }
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
            Shortcut::Select(_) | Shortcut::Cycle(_)
                if !self.overlay.is_open() || matches!(self.overlay, Overlay::ContextMenu(_)) =>
            {
                self.dismiss_overlay(window, cx);
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
            Shortcut::NewWindow => Action::NewWindow,
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
            (format!("{ctrl}-n"), Shortcut::NewWindow),
            (format!("{ctrl}-shift-n"), Shortcut::NewIncognitoWindow),
            (format!("{ctrl}-f"), Shortcut::Find),
            (format!("{ctrl}-="), Shortcut::Zoom(PageZoom::In)),
            (format!("{ctrl}-shift-+"), Shortcut::Zoom(PageZoom::In)),
            (format!("{ctrl}-add"), Shortcut::Zoom(PageZoom::In)),
            (format!("{ctrl}--"), Shortcut::Zoom(PageZoom::Out)),
            (format!("{ctrl}-0"), Shortcut::Zoom(PageZoom::Reset)),
            ("ctrl-shift-i".into(), Shortcut::DevTools),
            ("f12".into(), Shortcut::DevTools),
            (format!("alt-{ctrl}-i"), Shortcut::DevTools),
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
    fn browser_tool_shortcuts_follow_keybinding_overrides_and_unbinding() {
        use shortcuts::BindingEdit;
        let mut settings = Keybindings::new();
        shortcuts::set_binding(
            &mut settings,
            "devTools",
            BindingEdit::Set("F8"),
            platform(),
        )
        .unwrap();
        shortcuts::set_binding(&mut settings, "find", BindingEdit::Set("F12"), platform()).unwrap();
        shortcuts::set_binding(&mut settings, "zoomIn", BindingEdit::Unbind, platform()).unwrap();
        let bindings = Bindings::new(&settings);
        let os = match platform() {
            Platform::Linux => gpui::OperatingSystem::Linux,
            Platform::Mac => gpui::OperatingSystem::Mac,
            Platform::Windows => gpui::OperatingSystem::Windows,
        };
        for (keys, expected) in [
            ("f8", Some(Shortcut::DevTools)),
            ("f12", Some(Shortcut::Find)),
            ("secondary-f", None),
            ("secondary-=", None),
            ("secondary-add", None),
            ("ctrl-shift-i", None),
        ] {
            assert_eq!(
                bindings.action(&Keystroke::parse_with_os(keys, os).unwrap()),
                expected,
                "{keys}"
            );
        }
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
