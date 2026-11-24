//! The sidebar's Space editor (chromium/shell/ui/index.html:124-144, ui/app.ts:984-1051) and
//! what Spaces do from their menu: rename, recolour, keep their own cookies, open in a window of
//! their own, and go.

use anyhow::{Context as _, Result};
use flecha_core::{SpaceId, WindowId};
use gpui::{
    BoxShadow, ClickEvent, Context, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::icons::icon;
use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{
    Overlay, SHELL_SWITCH, Shell, button, danger_button, hairline, overlay_panel, popover_footer,
    popover_header, primary_button, ring, switch,
};
use crate::{Action, Controller, dispatch, now};

/// The icons and colours offered for a Space (chromium/shell/ui/spaceChoices.ts).
pub(crate) const SPACE_EMOJIS: [&str; 30] = [
    "🏠", "💼", "🚀", "🧪", "🛠️", "🎨", "📚", "🧠", "💡", "🎮", "🎧", "🎬", "✈️", "🌱", "☕️", "🔥",
    "⭐️", "🌙", "🌊", "🐙", "🦊", "🐳", "⚡️", "🎯", "📈", "💬", "📝", "🗂️", "🛒", "❤️",
];
pub(crate) const SPACE_COLORS: [&str; 12] = [
    "#cdd9f7", "#c9e3f5", "#cfe8ec", "#d3ecd5", "#d9e8c6", "#f5e7c4", "#f7dcc0", "#f6d6cc",
    "#f4cfdc", "#e8d6f3", "#d6d3f5", "#dfe3e8",
];

/// What a Space's menu or editor changes, by its id.
#[derive(Clone, Debug)]
pub(crate) enum SpaceAction {
    Name(String),
    /// An empty emoji clears it.
    Emoji(String),
    Color(String),
    Isolate(bool),
    OpenInNewWindow,
    Delete,
}

impl Controller {
    pub(crate) fn space_action(
        &mut self,
        id: &SpaceId,
        action: SpaceAction,
    ) -> Result<Option<WindowId>> {
        self.model.core.space(id).context("The Space is gone")?;
        match action {
            SpaceAction::Name(name) => {
                self.model
                    .core
                    .update_space(id, Some(&name), None, None, None);
            }
            SpaceAction::Emoji(emoji) => {
                self.model
                    .core
                    .update_space(id, None, Some(&emoji), None, None);
            }
            SpaceAction::Color(color) => {
                self.model
                    .core
                    .update_space(id, None, None, Some(&color), None);
            }
            SpaceAction::Isolate(isolated) => self.set_space_isolated(id, isolated),
            SpaceAction::OpenInNewWindow => {
                return self.model.open_space_window(id).map(Some);
            }
            SpaceAction::Delete => {
                // Core keeps the last Space, and then its transients stay open too.
                let spaces = self.model.core.spaces();
                if spaces.len() < 2 || spaces.iter().all(|space| &space.id != id) {
                    return Ok(None);
                }
                for tab in self.model.close_space_transients(id) {
                    self.engine.close(&tab);
                }
                for tab in self.model.core.delete_space(id, now()) {
                    self.engine.close(&tab);
                }
            }
        }
        Ok(None)
    }

    /// Gives the Space its own cookies or the shared ones. Its tabs reopen in their new
    /// context, and the Peeks and Flechinhas it opened close.
    pub(crate) fn set_space_isolated(&mut self, id: &SpaceId, isolated: bool) {
        // A stale menu may ask for the setting the Space already has; nothing changes then.
        if self
            .model
            .core
            .space(id)
            .is_none_or(|space| space.isolated == isolated)
        {
            return;
        }
        for tab in self.model.core.set_isolated(id, isolated) {
            self.reconcile_partition(&tab);
        }
        for tab in self.model.close_space_transients(id) {
            self.engine.close(&tab);
        }
    }
}

fn space_color(value: &str) -> gpui::Rgba {
    tokens::parse_color(value).unwrap_or_else(|| palette().tint)
}

impl Shell {
    /// `editSpace` (chromium/shell/background.ts:1181-1186): the editor opens in that Space.
    pub(crate) fn open_space_editor(
        &mut self,
        id: &SpaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = {
            let state = self.state.borrow();
            if state.model.is_incognito(&self.id) || state.model.core.window(&self.id).is_none() {
                return;
            }
            let Some(space) = state.model.core.space(id) else {
                return;
            };
            space.name.clone()
        };
        let shown = self
            .state
            .borrow()
            .model
            .core
            .window(&self.id)
            .is_some_and(|current| &current.active_space_id == id);
        if !shown {
            dispatch(&self.state, &self.id, Action::Switch(id.clone()), cx);
        }
        self.show_overlay(
            Overlay::SpaceEditor(id.clone(), TextField::new(name, true)),
            window,
            cx,
        );
    }

    /// New Space from the footer, a shortcut or the command bar: shipping opens the editor on
    /// it (`newSpace`, background.ts:1296).
    pub(crate) fn new_space(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.state.borrow().model.core.window(&self.id).is_none() {
            return false;
        }
        dispatch(&self.state, &self.id, Action::NewSpace, cx);
        let space = self
            .state
            .borrow()
            .model
            .core
            .window(&self.id)
            .map(|current| current.active_space_id.clone());
        if let Some(space) = space {
            self.open_space_editor(&space, window, cx);
        }
        true
    }

    pub(crate) fn space_editor_key(
        &mut self,
        stroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stroke.key == "enter" && stroke.modifiers == gpui::Modifiers::none() {
            self.dismiss_overlay(window, cx);
            return;
        }
        let Overlay::SpaceEditor(id, input) = &mut self.overlay else {
            return;
        };
        let before = input.text().to_owned();
        input.key(stroke, cx);
        let name = input.text().to_owned();
        let id = id.clone();
        cx.notify();
        // Each keystroke renames the Space, as `input` does in the shipping editor; an empty
        // field leaves the name alone.
        if name != before && !flecha_core::input_text::trim_input(&name).is_empty() {
            dispatch(
                &self.state,
                &self.id,
                Action::Space(id, SpaceAction::Name(name)),
                cx,
            );
        }
    }

    pub(crate) fn confirm_delete_space(
        &mut self,
        id: &SpaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.borrow().model.core.spaces().len() > 1 {
            self.show_overlay(Overlay::DeleteSpace(id.clone()), window, cx);
        }
    }

    /// `#space-editor` (ui/index.html:124-144, ui/styles.css:1417-1592).
    pub(crate) fn space_editor_panel(
        &self,
        id: &SpaceId,
        input: &TextField,
        cx: &Context<Self>,
    ) -> Div {
        let state = self.state.borrow();
        let Some(space) = state.model.core.space(id) else {
            return overlay_panel();
        };
        let palette = palette();
        let deletable = state.model.core.spaces().len() > 1;
        let label = |text: &'static str| {
            div()
                .mt(px(tokens::FIELD_LABEL_GAP))
                .text_size(px(tokens::TEXT_XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text_3)
                .child(text)
        };
        let edit = |action: SpaceAction| {
            let action = Action::Space(id.clone(), action);
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                dispatch(&this.state, &this.id, action.clone(), cx);
            })
        };
        let current_emoji = space.emoji.as_deref().filter(|emoji| !emoji.is_empty());
        let emoji_cell = |id: gpui::ElementId, selected: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .aspect_ratio(1.)
                .rounded(px(tokens::RADIUS_SM))
                .text_size(px(tokens::EMOJI_TEXT))
                .text_color(palette.text)
                .cursor_pointer()
                .hover(|cell| cell.bg(palette.surface_2))
                .when(selected, |cell| {
                    cell.bg(palette.surface_2).shadow(vec![BoxShadow {
                        inset: true,
                        ..ring(palette.selection, tokens::EMOJI_SELECTED_RING)
                    }])
                })
        };
        let emojis = div()
            .grid()
            .grid_cols(tokens::EMOJI_COLUMNS as u16)
            .gap(px(tokens::EMOJI_GAP))
            .child(
                emoji_cell("space-emoji-none".into(), current_emoji.is_none())
                    .aria_label("No icon")
                    .on_click(edit(SpaceAction::Emoji(String::new())))
                    .child(icon("close", tokens::EMOJI_ICON, palette.text_3)),
            )
            .children(SPACE_EMOJIS.iter().enumerate().map(|(index, emoji)| {
                emoji_cell(("space-emoji", index).into(), current_emoji == Some(*emoji))
                    .on_click(edit(SpaceAction::Emoji((*emoji).into())))
                    .child(*emoji)
            }));
        // The shipping editor ends the row with a native colour picker, which GPUI lacks; a
        // custom colour shows as its own selected swatch.
        let custom = (!SPACE_COLORS.contains(&space.color.as_str())).then(|| space.color.clone());
        // `.swatch`'s hairline: `inset 0 0 0 0.5px rgba(0, 0, 0, 0.12)` (ui/styles.css:1503).
        let swatch_line = hairline(gpui::Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.12,
        });
        let swatch = |id: gpui::ElementId, color: &str, selected: bool| {
            let mut shadow = vec![swatch_line.clone()];
            if selected {
                shadow.push(ring(palette.surface, tokens::SWATCH_SELECTED_GAP));
                shadow.push(ring(palette.selection, tokens::SWATCH_SELECTED_RING));
            }
            div()
                .id(id)
                .aspect_ratio(1.)
                .rounded_full()
                .bg(space_color(color))
                .shadow(shadow)
                .cursor_pointer()
                .aria_label(color.to_owned())
        };
        let colors = div()
            .grid()
            .grid_cols(tokens::SWATCH_COLUMNS as u16)
            .gap(px(tokens::SWATCH_GAP))
            .children(SPACE_COLORS.iter().enumerate().map(|(index, color)| {
                swatch(("space-color", index).into(), color, space.color == *color)
                    .on_click(edit(SpaceAction::Color((*color).into())))
            }))
            .children(custom.map(|color| swatch("space-color-custom".into(), &color, true)));
        let isolated = space.isolated;
        let toggle = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens::SPACE_3))
            .mt(px(tokens::TOGGLE_ROW_GAP))
            .py(px(tokens::TOGGLE_ROW_PAD_Y))
            .px(px(tokens::TOGGLE_ROW_PAD_X))
            .rounded(px(tokens::RADIUS_MD))
            .bg(palette.surface_2)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Separate cookies and logins"),
                    )
                    .child(
                        div()
                            .mt(px(tokens::TOGGLE_ROW_NOTE_GAP))
                            .text_size(px(tokens::TEXT_XS))
                            .text_color(palette.text_3)
                            .child("Sign in as someone else in this Space."),
                    ),
            )
            .child(
                switch(
                    "space-isolated-toggle",
                    isolated,
                    "Separate cookies and logins",
                    SHELL_SWITCH,
                )
                .on_click(edit(SpaceAction::Isolate(!isolated))),
            );
        let delete = id.clone();
        overlay_panel()
            .p_0()
            .gap_0()
            .child(popover_header("Edit Space"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(tokens::SPACE_2))
                    .pt(px(tokens::EDITOR_PAD_TOP))
                    .px(px(tokens::EDITOR_PAD_X))
                    .pb(px(tokens::EDITOR_PAD_BOTTOM))
                    .child(input.render("Space name"))
                    .child(label("Icon"))
                    .child(emojis)
                    .child(label("Colour"))
                    .child(colors)
                    .child(toggle),
            )
            .child(
                popover_footer()
                    .child(if deletable {
                        danger_button("space-delete", "Delete Space").on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                this.confirm_delete_space(&delete, window, cx);
                            },
                        ))
                    } else {
                        danger_button("space-delete", "Delete Space")
                            .opacity(tokens::MENU_DISABLED_OPACITY)
                            .cursor_default()
                    })
                    .child(primary_button("space-done", "Done").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                    ))),
            )
    }

    /// Electron's "Delete “Space”?" question (chromium/shell/windowUi.ts:225-240), as
    /// `.shell-dialog` (shell.css:139-172) draws it. The rewrite has no undo yet, so it says
    /// where the tabs go instead of how to bring them back.
    pub(crate) fn delete_space_panel(&self, id: &SpaceId, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let Some(space) = state.model.core.space(id) else {
            return overlay_panel();
        };
        let delete = Action::Space(id.clone(), SpaceAction::Delete);
        overlay_panel()
            .gap_0()
            .p(px(tokens::DIALOG_PAD))
            .child(
                div()
                    .text_size(px(tokens::TEXT_LG))
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Delete “{}”?", space.name)),
            )
            .child(
                div()
                    .mt(px(tokens::DIALOG_TEXT_GAP_TOP))
                    .mb(px(tokens::DIALOG_TEXT_GAP_BOTTOM))
                    .text_color(palette().text_2)
                    .child("Its pinned and today tabs are closed and archived."),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(tokens::DIALOG_BUTTON_GAP))
                    .child(
                        danger_button("delete-space", "Delete Space").on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                this.dismiss_overlay(window, cx);
                                dispatch(&this.state, &this.id, delete.clone(), cx);
                            },
                        )),
                    )
                    .child(
                        button("cancel-delete-space", "Cancel").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                        )),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_actions_edit_isolate_open_and_delete_the_space() {
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
        let act = |controller: &mut Controller, action| {
            controller
                .act(&window, Action::Space(second.clone(), action))
                .unwrap();
        };
        act(&mut controller, SpaceAction::Name("Work".into()));
        act(&mut controller, SpaceAction::Emoji("💼".into()));
        act(&mut controller, SpaceAction::Color("#F4CFDC".into()));
        let space = controller.model.core.space(&second).unwrap();
        assert_eq!(
            (
                space.name.as_str(),
                space.emoji.as_deref(),
                space.color.as_str()
            ),
            ("Work", Some("💼"), "#f4cfdc")
        );
        act(&mut controller, SpaceAction::Emoji(String::new()));
        assert_eq!(controller.model.core.space(&second).unwrap().emoji, None);

        let tab = controller
            .model
            .open(&window, "https://space.example/".into(), true)
            .unwrap();
        assert_eq!(
            controller.model.partition(&tab),
            Some(flecha_cef::Partition::Shared)
        );
        act(&mut controller, SpaceAction::Isolate(true));
        assert_eq!(
            controller.model.partition(&tab),
            Some(flecha_cef::Partition::Isolated(second.clone()))
        );

        let windows = controller.model.core.windows().len();
        act(&mut controller, SpaceAction::OpenInNewWindow);
        assert_eq!(controller.model.core.windows().len(), windows + 1);
        let opened = controller.front.clone().unwrap();
        assert_ne!(opened, window);
        assert_eq!(
            controller
                .model
                .core
                .window(&opened)
                .unwrap()
                .active_space_id,
            second
        );

        act(&mut controller, SpaceAction::Delete);
        assert!(controller.model.core.space(&second).is_none());
        assert!(controller.model.core.tab(&tab).is_none());
        assert_eq!(
            controller.model.core.archive()[0].url,
            "https://space.example/"
        );
        assert_eq!(
            controller.model.core.focused_window().active_space_id,
            first
        );
        // The last Space stays, and so does the Peek it opened.
        controller
            .act(&window, Action::Peek("https://peek.example/".into()))
            .unwrap();
        assert!(controller.model.peeks.contains_key(&window));
        controller
            .act(&window, Action::Space(first.clone(), SpaceAction::Delete))
            .unwrap();
        assert!(controller.model.core.space(&first).is_some());
        assert!(controller.model.peeks.contains_key(&window));
        // A stale toggle that asks for the current setting leaves the Peek open too.
        controller
            .act(
                &window,
                Action::Space(first.clone(), SpaceAction::Isolate(false)),
            )
            .unwrap();
        assert!(controller.model.peeks.contains_key(&window));
    }
}
