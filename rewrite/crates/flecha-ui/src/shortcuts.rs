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
use crate::tab_menu::TabAction;
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
    NewFlechinha,
    /// Pins a Today tab, or unpins a pinned tab or favorite.
    Pin,
    Duplicate,
    Mute,
    ClearToday,
    /// Copies the shown tab's link, as Markdown when set.
    CopyLink(bool),
    /// The next Space, or the previous one when set.
    StepSpace(bool),
    SelectSpace(usize),
    NewSpace,
    ExitSplit,
    ViewSource,
    // The command bar's actions without a key of their own.
    EditSpace,
    Archive,
    Downloads,
    RestoreBeforeImport,
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

pub(crate) fn shortcut_for_id(id: &str) -> Option<Shortcut> {
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
        "newFlechinha" => Shortcut::NewFlechinha,
        "pinTab" => Shortcut::Pin,
        "duplicateTab" => Shortcut::Duplicate,
        "muteTab" => Shortcut::Mute,
        "clearToday" => Shortcut::ClearToday,
        "copyLink" => Shortcut::CopyLink(false),
        "copyMarkdown" => Shortcut::CopyLink(true),
        "nextSpace" => Shortcut::StepSpace(false),
        "previousSpace" => Shortcut::StepSpace(true),
        "newSpace" => Shortcut::NewSpace,
        "exitSplit" => Shortcut::ExitSplit,
        "viewSource" => Shortcut::ViewSource,
        id => {
            if let Some(number) = id.strip_prefix("selectSpace:") {
                return Some(Shortcut::SelectSpace(
                    number.parse::<usize>().ok()?.checked_sub(1)?,
                ));
            }
            let number = id.strip_prefix("selectTab:")?.parse::<usize>().ok()?;
            Shortcut::Select(number.checked_sub(1)?)
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
            && (if event.is_held && runs_once(shortcut) {
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
                Overlay::Rename(..) => self.rename_key(stroke, window, cx),
                Overlay::SpaceEditor(..) => self.space_editor_key(stroke, window, cx),
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
                if !self.overlay.is_open()
                    || matches!(self.overlay, Overlay::ContextMenu(_) | Overlay::Menu(_)) =>
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
            _ => return self.invoke_flecha_command(shortcut, window, cx),
        };
        dispatch(&self.state, &self.id, action, cx);
        true
    }

    /// The commands on the shown tab and Space that shipping runs by key or from the command
    /// bar (`chromium/shell/background.ts:1551-1586`). Where the window can't run one (an
    /// incognito window has one Space; a Flechinha has no sidebar), the key goes on to the page.
    fn invoke_flecha_command(
        &mut self,
        shortcut: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(shown) = ShownWindow::read(&self.state.borrow(), &self.id)
            .filter(|shown| shown.can_run(shortcut))
        else {
            return false;
        };
        let action = match shortcut {
            Shortcut::NewFlechinha => return self.open_command(Command::Flechinha, window, cx),
            Shortcut::NewSpace => return self.new_space(window, cx),
            Shortcut::EditSpace => {
                self.open_space_editor(&shown.space, window, cx);
                return matches!(self.overlay, Overlay::SpaceEditor(..));
            }
            Shortcut::Archive => {
                self.toggle_archive(window, cx);
                return true;
            }
            Shortcut::Downloads => {
                self.toggle_downloads(window, cx);
                return true;
            }
            Shortcut::RestoreBeforeImport => {
                self.open_restore_before_import(window, cx);
                return true;
            }
            Shortcut::CopyLink(markdown) => {
                let Some((text, message)) = shown.link(markdown) else {
                    return false;
                };
                self.copy_text(text, message, cx);
                return true;
            }
            Shortcut::Pin => {
                let (Some(id), Some(today)) = (shown.tab.clone(), shown.today) else {
                    return false;
                };
                Action::Sidebar(if today {
                    SidebarAction::Pin(id, None)
                } else {
                    SidebarAction::ToToday(id)
                })
            }
            Shortcut::Duplicate | Shortcut::Mute | Shortcut::ViewSource => {
                let Some(id) = shown.tab.clone() else {
                    return false;
                };
                Action::Tab(
                    id,
                    match shortcut {
                        Shortcut::Duplicate => TabAction::Duplicate,
                        Shortcut::Mute => TabAction::ToggleMute,
                        _ => TabAction::ViewSource,
                    },
                )
            }
            Shortcut::ClearToday => Action::ClearToday,
            Shortcut::ExitSplit => Action::ExitSplit,
            Shortcut::StepSpace(back) => {
                let count = shown.spaces.len();
                let Some(index) = shown.spaces.iter().position(|id| *id == shown.space) else {
                    return false;
                };
                let next = if back {
                    (index + count - 1) % count
                } else {
                    (index + 1) % count
                };
                Action::Switch(shown.spaces[next].clone())
            }
            Shortcut::SelectSpace(index) => {
                let Some(space) = shown.spaces.get(index) else {
                    return false;
                };
                Action::Switch(space.clone())
            }
            _ => return false,
        };
        if shown.incognito && crate::incognito::refusal(&action).is_some() {
            return false;
        }
        self.dismiss_overlay(window, cx);
        dispatch(&self.state, &self.id, action, cx);
        true
    }
}

/// What the shell's commands act on in a Flecha window: its shown tab and Space.
pub(crate) struct ShownWindow {
    incognito: bool,
    tab: Option<TabId>,
    /// Whether the shown tab is a Today tab; `None` without one.
    today: Option<bool>,
    web_page: bool,
    link: Option<(String, String)>,
    space: flecha_core::SpaceId,
    spaces: Vec<flecha_core::SpaceId>,
    split: bool,
}

impl ShownWindow {
    pub(crate) fn read(state: &Controller, window: &WindowId) -> Option<Self> {
        let core = state.model.state(window)?;
        let current = core.window(window)?;
        let tab = state.model.active(window).cloned();
        let shown = tab.as_ref().and_then(|id| core.tab(id));
        Some(Self {
            incognito: state.model.is_incognito(window),
            today: tab
                .as_ref()
                .and_then(|id| core.locate(id))
                .map(|placement| matches!(placement, flecha_core::Placement::Today { .. })),
            web_page: shown.is_some_and(|tab| {
                tab.url.starts_with("http://") || tab.url.starts_with("https://")
            }),
            link: shown.map(|tab| {
                (
                    tab.url.clone(),
                    tab.custom_title
                        .clone()
                        .unwrap_or_else(|| tab.title.clone()),
                )
            }),
            tab,
            space: current.active_space_id.clone(),
            spaces: core.spaces().iter().map(|space| space.id.clone()).collect(),
            split: current.visible_tabs().len() > 1,
        })
    }

    /// Whether this window can run `shortcut` now, which the command bar offers only then.
    pub(crate) fn can_run(&self, shortcut: Shortcut) -> bool {
        match shortcut {
            Shortcut::NewFlechinha
            | Shortcut::NewSpace
            | Shortcut::EditSpace
            | Shortcut::Archive
            | Shortcut::StepSpace(_)
            | Shortcut::SelectSpace(_)
            | Shortcut::RestoreBeforeImport => !self.incognito,
            Shortcut::Pin => !self.incognito && self.today.is_some(),
            Shortcut::Duplicate
            | Shortcut::Mute
            | Shortcut::CopyLink(_)
            | Shortcut::Browser(_)
            | Shortcut::DevTools => self.tab.is_some(),
            Shortcut::ViewSource => self.web_page,
            Shortcut::ExitSplit => self.split,
            _ => true,
        }
    }

    /// `copyLink` and `copyMarkdown`: the text and the confirmation.
    fn link(&self, markdown: bool) -> Option<(String, &'static str)> {
        let (url, name) = self.link.as_ref()?;
        Some(if markdown {
            (format!("[{name}]({url})"), "Markdown link copied")
        } else {
            (url.clone(), "Link copied")
        })
    }
}

/// A held key runs these once: opening something again, or flipping a tab's pin or mute back
/// and forth, is not what holding the key means.
fn runs_once(shortcut: Shortcut) -> bool {
    matches!(
        shortcut,
        Shortcut::Command(Command::OpenSettings)
            | Shortcut::SettingsTab(_)
            | Shortcut::Find
            | Shortcut::DevTools
            | Shortcut::NewFlechinha
            | Shortcut::Pin
            | Shortcut::Duplicate
            | Shortcut::Mute
            | Shortcut::ClearToday
            | Shortcut::CopyLink(_)
            | Shortcut::NewSpace
            | Shortcut::ExitSplit
            | Shortcut::ViewSource
            | Shortcut::EditSpace
            | Shortcut::Archive
            | Shortcut::Downloads
            | Shortcut::RestoreBeforeImport
    )
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
    fn flecha_commands_and_space_numbers_have_their_default_keys() {
        let bindings = Bindings::new(&Keybindings::new());
        let os = match platform() {
            Platform::Linux => gpui::OperatingSystem::Linux,
            Platform::Mac => gpui::OperatingSystem::Mac,
            Platform::Windows => gpui::OperatingSystem::Windows,
        };
        for id in [
            "newFlechinha",
            "pinTab",
            "duplicateTab",
            "muteTab",
            "clearToday",
            "copyLink",
            "copyMarkdown",
            "nextSpace",
            "previousSpace",
            "newSpace",
            "exitSplit",
            "viewSource",
        ] {
            assert!(has_action(id), "{id}");
            let keys = shortcuts::default_keys(id, platform()).expect(id);
            let stroke = Keystroke::parse_with_os(&gpui_keys(keys), os).unwrap();
            assert_eq!(bindings.action(&stroke), shortcut_for_id(id), "{id}");
        }
        let fixed = shortcuts::fixed_shortcuts(platform());
        for (keys, id) in fixed
            .iter()
            .filter(|(_, id)| id.starts_with("selectSpace:"))
        {
            let stroke = Keystroke::parse_with_os(&gpui_keys(keys), os).unwrap();
            assert!(shortcut_for_id(id).is_some(), "{id}");
            assert_eq!(bindings.action(&stroke), shortcut_for_id(id), "{id}");
        }
        if platform() == Platform::Linux {
            assert_eq!(
                bindings.action(&Keystroke::parse_with_os("ctrl-alt-2", os).unwrap()),
                Some(Shortcut::SelectSpace(1))
            );
        }
        assert_eq!(shortcut_for_id("selectSpace:0"), None);
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
