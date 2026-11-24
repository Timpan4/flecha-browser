use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{
    DownloadConfig, DownloadEvent, DownloadHandle, DownloadMessage, IncognitoSession,
};
use flecha_core::{EpochMillis, Settings, TabId, WindowId};
use flecha_downloads::{
    AskReason, DownloadId, DownloadList, DownloadRow, DownloadSettings, RowState, State, Warning,
};
use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, Keystroke, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, rgb,
};

use crate::text_field::TextField;
use crate::view::{Overlay, Shell, button, overlay_panel};
use crate::{Action, Controller, dispatch};

#[derive(Debug, PartialEq, Eq)]
enum Prompt {
    Path {
        suggested: PathBuf,
        reason: AskReason,
    },
    Held {
        id: DownloadId,
        warning: Warning,
    },
}

#[derive(Clone)]
pub(crate) enum DownloadCommand {
    Accept(Option<PathBuf>),
    Keep,
    Cancel,
    Pause,
    Resume,
}

pub(crate) struct Downloads {
    profile: PathBuf,
    isolated: bool,
    prompts: BTreeMap<DownloadHandle, Prompt>,
    active: BTreeMap<DownloadHandle, Option<TabId>>,
    /// The incognito session of each download in `active`, or `None` for a regular one. Only
    /// windows of that session show it.
    sessions: BTreeMap<DownloadHandle, Option<IncognitoSession>>,
    config_error: Option<String>,
}

impl Downloads {
    pub(crate) fn new(profile: &Path, isolated: bool) -> Self {
        Self {
            profile: profile.to_owned(),
            isolated,
            prompts: BTreeMap::new(),
            active: BTreeMap::new(),
            sessions: BTreeMap::new(),
            config_error: None,
        }
    }

    fn config(&self, settings: &Settings) -> Result<DownloadConfig> {
        let xdg = std::env::var_os("XDG_DOWNLOAD_DIR").map(PathBuf::from);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        download_config(
            &self.profile,
            self.isolated,
            settings,
            xdg.as_deref(),
            home.as_deref(),
        )
    }

    fn track(&mut self, message: DownloadMessage, list: &DownloadList) {
        let handle = message.event.handle();
        let incognito = message.incognito;
        match message.event {
            DownloadEvent::AskPath {
                handle,
                suggested,
                reason,
                ..
            } => {
                self.prompts
                    .insert(handle, Prompt::Path { suggested, reason });
                self.active.insert(handle, message.tab);
            }
            DownloadEvent::Held {
                handle,
                id,
                warning,
            } => {
                self.prompts.insert(handle, Prompt::Held { id, warning });
                self.active.insert(handle, message.tab);
            }
            DownloadEvent::Started { handle, id, .. } => {
                self.prompts.remove(&handle);
                self.track_row(handle, id, message.tab, list);
            }
            DownloadEvent::Updated { handle, id } => {
                if !self.prompts.contains_key(&handle) {
                    self.track_row(handle, id, message.tab, list);
                }
            }
            DownloadEvent::Blocked { handle, .. } | DownloadEvent::Cancelled { handle, .. } => {
                self.prompts.remove(&handle);
                self.active.remove(&handle);
            }
        }
        if self.active.contains_key(&handle) {
            self.sessions.insert(handle, incognito);
        } else {
            self.sessions.remove(&handle);
        }
    }

    fn track_row(
        &mut self,
        handle: DownloadHandle,
        id: DownloadId,
        tab: Option<TabId>,
        list: &DownloadList,
    ) {
        if list
            .get(id)
            .is_some_and(|row| matches!(row.state(), State::InProgress | State::Paused))
        {
            self.active.insert(handle, tab);
        } else {
            self.active.remove(&handle);
        }
    }

    fn in_session(&self, handle: &DownloadHandle, session: Option<IncognitoSession>) -> bool {
        self.sessions.get(handle).copied().flatten() == session
    }

    /// The downloads asking a question or running in windows of `session`.
    pub(crate) fn count(&self, session: Option<IncognitoSession>) -> usize {
        self.active
            .keys()
            .filter(|handle| self.in_session(handle, session))
            .count()
    }

    fn prompts_in(
        &self,
        session: Option<IncognitoSession>,
    ) -> impl DoubleEndedIterator<Item = (&DownloadHandle, &Prompt)> {
        self.prompts
            .iter()
            .filter(move |(handle, _)| self.in_session(handle, session))
    }

