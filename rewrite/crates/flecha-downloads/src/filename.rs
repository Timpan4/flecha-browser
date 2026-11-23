//! The name a download is saved under, as Chromium generates it: `net::GenerateFileName`
//! (`net/base/filename_util_internal.cc`) with `ReplaceIllegalCharactersInPath`
//! (`base/i18n/file_util_icu.cc`) and `IsReservedNameOnWindows` (`base/files/file_util.cc`).
//!
//! MIME-based extension correction is left out: it needs Chromium's MIME tables. CEF's
//! `OnBeforeDownload` hands over the name Chromium generated with it; pass that as
//! [`NameSources::suggested_name`] and this sanitizing applies again on top.

use std::fmt;

use percent_encoding::percent_decode_str;
use unicode_general_category::{GeneralCategory, get_general_category};
use url::Url;

use crate::Platform;
use crate::content_disposition::content_disposition_filename;

/// `IDS_DEFAULT_DOWNLOAD_FILENAME` in `chrome/app/generated_resources.grd`, and the TS shell's
/// fallback in `downloads.ts`.
pub const DEFAULT_NAME: &str = "download";

/// A single path component that can't climb out of the folder it is joined to. Only
/// [`generate_file_name`] makes one.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileName(String);

impl FileName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a name can come from, in Chromium's order of preference.
#[derive(Clone, Copy, Debug, Default)]
pub struct NameSources<'a> {
    /// The URL the bytes come from.
    pub url: &'a str,
    pub content_disposition: Option<&'a str>,
    /// A page's `download` attribute, or the name Chromium already generated.
    pub suggested_name: Option<&'a str>,
}

/// `GetSuggestedFilenameImpl` with `default_name` set to [`DEFAULT_NAME`], as
/// `DownloadTargetDeterminer::GenerateFileName` calls it. Because a default name is given, the
/// URL's host is never used.
pub fn generate_file_name(sources: NameSources<'_>, platform: Platform) -> FileName {
    let mut name = sources
        .content_disposition
        .and_then(content_disposition_filename)
        .unwrap_or_default();
    if name.is_empty() {
        name = sources.suggested_name.unwrap_or_default().into();
    }
    if name.is_empty() {
        name = file_name_from_url(sources.url);
    }
    sanitize_generated_file_name(&mut name, platform == Platform::Windows);
    if name.chars().all(|c| c == '-' || c == '_') {
        name = DEFAULT_NAME.into();
    }
    let mut name = replace_illegal_characters(&name, platform);
    if platform == Platform::Windows {
        // EnsureSafeExtension: shell-integrated extensions become ".download".
        let (stem, extension) = split_extension(&name);
        if is_shell_integrated_extension(extension.trim_start_matches('.')) {
            name = format!("{stem}.{DEFAULT_NAME}");
        }
        if is_reserved_name_on_windows(&name) {
            name.insert(0, '_');
        }
    }
    FileName(name)
}

/// `GetFileNameFromURL`: the last path segment, unescaped. about: and data: URLs have none.
fn file_name_from_url(url: &str) -> String {
    let Ok(url) = Url::parse(url) else {
        return String::new();
    };
    if matches!(url.scheme(), "about" | "data") {
        return String::new();
    }
    let segment = url.path().rsplit('/').next().unwrap_or_default();
    // url::ExtractFileName stops at a `;` parameter.
    let segment = segment.split(';').next().unwrap_or_default();
    String::from_utf8_lossy(&percent_decode_str(segment).collect::<Vec<u8>>()).into_owned()
}

/// `SanitizeGeneratedFileName`.
fn sanitize_generated_file_name(name: &mut String, replace_trailing: bool) {
    if name.is_empty() {
        return;
    }
    if replace_trailing {
        // CreateFile() strips trailing dots and spaces (Microsoft KB 115827).
        let length = name.chars().count();
        let kept = name.trim_end_matches([' ', '.']).trim_end().len();
        name.truncate(kept);
        if name.is_empty() {
            return;
        }
        let trimmed = length - name.chars().count();
        name.extend(std::iter::repeat_n('_', trimmed));
    }
    *name = name.trim_matches('.').replace(['/', '\\'], "_");
}

