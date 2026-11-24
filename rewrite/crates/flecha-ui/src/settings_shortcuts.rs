use anyhow::anyhow;
use flecha_core::shortcuts::{self, BindingEdit, BindingError, SHORTCUTS};
use gpui::{
    ClickEvent, Context, Div, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
};

use super::{Draft, Field, SettingsPanel, muted, row, section, settings_button};
use crate::shortcuts::{has_action, platform};
use crate::view::Shell;

impl Shell {
    pub(super) fn shortcut_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let bindings = &state.model.core.settings.keybindings;
        let reset = settings_button("reset-shortcuts", "Reset all defaults").on_click(cx.listener(
            |this, _: &ClickEvent, _, cx| {
                let result = this.state.borrow_mut().edit_settings(|settings| {
                    settings.keybindings.clear();
                    Ok("Restored all shortcut defaults.".into())
                });
                if let Some(panel) = this.settings_mut() {
                    panel.field = None;
                    panel.result(result);
                }
                cx.refresh_windows();
            },
        ));
        let mut content = div().flex().flex_col().gap_2().child(row().child(reset));
        let mut group = "";
        for command in SHORTCUTS {
            if command.group != group {
                group = command.group;
                content = content.child(section(group));
            }
            let id = command.id;
            let effective = shortcuts::keys_for(bindings, id, platform());
            let default = shortcuts::default_keys(id, platform());
            let edit =
                settings_button(format!("edit-{id}"), label(effective)).on_click(cx.listener({
                    let text = effective.or(default).unwrap_or_default().to_owned();
                    move |this, _: &ClickEvent, _, cx| {
                        if let Some(panel) = this.settings_mut() {
                            panel.begin(Field::Shortcut(id), text.clone());
                            cx.notify();
                        }
                    }
                }));
            let mut entry = div().flex().flex_col().gap_1().child(
                row()
                    .justify_between()
                    .child(div().child(command.label))
                    .child(
                        row()
                            .child(edit)
                            .child(muted(format!("Default {}", label(default)))),
                    ),
            );
            if !has_action(id) {
                entry = entry.child(muted("Action unavailable in this build"));
            }
            if let Some(Draft {
                field: Field::Shortcut(active),
                text,
            }) = &panel.field
                && *active == id
            {
                entry =
                    entry.child(text.render("Example: Ctrl+Shift+K")).child(
                        row()
                            .child(settings_button(format!("save-{id}"), "Save").on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let Some(Draft { text, .. }) =
                                        this.settings_mut().and_then(|p| p.field.as_ref())
                                    else {
                                        return;
                                    };
                                    let text = text.text().trim().to_owned();
                                    this.change_binding(id, BindingEdit::Set(&text), cx);
                                }),
                            ))
                            .child(settings_button(format!("reset-{id}"), "Reset").on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.change_binding(id, BindingEdit::Reset, cx)
                                }),
                            ))
                            .child(settings_button(format!("unbind-{id}"), "Unbind").on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.change_binding(id, BindingEdit::Unbind, cx)
                                }),
                            )),
                    );
            }
            content = content.child(entry);
        }
        content
    }

    pub(super) fn change_binding(
        &mut self,
        id: &str,
        edit: BindingEdit<'_>,
        cx: &mut Context<Self>,
    ) {
        let result = self.state.borrow_mut().edit_settings(|settings| {
            let displaced = shortcuts::set_binding(&mut settings.keybindings, id, edit, platform())
                .map_err(|error| {
                    anyhow!(match error {
                        BindingError::UnknownCommand => "Unknown shortcut action.",
                        BindingError::InvalidAccelerator =>
                            "Enter a modifier and key, or a function key.",
                        BindingError::Reserved =>
                            "That shortcut is reserved for tab, Space or Flechinha navigation.",
                    })
                })?;
            let labels: Vec<_> = SHORTCUTS
                .iter()
                .filter(|command| displaced.contains(&command.id))
                .map(|command| command.label)
                .collect();
            Ok(if labels.is_empty() {
                String::new()
            } else {
                format!("Cleared the previous binding for {}.", labels.join(", "))
            })
        });
        if let Some(panel) = self.settings_mut() {
            if result.is_ok() {
                panel.field = None;
            }
            panel.result(result);
        }
        cx.refresh_windows();
    }
}

fn label(keys: Option<&str>) -> String {
    keys.map_or_else(
        || "Unbound".into(),
        |keys| shortcuts::display_keys(Some(keys), platform()),
    )
}