    pub(crate) fn uses_tab(&self, tab: &TabId) -> bool {
        self.active
            .values()
            .any(|owner| owner.as_ref() == Some(tab))
    }
}

fn download_config(
    profile: &Path,
    isolated: bool,
    settings: &Settings,
    xdg: Option<&Path>,
    home: Option<&Path>,
) -> Result<DownloadConfig> {
    let fallback = std::path::absolute(profile)?.join("downloads");
    let mut download_settings = if isolated {
        DownloadSettings {
            folder: fallback,
            ask_where_to_save: settings.ask_where_to_save.unwrap_or(false),
        }
    } else {
        let default = xdg
            .filter(|dir| dir.is_absolute() && dir.to_str().is_some())
            .map(Path::to_owned)
            .or_else(|| {
                home.filter(|dir| dir.is_absolute() && dir.to_str().is_some())
                    .map(|dir| dir.join("Downloads"))
            })
            .unwrap_or(fallback);
        DownloadSettings::from_settings(settings, &default)
    };
    let folder = &mut download_settings.folder;
    if isolated {
        let root = profile.canonicalize()?;
        *folder = root.join("downloads");
        match fs::create_dir(&*folder) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        ensure!(
            fs::symlink_metadata(&*folder)?.is_dir(),
            "Isolated downloads folder must be a directory inside the profile"
        );
        for entry in fs::read_dir(&*folder)? {
            ensure!(
                !entry?.file_type()?.is_symlink(),
                "Isolated downloads folder contains a symbolic link"
            );
        }
    } else {
        fs::create_dir_all(&*folder).context("Create downloads folder")?;
    }
    ensure!(
        folder.to_str().is_some(),
        "Downloads folder must be a UTF-8 path"
    );
    Ok(DownloadConfig {
        settings: download_settings,
        https_first_mode: false,
    })
}

fn destination(path: &Path, confined_to: Option<&Path>) -> Result<PathBuf> {
    let name = path
        .file_name()
        .filter(|_| path.is_absolute() && path.to_str().is_some())
        .context("Enter an absolute file path")?;
    let parent = path
        .parent()
        .context("Enter a file path")?
        .canonicalize()
        .context("Open the destination folder")?;
    ensure!(parent.is_dir(), "Destination must be in a folder");
    if let Some(profile) = confined_to {
        ensure!(
            parent.starts_with(profile.canonicalize()?),
            "Save inside the isolated profile"
        );
    }
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(path),
        Err(error) => Err(error.into()),
        Ok(_) => anyhow::bail!("A file already exists at this destination. Choose another path"),
    }
}

impl Controller {
    pub(crate) fn sync_download_config(&mut self) {
        match self.downloads.config(&self.model.core.settings) {
            Ok(config) => {
                self.downloads.config_error = None;
                self.engine.set_download_config(Some(config));
            }
            Err(error) => {
                self.downloads.config_error = Some(format!("Downloads are unavailable. {error:#}"));
                self.engine.set_download_config(None);
            }
        }
    }

    /// The rows a window's downloads panel lists. Each incognito session lists only its own
    /// downloads, and regular windows none of them, as each Chromium profile keeps its own.
    pub(crate) fn download_rows(&self, window: &WindowId) -> Vec<DownloadRow> {
        let session = self.model.incognito_session(window);
        self.engine
            .downloads()
            .rows(EpochMillis::MIN)
            .into_iter()
            .filter(|row| self.engine.download_session(row.id) == session)
            .collect()
    }

    pub(crate) fn drain_downloads(&mut self) -> bool {
        let Some(first) = self.engine.next_download_event() else {
            return false;
        };
        let list = self.engine.downloads();
        let mut current = Some(first);
        while let Some(message) = current {
            if matches!(message.event, DownloadEvent::Blocked { .. }) {
                self.status = self
                    .downloads
                    .config_error
                    .clone()
                    .unwrap_or_else(|| "Download blocked.".into());
            }
            self.downloads.track(message, &list);
            current = self.engine.next_download_event();
        }
        true
    }

