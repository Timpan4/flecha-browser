use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, ensure};
use cef::{CefString, ImplBrowserHost as _};
use flecha_cef::{
    DownloadConfig, DownloadEvent, DownloadHandle, DownloadMessage, IncognitoSession, Owner,
    Partition,
};
use flecha_core::{EpochMillis, Settings, TabId, WindowId};
use flecha_downloads::{
    AskReason, DownloadId, DownloadList, DownloadRow, DownloadSettings, RowState, State, Warning,
};
use gpui::{
    AppContext as _, ClickEvent, Context, Div, FontWeight, InteractiveElement as _, Keystroke,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, icon_button, overlay_panel, popover_header, text_button};
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
    attention: BTreeSet<WindowId>,
    attempted_picker: BTreeSet<DownloadHandle>,
    picking: Option<DownloadHandle>,
    retries: BTreeMap<TabId, Vec<(Partition, String)>>,
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
            attention: BTreeSet::new(),
            attempted_picker: BTreeSet::new(),
            picking: None,
            retries: BTreeMap::new(),
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
        if !matches!(self.prompts.get(&handle), Some(Prompt::Path { .. })) {
            self.attempted_picker.remove(&handle);
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
    pub(crate) fn forget_download_window(&mut self, window: &WindowId) {
        self.downloads.attention.remove(window);
    }

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
        self.downloads
            .retries
            .retain(|tab, _| self.model.partition(tab).is_some());
        let Some(first) = self.engine.next_download_event() else {
            return false;
        };
        let list = self.engine.downloads();
        let mut current = Some(first);
        while let Some(message) = current {
            if matches!(
                message.event,
                DownloadEvent::Started { .. }
                    | DownloadEvent::AskPath { .. }
                    | DownloadEvent::Held { .. }
            ) {
                let owner = message.tab.as_ref().and_then(|tab| self.engine.owner(tab));
                let window = match owner {
                    Some(
                        Owner::Tab(window)
                        | Owner::Peek(window)
                        | Owner::Flechinha(window)
                        | Owner::Popup(window),
                    ) => Some(window),
                    None => self.model.focused_window_for(&message.incognito.map_or(
                        flecha_cef::Partition::Shared,
                        flecha_cef::Partition::Incognito,
                    )),
                };
                if let Some(window) = window.filter(|window| self.windows.contains_key(*window)) {
                    self.downloads.attention.insert(window.clone());
                }
            }
            if matches!(message.event, DownloadEvent::Blocked { .. }) {
                let text = self
                    .downloads
                    .config_error
                    .clone()
                    .unwrap_or_else(|| "Download blocked.".into());
                self.status.set(message.incognito, text);
            }
            self.downloads.track(message, &list);
            current = self.engine.next_download_event();
        }
        true
    }

    pub(crate) fn clear_completed_downloads(&mut self, window: &WindowId) -> Result<()> {
        for row in self.download_rows(window) {
            if row.state == RowState::Completed {
                self.engine.remove_download(row.id)?;
            }
        }
        Ok(())
    }

    pub(crate) fn retry_download(&mut self, window: &WindowId, id: DownloadId) -> Result<()> {
        ensure!(
            self.engine.download_session(id) == self.model.incognito_session(window),
            "Download belongs to another session"
        );
        let url = self.engine.downloads().retry(id)?;
        let partition = self
            .engine
            .download_partition(id)
            .context("Download is no longer available")?;
        let destination = self.window_for_partition(window, &partition)?;
        let tab = if let Some((_, tab)) = self.model.keyboard_pane(&destination)
            && self.model.partition(tab).as_ref() == Some(&partition)
        {
            tab.clone()
        } else {
            let tab = self.model.open(&destination, "about:blank".into(), true)?;
            self.update_visibility();
            self.save()?;
            tab
        };
        if &destination != window {
            self.front = Some(destination);
        }
        self.downloads
            .retries
            .entry(tab.clone())
            .or_default()
            .push((partition, url));
        self.start_download_retries(&tab);
        Ok(())
    }

    pub(crate) fn start_download_retries(&mut self, tab: &TabId) {
        if self.downloads.retries.contains_key(tab)
            && let Some(host) = self.engine.host(tab)
            && let Some(urls) = self.downloads.retries.remove(tab)
        {
            for (partition, url) in urls {
                if self.engine.partition(tab) == Some(&partition) {
                    host.start_download(Some(&CefString::from(url.as_str())));
                }
            }
        }
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
    pub(crate) fn sync_downloads_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser_dialog.is_some() || !window.is_window_active() {
            return;
        }
        if !self.overlay.is_open() {
            let show = self.state.borrow_mut().downloads.attention.remove(&self.id);
            if show {
                self.show_overlay(Overlay::Downloads(DownloadsPanel::default()), window, cx);
            }
        }
        if !matches!(self.overlay, Overlay::Downloads(_)) {
            return;
        }
        let next = {
            let mut state = self.state.borrow_mut();
            state.downloads.attention.remove(&self.id);
            let session = state.model.incognito_session(&self.id);
            let downloads = &state.downloads;
            if downloads.picking.is_some() {
                return;
            }
            downloads
                .prompts_in(session)
                .find_map(|(handle, prompt)| match prompt {
                    Prompt::Path {
                        suggested,
                        reason: AskReason::Preference | AskReason::SaveAs,
                    } if !downloads.attempted_picker.contains(handle) => {
                        Some((*handle, suggested.clone()))
                    }
                    _ => None,
                })
        };
        if let Some((handle, path)) = next {
            self.pick_download_path(handle, &path, cx);
        }
    }

    fn pick_download_path(&self, handle: DownloadHandle, suggested: &Path, cx: &mut Context<Self>) {
        let Some(directory) = suggested.parent() else {
            return;
        };
        let mut state = self.state.borrow_mut();
        let downloads = &mut state.downloads;
        downloads.attempted_picker.insert(handle);
        downloads.picking = Some(handle);
        drop(state);

        let picker = cx.prompt_for_new_path(
            directory,
            suggested.file_name().and_then(|name| name.to_str()),
        );
        let state = Rc::downgrade(&self.state);
        let window = self.id.clone();
        cx.spawn(async move |_, cx| {
            let result = picker.await;
            if let Some(state) = state.upgrade() {
                cx.update(|cx| {
                    let mut state = state.borrow_mut();
                    state.downloads.picking = None;
                    if matches!(
                        state.downloads.prompts.get(&handle),
                        Some(Prompt::Path { .. })
                    ) {
                        let command = match result {
                            Ok(Ok(Some(path))) => Some(DownloadCommand::Accept(Some(path))),
                            Ok(Ok(None)) => Some(DownloadCommand::Cancel),
                            // The editable path stays in the panel when the platform picker fails.
                            _ => None,
                        };
                        if let Some(command) = command
                            && let Err(error) = state.manage_download(handle, command)
                        {
                            let session = state.model.incognito_session(&window);
                            state.status.set(session, error.to_string());
                        }
                    }
                    cx.refresh_windows();
                });
            }
        })
        .detach();
    }

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
        let can_clear = rows.iter().any(|row| row.state == RowState::Completed);
        let palette = palette();
        // #downloads-list (ui/styles.css:1055-1060).
        let mut content = div()
            .id("download-rows")
            .flex()
            .flex_col()
            .min_h_0()
            .px(px(tokens::DOWNLOADS_LIST_PAD))
            .pb(px(tokens::DOWNLOADS_LIST_PAD))
            .overflow_y_scroll();
        let note = |text: String| {
            div()
                .pt(px(tokens::DOWNLOADS_EMPTY_PAD_TOP))
                .px(px(tokens::DOWNLOADS_EMPTY_PAD_X))
                .pb(px(tokens::DOWNLOADS_EMPTY_PAD_BOTTOM))
                .text_color(palette.text_3)
                .child(text)
        };
        if let Some(error) = &state.downloads.config_error {
            content = content.child(note(error.clone()));
        }
        if state.downloads.prompts_in(session).next().is_none() && rows.is_empty() {
            // #downloads-list .empty (ui/styles.css:1144-1147).
            content = content.child(note("No downloads yet.".into()));
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
                    download_card(name, false)
                        .child(download_status(match warning {
                            Warning::Insecure => {
                                "This download was delivered over an insecure connection."
                            }
                            _ => "This file type can harm your computer.",
                        }))
                        .child(
                            download_actions()
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
        // .popover (ui/styles.css:1010-1022) carries no padding of its own.
        overlay_panel()
            .p_0()
            .gap_0()
            .child(
                popover_header("Downloads")
                    .when(can_clear, |header| {
                        header.child(self.action_button(
                            "clear-downloads",
                            "Clear",
                            Action::ClearDownloads,
                            cx,
                        ))
                    })
                    .child(
                        icon_button("close-downloads", "close", "Close").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                        )),
                    ),
            )
            .when_some(state.status_for(&self.id), |panel, status| {
                panel.child(note(status.to_owned()).pt_0())
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
                    .gap(px(tokens::DOWNLOAD_ACTIONS_GAP))
                    .child(download_status(suggested.clone()).min_w_0().truncate())
                    .child(
                        text_button(
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
        download_card(
            match reason {
                AskReason::Conflict => "Choose a path. The suggested name is in use.",
                AskReason::NameTooLong => "Choose a shorter file name.",
                _ => "Save download as",
            },
            false,
        )
        .child(field)
        .child(
            download_actions()
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
    ) -> impl gpui::IntoElement {
        let id = row.id;
        let completed = row.state == RowState::Completed;
        let state = match row.state {
            RowState::Progressing if row.paused => "Paused",
            RowState::Progressing => "Downloading",
            RowState::Completed => "Complete",
            RowState::Cancelled => "Cancelled",
            RowState::Interrupted => "Interrupted",
        };
        let stopped = matches!(row.state, RowState::Cancelled | RowState::Interrupted);
        let mut card = download_card(row.filename, stopped)
            .child(download_status(row.path).min_w_0().truncate());
        card = card.child(download_status(if row.total > 0 {
            format!("{state} · {} of {} bytes", row.received, row.total)
        } else if row.received > 0 {
            format!("{state} · {} bytes", row.received)
        } else {
            state.to_owned()
        }));
        if row.state == RowState::Progressing {
            // .download .bar and .fill (ui/styles.css:1128-1142); renderDownloads (ui/app.ts:1574)
            // fills it only when the total is known.
            let share = if row.total > 0 {
                (row.received as f32 / row.total as f32).clamp(0., 1.)
            } else {
                0.
            };
            card = card.child(
                div()
                    .h(px(tokens::DOWNLOAD_BAR))
                    .mt(px(tokens::DOWNLOAD_BAR_GAP))
                    .rounded(px(tokens::DOWNLOAD_BAR_RADIUS))
                    .bg(palette().surface_line)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(share))
                            .bg(palette().selection),
                    ),
            );
        }
        if let Some(handle) = handle {
            let mut actions = download_actions();
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
        if completed {
            card = card.child(download_actions().child(
                text_button(("reveal-download", id as usize), "Show in Folder").on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_download(id, true, cx);
                    }),
                ),
            ));
        } else if row.state == RowState::Interrupted && !row.can_resume {
            card = card.child(download_actions().child(
                text_button(("retry-download", id as usize), "Retry").on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, Action::RetryDownload(id), cx);
                    },
                )),
            ));
        }
        card.id(("download-row", id as usize))
            .when(completed, |card| {
                card.cursor_pointer()
                    .aria_label("Open downloaded file")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.open_download(id, false, cx)
                    }))
            })
    }

    fn open_download(&self, id: DownloadId, reveal: bool, cx: &mut Context<Self>) {
        let state = self.state.borrow();
        let session = state.model.incognito_session(&self.id);
        let list = state.engine.downloads();
        let Some(download) = list.get(id).filter(|entry| {
            entry.state() == State::Complete
                && entry.warning().is_none()
                && state.engine.download_session(id) == session
        }) else {
            return;
        };
        let path = download.path().to_owned();
        let profile = state
            .downloads
            .isolated
            .then(|| state.downloads.profile.clone());
        let work = cx.background_spawn(async move {
            ensure!(path.is_absolute(), "Download has no local file");
            let path = path.canonicalize().context("Find the downloaded file")?;
            ensure!(path.is_file(), "Download has no local file");
            if let Some(profile) = profile {
                ensure!(
                    path.starts_with(profile.canonicalize()?),
                    "Download is outside the isolated profile"
                );
            }
            let target = if reveal {
                path.parent().context("Download has no containing folder")?
            } else {
                &path
            };
            open::that(target).context("Open the downloaded file or its folder")
        });
        cx.spawn(async move |shell, cx| {
            if let Err(error) = work.await {
                let _ = shell.update(cx, |this, cx| {
                    this.state
                        .borrow_mut()
                        .status
                        .set(session, error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn download_button(
        &self,
        handle: DownloadHandle,
        id: &'static str,
        label: &'static str,
        command: DownloadCommand,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        text_button((id, handle.0 as usize), label).on_click(cx.listener(
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

/// `.download` (ui/styles.css:1062-1094): a name over its status, which highlights on hover.
/// A cancelled or interrupted download's name is struck through in `--text-3`.
fn download_card(title: impl Into<gpui::SharedString>, stopped: bool) -> Div {
    let palette = palette();
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .py(px(tokens::DOWNLOAD_PAD_Y))
        .pr(px(tokens::DOWNLOAD_PAD_RIGHT))
        .pl(px(tokens::DOWNLOAD_PAD_LEFT))
        .rounded(px(tokens::RADIUS_MD))
        .hover(move |style| style.bg(palette.surface_2))
        .child(
            div()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .when(stopped, |name| {
                    name.text_color(palette.text_3).line_through()
                })
                .child(title.into()),
        )
}

/// `.download .status` (ui/styles.css:1084-1088).
fn download_status(text: impl Into<gpui::SharedString>) -> Div {
    div()
        .text_size(px(tokens::TEXT_XS))
        .font_family(tokens::MONO)
        .text_color(palette().text_2)
        .child(text.into())
}

/// `.download .actions` on a line of its own (ui/styles.css:1102-1118).
fn download_actions() -> Div {
    div()
        .flex()
        .flex_wrap()
        .justify_end()
        .gap(px(tokens::DOWNLOAD_ACTIONS_GAP))
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
