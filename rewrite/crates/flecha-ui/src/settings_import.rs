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
    rgb,
};
use uuid::Uuid;

use super::{Draft, Field, SettingsPanel, SettingsTab, muted, row, section};
use crate::view::{Overlay, Shell, button};
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
        if replace {
            for id in self.engine.ids() {
                self.history.forget(id);
            }
            // Exports can reuse both tab and window IDs for unrelated pages.
            self.engine.close_all();
            self.sleep.clear_pages(&mut self.engine);
            self.model.peeks.clear();
            self.model.flechinhas.clear();
            self.model.popups.clear();
            self.front = Some(self.model.core.focused_window().id.clone());
        } else {
            let retired: Vec<_> = self
                .engine
                .ids()
                .filter(|tab| {
                    self.model.core.tab(tab).is_none()
                        && matches!(self.engine.owner(tab), Some(flecha_cef::Owner::Tab(_)))
                        || self.engine.partition(tab) != self.model.partition(tab).as_ref()
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
                            button("import-cancel-restore", "Cancel").on_click(cx.listener(
                                |this, _: &ClickEvent, _, cx| this.prompt_restore(false, cx),
                            )),
                        )
                        .child(button("import-confirm-restore", "Restore").on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.confirm_restore(cx)),
                        )),
                )
            } else if self.state.borrow().store.has_before_import() {
                restore.child(button("import-restore", RESTORE_BEFORE_IMPORT).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| this.prompt_restore(true, cx)),
                ))
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
                browsers.child(
                    row().child(button("detect-browsers", "Detect browsers").on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.detect_browsers(cx)),
                    )),
                );
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
                button(format.label(), format.label())
                    .when(import.format == format, |button| button.bg(rgb(0x27547b)))
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
                button(
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
            button("import-browse", "Browse…")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.browse_import(cx))),
        );
        if import.preview_task.is_some() {
            actions = actions.child(muted("Reading import file…"));
        } else {
            actions = actions.child(
                button("import-preview", "Preview")
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
                    summary.child(row().child(button("import-apply", label).on_click(
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
                state.status.clone_from(message);
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
}