    pub(crate) fn manage_download(
        &mut self,
        handle: DownloadHandle,
        command: DownloadCommand,
    ) -> Result<()> {
        let confined_to = self
            .downloads
            .isolated
            .then_some(self.downloads.profile.as_path());
        match command {
            DownloadCommand::Accept(path) => {
                let Some(Prompt::Path { suggested, .. }) = self.downloads.prompts.get(&handle)
                else {
                    anyhow::bail!("This download is no longer asking for a path");
                };
                let path = destination(path.as_deref().unwrap_or(suggested), confined_to)?;
                self.engine.accept_download(handle, Some(path))?;
            }
            DownloadCommand::Keep => {
                let Some(Prompt::Held { id, .. }) = self.downloads.prompts.get(&handle) else {
                    anyhow::bail!("This download is no longer awaiting a decision");
                };
                let list = self.engine.downloads();
                let row = list.get(*id).context("Download is no longer available")?;
                destination(row.path(), confined_to)?;
                self.engine.accept_download(handle, None)?;
            }
            DownloadCommand::Cancel => self.engine.cancel_download(handle)?,
            DownloadCommand::Pause => self.engine.pause_download(handle)?,
            DownloadCommand::Resume => self.engine.resume_download(handle)?,
        }
        self.drain_downloads();
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct DownloadsPanel {
    draft: Option<(DownloadHandle, TextField)>,
}

impl DownloadsPanel {
    pub(crate) fn refresh(&mut self, downloads: &Downloads) {
        if self.draft.as_ref().is_some_and(|(handle, _)| {
            !matches!(downloads.prompts.get(handle), Some(Prompt::Path { .. }))
        }) {
            self.draft = None;
        }
    }
}

impl Shell {
    pub(crate) fn toggle_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Downloads(_)) {
            self.dismiss_overlay(window, cx);
        } else {
            self.show_overlay(Overlay::Downloads(DownloadsPanel::default()), window, cx);
        }
    }

