//! The downloads list the sidebar shows, driven by CEF's `OnDownloadUpdated` snapshots.
//!
//! States follow Chromium's `DownloadItem` (`IN_PROGRESS` with a paused flag, `COMPLETE`,
//! `CANCELLED`, `INTERRUPTED`); removal drops an entry. Rows match the TS shell's
//! `DownloadInfo` (`chromium/shell/downloads.ts` `toInfo`, `chromium/shared/types.d.ts`).
//!
//! Entries carry Flecha's own id. CEF's `DownloadItem::GetId` only names a download for this
//! session, so the list maps it per session and never stores it.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use flecha_core::EpochMillis;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::danger::Warning;
use crate::filename::DEFAULT_NAME;

pub type DownloadId = u64;

/// `DownloadInterruptReason` (`download_interrupt_reason_values.h`), the values CEF's
/// `cef_download_interrupt_reason_t` mirrors.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InterruptReason {
    FileFailed,
    FileAccessDenied,
    FileNoSpace,
    FileNameTooLong,
    FileTooLarge,
    FileVirusInfected,
    FileTransientError,
    FileBlocked,
    FileSecurityCheckFailed,
    FileTooShort,
    FileHashMismatch,
    FileSameAsSource,
    NetworkFailed,
    NetworkTimeout,
    NetworkDisconnected,
    NetworkServerDown,
    NetworkInvalidRequest,
    ServerFailed,
    ServerNoRange,
    ServerBadContent,
    ServerUnauthorized,
    ServerCertProblem,
    ServerForbidden,
    ServerUnreachable,
    ServerContentLengthMismatch,
    ServerCrossOriginRedirect,
    UserCanceled,
    UserShutdown,
    Crash,
}

impl InterruptReason {
    pub const ALL: [Self; 29] = [
        Self::FileFailed,
        Self::FileAccessDenied,
        Self::FileNoSpace,
        Self::FileNameTooLong,
        Self::FileTooLarge,
        Self::FileVirusInfected,
        Self::FileTransientError,
        Self::FileBlocked,
        Self::FileSecurityCheckFailed,
        Self::FileTooShort,
        Self::FileHashMismatch,
        Self::FileSameAsSource,
        Self::NetworkFailed,
        Self::NetworkTimeout,
        Self::NetworkDisconnected,
        Self::NetworkServerDown,
        Self::NetworkInvalidRequest,
        Self::ServerFailed,
        Self::ServerNoRange,
        Self::ServerBadContent,
        Self::ServerUnauthorized,
        Self::ServerCertProblem,
        Self::ServerForbidden,
        Self::ServerUnreachable,
        Self::ServerContentLengthMismatch,
        Self::ServerCrossOriginRedirect,
        Self::UserCanceled,
        Self::UserShutdown,
        Self::Crash,
    ];

    /// The `cef_download_interrupt_reason_t` value. `NONE` (0) is no reason.
    pub fn code(self) -> i32 {
        match self {
            Self::FileFailed => 1,
            Self::FileAccessDenied => 2,
            Self::FileNoSpace => 3,
            Self::FileNameTooLong => 5,
            Self::FileTooLarge => 6,
            Self::FileVirusInfected => 7,
            Self::FileTransientError => 10,
            Self::FileBlocked => 11,
            Self::FileSecurityCheckFailed => 12,
            Self::FileTooShort => 13,
            Self::FileHashMismatch => 14,
            Self::FileSameAsSource => 15,
            Self::NetworkFailed => 20,
            Self::NetworkTimeout => 21,
            Self::NetworkDisconnected => 22,
            Self::NetworkServerDown => 23,
            Self::NetworkInvalidRequest => 24,
            Self::ServerFailed => 30,
            Self::ServerNoRange => 31,
            Self::ServerBadContent => 33,
            Self::ServerUnauthorized => 34,
            Self::ServerCertProblem => 35,
            Self::ServerForbidden => 36,
            Self::ServerUnreachable => 37,
            Self::ServerContentLengthMismatch => 38,
            Self::ServerCrossOriginRedirect => 39,
            Self::UserCanceled => 40,
            Self::UserShutdown => 41,
            Self::Crash => 50,
        }
    }

