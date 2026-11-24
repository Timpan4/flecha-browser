use std::collections::BTreeSet;

use anyhow::{Context as _, Result, ensure};
use flecha_boosts::{BoostKind, BoostName, CompiledBoost, applies, boost_host_for};
use gpui::{
    AppContext as _, ClickEvent, Context, CursorStyle, Div, InteractiveElement as _, Keystroke,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Task, div,
    prelude::FluentBuilder as _, px,
};

use super::{Draft, Field, SettingsPanel, muted, row, section};
use crate::Controller;
use crate::boosts::compile_script;
use crate::text_field::TextField;
use crate::theme::palette;
use crate::view::{Shell, button};

#[derive(Default)]
pub(super) struct Editor {
    selected: Option<BoostName>,
    host: String,
    source: TextField,
    baseline: String,
    compiling: Option<Task<()>>,
    confirm_delete: bool,
    revision: u64,
}

impl Editor {
    fn select(&mut self, name: BoostName, source: String) {
        self.selected = Some(name);
        self.baseline.clone_from(&source);
        self.source = TextField::multiline(source, false);
        self.confirm_delete = false;
    }

    pub(super) fn dirty(&self) -> bool {
        self.source.text() != self.baseline
    }

    pub(super) fn refresh(&mut self, state: &Controller) {
        let revision = state.boosts_revision();
        if revision != self.revision && !self.dirty() && self.compiling.is_none() {
            self.revision = revision;
            if let Some(name) = self.selected.clone()
                && let Some(file) = state.boosts.all().get(name.as_str())
                && file.code != self.baseline
            {
                self.select(name, file.code);
            }
        }
    }

    pub(super) fn key(&mut self, stroke: &Keystroke, cx: &mut Context<Shell>) {
        self.source.key(stroke, cx);
    }
}

impl Controller {
    fn active_boost_url(&self, window: &flecha_core::WindowId) -> Option<&str> {
        self.model
            .active(window)
            .and_then(|tab| self.model.core.tab(tab))
            .map(|tab| tab.url.as_str())
    }

    fn save_boost(
        &mut self,
        name: &BoostName,
        baseline: &str,
        source: &str,
        build: Option<CompiledBoost>,
    ) -> Result<String> {
        let current = self
            .boosts
            .all()
            .get(name.as_str())
            .context("This Boost was deleted")?;
        ensure!(
            current.code == baseline,
            "This Boost was edited in another window. Discard this draft to reload the saved version"
        );
        let error = build.as_ref().and_then(|build| build.error.clone());
        self.edit_boosts(|boosts| boosts.save(name, source, build))?;
        Ok(if error.is_some() {
            "Saved source with a compile error.".into()
        } else {
            "Boost saved.".into()
        })
    }
}

