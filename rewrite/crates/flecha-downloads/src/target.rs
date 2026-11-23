//! Where a download is saved: the folder and ask-where-to-save prefs, long names and
//! collisions, following `DownloadTargetDeterminer::DoGenerateTargetPath` and
//! `NeedsConfirmation` (`chrome/browser/download/download_target_determiner.cc`) and
//! `download_path_reservation_tracker.cc`.
//!
//! One deviation: Chromium's `filename_generation::TruncateFilename` doesn't truncate on Linux
//! because it can't assume a UTF-8 file system, and asks for a path instead. Names here are
//! Rust strings, so Linux truncates by UTF-8 bytes on a character boundary, as Chromium does
//! on macOS.

use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use flecha_core::Settings;
use unicode_normalization::UnicodeNormalization;

use crate::Platform;
use crate::filename::{FileName, split_extension};

/// `DownloadPathReservationTracker::kMaxUniqueFiles`.
pub const MAX_UNIQUE_FILES: u32 = 100;
/// `filename_generation::kTruncatedNameLengthLowerbound`.
const TRUNCATED_NAME_LOWER_BOUND: usize = 5;
/// `kIntermediateNameSuffixLength`: room for `.crdownload`.
pub(crate) const INTERMEDIATE_SUFFIX: &str = ".crdownload";
/// `kZoneIdentifierLength` on Windows.
const ZONE_IDENTIFIER: &str = ":Zone.Identifier";

/// Chromium's `download.default_directory` and `download.prompt_for_download` prefs, read from
/// Flecha's settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadSettings {
    pub folder: PathBuf,
    pub ask_where_to_save: bool,
}

impl DownloadSettings {
    /// `default_folder` is the platform's downloads folder (`DownloadPrefs::
    /// GetDefaultDownloadDirectory`, the XDG download dir on Linux); it must be absolute. A
    /// saved folder that isn't absolute falls back to it, as `DownloadPrefs` resets one.
    /// "Ask where to save" is off unless set (`RegisterProfilePrefs` registers `false`).
    pub fn from_settings(settings: &Settings, default_folder: &Path) -> Self {
        let folder = settings
            .downloads_folder
            .as_deref()
            .map(Path::new)
            .filter(|folder| folder.is_absolute())
            .unwrap_or(default_folder);
        Self {
            folder: folder.to_path_buf(),
            ask_where_to_save: settings.ask_where_to_save.unwrap_or(false),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FolderError {
    NotAbsolute,
    NotUnicode,
}

/// Settings' folder chooser picked `folder` (`DownloadPrefs::SetDownloadPath`).
pub fn set_downloads_folder(settings: &mut Settings, folder: &Path) -> Result<(), FolderError> {
    if !folder.is_absolute() {
        return Err(FolderError::NotAbsolute);
    }
    let folder = folder.to_str().ok_or(FolderError::NotUnicode)?;
    settings.downloads_folder = Some(folder.into());
    Ok(())
}

/// Why the person is asked where to save (`DownloadConfirmationReason`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AskReason {
    /// "Save link as" (`SAVE_AS`).
    SaveAs,
    /// Ask-where-to-save is on (`PREFERENCE`).
    Preference,
    /// The name can't be shortened to fit the file system (`NAME_TOO_LONG`).
    NameTooLong,
    /// Every uniquified name is taken (`TARGET_CONFLICT`).
    Conflict,
}

/// CEF's `BeforeDownloadCallback::Continue(path, show_dialog)` arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetDecision {
    Save(PathBuf),
    Ask {
        suggested: PathBuf,
        reason: AskReason,
    },
}

impl TargetDecision {
    pub fn path(&self) -> &Path {
        match self {
            Self::Save(path)
            | Self::Ask {
                suggested: path, ..
            } => path,
        }
    }

