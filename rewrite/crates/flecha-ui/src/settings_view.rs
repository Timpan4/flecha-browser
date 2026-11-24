#[path = "settings_boosts.rs"]
mod boosts;
#[path = "settings_general.rs"]
mod general;
#[path = "settings_import.rs"]
mod import;
#[path = "settings_shortcuts.rs"]
mod shortcuts;
#[path = "settings_tabs.rs"]
mod tabs;

pub(crate) use import::RESTORE_BEFORE_IMPORT;

use anyhow::Result;
use flecha_core::Settings;
use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, Keystroke, Modifiers, ParentElement as _,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};

use crate::Controller;
use crate::text_field::TextField;
use crate::view::{Overlay, Shell, button, overlay_panel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Shortcuts,
    Boosts,
    Import,
    TabsSpaces,
}

impl SettingsTab {
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Shortcuts => "Shortcuts",
            Self::Boosts => "Boosts",
            Self::Import => "Import",
            Self::TabsSpaces => "Tabs & Spaces",
        }
    }
}

pub(crate) struct SettingsPanel {
    tab: SettingsTab,
    field: Option<Draft>,
    boosts: boosts::Editor,
    import: import::ImportPanel,
    manager: tabs::Manager,
    scroll: ScrollHandle,
    message: Option<(String, bool)>,
}

#[derive(Clone, PartialEq, Eq)]
enum Field {
    General(general::GeneralField),
    Shortcut(&'static str),
    BoostHost,
    BoostSource,
    ImportPath,
}

struct Draft {
    field: Field,
    text: TextField,
}

impl SettingsPanel {
    pub(crate) fn refresh(&mut self, state: &Controller) {
        if self.tab == SettingsTab::Boosts {
            self.boosts.refresh(state);
        }
    }

    pub(crate) fn prepare_close(&mut self) -> bool {
        if self.boosts.dirty() {
            self.select(SettingsTab::Boosts);
            self.result(Err(anyhow::anyhow!(
                "Save or discard the Boost draft before closing Settings"
            )));
            return false;
        }
        true
    }

    fn new(tab: SettingsTab) -> Self {
        Self {
            tab,
            field: None,
            boosts: boosts::Editor::default(),
            import: import::ImportPanel::default(),
            manager: tabs::Manager::default(),
            scroll: ScrollHandle::new(),
            message: None,
        }
    }

    fn select(&mut self, tab: SettingsTab) {
        if self.tab != tab {
            self.tab = tab;
            self.field = None;
            self.message = None;
            self.import.confirm_restore = false;
            self.scroll = ScrollHandle::new();
            if tab == SettingsTab::TabsSpaces {
                self.manager.focus_search();
            }
        }
    }

    fn begin(&mut self, field: Field, text: String) {
        let text = if matches!(field, Field::General(general::GeneralField::Environment(_))) {
            TextField::multiline(text, true)
        } else {
            TextField::new(text, true)
        };
        self.field = Some(Draft { field, text });
        self.message = None;
    }

    fn result(&mut self, result: Result<String>) {
        self.message = match result {
            Ok(message) if message.is_empty() => None,
            Ok(message) => Some((message, false)),
            Err(error) => Some((format!("{error:#}"), true)),
        };
    }
}

impl Controller {
    pub(crate) fn edit_settings<T>(
        &mut self,
        edit: impl FnOnce(&mut Settings) -> Result<T>,
    ) -> Result<T> {
        let mut settings = self.model.core.settings.clone();
        let result = edit(&mut settings)?;
        if settings != self.model.core.settings {
            let previous = std::mem::replace(&mut self.model.core.settings, settings);
            if let Err(error) = self.save() {
                self.model.core.settings = previous;
                return Err(error.context("Save settings"));
            }
            self.sync_engine_settings(&previous);
        }
        Ok(result)
    }

    fn sync_engine_settings(&mut self, previous: &Settings) {
        let current = &self.model.core.settings;
        let adblock_changed = previous.ad_block != current.ad_block
            || previous.ad_block_allowed != current.ad_block_allowed;
        let downloads_changed = previous.downloads_folder != current.downloads_folder
            || previous.ask_where_to_save != current.ask_where_to_save;
        if adblock_changed {
            self.sync_adblock();
        }
        if downloads_changed {
            self.sync_download_config();
        }
    }
}

impl Shell {
    pub(crate) fn settings_before_shortcuts(
        &mut self,
        stroke: &Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(panel) = self.settings_mut() else {
            return false;
        };
        if panel.tab == SettingsTab::TabsSpaces && panel.manager.before_shortcuts(stroke) {
            cx.notify();
            return true;
        }
        false
    }

