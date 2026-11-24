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
    ClickEvent, Context, Div, FontWeight, InteractiveElement as _, Keystroke, Modifiers,
    ParentElement as _, ScrollHandle, StatefulInteractiveElement as _, Styled as _, Window, div,
    linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px,
};

use crate::Controller;
use crate::icons::icon;
use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, text_button};

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

    /// Settings as the shipping settings page lays it out (ui/settings.css:46-170): a list of
    /// panes on the left and the shown pane's cards on `--pane-bg`.
    pub(crate) fn settings_panel(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let palette = palette();
        // The panes in the order and under the headings of settings.html:19-34, with the icon
        // and colour each has there.
        type Pane = (SettingsTab, &'static str, &'static str);
        let groups: [(Option<&'static str>, &[Pane]); 3] = [
            (
                None,
                &[
                    (SettingsTab::General, "settings", "gray"),
                    (SettingsTab::TabsSpaces, "stack", "indigo"),
                    (SettingsTab::Import, "import", "teal"),
                ],
            ),
            (Some("Developer"), &[(SettingsTab::Boosts, "zap", "orange")]),
            (
                Some("Help"),
                &[(SettingsTab::Shortcuts, "keyboard", "gray")],
            ),
        ];
        let mut nav = div()
            .id("settings-nav")
            .flex()
            .flex_col()
            .flex_none()
            .min_h_0()
            .overflow_y_scroll()
            .w(px(tokens::SETTINGS_NAV))
            .h_full()
            .gap(px(tokens::SETTINGS_NAV_GAP))
            .px(px(tokens::SETTINGS_NAV_PAD_X))
            .pb(px(tokens::SETTINGS_NAV_PAD_BOTTOM))
            .rounded_l(px(tokens::PAGE_RADIUS))
            .bg(palette.surface)
            .child(div().flex_none().h(px(tokens::SETTINGS_TITLEBAR)));
        for (heading, tabs) in groups {
            if let Some(heading) = heading {
                nav = nav.child(
                    div()
                        .mt(px(tokens::SETTINGS_NAV_LABEL_TOP))
                        .mb(px(tokens::SETTINGS_NAV_LABEL_BOTTOM))
                        .px(px(tokens::SETTINGS_NAV_LABEL_PAD))
                        .text_size(px(tokens::TEXT_XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.text_3)
                        .child(heading),
                );
            }
            for &(tab, glyph, tint) in tabs {
                nav = nav.child(nav_item(tab, glyph, tint, tab == panel.tab, cx));
            }
        }
        let content = match panel.tab {
            SettingsTab::General => self.general_settings(panel, cx),
            SettingsTab::Shortcuts => self.shortcut_settings(panel, cx),
            SettingsTab::Boosts => self.boosts_settings(panel, cx),
            SettingsTab::Import => self.import_settings(panel, cx),
            SettingsTab::TabsSpaces => self.tabs_settings(panel, cx),
        };
        div()
            .flex()
            .size_full()
            .min_h_0()
            .occlude()
            .text_color(palette.text)
            .child(nav)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .rounded_r(px(tokens::PAGE_RADIUS))
                    .bg(palette.pane_bg)
                    .shadow(tokens::shadow_pane_edge(palette.dark))
                    .child(
                        // `.pane-titlebar` and its `h1` (ui/settings.css:144-161).
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .h(px(tokens::SETTINGS_TITLEBAR))
                            .px(px(tokens::SETTINGS_PANE_PAD_X))
                            .child(
                                div()
                                    .text_size(px(tokens::TEXT_LG))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(panel.tab.label()),
                            )
                            .child(text_button("settings-close", "Close").on_click(cx.listener(
                                |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                            ))),
                    )
                    .when_some(panel.message.as_ref(), |pane, (text, error)| {
                        pane.child(
                            div()
                                .mx(px(tokens::SETTINGS_PANE_PAD_X))
                                .mb(px(tokens::SETTINGS_CARD_GAP))
                                .px(px(tokens::SETTINGS_ROW_PAD_X))
                                .py(px(tokens::SETTINGS_ROW_PAD_Y))
                                .rounded(px(tokens::RADIUS_LG))
                                .bg(palette.card)
                                .shadow(tokens::shadow_card(palette.dark))
                                .when(*error, |message| message.text_color(palette.red))
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
                            .pt(px(tokens::SETTINGS_PANE_PAD_TOP))
                            .px(px(tokens::SETTINGS_PANE_PAD_X))
                            .pb(px(tokens::SETTINGS_PANE_PAD_BOTTOM))
                            .child(content),
                    ),
            )
    }
}

/// `.nav-item` (ui/settings.css:74-92) with its coloured `.nav-icon` (ui/settings.css:94-131).
fn nav_item(
    tab: SettingsTab,
    glyph: &'static str,
    tint: &'static str,
    selected: bool,
    cx: &Context<Shell>,
) -> gpui::Stateful<Div> {
    let palette = palette();
    let stop = |end: &str| tokens::color(&format!(".nav-icon.{tint} {end}"), false);
    div()
        .id(tab.label())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(tokens::SETTINGS_NAV_ITEM_GAP))
        .h(px(tokens::SETTINGS_NAV_ITEM))
        .px(px(tokens::SETTINGS_NAV_ITEM_PAD))
        .rounded(px(tokens::RADIUS_SM))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .map(|item| {
            if selected {
                item.bg(palette.selection)
                    .text_color(palette.selection_text)
            } else {
                item.hover(|item| item.bg(palette.fill_hover))
            }
        })
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(tokens::SETTINGS_NAV_ICON))
                .rounded(px(tokens::SETTINGS_NAV_ICON_RADIUS))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(stop("top"), 0.),
                    linear_color_stop(stop("bottom"), 1.),
                ))
                .shadow(tokens::shadow_nav_icon())
                .child(icon(
                    glyph,
                    tokens::SETTINGS_NAV_GLYPH,
                    tokens::color(".nav-icon color", false),
                )),
        )
        .child(tab.label())
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if let Some(panel) = this.settings_mut() {
                panel.select(tab);
                cx.notify();
            }
        }))
}