    pub fn asks(&self) -> bool {
        matches!(self, Self::Ask { .. })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TargetRequest<'a> {
    pub name: &'a FileName,
    pub settings: &'a DownloadSettings,
    /// The folder the person last picked in a save dialog (`DownloadPrefs::SaveFilePath`).
    /// Prompts start there; a relative path is ignored.
    pub last_prompt_folder: Option<&'a Path>,
    /// "Save link as" (`TARGET_DISPOSITION_PROMPT`).
    pub save_as: bool,
    /// The folder's file name limit (`base::GetMaximumPathComponentLength`): bytes on Linux
    /// and macOS, UTF-16 units on Windows. `None` when unknown; Chromium then doesn't
    /// truncate. [`name_max`] reads it on Unix.
    pub max_name_length: Option<usize>,
    /// The local start time, for the last-resort ` - <timestamp>` uniquifier.
    pub started: NaiveDateTime,
    pub platform: Platform,
}

/// Picks the path. `in_use` answers whether a path exists or another download in progress
/// has reserved it ([`crate::DownloadList::reserves`]).
pub fn decide_target(request: TargetRequest<'_>, in_use: impl Fn(&Path) -> bool) -> TargetDecision {
    let reason = if request.save_as {
        Some(AskReason::SaveAs)
    } else if request.settings.ask_where_to_save {
        Some(AskReason::Preference)
    } else {
        None
    };
    let folder = match (reason, request.last_prompt_folder) {
        (Some(_), Some(last)) if last.is_absolute() => last,
        _ => &request.settings.folder,
    };
    // A failed reservation replaces the confirmation reason, as in
    // `ReserveVirtualPathDone`.
    let finish = |path: PathBuf, failure: Option<AskReason>| match failure.or(reason) {
        Some(reason) => TargetDecision::Ask {
            suggested: path,
            reason,
        },
        None => TargetDecision::Save(path),
    };

    let units = |text: &str| length_in(text, request.platform);
    let reserve = match request.platform {
        Platform::Windows => units(INTERMEDIATE_SUFFIX).max(units(ZONE_IDENTIFIER)),
        Platform::Linux | Platform::Mac => units(INTERMEDIATE_SUFFIX),
    };
    let fit = |name: &str, extra: usize| -> Option<String> {
        match request.max_name_length {
            None => Some(name.into()),
            Some(max) => {
                let limit = max
                    .checked_sub(reserve + extra)
                    .filter(|limit| *limit > 0)?;
                truncate(name, limit, request.platform)
            }
        }
    };

    let original = request.name.as_str();
    let Some(name) = fit(original, 0) else {
        return finish(folder.join(original), Some(AskReason::NameTooLong));
    };
    let path = folder.join(&name);
    if !in_use(&path) {
        return finish(path, None);
    }
    for attempt in 1..=MAX_UNIQUE_FILES + 1 {
        let suffix = if attempt > MAX_UNIQUE_FILES {
            format!(" - {}", request.started.format("%Y-%m-%dT%H%M%S%.3f"))
        } else {
            format!(" ({attempt})")
        };
        let Some(fitted) = fit(&name, units(&suffix)) else {
            break;
        };
        let (stem, extension) = split_extension(&fitted);
        let candidate = folder.join(format!("{stem}{suffix}{extension}"));
        if !in_use(&candidate) {
            return finish(candidate, None);
        }
    }
    finish(path, Some(AskReason::Conflict))
}

fn length_in(text: &str, platform: Platform) -> usize {
    match platform {
        Platform::Windows => text.encode_utf16().count(),
        Platform::Linux | Platform::Mac => text.len(),
    }
}

/// `filename_generation::TruncateFilename`: shortens the part before the extension, never
/// below five units, on a character boundary.
fn truncate(name: &str, limit: usize, platform: Platform) -> Option<String> {
    if length_in(name, platform) <= limit {
        return Some(name.into());
    }
    let (stem, extension) = split_extension(name);
    let extension_length = length_in(extension, platform);
    if limit < TRUNCATED_NAME_LOWER_BOUND + extension_length {
        return None;
    }
    let budget = limit - extension_length;
    let mut used = 0;
    let mut end = 0;
    for (index, c) in stem.char_indices() {
        let width = match platform {
            Platform::Windows => c.len_utf16(),
            Platform::Linux | Platform::Mac => c.len_utf8(),
        };
        if used + width > budget {
            break;
        }
        used += width;
        end = index + c.len_utf8();
    }
    if used < TRUNCATED_NAME_LOWER_BOUND {
        return None;
    }
    Some(format!("{}{extension}", &stem[..end]))
}

/// `pathconf(folder, _PC_NAME_MAX)`, as `base::GetMaximumPathComponentLength` reads it.
#[cfg(unix)]
pub fn name_max(folder: &Path) -> Option<usize> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a NUL-terminated string that outlives the call.
    let max = unsafe { libc::pathconf(path.as_ptr(), libc::_PC_NAME_MAX) };
    usize::try_from(max).ok().filter(|max| *max > 0)
}

/// `FilePath::CompareEqualIgnoreCase` on `platform`, which the reservation tracker uses for
/// every reserved path (`download_path_reservation_tracker.cc`, `IsPathReservedInternal`).
pub(crate) fn same_path_ignoring_case(a: &Path, b: &Path, platform: Platform) -> bool {
    if a == b {
        return true;
    }
    if platform == Platform::Linux {
        // The POSIX version is `strncasecmp`, which folds ASCII letters only.
        return a
            .as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b.as_os_str().as_encoded_bytes());
    }
    let (Some(a), Some(b)) = (a.to_str(), b.to_str()) else {
        return false;
    };
    match platform {
        Platform::Windows => a
            .encode_utf16()
            .map(char_upper_w)
            .eq(b.encode_utf16().map(char_upper_w)),
        Platform::Linux | Platform::Mac => hfs_folded(a).eq(hfs_folded(b)),
    }
}

/// `CharUpperW` on one UTF-16 unit: its single-character uppercase, if it has one.
fn char_upper_w(unit: u16) -> u16 {
    let Some(c) = char::from_u32(u32::from(unit)) else {
        return unit;
    };
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(upper), None) => u16::try_from(u32::from(upper)).unwrap_or(unit),
        _ => unit,
    }
}

/// `HFSFastUnicodeCompare` on the decomposed form: Apple's TN1150 table drops U+200C to
/// U+200F, U+202A to U+202E, U+206A to U+206F and U+FEFF, and lower-cases only the BMP. Its
/// case table is approximated by Unicode's single-character lowercase, which covers its
/// mappings, so the port may reserve more names than macOS would but never fewer.
fn hfs_folded(text: &str) -> impl Iterator<Item = char> + '_ {
    text.nfd()
        .filter(|c| {
            !matches!(
                c,
                '\u{200C}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{206A}'..='\u{206F}' | '\u{FEFF}'
            )
        })
        .map(|c| {
            if u32::from(c) > 0xFFFF {
                return c;
            }
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(lower), None) => lower,
                _ => c,
            }
        })
}