    pub fn from_code(code: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.code() == code)
    }

    /// Whether the person can resume after this interruption: `GetDownloadResumeMode`
    /// (`components/download/internal/common/download_utils.cc`) gives `USER_CONTINUE` or
    /// `USER_RESTART`. The reasons Chromium resumes on its own count too: an interruption that
    /// stays visible has used up those automatic attempts.
    fn allows_resume(self) -> bool {
        !matches!(
            self,
            Self::NetworkInvalidRequest
                | Self::FileVirusInfected
                | Self::ServerBadContent
                | Self::UserShutdown
                | Self::UserCanceled
                | Self::FileBlocked
                | Self::FileSecurityCheckFailed
                | Self::ServerUnauthorized
                | Self::ServerCertProblem
                | Self::ServerForbidden
                | Self::ServerCrossOriginRedirect
                | Self::FileSameAsSource
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    InProgress,
    Paused,
    Complete,
    Cancelled,
    Interrupted,
}

/// One download.
#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub(crate) id: DownloadId,
    pub(crate) url: String,
    pub(crate) path: PathBuf,
    pub(crate) received: u64,
    pub(crate) total: u64,
    pub(crate) state: State,
    pub(crate) interrupt_reason: Option<InterruptReason>,
    pub(crate) started_at: EpochMillis,
    pub(crate) warning: Option<Warning>,
    pub(crate) prompted: bool,
}

impl Download {
    pub fn id(&self) -> DownloadId {
        self.id
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn interrupt_reason(&self) -> Option<InterruptReason> {
        self.interrupt_reason
    }
    pub fn warning(&self) -> Option<Warning> {
        self.warning
    }
    pub fn received(&self) -> u64 {
        self.received
    }
    pub fn total(&self) -> u64 {
        self.total
    }
    pub fn started_at(&self) -> EpochMillis {
        self.started_at
    }

    fn in_progress(&self) -> bool {
        matches!(self.state, State::InProgress | State::Paused)
    }
}

/// The state part of a CEF `DownloadItem`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotState {
    InProgress,
    Paused,
    Complete,
    Cancelled,
    Interrupted(InterruptReason),
}

impl SnapshotState {
    /// From `is_in_progress`, `is_paused`, `is_complete`, `is_canceled`, `is_interrupted` and
    /// `get_interrupt_reason`. `None` for a combination or reason CEF doesn't define.
    pub fn from_cef(
        in_progress: bool,
        paused: bool,
        complete: bool,
        canceled: bool,
        interrupted: bool,
        interrupt_reason: i32,
    ) -> Option<Self> {
        match (in_progress, complete, canceled, interrupted) {
            (true, false, false, false) if paused => Some(Self::Paused),
            (true, false, false, false) => Some(Self::InProgress),
            (false, true, false, false) => Some(Self::Complete),
            (false, false, true, false) => Some(Self::Cancelled),
            (false, false, false, true) => {
                InterruptReason::from_code(interrupt_reason).map(Self::Interrupted)
            }
            _ => None,
        }
    }
}

/// A CEF `OnDownloadUpdated` report.
#[derive(Clone, Copy, Debug)]
pub struct Snapshot<'a> {
    pub state: SnapshotState,
    pub received: i64,
    /// `get_total_bytes`; zero or negative when unknown.
    pub total: i64,
    /// `get_full_path`; empty until the target is known.
    pub full_path: &'a Path,
}