fn row() -> Div {
    div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(tokens::SPACE_2))
}

/// A group of settings on a `.card` (ui/settings.css:218-226), under its name in the style of
/// `.nav-label` (ui/settings.css:66-72): the shipping panes put one topic on each card.
fn section(label: &'static str) -> Div {
    let palette = palette();
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SETTINGS_ROW_PAD_Y))
        .mb(px(tokens::SETTINGS_CARD_GAP))
        .px(px(tokens::SETTINGS_ROW_PAD_X))
        .py(px(tokens::SETTINGS_ROW_PAD_Y))
        .rounded(px(tokens::RADIUS_LG))
        .bg(palette.card)
        .shadow(tokens::shadow_card(palette.dark))
        .child(
            div()
                .text_size(px(tokens::TEXT_XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text_3)
                .child(label),
        )
}

/// `.row-text small` (ui/settings.css:264-270).
fn muted(label: impl Into<gpui::SharedString>) -> Div {
    div()
        .text_size(px(tokens::TEXT_SM))
        .text_color(palette().text_2)
        .child(label.into())
}

/// `.button` (ui/settings.css:282-304): a raised button on a card.
pub(crate) fn settings_button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    settings_choice(id, name, false)
}

/// A raised button that can be selected without losing contrast on hover.
pub(crate) fn settings_choice(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
    selected: bool,
) -> gpui::Stateful<Div> {
    let palette = palette();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(tokens::SETTINGS_BUTTON))
        .px(px(tokens::SETTINGS_BUTTON_PAD))
        .rounded(px(tokens::RADIUS_SM))
        .shadow(tokens::shadow_button(palette.dark))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|button| {
            if selected {
                button
                    .bg(palette.selection)
                    .text_color(palette.selection_text)
            } else {
                button
                    .bg(palette.card)
                    .hover(|button| button.bg(palette.surface_2))
            }
        })
        .child(name.into())
}
