use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use flecha_core::{BrowserState, WindowId, import::ImportMode, storage};
use flecha_import::{
    BrowserLocations, DetectedBrowser, FlechaExport, ImportOptions, ImportPlan, ImportPreview,
    Platform, detect_browsers, read_arc, read_bookmarks_html_file,
};
use flecha_services::StateStore;
use gpui::{
    AppContext as _, ClickEvent, Context, Div, ParentElement as _, PathPromptOptions,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, prelude::FluentBuilder as _,
};
use uuid::Uuid;

use super::{Draft, Field, SettingsPanel, SettingsTab, muted, row, section, settings_button};
use crate::theme::palette;
use crate::view::{Overlay, Shell};
use crate::{Controller, now};

pub(crate) const RESTORE_BEFORE_IMPORT: &str = "Restore Spaces From Before Import";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Format {
    #[default]
    Bookmarks,
    Arc,
    Flecha,
}

impl Format {
    fn label(self) -> &'static str {
        match self {
            Self::Bookmarks => "Bookmarks HTML",
            Self::Arc => "Arc sidebar",
            Self::Flecha => "Flecha JSON",
        }
    }
}

enum PreparedImport {
    Plan(ImportPlan),
    Export(FlechaExport),
}

impl PreparedImport {
    fn read(format: Format, path: &Path) -> Result<Self> {
        ensure!(!path.as_os_str().is_empty(), "Enter a file path");
        Ok(match format {
            Format::Bookmarks => Self::Plan(read_bookmarks_html_file(path)?),
            Format::Arc => {
                ensure!(
                    path.file_name()
                        .is_some_and(|name| name == "StorableSidebar.json"),
                    "Choose Arc’s StorableSidebar.json file"
                );
                let root = path
                    .parent()
                    .filter(|root| !root.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let (arc, warnings) = read_arc(root)?;
                Self::Plan(ImportPlan {
                    name: "Arc".into(),
                    arc: Some(arc),
                    bookmarks: None,
                    bookmark_count: 0,
                    history: Vec::new(),
                    warnings,
                })
            }
            Format::Flecha => Self::Export(FlechaExport::read(path)?),
        })
    }

    fn apply(
        self,
        core: &BrowserState,
        window: &WindowId,
        store: &StateStore,
    ) -> Result<(BrowserState, String)> {
        let mut next = core.clone();
        let message = match self {
            Self::Plan(plan) => {
                store
                    .write(&storage::set_aside_before_import(core)?)
                    .context("Save the pre-import backup")?;
                let outcome = plan
                    .apply(
                        &mut next,
                        window,
                        now(),
                        ImportOptions {
                            mode: ImportMode::Append,
                            isolate: &[],
                            spaces_key: None,
                        },
                        || Uuid::new_v4().to_string(),
                        |_| Err(()),
                    )
                    .map_err(|error| anyhow::anyhow!("Import failed: {error:?}"))?;
                let summary = if outcome.changed {
                    format!(
                        "Imported {} Spaces, {} tabs and {} archived tabs.",
                        outcome.spaces, outcome.bookmarks, outcome.archived
                    )
                } else {
                    "No data was imported.".into()
                };
                [summary]
                    .into_iter()
                    .chain(outcome.warnings)
                    .chain(outcome.details)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Self::Export(export) => {
                export
                    .apply(&mut next, |backup| store.write(&backup))
                    .map_err(|error| match error {
                        flecha_import::ReplaceError::Encode => {
                            anyhow::anyhow!("Encode the pre-import backup")
                        }
                        flecha_import::ReplaceError::Save(error) => {
                            error.context("Save the pre-import backup")
                        }
                    })?;
                "Flecha state imported. The previous state was backed up.".into()
            }
        };
        Ok((next, message))
    }

    fn preview(&self, core: &BrowserState) -> ImportSummary {
        match self {
            Self::Plan(plan) => plan_summary(plan.preview(core, false)),
            Self::Export(export) => export_summary(export),
        }
    }
}

struct ImportSummary {
    description: String,
    notes: Vec<String>,
    apply_label: Option<&'static str>,
}

#[derive(Default)]
pub(super) struct ImportPanel {
    format: Format,
    path: PathBuf,
    detected: Option<Vec<DetectedBrowser>>,
    detect_task: Option<Task<()>>,
    picker_task: Option<Task<()>>,
    preview: Option<(PreparedImport, ImportSummary)>,
    preview_task: Option<Task<()>>,
    pub(super) confirm_restore: bool,
}

impl ImportPanel {
    pub(super) fn clear_preview(&mut self) {
        self.preview = None;
        self.preview_task = None;
    }
}

impl Controller {
    fn apply_import(&mut self, import: PreparedImport, window: &WindowId) -> Result<String> {
        self.check_import_storage()?;
        if let PreparedImport::Export(export) = &import {
            self.check_incognito_id_collisions(export.state())?;
        }
        let replace = matches!(import, PreparedImport::Export(_));
        let (next, message) = import.apply(&self.model.core, window, &self.store)?;
        self.save_import_state(next, replace, message)
    }

    fn restore_before_import(&mut self) -> Result<String> {
        self.check_import_storage()?;
        let next = self
            .store
            .read_before_import()
            .context("Could not restore Spaces from before import")?
            .context("There are no Spaces from before an import")?;
        let message = format!(
            "Restored {} Spaces and {} tabs.",
            next.spaces().len(),
            next.open_tabs().len(),
        );
        self.save_import_state(next, true, message)
    }

    fn check_import_storage(&self) -> Result<()> {
        ensure!(
            self.session_dir
                .symlink_metadata()
                .context("Inspect the Flecha session directory")?
                .is_dir(),
            "Flecha session storage must be a local profile directory"
        );
        Ok(())
    }

    fn save_import_state(
        &mut self,
        next: BrowserState,
        replace: bool,
        mut message: String,
    ) -> Result<String> {
        self.check_incognito_id_collisions(&next)?;
        let write = self.snapshots().save_current(&next)?;
        let synced = self
            .store
            .replace_snapshot(&write)
            .context("Save session state")?;
        let previous = std::mem::replace(&mut self.model.core, next);
        if let Err(error) = synced {
            message.push_str(&format!(
                " Could not sync the session directory to disk: {error}."
            ));
        }
        self.sync_engine_settings(&previous.settings);
        // Imports replace the regular profile's state. Incognito pages stay as they are.
        if replace {
            // Exports can reuse both tab and window IDs for unrelated pages.
            for id in self.close_regular_pages() {
                self.history.forget(&id);
                self.engine.close(&id);
            }
            self.sleep.clear_pages(&mut self.engine);
            if !self.model.regular_closed() {
                self.front = Some(self.model.core.focused_window().id.clone());
            }
        } else {
            let retired: Vec<_> = self
                .engine
                .ids()
                .filter(|tab| {
                    self.is_regular_page(tab)
                        && (self.model.core.tab(tab).is_none()
                            && matches!(self.engine.owner(tab), Some(flecha_cef::Owner::Tab(_)))
                            || self.engine.partition(tab) != self.model.partition(tab).as_ref())
                })
                .cloned()
                .collect();
            for id in retired {
                self.history.forget(&id);
                self.engine.close(&id);
            }
        }
        self.update_visibility();
        Ok(message)
    }

    fn check_incognito_id_collisions(&self, next: &BrowserState) -> Result<()> {
        let private_window_collision = self
            .model
            .incognito
            .as_ref()
            .into_iter()
            .flat_map(|incognito| {
                incognito
                    .state
                    .windows()
                    .iter()
                    .map(|window| &window.id)
                    .chain(
                        incognito
                            .state
                            .closed_windows()
                            .iter()
                            .map(|closed| &closed.window.id),
                    )
            })
            .chain(
                self.model
                    .flechinhas
                    .iter()
                    .filter(|(_, page)| page.partition.is_incognito())
                    .map(|(window, _)| window),
            )
            .chain(
                self.model
                    .popups
                    .iter()
                    .filter(|(_, popup)| popup.page.partition.is_incognito())
                    .map(|(window, _)| window),
            )
            .any(|id| {
                next.window(id).is_some()
                    || next
                        .closed_windows()
                        .iter()
                        .any(|closed| &closed.window.id == id)
            });
        ensure!(
            !private_window_collision,
            "Close incognito windows before importing a state that reuses an open incognito window ID"
        );
        let private_tab_collision = self
            .model
            .incognito
            .as_ref()
            .into_iter()
            .flat_map(|incognito| {
                incognito.state.open_tabs().into_keys().chain(
                    incognito.state.closed_windows().iter().flat_map(|closed| {
                        closed
                            .window
                            .spaces
                            .values()
                            .flat_map(|space| space.today.iter())
                    }),
                )
            })
            .chain(
                self.model
                    .peeks
                    .values()
                    .filter(|page| page.partition.is_incognito())
                    .map(|page| &page.id),
            )
            .chain(
                self.model
                    .flechinhas
                    .values()
                    .filter(|page| page.partition.is_incognito())
                    .map(|page| &page.id),
            )
            .chain(
                self.model
                    .popups
                    .values()
                    .filter(|popup| popup.page.partition.is_incognito())
                    .map(|popup| &popup.page.id),
            )
            .chain(self.engine.ids().filter(|id| !self.is_regular_page(id)))
            .any(|id| {
                next.tab(id).is_some()
                    || next.closed_windows().iter().any(|closed| {
                        closed
                            .window
                            .spaces
                            .values()
                            .any(|space| space.today.contains(id))
                    })
            });
        ensure!(
            !private_tab_collision,
            "Close incognito windows before importing a state that reuses an open incognito tab ID"
        );
        Ok(())
    }
}

impl Shell {
    pub(crate) fn open_restore_before_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut panel = SettingsPanel::new(SettingsTab::Import);
        panel.import.confirm_restore = true;
        self.show_overlay(Overlay::Settings(Box::new(panel)), window, cx);
    }

    pub(super) fn import_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let import = &panel.import;
        let restore = section("Pre-import backup");
        let restore =
            if import.confirm_restore {
                restore
                .child(
                    "Replace your current Spaces and tabs with those from before your last import?",
                )
                .child(
                    row()
                        .child(
                            settings_button("import-cancel-restore", "Cancel").on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| this.prompt_restore(false, cx),
                            )),
                        )
                        .child(settings_button("import-confirm-restore", "Restore").on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_restore(cx)),
                        )),
                )
            } else if self.state.borrow().store.has_before_import() {
                restore.child(
                    settings_button("import-restore", RESTORE_BEFORE_IMPORT).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.prompt_restore(true, cx)),
                    ),
                )
            } else {
                restore.child(muted("A backup will appear here after your first import."))
            };
        if import.confirm_restore {
            return restore;
        }
        let mut browsers = section("Installed browsers");
        if import.detect_task.is_some() {
            browsers = browsers.child(muted("Detecting browsers…"));
        } else {
            browsers =
                browsers.child(row().child(
                    settings_button("detect-browsers", "Detect browsers").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.detect_browsers(cx)),
                    ),
                ));
        }
        if let Some(detected) = &import.detected {
            if detected.is_empty() {
                browsers = browsers.child(muted("No supported browser profiles found."));
            }
            for browser in detected {
                let mut data = Vec::new();
                if browser.has_bookmarks {
                    data.push("bookmarks");
                }
                if browser.has_history {
                    data.push("history");
                }
                browsers = browsers.child(div().child(format!(
                    "{} · {} profiles{}",
                    browser.name,
                    browser.profiles,
                    if data.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", data.join(", "))
                    },
                )));
            }
        }
        let formats = [Format::Bookmarks, Format::Arc, Format::Flecha]
            .into_iter()
            .map(|format| {
                settings_button(format.label(), format.label())
                    .when(import.format == format, |button| {
                        button
                            .bg(palette().selection)
                            .text_color(palette().selection_text)
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(panel) = this.settings_mut() {
                            panel.import.format = format;
                            panel.import.clear_preview();
                            panel.message = None;
                            cx.notify();
                        }
                    }))
            });
        let mut source = section("Import from file").child(row().children(formats));
        if import.format == Format::Arc {
            source = source.child(muted(
                "Choose StorableSidebar.json from the Arc profile folder.",
            ));
        }
        if let Some(Draft {
            field: Field::ImportPath,
            text,
        }) = &panel.field
        {
            source = source.child(text.render("Path to the import file"));
        } else {
            source = source.child(
                settings_button(
                    "import-type-path",
                    if import.path.as_os_str().is_empty() {
                        "Enter a file path".into()
                    } else {
                        import.path.to_string_lossy().into_owned()
                    },
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(panel) = this.settings_mut() {
                        panel.begin(
                            Field::ImportPath,
                            panel.import.path.to_string_lossy().into_owned(),
                        );
                        cx.notify();
                    }
                })),
            );
        }
        let mut actions = row().child(
            settings_button("import-browse", "Browse…")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.browse_import(cx))),
        );
        if import.preview_task.is_some() {
            actions = actions.child(muted("Reading import file…"));
        } else {
            actions = actions.child(
                settings_button("import-preview", "Preview")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.preview_import(cx))),
            );
        }
        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(restore)
            .child(browsers)
            .child(source.child(actions));
        if let Some((_, preview)) = &import.preview {
            let summary = section("Preview")
                .child(div().child(preview.description.clone()))
                .children(preview.notes.iter().cloned().map(muted))
                .when_some(preview.apply_label, |summary, label| {
                    summary.child(row().child(settings_button("import-apply", label).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_import(cx)),
                    )))
                });
            content = content.child(summary);
        }
        content
    }

    fn prompt_restore(&mut self, confirm: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.settings_mut() {
            panel.import.confirm_restore = confirm;
            panel.field = None;
            panel.message = None;
            cx.notify();
        }
    }

    fn confirm_restore(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.settings_mut() {
            panel.import.confirm_restore = false;
            panel.import.clear_preview();
        }
        let result = self.state.borrow_mut().restore_before_import();
        self.finish_import(result, cx);
    }

    fn detect_browsers(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        let task = cx.background_spawn(async {
            browser_locations().map(|locations| detect_browsers(&locations))
        });
        panel.import.detect_task = Some(cx.spawn(async move |shell, cx| {
            let result = task.await;
            let _ = shell.update(cx, |this, cx| {
                if let Some(panel) = this.settings_mut() {
                    panel.import.detect_task = None;
                    match result {
                        Ok(browsers) => panel.import.detected = Some(browsers),
                        Err(error) => panel.result(Err(error)),
                    }
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn browse_import(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        if panel.import.picker_task.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose an import file".into()),
        });
        panel.import.picker_task = Some(cx.spawn(async move |shell, cx| {
            let result = paths
                .await
                .context("File picker was closed")
                .and_then(|result| result);
            let _ = shell.update(cx, |this, cx| {
                let Some(panel) = this.settings_mut() else {
                    return;
                };
                panel.import.picker_task = None;
                if panel.tab != SettingsTab::Import {
                    return;
                }
                match result {
                    Ok(Some(paths)) => {
                        if let Some(path) = paths.into_iter().next() {
                            panel.import.path = path;
                            panel.field = None;
                            this.preview_import(cx);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        panel.result(Err(error));
                        cx.notify();
                    }
                }
            });
        }));
    }

    pub(super) fn preview_import(&mut self, cx: &mut Context<Self>) {
        let core = self.state.borrow().model.core.clone();
        let Some(panel) = self.settings_mut() else {
            return;
        };
        if let Some(Draft {
            field: Field::ImportPath,
            text,
        }) = &panel.field
        {
            panel.import.path = PathBuf::from(text.text().trim());
        }
        panel.import.clear_preview();
        panel.message = None;
        let (format, path) = (panel.import.format, panel.import.path.clone());
        let work = cx.background_spawn(async move {
            let import = PreparedImport::read(format, &path)?;
            let summary = import.preview(&core);
            Ok((import, summary))
        });
        panel.import.preview_task = Some(cx.spawn(async move |shell, cx| {
            let result = work.await;
            let _ = shell.update(cx, |this, cx| {
                if let Some(panel) = this.settings_mut() {
                    panel.import.preview_task = None;
                    match result {
                        Ok(preview) => panel.import.preview = Some(preview),
                        Err(error) => panel.result(Err(error)),
                    }
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn confirm_import(&mut self, cx: &mut Context<Self>) {
        let Some((preview, summary)) = self
            .settings_mut()
            .and_then(|panel| panel.import.preview.take())
        else {
            return;
        };
        if summary.apply_label.is_none() {
            return;
        }
        let result = self.state.borrow_mut().apply_import(preview, &self.id);
        self.finish_import(result, cx);
    }

    fn finish_import(&mut self, result: Result<String>, cx: &mut Context<Self>) {
        {
            let mut state = self.state.borrow_mut();
            if state.model.core.window(&self.id).is_none()
                && let Ok(message) = &result
            {
                state.status.set(None, message.clone());
            }
        }
        if let Some(panel) = self.settings_mut() {
            panel.result(result);
        }
        cx.refresh_windows();
    }
}

fn browser_locations() -> Result<BrowserLocations> {
    let platform = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    };
    let path = |name| {
        env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let home = path(if matches!(platform, Platform::Windows) {
        "USERPROFILE"
    } else {
        "HOME"
    })
    .context("Home directory is unavailable")?;
    let mut locations = BrowserLocations::new(platform, home);
    locations.xdg_config_home = path("XDG_CONFIG_HOME");
    locations.chrome_config_home = path("CHROME_CONFIG_HOME");
    locations.helium_config_home = path("HELIUM_CONFIG_HOME");
    locations.local_app_data = path("LOCALAPPDATA");
    locations.roaming_app_data = path("APPDATA");
    Ok(locations)
}

fn plan_summary(preview: ImportPreview) -> ImportSummary {
    let (mut pinned, mut today, mut splits) = (0, 0, 0);
    for space in &preview.spaces {
        pinned += space.pinned;
        today += space.today;
        splits += space.splits;
    }
    ImportSummary {
        description: format!(
            "{} · {} Spaces · {pinned} pinned · {today} Today · {splits} splits · {} favorites · {} archived",
            preview.name,
            preview.spaces.len(),
            preview.favorites,
            preview.archived,
        ),
        notes: preview
            .warnings
            .into_iter()
            .chain(preview.skipped)
            .collect(),
        apply_label: (!preview.spaces.is_empty() || preview.favorites > 0 || preview.archived > 0)
            .then_some("Import"),
    }
}

fn export_summary(export: &FlechaExport) -> ImportSummary {
    let state = export.state();
    ImportSummary {
        description: format!(
            "{} windows · {} Spaces · {} tabs · {} favorites · {} archived tabs",
            state.windows().len(),
            state.spaces().len(),
            state.open_tabs().len(),
            state.favorites().len(),
            state.archive().len(),
        ),
        notes: vec!["Importing this export replaces your current Spaces, tabs and settings after saving a backup.".into()],
        apply_label: Some("Back up and replace state"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use flecha_core::SearchEngine;

    use super::*;
    use crate::Controller;

    fn backup(profile: &Path) -> Result<BrowserState> {
        let bytes = fs::read(profile.join("session/before-import.json"))?;
        let document = serde_json::from_slice(&bytes)?;
        Ok(storage::state_before_import(Some(&document))?.unwrap())
    }

    #[test]
    fn bookmarks_preview_and_apply_persist_the_import_and_its_backup() -> Result<()> {
        let source = tempfile::tempdir()?;
        let path = source.path().join("bookmarks.html");
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><p>
            <DT><A HREF="https://example.org/">Example</A>
            </DL><p>"#;
        fs::write(&path, html)?;
        let profile = tempfile::tempdir()?;
        let mut state = Controller::open_test_profile(profile.path())?;
        let original = state.model.core.clone();
        let window = original.focused_window().id.clone();
        let import = PreparedImport::read(Format::Bookmarks, &path)?;
        let PreparedImport::Plan(plan) = &import else {
            unreachable!()
        };
        let preview = plan.preview(&original, false);
        assert_eq!(preview.spaces.len(), 1);
        assert_eq!(preview.spaces[0].pinned, 1);
        assert!(!profile.path().join("session/before-import.json").exists());

        state.apply_import(import, &window)?;
        assert_eq!(backup(profile.path())?, original);
        let reopened = Controller::open_test_profile(profile.path())?;
        assert_eq!(reopened.model.core, state.model.core);
        assert!(
            reopened
                .model
                .core
                .open_tabs()
                .values()
                .any(|tab| tab.url == "https://example.org/")
        );
        assert_eq!(fs::read_to_string(path)?, html);
        Ok(())
    }

    #[test]
    fn flecha_export_preview_precedes_a_backed_up_whole_state_replacement() -> Result<()> {
        let source = tempfile::tempdir()?;
        let path = source.path().join("flecha.json");
        let source_state = Controller::open_test_profile(source.path())?;
        let mut replacement = source_state.model.core;
        replacement.settings.search_engine = SearchEngine::Brave;
        let file = storage::encode(&replacement)?;
        fs::write(&path, &file)?;
        let profile = tempfile::tempdir()?;
        let mut state = Controller::open_test_profile(profile.path())?;
        let original = state.model.core.clone();
        let window = original.focused_window().id.clone();
        let import = PreparedImport::read(Format::Flecha, &path)?;
        let PreparedImport::Export(export) = &import else {
            unreachable!()
        };
        assert_eq!(export.state().spaces().len(), 1);
        assert_eq!(state.model.core, original);

        state.apply_import(import, &window)?;
        assert_eq!(backup(profile.path())?, original);
        assert_eq!(state.model.core, replacement);
        assert_eq!(
            Controller::open_test_profile(profile.path())?.model.core,
            replacement
        );
        assert_eq!(fs::read_to_string(path)?, file);
        Ok(())
    }

    #[test]
    fn restore_after_import_recovers_the_saved_state_and_preserves_the_backup() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut state = Controller::open_test_profile(profile.path())?;
        let window = state.model.core.focused_window().id.clone();
        state.act(
            &window,
            crate::Action::Open("https://example.test/original".into()),
        )?;
        let original = state.model.core.clone();
        let mut imported = original.clone();
        imported.settings.search_engine = SearchEngine::Brave;
        imported.add_space(flecha_core::SpaceId::new("imported"));
        let export = FlechaExport::parse(&storage::encode(&imported)?)?;
        state.apply_import(PreparedImport::Export(export), &window)?;
        assert_eq!(state.model.core, imported);
        let backup_path = profile.path().join("session/before-import.json");
        let saved_backup = fs::read(&backup_path)?;

        state.restore_before_import()?;
        assert_eq!(state.model.core, original);
        assert_eq!(
            Controller::open_test_profile(profile.path())?.model.core,
            original
        );
        assert_eq!(fs::read(backup_path)?, saved_backup);
        Ok(())
    }

    #[test]
    fn imports_and_restores_leave_incognito_windows_and_pages_alone() -> Result<()> {
        use crate::Action;

        let profile = tempfile::tempdir()?;
        let mut state = Controller::open_test_profile(profile.path())?;
        let window = state.model.core.focused_window().id.clone();
        state.act(
            &window,
            Action::Open("https://example.test/original".into()),
        )?;
        state.act(&window, Action::Peek("https://example.test/peek".into()))?;
        state.act(&window, Action::NewIncognitoWindow)?;
        let private = state.model.incognito_windows()[0].id.clone();
        state.act(
            &private,
            Action::Open("https://private.test/detached".into()),
        )?;
        let detached = state.model.active(&private).unwrap().clone();
        state.act(&private, Action::DetachTab(detached))?;
        state.act(&private, Action::Open("https://private.test/draft".into()))?;
        state.act(&private, Action::Peek("https://private.test/peek".into()))?;
        let private_tab = state.model.active(&private).unwrap().clone();
        let private_peek = state.model.peeks[&private].id.clone();
        let flechinha = state.model.flechinhas.keys().next().unwrap().clone();
        let private_intact = |state: &Controller| {
            assert_eq!(state.model.incognito_windows()[0].id, private);
            assert_eq!(state.model.active(&private), Some(&private_tab));
            assert_eq!(
                state.model.peeks.get(&private).map(|page| &page.id),
                Some(&private_peek)
            );
            assert!(state.model.flechinhas.contains_key(&flechinha));
        };

        let mut imported = state.model.core.clone();
        imported.add_space(flecha_core::SpaceId::new("imported"));
        let export = FlechaExport::parse(&storage::encode(&imported)?)?;
        state.apply_import(PreparedImport::Export(export), &window)?;
        private_intact(&state);
        assert!(
            !state.model.peeks.contains_key(&window),
            "the regular Peek outlived the import"
        );

        state.restore_before_import()?;
        private_intact(&state);
        assert_eq!(state.front.as_ref(), Some(&window));

        // From an incognito window with the regular windows closed, they stay closed.
        state.act(&window, Action::CloseWindow)?;
        state.front = None;
        state.restore_before_import()?;
        private_intact(&state);
        assert!(state.model.regular_windows().is_empty());
        assert_eq!(
            state.front, None,
            "restore brought a closed regular window to the front"
        );
        Ok(())
    }

    #[test]
    fn restore_refuses_missing_unreadable_and_newer_backups_without_replacing_current_state()
    -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut state = Controller::open_test_profile(profile.path())?;
        state.save()?;
        let session = profile.path().join("session");
        let original = state.model.core.clone();
        let current = fs::read(session.join("state.json"))?;
        assert!(!state.store.has_before_import());
        assert!(
            format!("{:#}", state.restore_before_import().unwrap_err())
                .contains("no Spaces from before an import")
        );

        let backup = session.join("before-import.json");
        let newer = serde_json::json!(format!(r#"{{"version":{}}}"#, storage::SCHEMA_VERSION + 1))
            .to_string();
        for (invalid, expected) in [
            ("broken", "Read the pre-import backup"),
            (r#""broken""#, "backup is unreadable"),
            (&newer, "unsupported state schema"),
        ] {
            fs::write(&backup, invalid)?;
            assert!(state.store.has_before_import());
            let error = state.restore_before_import().unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(state.model.core, original);
            assert_eq!(fs::read(session.join("state.json"))?, current);
            assert_eq!(fs::read_to_string(&backup)?, invalid);
        }
        Ok(())
    }

    #[test]
    fn arc_file_is_previewed_from_its_explicit_path() -> Result<()> {
        let source = tempfile::tempdir()?;
        let path = source.path().join("StorableSidebar.json");
        fs::write(
            &path,
            r#"{"sidebar":{"containers":[{"spaces":[{"title":"Arc Work"}],"items":[]}]}}"#,
        )?;
        let profile = tempfile::tempdir()?;
        let state = Controller::open_test_profile(profile.path())?;
        let PreparedImport::Plan(plan) = PreparedImport::read(Format::Arc, &path)? else {
            unreachable!();
        };
        let preview = plan.preview(&state.model.core, false);
        assert_eq!(preview.spaces[0].name, "Arc Work");
        Ok(())
    }

    #[test]
    fn invalid_paths_and_failed_backups_leave_current_state_unchanged() -> Result<()> {
        let profile = tempfile::tempdir()?;
        for (format, name) in [
            (Format::Bookmarks, "missing.html"),
            (Format::Arc, "StorableSidebar.json"),
            (Format::Flecha, "missing.json"),
        ] {
            assert!(PreparedImport::read(format, &profile.path().join(name)).is_err());
        }
        let mut state = Controller::open_test_profile(profile.path())?;
        state.save()?;
        let original = state.model.core.clone();
        let window = original.focused_window().id.clone();
        fs::create_dir(profile.path().join("session/before-import.json"))?;
        let plan = flecha_import::read_bookmarks_html(
            r#"<DL><DT><A HREF="https://example.org/">Example</A></DL>"#,
        );
        let export = FlechaExport::parse(&storage::encode(&original)?)?;
        for import in [PreparedImport::Plan(plan), PreparedImport::Export(export)] {
            let error = state.apply_import(import, &window).unwrap_err();
            assert!(format!("{error:#}").contains("pre-import backup"));
            assert_eq!(state.model.core, original);
        }
        assert_eq!(
            Controller::open_test_profile(profile.path())?.model.core,
            original
        );
        Ok(())
    }

    #[test]
    fn import_rejects_ids_colliding_with_live_incognito_windows_or_tabs() -> Result<()> {
        use crate::Action;

        let profile = tempfile::tempdir()?;
        let mut state = Controller::open_test_profile(profile.path())?;
        let regular_window = state.model.core.focused_window().id.clone();
        state.act(
            &regular_window,
            Action::Open("https://regular.example/".into()),
        )?;
        let original = state.model.core.clone();
        let disk_before = fs::read(profile.path().join("session/state.json"))?;
        let backup_path = profile.path().join("session/before-import.json");
        fs::write(&backup_path, b"sentinel-backup")?;

        state.act(&regular_window, Action::NewIncognitoWindow)?;
        let incognito_window = state
            .model
            .incognito
            .as_ref()
            .unwrap()
            .state
            .focused_window()
            .id
            .clone();
        state.act(
            &incognito_window,
            Action::Open("https://private.example/".into()),
        )?;
        let incognito_tab = state.model.active(&incognito_window).unwrap().clone();

        // A replacement reusing a live incognito window ID must be rejected before saving or
        // overwriting the pre-import backup.
        let mut window_collision =
            BrowserState::new(flecha_core::SpaceId::new("space"), incognito_window.clone());
        window_collision.open_today(
            flecha_core::TabId::new("imported-tab"),
            flecha_core::Tab::new("https://imported.example/", 0),
            &incognito_window,
            None,
            true,
        );
        let export = FlechaExport::parse(&storage::encode(&window_collision)?)?;
        let err = state
            .apply_import(PreparedImport::Export(export), &regular_window)
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("incognito window ID"),
            "{err:#}"
        );
        assert_eq!(state.model.core, original);
        assert_eq!(
            fs::read(profile.path().join("session/state.json"))?,
            disk_before
        );
        assert_eq!(fs::read(&backup_path)?, b"sentinel-backup");

        // A replacement whose closed_windows history reuses a live incognito window ID must
        // also be rejected so reopen_last cannot collide with the live private window.
        let mut closed_window_collision = BrowserState::new(
            flecha_core::SpaceId::new("space"),
            flecha_core::WindowId::new("imported-open-window"),
        );
        closed_window_collision.open_window(incognito_window.clone(), None);
        closed_window_collision.open_today(
            flecha_core::TabId::new("closed-window-tab"),
            flecha_core::Tab::new("https://closed.example/", 0),
            &incognito_window,
            None,
            true,
        );
        let _ = closed_window_collision.close_window(&incognito_window, 1);
        let export = FlechaExport::parse(&storage::encode(&closed_window_collision)?)?;
        let err = state
            .apply_import(PreparedImport::Export(export), &regular_window)
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("incognito window ID"),
            "{err:#}"
        );
        assert_eq!(state.model.core, original);
        assert_eq!(fs::read(&backup_path)?, b"sentinel-backup");

        // A replacement reusing a live incognito tab ID must also be rejected before saving.
        let mut tab_collision = BrowserState::new(
            flecha_core::SpaceId::new("space"),
            flecha_core::WindowId::new("imported-window"),
        );
        tab_collision.open_today(
            incognito_tab,
            flecha_core::Tab::new("https://imported.example/", 0),
            &flecha_core::WindowId::new("imported-window"),
            None,
            true,
        );
        let export = FlechaExport::parse(&storage::encode(&tab_collision)?)?;
        let err = state
            .apply_import(PreparedImport::Export(export), &regular_window)
            .unwrap_err();
        assert!(format!("{err:#}").contains("incognito tab ID"), "{err:#}");
        assert_eq!(state.model.core, original);
        assert_eq!(
            fs::read(profile.path().join("session/state.json"))?,
            disk_before
        );
        Ok(())
    }
}