/// A download CEF announced in `OnBeforeDownload`.
#[derive(Clone, Debug)]
pub struct NewDownload {
    pub cef_id: u32,
    pub url: String,
    /// The decided path, or the one a save dialog starts from.
    pub target: PathBuf,
    pub started_at: EpochMillis,
    pub warning: Option<Warning>,
    /// Whether a save dialog asks for the path; its answer becomes the next dialog's folder.
    pub prompted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListError {
    /// Patch 0058's "No download with id *."
    NoDownload(DownloadId),
    /// Patch 0058's "Download * isn't waiting for a decision."
    NotHeld(DownloadId),
    UnknownCefId(u32),
    CefIdInUse(u32),
    /// The download can't go from one state to the other.
    Transition {
        id: DownloadId,
        from: State,
        to: State,
    },
    /// A held download must be kept before it can finish.
    CompletedWhileHeld(DownloadId),
    /// The command doesn't apply in the download's current state.
    NotAllowed {
        id: DownloadId,
        state: State,
    },
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDownload(id) => write!(f, "No download with id {id}."),
            Self::NotHeld(id) => write!(f, "Download {id} isn't waiting for a decision."),
            Self::UnknownCefId(id) => write!(f, "No download for CEF id {id}."),
            Self::CefIdInUse(id) => write!(f, "CEF id {id} is already in the list."),
            Self::Transition { id, from, to } => {
                write!(f, "Download {id} can't go from {from:?} to {to:?}.")
            }
            Self::CompletedWhileHeld(id) => {
                write!(f, "Download {id} finished while waiting for a decision.")
            }
            Self::NotAllowed { id, state } => {
                write!(f, "Download {id} can't do that while {state:?}.")
            }
        }
    }
}

impl std::error::Error for ListError {}

/// The sidebar's row state (`DownloadInfo['state']`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    Progressing,
    Completed,
    Cancelled,
    Interrupted,
}

/// `DownloadInfo` from `chromium/shared/types.d.ts`, plus `paused`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRow {
    pub id: DownloadId,
    pub filename: String,
    pub path: String,
    pub url: String,
    pub received: u64,
    pub total: u64,
    pub state: RowState,
    pub started_at: EpochMillis,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<Warning>,
    pub can_resume: bool,
    pub paused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DownloadList {
    pub(crate) entries: Vec<Download>,
    pub(crate) next_id: DownloadId,
    pub(crate) last_prompt_folder: Option<PathBuf>,
    live: HashMap<u32, DownloadId>,
}

impl PartialEq for DownloadList {
    /// The persisted part: the per-session CEF ids don't count.
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
            && self.next_id == other.next_id
            && self.last_prompt_folder == other.last_prompt_folder
    }
}

impl DownloadList {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn restored(
        entries: Vec<Download>,
        next_id: DownloadId,
        last_prompt_folder: Option<PathBuf>,
    ) -> Self {
        Self {
            entries,
            next_id,
            last_prompt_folder,
            live: HashMap::new(),
        }
    }

    pub fn get(&self, id: DownloadId) -> Option<&Download> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Download> {
        self.entries.iter()
    }

    /// Where the last save dialog was answered (`DownloadPrefs::SaveFilePath`).
    pub fn last_prompt_folder(&self) -> Option<&Path> {
        self.last_prompt_folder.as_deref()
    }

    /// Settings picked a new downloads folder; the next dialog starts there too
    /// (`DownloadPrefs::SetDownloadPath` sets `SaveFilePath`).
    pub fn downloads_folder_changed(&mut self, folder: &Path) {
        self.last_prompt_folder = Some(folder.to_path_buf());
    }