    pub(crate) fn downloads_key(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) {
        let Overlay::Downloads(panel) = &mut self.overlay else {
            return;
        };
        if let Some((handle, field)) = &mut panel.draft {
            if stroke.key == "enter" {
                let action =
                    Action::Download(*handle, DownloadCommand::Accept(Some(field.text().into())));
                dispatch(&self.state, &self.id, action, cx);
            } else {
                field.key(stroke, cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn downloads_panel(&self, panel: &DownloadsPanel, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let list = state.engine.downloads();
        let session = state.model.incognito_session(&self.id);
        let rows = state.download_rows(&self.id);
        let mut content = div()
            .id("download-rows")
            .flex()
            .flex_col()
            .gap_2()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(error) = &state.downloads.config_error {
            content = content.child(error.clone());
        }
        if state.downloads.prompts_in(session).next().is_none() && rows.is_empty() {
            content = content.child("No downloads yet.");
        }
        for (&handle, prompt) in state.downloads.prompts_in(session).rev() {
            let row = match prompt {
                Prompt::Path { suggested, reason } => {
                    self.download_path_row(handle, suggested, *reason, panel, cx)
                }
                Prompt::Held { id, warning } => {
                    let name = list
                        .get(*id)
                        .map_or_else(|| "Download".into(), |entry| list.row(entry).filename);
                    download_card(name)
                        .child(match warning {
                            Warning::Insecure => {
                                "This download was delivered over an insecure connection."
                            }
                            _ => "This file type can harm your computer.",
                        })
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .child(self.download_button(
                                    handle,
                                    "keep",
                                    "Keep dangerous",
                                    DownloadCommand::Keep,
                                    cx,
                                ))
                                .child(self.download_button(
                                    handle,
                                    "discard",
                                    "Discard",
                                    DownloadCommand::Cancel,
                                    cx,
                                )),
                        )
                }
            };
            content = content.child(row);
        }
        for row in rows {
            if row.warning.is_none() {
                let handle = state.engine.download_handle(row.id);
                content = content.child(self.download_row(row, handle, cx));
            }
        }
        overlay_panel()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_lg().child("Downloads"))
                    .child(button("close-downloads", "Close").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                    ))),
            )
            .when(!state.status.is_empty(), |panel| {
                panel.child(state.status.clone())
            })
            .child(content)
    }

    fn download_path_row(
        &self,
        handle: DownloadHandle,
        suggested: &Path,
        reason: AskReason,
        panel: &DownloadsPanel,
        cx: &Context<Self>,
    ) -> Div {
        let suggested = suggested.display().to_string();
        let (field, path) = match &panel.draft {
            Some((editing, field)) if *editing == handle => (
                field.render("Destination file path"),
                Some(field.text().into()),
            ),
            _ => (
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .child(div().truncate().child(suggested.clone()))
                    .child(
                        button(
                            ("edit-download-path", handle.0 as usize),
                            "Edit destination",
                        )
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                if let Overlay::Downloads(panel) = &mut this.overlay {
                                    panel.draft =
                                        Some((handle, TextField::new(suggested.clone(), true)));
                                    cx.notify();
                                }
                            },
                        )),
                    ),
                None,
            ),
        };
        download_card(match reason {
            AskReason::Conflict => "Choose a path. The suggested name is in use.",
            AskReason::NameTooLong => "Choose a shorter file name.",
            _ => "Save download as",
        })
        .child(field)
        .child(
            div()
                .flex()
                .gap_1()
                .child(self.download_button(
                    handle,
                    "accept",
                    "Accept",
                    DownloadCommand::Accept(path),
                    cx,
                ))
                .child(self.download_button(
                    handle,
                    "cancel-path",
                    "Cancel",
                    DownloadCommand::Cancel,
                    cx,
                )),
        )
    }

    fn download_row(
        &self,
        row: DownloadRow,
        handle: Option<DownloadHandle>,
        cx: &Context<Self>,
    ) -> Div {
        let state = match row.state {
            RowState::Progressing if row.paused => "Paused",
            RowState::Progressing => "Downloading",
            RowState::Completed => "Complete",
            RowState::Cancelled => "Cancelled",
            RowState::Interrupted => "Interrupted",
        };
        let mut card = download_card(row.filename).child(div().truncate().child(row.path));
        card = card.child(if row.total > 0 {
            format!("{state} · {} of {} bytes", row.received, row.total)
        } else if row.received > 0 {
            format!("{state} · {} bytes", row.received)
        } else {
            state.to_owned()
        });
        if let Some(handle) = handle {
            let mut actions = div().flex().gap_1();
            if row.can_resume {
                actions = actions.child(self.download_button(
                    handle,
                    "resume",
                    "Resume",
                    DownloadCommand::Resume,
                    cx,
                ));
            }
            if row.state == RowState::Progressing {
                if !row.paused {
                    actions = actions.child(self.download_button(
                        handle,
                        "pause",
                        "Pause",
                        DownloadCommand::Pause,
                        cx,
                    ));
                }
                actions = actions.child(self.download_button(
                    handle,
                    "cancel",
                    "Cancel",
                    DownloadCommand::Cancel,
                    cx,
                ));
            }
            card = card.child(actions);
        }
        card
    }

    fn download_button(
        &self,
        handle: DownloadHandle,
        id: &'static str,
        label: &'static str,
        command: DownloadCommand,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        button((id, handle.0 as usize), label).on_click(cx.listener(
            move |this, _: &ClickEvent, _, cx| {
                dispatch(
                    &this.state,
                    &this.id,
                    Action::Download(handle, command.clone()),
                    cx,
                );
            },
        ))
    }
}

fn download_card(title: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .min_w_0()
        .rounded_md()
        .bg(rgb(0x243640))
        .child(div().truncate().child(title.into()))
}

#[cfg(test)]
mod tests {
    use flecha_downloads::{NewDownload, Snapshot, SnapshotState};

    use super::*;

    #[test]
    fn path_and_danger_prompts_follow_download_events() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut downloads = Downloads::new(profile.path(), true);
        let handle = DownloadHandle(1);
        let tab = TabId::new("downloader");
        let url = "https://example.test/file";
        let path = profile.path().join("file");
        let mut list = DownloadList::new();

        downloads.track(
            DownloadMessage {
                tab: Some(tab.clone()),
                incognito: None,
                event: DownloadEvent::AskPath {
                    handle,
                    url: url.into(),
                    suggested: path.clone(),
                    reason: AskReason::Preference,
                },
            },
            &list,
        );
        assert_eq!(
            downloads.prompts.get(&handle),
            Some(&Prompt::Path {
                suggested: path.clone(),
                reason: AskReason::Preference,
            })
        );
        assert_eq!(downloads.count(None), 1);
        assert!(downloads.uses_tab(&tab));