/// `ReplaceIllegalCharactersInPath(name, '_')`. `_` is legal at the ends, so only the first
/// and last characters are checked against the end-only set.
fn replace_illegal_characters(name: &str, platform: Platform) -> String {
    let could_be_short_name = platform == Platform::Windows
        && name.encode_utf16().count() <= 12
        && has_valid_dot_position_for_short_name(name)
        && !name.chars().any(is_illegal_in_short_name);
    let last = name.chars().count().saturating_sub(1);
    name.chars()
        .enumerate()
        .map(|(index, c)| {
            let at_end = index == 0 || index == last;
            if is_illegal_anywhere(c)
                || (could_be_short_name && c == '~')
                || (at_end && is_illegal_at_ends(c))
            {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// `[["*/:<>?\\|][:Cc:][:Cf:]]` plus the noncharacters.
fn is_illegal_anywhere(c: char) -> bool {
    matches!(c, '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|')
        || matches!(
            get_general_category(c),
            GeneralCategory::Control | GeneralCategory::Format
        )
        || ('\u{FDD0}'..='\u{FDEF}').contains(&c)
        || (u32::from(c) & 0xFFFE) == 0xFFFE
}

/// `[[:WSpace:][.~]]`.
fn is_illegal_at_ends(c: char) -> bool {
    c.is_whitespace() || c == '.' || c == '~'
}

/// `[[:WSpace:]["\/[]:+|<>=;?,*]]`.
fn is_illegal_in_short_name(c: char) -> bool {
    c.is_whitespace() || "\"\\/[]:+|<>=;?,*".contains(c)
}

fn has_valid_dot_position_for_short_name(name: &str) -> bool {
    let size = name.encode_utf16().count();
    match (name.find('.'), name.rfind('.')) {
        (None, _) => size <= 8,
        (Some(first), Some(last)) if first != last => false,
        (Some(first), _) => {
            let first = name[..first].encode_utf16().count();
            first > 0 && first <= 8 && first + 4 >= size
        }
    }
}

/// `IsShellIntegratedExtension`: .local, .lnk, .scf, .url, and CLSIDs (`{...}`).
fn is_shell_integrated_extension(extension: &str) -> bool {
    let lower = extension.to_ascii_lowercase();
    matches!(lower.as_str(), "local" | "lnk" | "scf" | "url")
        || (lower.starts_with('{') && lower.ends_with('}'))
}

/// `IsReservedNameOnWindows`.
pub fn is_reserved_name_on_windows(name: &str) -> bool {
    const DEVICES: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "clock$",
    ];
    const MAGIC: &[&str] = &["desktop.ini", "thumbs.db", "conin$", "conout$"];
    let lower = name.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches([' ', '.']);
    let prefix = trimmed
        .split_once('.')
        .map_or(trimmed, |(prefix, _)| prefix);
    DEVICES
        .iter()
        .any(|device| trimmed == *device || prefix == *device)
        || MAGIC.contains(&trimmed)
}

/// `FilePath::Extension`'s split: `foo.tar.gz` is `("foo", ".tar.gz")`, `.bashrc` is
/// `("", ".bashrc")`, and a name without a dot has no extension.
pub(crate) fn split_extension(name: &str) -> (&str, &str) {
    const DOUBLE_SUFFIXES: &[&str] = &["bz", "bz2", "gz", "lz", "lzma", "lzo", "xz", "z", "zst"];
    const DOUBLE_EXTENSIONS: &[&str] = &["user.js"];
    if name == "." || name == ".." {
        return (name, "");
    }
    let Some(last) = name.rfind('.') else {
        return (name, "");
    };
    if last == 0 {
        return name.split_at(0);
    }
    let Some(penultimate) = name[..last].rfind('.') else {
        return name.split_at(last);
    };
    if DOUBLE_EXTENSIONS
        .iter()
        .any(|double| name[penultimate + 1..].eq_ignore_ascii_case(double))
    {
        return name.split_at(penultimate);
    }
    let final_extension = &name[last + 1..];
    let gap = last - penultimate;
    if DOUBLE_SUFFIXES
        .iter()
        .any(|suffix| final_extension.eq_ignore_ascii_case(suffix))
        && gap > 1
        && gap <= 5
    {
        return name.split_at(penultimate);
    }
    name.split_at(last)
}