impl Shell {
    pub(super) fn boosts_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let active_url = state.active_boost_url(&self.id);
        let active_host = boost_host_for(active_url);
        let mut content = div().flex().flex_col().gap_3();
        if let Some(error) = state.boosts.load_error() {
            return content.child(section("Boosts")
                .child(div().child(format!("{error:#}")))
                .child(muted("Flecha kept the saved file. Reopen Flecha after restoring the Boosts snapshot.")));
        }
        let editor = &panel.boosts;
        let mut create = section("New Boost");
        if let Some(host) = &active_host {
            create = create.child(muted(format!("Active site · {host}")));
        }
        let host = if editor.host.is_empty() {
            active_host.as_deref().unwrap_or_default()
        } else {
            &editor.host
        };
        if let Some(Draft {
            field: Field::BoostHost,
            text,
        }) = &panel.field
        {
            create = create.child(text.render("Hostname or all"));
        } else {
            create = create.child(
                row().child(div().child("Host")).child(
                    button(
                        "boost-host",
                        if host.is_empty() {
                            "Enter hostname"
                        } else {
                            host
                        }
                        .to_owned(),
                    )
                    .on_click(cx.listener({
                        let host = host.to_owned();
                        move |this, _: &ClickEvent, _, cx| {
                            if let Some(panel) = this.settings_mut() {
                                panel.begin(Field::BoostHost, host.clone());
                                cx.notify();
                            }
                        }
                    })),
                ),
            );
        }
        create = create.child(
            row().children(
                [
                    ("new-css", "New CSS", BoostKind::Styles),
                    (
                        "new-script",
                        "New JavaScript / TypeScript",
                        BoostKind::Script,
                    ),
                ]
                .into_iter()
                .map(|(id, label, kind)| {
                    button(id, label).on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.create_boost(kind, cx)),
                    )
                }),
            ),
        );
        content = content.child(create);

        let mut list = section("Saved Boosts");
        let enabled: BTreeSet<_> = state
            .boosts()
            .iter()
            .map(|(name, ..)| name.as_str())
            .collect();
        let mut entries: Vec<_> = state.boosts.all().iter().collect();
        entries
            .sort_by(|(a, ..), (b, ..)| a.as_str().encode_utf16().cmp(b.as_str().encode_utf16()));
        if entries.is_empty() {
            list = list.child(muted("No Boosts saved."));
        }
        for (name, _, _) in entries {
            let enabled = enabled.contains(name.as_str());
            let matches = active_url.is_some_and(|url| {
                applies(url, name.host())
                    || (name.kind() == BoostKind::Script
                        && flecha_boosts::user_script_matches(url, name.host()))
            });
            let selected = editor.selected.as_ref() == Some(name);
            let select = button(format!("select-boost-{name}"), name.to_string())
                .when(selected, |button| button.bg(palette().accent))
                .on_click(cx.listener({
                    let name = name.clone();
                    move |this, _: &ClickEvent, _, cx| this.select_boost(name.clone(), cx)
                }));
            let toggle = button(
                format!("toggle-boost-{name}"),
                if enabled { "Disable" } else { "Enable" },
            )
            .on_click(cx.listener({
                let name = name.clone();
                move |this, _: &ClickEvent, _, cx| {
                    let result = this
                        .state
                        .borrow_mut()
                        .edit_boosts(|boosts| boosts.set_enabled(&name, !enabled))
                        .map(|()| {
                            if enabled {
                                "Boost disabled.".into()
                            } else {
                                "Boost enabled.".into()
                            }
                        });
                    if let Some(panel) = this.settings_mut() {
                        panel.result(result);
                    }
                    cx.refresh_windows();
                }
            }));
            list = list.child(
                row()
                    .justify_between()
                    .child(
                        row()
                            .child(select)
                            .child(muted(if enabled { "Enabled" } else { "Disabled" }))
                            .when(matches, |row| row.child(muted("Matches active tab"))),
                    )
                    .child(toggle),
            );
        }
        content = content.child(list);

        if let Some(name) = &editor.selected {
            let current = state.boosts.all().get(name.as_str());
            let mut edit = section("Source").child(row().child(div().child(name.to_string())));
            if let Some(build_error) = current.as_ref().and_then(|file| file.error.as_ref()) {
                edit = edit.child(
                    div()
                        .rounded_md()
                        .p_2()
                        .bg(palette().destructive)
                        .child(div().child("Compile error"))
                        .child(div().whitespace_normal().child(build_error.clone())),
                );
            }
            if current.is_none() {
                edit = edit.child(muted("This Boost was deleted in another window."));
            }
            edit = edit.child(
                div()
                    .id("boost-source")
                    .max_h(px(280.))
                    .overflow_scroll()
                    .cursor(CursorStyle::IBeam)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(panel) = this.settings_mut() {
                            panel.field = Some(Draft {
                                field: Field::BoostSource,
                                text: TextField::default(),
                            });
                            cx.notify();
                        }
                    }))
                    .child(editor.source.render("Enter source code")),
            );
            let mut actions = row();
            if editor.compiling.is_some() {
                actions = actions.child(muted("Compiling…"));
            } else if current.is_some() {
                actions = actions.child(
                    button(
                        "boost-save",
                        if name.kind() == BoostKind::Styles {
                            "Save CSS"
                        } else {
                            "Compile and save"
                        },
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_boost(cx))),
                );
            }
            if editor.dirty() {
                actions = actions.child(muted("Unsaved changes")).child(
                    button("boost-discard", "Discard changes").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.discard_boost(cx)),
                    ),
                );
            }
            if current.is_some() && editor.compiling.is_none() {
                let label = if editor.confirm_delete {
                    "Confirm delete"
                } else {
                    "Delete…"
                };
                actions = actions
                    .child(button("boost-delete", label).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.delete_boost(cx)),
                    ));
                if editor.confirm_delete {
                    actions = actions.child(button("boost-cancel-delete", "Cancel").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(panel) = this.settings_mut() {
                                panel.boosts.confirm_delete = false;
                                cx.notify();
                            }
                        }),
                    ));
                }
            }
            content = content.child(edit.child(actions));
        }
        content
    }

    fn create_boost(&mut self, kind: BoostKind, cx: &mut Context<Self>) {
        let active = {
            let state = self.state.borrow();
            boost_host_for(state.active_boost_url(&self.id)).unwrap_or_default()
        };
        let Some(panel) = self.settings_mut() else {
            return;
        };
        if panel.boosts.dirty() || panel.boosts.compiling.is_some() {
            panel.result(Err(anyhow::anyhow!(
                "Save or discard the current draft first"
            )));
            cx.notify();
            return;
        }
        let host = match &panel.field {
            Some(Draft {
                field: Field::BoostHost,
                text,
            }) => text.text().to_owned(),
            _ if !panel.boosts.host.is_empty() => panel.boosts.host.clone(),
            _ => active,
        };
        let result = self
            .state
            .borrow_mut()
            .edit_boosts(|boosts| boosts.create(&host, kind));
        match result {
            Ok(name) => {
                if let Some(panel) = self.settings_mut() {
                    panel.boosts.host = host;
                }
                self.select_boost(name, cx);
            }
            Err(error) => {
                if let Some(panel) = self.settings_mut() {
                    panel.result(Err(error));
                }
            }
        }
        cx.refresh_windows();
    }

    fn select_boost(&mut self, name: BoostName, cx: &mut Context<Self>) {
        let file = self.state.borrow().boosts.all().get(name.as_str());
        if let Some(panel) = self.settings_mut() {
            if panel.boosts.dirty() || panel.boosts.compiling.is_some() {
                panel.result(Err(anyhow::anyhow!(
                    "Save or discard the current draft first"
                )));
            } else if let Some(file) = file {
                panel.boosts.select(name, file.code);
                panel.field = Some(Draft {
                    field: Field::BoostSource,
                    text: TextField::default(),
                });
                panel.message = None;
            }
            cx.notify();
        }
    }

    fn discard_boost(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self
            .settings_mut()
            .and_then(|panel| panel.boosts.selected.clone())
        else {
            return;
        };
        let file = self.state.borrow().boosts.all().get(name.as_str());
        if let Some(panel) = self.settings_mut() {
            panel.boosts.compiling = None;
            if let Some(file) = file {
                panel.boosts.select(name, file.code);
            } else {
                panel.boosts.selected = None;
                panel.boosts.baseline.clear();
                panel.boosts.source = TextField::default();
            }
            panel.message = None;
            cx.notify();
        }
    }

    fn save_boost(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        let Some(name) = panel.boosts.selected.clone() else {
            return;
        };
        let baseline = panel.boosts.baseline.clone();
        let source = panel.boosts.source.text().to_owned();
        if name.kind() == BoostKind::Styles {
            self.finish_boost_save(name, baseline, source, None, cx);
            return;
        }
        let work = cx.background_spawn({
            let (name, source) = (name.clone(), source.clone());
            async move { compile_script(&name, &source) }
        });
        panel.boosts.compiling = Some(cx.spawn(async move |shell, cx| {
            let build = work.await;
            let _ = shell.update(cx, |this, cx| {
                this.finish_boost_save(name, baseline, source, Some(build), cx);
            });
        }));
        cx.notify();
    }

    fn finish_boost_save(
        &mut self,
        name: BoostName,
        baseline: String,
        source: String,
        build: Option<CompiledBoost>,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .state
            .borrow_mut()
            .save_boost(&name, &baseline, &source, build);
        if let Some(panel) = self.settings_mut() {
            panel.boosts.compiling = None;
            if result.is_ok() {
                panel.boosts.baseline = source;
            }
            panel.result(result);
        }
        cx.refresh_windows();
    }

    fn delete_boost(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        let Some(name) = panel.boosts.selected.clone() else {
            return;
        };
        if !panel.boosts.confirm_delete {
            panel.boosts.confirm_delete = true;
            cx.notify();
            return;
        }
        let result = self
            .state
            .borrow_mut()
            .edit_boosts(|boosts| boosts.delete(&name))
            .map(|()| "Boost deleted.".into());
        if let Some(panel) = self.settings_mut() {
            if result.is_ok() {
                panel.boosts.selected = None;
                panel.boosts.baseline.clear();
                panel.boosts.source = TextField::default();
                panel.field = None;
            }
            panel.boosts.confirm_delete = false;
            panel.result(result);
        }
        cx.refresh_windows();
    }
}