    pub(crate) fn toggle_settings(
        &mut self,
        tab: SettingsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.state.borrow().model.core.window(&self.id).is_none() {
            return false;
        }
        match &mut self.overlay {
            Overlay::Settings(panel) if panel.tab == tab => self.dismiss_overlay(window, cx),
            Overlay::Settings(panel) => {
                panel.select(tab);
                cx.notify();
            }
            _ => self.show_overlay(
                Overlay::Settings(Box::new(SettingsPanel::new(tab))),
                window,
                cx,
            ),
        }
        true
    }

    fn settings_mut(&mut self) -> Option<&mut SettingsPanel> {
        match &mut self.overlay {
            Overlay::Settings(panel) => Some(panel),
            _ => None,
        }
    }

    pub(crate) fn settings_key(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        if panel.tab == SettingsTab::TabsSpaces {
            self.tabs_settings_key(stroke, cx);
            return;
        }
        let Some(draft) = &mut panel.field else {
            return;
        };
        if draft.field == Field::BoostSource {
            panel.boosts.key(stroke, cx);
            cx.notify();
            return;
        }
        if stroke.modifiers == Modifiers::none() && stroke.key == "enter" {
            match &draft.field {
                Field::General(field)
                    if !matches!(field, general::GeneralField::Environment(_)) =>
                {
                    let (field, text) = (field.clone(), draft.text.text().to_owned());
                    self.submit_general(field, &text, cx);
                    return;
                }
                Field::Shortcut(id) => {
                    let (id, text) = (*id, draft.text.text().trim().to_owned());
                    self.change_binding(id, flecha_core::shortcuts::BindingEdit::Set(&text), cx);
                    return;
                }
                Field::ImportPath => {
                    self.preview_import(cx);
                    return;
                }
                Field::General(_) | Field::BoostSource | Field::BoostHost => {}
            }
        }
        let previous_path =
            (draft.field == Field::ImportPath).then(|| draft.text.text().to_owned());
        draft.text.key(stroke, cx);
        if previous_path.is_some_and(|path| path != draft.text.text()) {
            panel.import.clear_preview();
            panel.message = None;
        }
        cx.notify();
    }

    pub(crate) fn settings_panel(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let tabs = [
            SettingsTab::General,
            SettingsTab::Shortcuts,
            SettingsTab::Boosts,
            SettingsTab::Import,
            SettingsTab::TabsSpaces,
        ]
        .into_iter()
        .map(|tab| {
            let selected = tab == panel.tab;
            button(tab.label(), tab.label())
                .when(selected, |tab| tab.bg(rgb(0x27547b)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(panel) = this.settings_mut() {
                        panel.select(tab);
                        cx.notify();
                    }
                }))
        });
        let content = match panel.tab {
            SettingsTab::General => self.general_settings(panel, cx),
            SettingsTab::Shortcuts => self.shortcut_settings(panel, cx),
            SettingsTab::Boosts => self.boosts_settings(panel, cx),
            SettingsTab::Import => self.import_settings(panel, cx),
            SettingsTab::TabsSpaces => self.tabs_settings(panel, cx),
        };
        overlay_panel()
            .w(px(680.))
            .h_full()
            .gap_3()
            .child(
                row()
                    .justify_between()
                    .flex_shrink_0()
                    .child(div().text_lg().child("Settings"))
                    .child(button("settings-close", "Close").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                    ))),
            )
            .child(row().children(tabs).flex_shrink_0())
            .when_some(panel.message.as_ref(), |frame, (text, error)| {
                frame.child(
                    div()
                        .rounded_md()
                        .p_2()
                        .bg(rgb(if *error { 0x542f31 } else { 0x263c46 }))
                        .child(text.clone()),
                )
            })
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&panel.scroll)
                    .pr_2()
                    .child(content),
            )
    }
}

fn row() -> Div {
    div().flex().items_center().flex_wrap().gap_2()
}

fn section(label: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .py_2()
        .border_t_1()
        .border_color(rgb(0x405863))
        .child(div().text_base().child(label))
}

fn muted(label: impl Into<gpui::SharedString>) -> Div {
    div().text_color(rgb(0xa4b5c3)).child(label.into())
}
