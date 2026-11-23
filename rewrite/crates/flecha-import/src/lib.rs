//! Read imports from explicitly supplied browser roots, then apply them to Flecha.
//! Source files are read-only. Errors and warnings are safe to record without page data.

mod arc;
mod bookmarks;
mod browser;
mod input;
mod plan;
mod safari;
mod sqlite;

use std::fmt;
use std::path::Path;

use flecha_core::{BrowserState, storage};

pub use arc::{ArcImport, ArcProfile, ArcSpace, read_arc};
pub use bookmarks::{read_bookmarks_html, read_bookmarks_html_file};
pub use browser::{
    Browser, BrowserLocations, DetectedBrowser, Platform, detect_browsers, read_browser,
};
pub use plan::{
    AppliedImport, Bookmark, HistoryVisit, ImportOptions, ImportPlan, ImportPreview, PreviewSpace,
    SavedTab,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Missing,
    PermissionDenied,
    UnsafeFile,
    Unreadable,
    TooLarge,
    InvalidJson,
    InvalidSidebar,
    InvalidDatabase,
    InvalidPropertyList,
    InvalidFlechaState,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "File not found.",
            Self::PermissionDenied => "Permission denied.",
            Self::UnsafeFile => "Expected a regular file without symbolic links.",
            Self::Unreadable => "File could not be opened or read.",
            Self::TooLarge => "File exceeds the supported size.",
            Self::InvalidJson => "Invalid JSON or JSON nesting exceeds the parser limit.",
            Self::InvalidSidebar => "This does not look like an Arc StorableSidebar.json file.",
            Self::InvalidDatabase => "Invalid or unreadable browser database.",
            Self::InvalidPropertyList => "Invalid Safari bookmark property list.",
            Self::InvalidFlechaState => "That isn’t a Flecha state file.",
        })
    }
}

impl std::error::Error for ReadError {}

/// A decoded whole-state import. The caller owns confirmation and durable storage.
pub struct FlechaExport(storage::DecodedSnapshot);

impl FlechaExport {
    pub fn read(path: &Path) -> Result<Self, ReadError> {
        Self::parse(&input::read_selected_text(path)?)
    }

    pub fn parse(text: &str) -> Result<Self, ReadError> {
        storage::migrate_legacy_with_repairs(text)
            .or_else(|err| match err {
                storage::DecodeError::AlreadyMigrated => storage::decode_with_repairs(text),
                other => Err(other),
            })
            .map(Self)
            .map_err(|_| ReadError::InvalidFlechaState)
    }

    pub fn state(&self) -> &BrowserState {
        &self.0.state
    }

    pub fn repairs(&self) -> storage::SnapshotRepairs {
        self.0.repairs
    }

    /// Save the current state successfully before making the replacement visible.
    pub fn apply<E>(
        self,
        current: &mut BrowserState,
        save_backup: impl FnOnce(storage::BackupWrite) -> Result<(), E>,
    ) -> Result<(), ReplaceError<E>> {
        let backup = storage::set_aside_before_import(current).map_err(|_| ReplaceError::Encode)?;
        save_backup(backup).map_err(ReplaceError::Save)?;
        *current = self.0.state;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReplaceError<E> {
    Encode,
    Save(E),
}
