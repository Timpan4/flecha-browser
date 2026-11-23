//! Downloads for the GPUI shell to drive through CEF's `DownloadHandler`, independent of CEF
//! and GPUI. It ports patch 0058 (held downloads, the folder and ask-where-to-save prefs),
//! `chromium/shell/downloads.ts` (the sidebar list) and the Chromium rules behind them.
//!
//! - `OnBeforeDownload`: [`decide`] names the file, picks the path and says whether to hold
//!   it; [`DownloadList::begin`] records it.
//! - `OnDownloadUpdated`: [`SnapshotState::from_cef`] and [`DownloadList::update`].
//! - The sidebar: [`DownloadList::rows`], and the keep, discard, cancel, resume, retry and
//!   clear commands.
//! - Startup and saving: [`persist::load`] and [`persist::Ready::save`].

mod content_disposition;
pub mod danger;
mod filename;
mod list;
pub mod persist;
pub mod policy;
mod target;

use std::path::Path;

use chrono::NaiveDateTime;

pub use content_disposition::content_disposition_filename;
pub use danger::{
    DangerSignals, InsecureSignals, InsecureVerdict, Trigger, Warning, file_danger,
    insecure_verdict, is_url_potentially_trustworthy,
};
pub use filename::{
    DEFAULT_NAME, FileName, NameSources, generate_file_name, is_reserved_name_on_windows,
};
pub use list::{
    Download, DownloadId, DownloadList, DownloadRow, InterruptReason, ListError, NewDownload,
    RowState, Snapshot, SnapshotState, State, basename, display_name,
};
pub use policy::{DangerLevel, FileTypePolicies, canonical_extension};
#[cfg(unix)]
pub use target::name_max;
pub use target::{
    AskReason, DownloadSettings, FolderError, MAX_UNIQUE_FILES, TargetDecision, TargetRequest,
    decide_target, set_downloads_folder,
};

/// The platform whose rules apply. A value, not a `cfg`, so every platform's rules are
/// testable on any host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Linux,
    Mac,
    Windows,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Linux
        }
    }
}

/// What `OnBeforeDownload` knows, plus what the shell knows about how the download started.
#[derive(Clone, Copy, Debug)]
pub struct DownloadRequest<'a> {
    pub names: NameSources<'a>,
    pub settings: &'a DownloadSettings,
    pub save_as: bool,
    pub max_name_length: Option<usize>,
    pub started: NaiveDateTime,
    pub danger: DangerSignals,
    pub insecure: InsecureSignals<'a>,
    pub platform: Platform,
}

/// What to do with the download.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Proceed,
    /// Wait for the person to keep or discard it.
    Hold(Warning),
    /// Cancel it without asking (insecure `SILENT_BLOCK`).
    Block,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decision {
    pub file_name: FileName,
    pub target: TargetDecision,
    pub verdict: Verdict,
}

/// Names the file, picks the path, and decides whether it is held, in
/// `DownloadTargetDeterminer`'s order. `exists` answers whether a path is on disk; the list's
/// downloads in progress reserve theirs.
pub fn decide(
    request: DownloadRequest<'_>,
    list: &DownloadList,
    exists: impl Fn(&Path) -> bool,
) -> Decision {
    let file_name = generate_file_name(request.names, request.platform);
    let target = decide_target(
        TargetRequest {
            name: &file_name,
            settings: request.settings,
            last_prompt_folder: list.last_prompt_folder(),
            save_as: request.save_as,
            max_name_length: request.max_name_length,
            started: request.started,
            platform: request.platform,
        },
        |path| exists(path) || list.reserves(path),
    );
    let target_name = target
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_name.as_str());
    let verdict = match insecure_verdict(target_name, request.insecure) {
        InsecureVerdict::SilentBlock => Verdict::Block,
        InsecureVerdict::Hold => Verdict::Hold(Warning::Insecure),
        // GetDangerLevel: a path the person picks in a dialog approves the download.
        InsecureVerdict::Safe if target.asks() => Verdict::Proceed,
        InsecureVerdict::Safe => {
            let policies = FileTypePolicies::chromium(request.platform);
            match file_danger(policies, target_name, request.danger) {
                DangerLevel::NotDangerous => Verdict::Proceed,
                DangerLevel::AllowOnUserGesture | DangerLevel::Dangerous => {
                    Verdict::Hold(Warning::Dangerous)
                }
            }
        }
    };
    Decision {
        file_name,
        target,
        verdict,
    }
}