    /// Whether a download in progress holds `path` (the reservation map in
    /// `download_path_reservation_tracker.cc`, kept only while `IN_PROGRESS`).
    pub fn reserves(&self, path: &Path) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.in_progress() && entry.path == path)
    }

    pub fn id_for_cef(&self, cef_id: u32) -> Option<DownloadId> {
        self.live.get(&cef_id).copied()
    }

    fn cef_id_of(&self, id: DownloadId) -> Option<u32> {
        self.live
            .iter()
            .find_map(|(cef, ours)| (*ours == id).then_some(*cef))
    }

    pub fn begin(&mut self, new: NewDownload) -> Result<DownloadId, ListError> {
        if self.live.contains_key(&new.cef_id) {
            return Err(ListError::CefIdInUse(new.cef_id));
        }
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(Download {
            id,
            url: new.url,
            path: new.target,
            received: 0,
            total: 0,
            state: State::InProgress,
            interrupt_reason: None,
            started_at: new.started_at,
            warning: new.warning,
            prompted: new.prompted,
        });
        self.live.insert(new.cef_id, id);
        Ok(id)
    }

    /// Applies a CEF snapshot.
    pub fn update(&mut self, cef_id: u32, snapshot: Snapshot<'_>) -> Result<(), ListError> {
        let id = self
            .id_for_cef(cef_id)
            .ok_or(ListError::UnknownCefId(cef_id))?;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(ListError::NoDownload(id))?;
        let (to, reason) = match snapshot.state {
            SnapshotState::InProgress => (State::InProgress, None),
            SnapshotState::Paused => (State::Paused, None),
            SnapshotState::Complete => (State::Complete, None),
            SnapshotState::Cancelled => (State::Cancelled, None),
            SnapshotState::Interrupted(InterruptReason::UserCanceled) => {
                (State::Cancelled, Some(InterruptReason::UserCanceled))
            }
            SnapshotState::Interrupted(reason) => (State::Interrupted, Some(reason)),
        };
        let from = entry.state;
        let allowed = match from {
            State::InProgress | State::Paused => true,
            State::Interrupted => matches!(
                to,
                State::InProgress | State::Paused | State::Interrupted | State::Cancelled
            ),
            State::Complete | State::Cancelled => to == from,
        };
        if !allowed {
            return Err(ListError::Transition { id, from, to });
        }
        if to == State::Complete && entry.warning.is_some() {
            return Err(ListError::CompletedWhileHeld(id));
        }
        if from == to && matches!(from, State::Complete | State::Cancelled) {
            return Ok(());
        }
        entry.state = to;
        entry.interrupt_reason = reason;
        entry.received = u64::try_from(snapshot.received).unwrap_or(0);
        entry.total = u64::try_from(snapshot.total).unwrap_or(0);
        if !snapshot.full_path.as_os_str().is_empty() {
            entry.path = snapshot.full_path.to_path_buf();
            if entry.prompted
                && let Some(folder) = entry.path.parent()
            {
                // DownloadTargetDeterminer::RequestConfirmationDone → SetSaveFilePath.
                self.last_prompt_folder = Some(folder.to_path_buf());
                entry.prompted = false;
            }
        }
        if !entry.in_progress() {
            // Patch 0058 reports a warning only while the download is in progress.
            entry.warning = None;
        }
        Ok(())
    }

    fn held(&self, id: DownloadId) -> Result<&Download, ListError> {
        let entry = self.get(id).ok_or(ListError::NoDownload(id))?;
        if entry.warning.is_none() || !entry.in_progress() {
            return Err(ListError::NotHeld(id));
        }
        Ok(entry)
    }

    /// Lets a held download continue (`keepDownload`). Returns its CEF id, if this session
    /// has one, for `BeforeDownloadCallback::Continue`.
    pub fn keep(&mut self, id: DownloadId) -> Result<Option<u32>, ListError> {
        self.held(id)?;
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.warning = None;
        }
        Ok(self.cef_id_of(id))
    }

    /// Cancels a held download and drops it from the list (`discardDownload`). The caller
    /// cancels the CEF item, which deletes what it saved.
    pub fn discard(&mut self, id: DownloadId) -> Result<(Download, Option<u32>), ListError> {
        self.held(id)?;
        let cef_id = self.cef_id_of(id);
        let removed = self.take(id).ok_or(ListError::NoDownload(id))?;
        Ok((removed, cef_id))
    }

    /// The CEF id to cancel a download in progress (`cancelDownload`).
    pub fn cancel(&self, id: DownloadId) -> Result<u32, ListError> {
        self.live_command(id, |entry| entry.in_progress() && entry.warning.is_none())
    }

    /// The CEF id to pause a download in progress.
    pub fn pause(&self, id: DownloadId) -> Result<u32, ListError> {
        self.live_command(id, |entry| {
            entry.state == State::InProgress && entry.warning.is_none()
        })
    }

    /// The CEF id to resume a paused or resumable interrupted download (`resumeDownload`).
    pub fn resume(&self, id: DownloadId) -> Result<u32, ListError> {
        self.live_command(id, |entry| {
            entry.state == State::Paused || self.can_resume(entry)
        })
    }

    /// The URL to download again (`retryDownload`, offered for interrupted rows).
    pub fn retry(&self, id: DownloadId) -> Result<String, ListError> {
        let entry = self.get(id).ok_or(ListError::NoDownload(id))?;
        if entry.state != State::Interrupted {
            return Err(ListError::NotAllowed {
                id,
                state: entry.state,
            });
        }
        Ok(entry.url.clone())
    }

    /// Drops a finished, cancelled or interrupted download from the list (Chromium's erase).
    /// The file stays on disk.
    pub fn remove(&mut self, id: DownloadId) -> Result<Download, ListError> {
        let entry = self.get(id).ok_or(ListError::NoDownload(id))?;
        if entry.in_progress() {
            return Err(ListError::NotAllowed {
                id,
                state: entry.state,
            });
        }
        self.take(id).ok_or(ListError::NoDownload(id))
    }

    /// `clearDownloads`: removes every row started after `since` that isn't progressing.
    pub fn clear(&mut self, since: EpochMillis) -> Vec<DownloadId> {
        let cleared: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.started_at > since && !entry.in_progress())
            .map(|entry| entry.id)
            .collect();
        for id in &cleared {
            self.take(*id);
        }
        cleared
    }

    /// The sidebar rows: downloads started after `since` (the shell's session start), newest
    /// first, as `chrome.downloads.search({ startedAfter, orderBy: ['-startTime'] })`.
    pub fn rows(&self, since: EpochMillis) -> Vec<DownloadRow> {
        let mut rows: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.started_at > since)
            .map(|entry| self.row(entry))
            .collect();
        rows.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
        rows
    }

    pub fn row(&self, entry: &Download) -> DownloadRow {
        let path = entry.path.to_string_lossy().into_owned();
        let state = match entry.state {
            State::InProgress | State::Paused => RowState::Progressing,
            State::Complete => RowState::Completed,
            State::Cancelled => RowState::Cancelled,
            State::Interrupted => RowState::Interrupted,
        };
        DownloadRow {
            id: entry.id,
            filename: display_name(&path, &entry.url),
            path,
            url: entry.url.clone(),
            received: entry.received,
            total: entry.total,
            state,
            started_at: entry.started_at,
            warning: entry.warning.filter(|_| entry.in_progress()),
            can_resume: entry.state == State::Paused || self.can_resume(entry),
            paused: entry.state == State::Paused,
        }
    }

    /// Resumable: interrupted for a reason that allows it, over HTTP(S) (`GetDownloadResumeMode`
    /// supports nothing else), and still known to CEF this session.
    fn can_resume(&self, entry: &Download) -> bool {
        entry.state == State::Interrupted
            && entry
                .interrupt_reason
                .is_some_and(InterruptReason::allows_resume)
            && Url::parse(&entry.url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            && self.cef_id_of(entry.id).is_some()
    }

    fn live_command(
        &self,
        id: DownloadId,
        allowed: impl Fn(&Download) -> bool,
    ) -> Result<u32, ListError> {
        let entry = self.get(id).ok_or(ListError::NoDownload(id))?;
        if !allowed(entry) {
            return Err(ListError::NotAllowed {
                id,
                state: entry.state,
            });
        }
        self.cef_id_of(id).ok_or(ListError::NotAllowed {
            id,
            state: entry.state,
        })
    }

    fn take(&mut self, id: DownloadId) -> Option<Download> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        self.live.retain(|_, ours| *ours != id);
        Some(self.entries.remove(index))
    }
}

/// `toInfo`'s name: the file's base name, else the URL path's last segment, else `download`.
pub fn display_name(path: &str, url: &str) -> String {
    let from_path = basename(path);
    if !from_path.is_empty() {
        return from_path.into();
    }
    let from_url = Url::parse(url)
        .map(|url| basename(url.path()).to_owned())
        .unwrap_or_default();
    if !from_url.is_empty() {
        return from_url;
    }
    DEFAULT_NAME.into()
}

/// `downloads.ts` `basename`: Windows paths (`C:\…`, `C:/…`, `\\server\…`) also split on
/// backslashes, which elsewhere can be part of a file name.
pub fn basename(file: &str) -> &str {
    let bytes = file.as_bytes();
    let windows = bytes.first() == Some(&b'\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
    if windows {
        file.rsplit(['\\', '/']).next().unwrap_or(file)
    } else {
        file.rsplit('/').next().unwrap_or(file)
    }
}