        let id = list.begin(NewDownload {
            cef_id: handle.0,
            url: url.into(),
            target: path.clone(),
            started_at: 0,
            warning: Some(Warning::Insecure),
            prompted: true,
        })?;
        downloads.track(
            DownloadMessage {
                tab: Some(tab.clone()),
                incognito: None,
                event: DownloadEvent::Held {
                    handle,
                    id,
                    warning: Warning::Insecure,
                },
            },
            &list,
        );
        assert_eq!(
            downloads.prompts.get(&handle),
            Some(&Prompt::Held {
                id,
                warning: Warning::Insecure
            })
        );
        assert_eq!(downloads.count(None), 1);

        list.keep(id)?;
        downloads.track(
            DownloadMessage {
                tab: Some(tab.clone()),
                incognito: None,
                event: DownloadEvent::Started {
                    handle,
                    id,
                    path: path.clone(),
                },
            },
            &list,
        );
        assert!(downloads.prompts.is_empty());
        assert_eq!(downloads.count(None), 1);

        list.update(
            handle.0,
            Snapshot {
                state: SnapshotState::Complete,
                received: 4,
                total: 4,
                full_path: &path,
            },
        )?;
        downloads.track(
            DownloadMessage {
                tab: Some(tab.clone()),
                incognito: None,
                event: DownloadEvent::Updated { handle, id },
            },
            &list,
        );
        assert_eq!(downloads.count(None), 0);
        assert!(!downloads.uses_tab(&tab));

        let handle = DownloadHandle(2);
        downloads.track(
            DownloadMessage {
                tab: Some(tab),
                incognito: None,
                event: DownloadEvent::AskPath {
                    handle,
                    url: url.into(),
                    suggested: path,
                    reason: AskReason::Conflict,
                },
            },
            &list,
        );
        downloads.track(
            DownloadMessage {
                tab: None,
                incognito: None,
                event: DownloadEvent::Cancelled { handle, id: None },
            },
            &list,
        );
        assert!(downloads.prompts.is_empty());
        assert_eq!(downloads.count(None), 0);

        // An incognito window's question is asked in that session's windows only.
        let session = IncognitoSession::new(1);
        let handle = DownloadHandle(3);
        downloads.track(
            DownloadMessage {
                tab: None,
                incognito: Some(session),
                event: DownloadEvent::AskPath {
                    handle,
                    url: url.into(),
                    suggested: profile.path().join("private"),
                    reason: AskReason::Preference,
                },
            },
            &list,
        );
        assert_eq!(downloads.count(None), 0);
        assert_eq!(downloads.count(Some(session)), 1);
        assert_eq!(downloads.prompts_in(None).count(), 0);
        assert_eq!(downloads.prompts_in(Some(session)).count(), 1);
        Ok(())
    }

    #[test]
    fn isolated_downloads_ignore_external_preferences_and_reject_escape_paths() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let saved_folder = outside.path().join("saved");
        let xdg_folder = outside.path().join("xdg");
        let settings = Settings {
            downloads_folder: Some(saved_folder.to_str().unwrap().into()),
            ask_where_to_save: Some(true),
            ..Settings::default()
        };
        let config = download_config(
            profile.path(),
            true,
            &settings,
            Some(&xdg_folder),
            Some(outside.path()),
        )?;
        let root = profile.path().canonicalize()?;
        assert_eq!(config.settings.folder, root.join("downloads"));
        assert!(config.settings.ask_where_to_save);
        assert!(!saved_folder.exists());
        assert!(!xdg_folder.exists());
        assert!(!outside.path().join("Downloads").exists());
        assert_eq!(
            destination(&config.settings.folder.join("file.txt"), Some(&root))?,
            root.join("downloads/file.txt")
        );
        assert!(destination(&outside.path().join("file.txt"), Some(&root)).is_err());
        assert!(destination(&root.join("../file.txt"), Some(&root)).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.join("outside"))?;
            assert!(destination(&root.join("outside/file.txt"), Some(&root)).is_err());
            let other = tempfile::tempdir()?;
            std::os::unix::fs::symlink(outside.path(), other.path().join("downloads"))?;
            assert!(download_config(other.path(), true, &settings, None, None).is_err());
        }
        Ok(())
    }
}
