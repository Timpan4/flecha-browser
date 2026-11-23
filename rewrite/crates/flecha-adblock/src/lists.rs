//! The filter lists and the checks a downloaded or cached copy must pass, ported from
//! `checkedText` in `chromium/shell/build-adblock.mjs`.
//!
//! The TypeScript checks are JavaScript regular expressions without the `u` flag. Their `i` flag
//! folds ASCII letters only (a non-ASCII character never matches an ASCII one), `\d` is ASCII,
//! and `^`/`$` in multiline mode also break at U+2028 and U+2029. Rust's `regex` and
//! `str::trim` differ on each of those, so the checks below are written out by hand.

use std::time::Duration;

use adblock::lists::{ParseOptions, ParsedLine, parse_filter};
use base64::Engine as _;
use md5::{Digest, Md5};

/// A filter list Flecha downloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilterList {
    /// The cache file stem, and the title the list must carry (compared ignoring ASCII case).
    pub id: &'static str,
    pub url: &'static str,
}

/// EasyList and EasyPrivacy, in the order the engine loads them (`LISTS` in build-adblock.mjs).
pub const LISTS: [FilterList; 2] = [
    FilterList {
        id: "easylist",
        url: "https://easylist.to/easylist/easylist.txt",
    },
    FilterList {
        id: "easyprivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
    },
];

/// How long a cached list counts as current: `MAX_AGE_MS` in build-adblock.mjs (four days).
pub const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);

/// Why a list was refused. The messages match build-adblock.mjs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListError {
    InvalidUtf8,
    NotAFilterList,
    WrongTitle,
    ChecksumMismatch,
    Unverifiable,
    NoNetworkRules,
}

impl std::fmt::Display for ListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidUtf8 => "The encoded data was not valid for encoding utf-8",
            Self::NotAFilterList => "not a filter list",
            Self::WrongTitle => "wrong filter list title",
            Self::ChecksumMismatch => "ABP checksum mismatch",
            Self::Unverifiable => {
                "filter list has no verifiable length, compression framing or ABP checksum"
            }
            Self::NoNetworkRules => "filter list contains no supported network rules",
        })
    }
}

impl std::error::Error for ListError {}

/// Decodes and checks one list. `require_checksum` is set for an uncompressed response with no
/// Content-Length, where only an ABP checksum can show the text is complete.
pub fn checked_text(id: &str, bytes: &[u8], require_checksum: bool) -> Result<String, ListError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ListError::InvalidUtf8)?;
    // TextDecoder drops a leading byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !has_header(text) {
        return Err(ListError::NotAFilterList);
    }
    let title = format!("! Title: {id}");
    if !js_lines(text).any(|line| line.eq_ignore_ascii_case(&title)) {
        return Err(ListError::WrongTitle);
    }
    match checksum_line(text) {
        Some(line) => {
            // ABP addChecksum normalizes CR and blank lines and omits the checksum line.
            let normalized = collapse_newlines(&text.replace('\r', ""));
            let normalized = remove_first_checksum_line(&normalized);
            let digest = Md5::digest(normalized.as_bytes());
            let checksum = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
            let stated = line.split(':').nth(1).map(js_trim).unwrap_or_default();
            if checksum != stated {
                return Err(ListError::ChecksumMismatch);
            }
        }
        None if require_checksum => return Err(ListError::Unverifiable),
        None => {}
    }
    let mut lines = text.split(['\r', '\n']).filter(|line| !line.is_empty());
    if lines.any(is_network_rule) {
        Ok(text.to_owned())
    } else {
        Err(ListError::NoNetworkRules)
    }
}

/// Whether adblock-rust reads `line` as a network filter the engine keeps (see
/// [`crate::engine::build_engine`]). build-adblock.mjs asked its own declarativeNetRequest
/// converter; the engine here is adblock-rust, so it decides.
pub(crate) fn is_network_rule(line: &str) -> bool {
    matches!(
        parse_filter(line, false, ParseOptions::default()),
        Ok(ParsedLine::Network(filter)) if !filter.is_redirect()
    )
}

/// `/^\[Adblock Plus(?: \d+(?:\.\d+)*)?\]\r?\n/`, with ASCII digits.
fn has_header(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("[Adblock Plus") else {
        return false;
    };
    let rest = match rest.strip_prefix(' ') {
        Some(version) => {
            let mut parts = version.splitn(2, ']');
            let number = parts.next().unwrap_or_default();
            if !number
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return false;
            }
            match parts.next() {
                Some(rest) => return rest.starts_with('\n') || rest.starts_with("\r\n"),
                None => return false,
            }
        }
        None => rest,
    };
    rest.starts_with("]\n") || rest.starts_with("]\r\n")
}

const JS_LINE_TERMINATORS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];

/// Lines as JavaScript's multiline `^` and `$` see them.
fn js_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(JS_LINE_TERMINATORS)
}

/// `/^[ \t]*checksum[ \t]*:/i` at the start of `line`, with ASCII case folding.
fn starts_with_checksum_label(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('!') else {
        return false;
    };
    let rest = rest.trim_start_matches([' ', '\t']);
    let Some(label) = rest.get(..8) else {
        return false;
    };
    label.eq_ignore_ascii_case("checksum")
        && rest[8..].trim_start_matches([' ', '\t']).starts_with(':')
}

/// The first match of `/^![ \t]*checksum[ \t]*:[^\r\n]*/im`. Lines start after any JavaScript
/// line terminator, but the match runs on to the next CR or LF.
fn checksum_line(text: &str) -> Option<&str> {
    line_starts(text, &JS_LINE_TERMINATORS).find_map(|start| {
        let rest = &text[start..];
        starts_with_checksum_label(rest)
            .then(|| rest.split(['\r', '\n']).next().unwrap_or_default())
    })
}

/// Byte offsets where JavaScript's multiline `^` matches.
fn line_starts<'a>(text: &'a str, terminators: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    std::iter::once(0).chain(
        text.char_indices()
            .filter(|(_, ch)| terminators.contains(ch))
            .map(|(index, ch)| index + ch.len_utf8()),
    )
}

/// `.replace(/\n+/g, '\n')`
fn collapse_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_newline = false;
    for ch in text.chars() {
        if ch == '\n' && previous_newline {
            continue;
        }
        previous_newline = ch == '\n';
        out.push(ch);
    }
    out
}

/// `.replace(/^![ \t]*checksum[ \t]*:[^\n]*\n?/im, '')` on text without CR.
fn remove_first_checksum_line(text: &str) -> String {
    let terminators = ['\n', '\u{2028}', '\u{2029}'];
    let Some(start) =
        line_starts(text, &terminators).find(|&start| starts_with_checksum_label(&text[start..]))
    else {
        return text.to_owned();
    };
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset + 1);
    format!("{}{}", &text[..start], &text[end..])
}

/// `String.prototype.trim`: JavaScript WhiteSpace and LineTerminator. Unlike `str::trim`, it
/// strips U+FEFF and keeps U+0085.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\u{000B}' | '\u{000C}' | '\r' | ' ' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(rules: &str) -> String {
        format!("[Adblock Plus 2.0]\n! Title: EasyList\n{rules}\n")
    }

    #[test]
    fn header_takes_ascii_digit_versions_only() {
        assert!(has_header("[Adblock Plus]\nx"));
        assert!(has_header("[Adblock Plus 2.0]\r\nx"));
        assert!(has_header("[Adblock Plus 3]\n"));
        for bad in [
            "[Adblock Plus 2.]\n",
            "[Adblock Plus .2]\n",
            "[Adblock Plus ]\n",
            "[Adblock Plus 2.0]",
            "[Adblock Plus 2.0]\r",
            "[Adblock Plus \u{0662}]\n",
            "[Adblock Plus \u{ff12}]\n",
            "[adblock plus 2.0]\n",
            " [Adblock Plus 2.0]\n",
        ] {
            assert!(!has_header(bad), "{bad:?}");
        }
    }

    #[test]
    fn title_folds_ascii_case_only() {
        assert!(checked_text("easylist", list("||ads.example^").as_bytes(), false).is_ok());
        let upper = "[Adblock Plus 2.0]\n! TITLE: EASYLIST\n||ads.example^\n";
        assert!(checked_text("easylist", upper.as_bytes(), false).is_ok());
        // U+017F folds to `s` and U+212A to `k` under Unicode rules, never under JavaScript's.
        for title in ["EaſyList", "Ea\u{17f}yList", "EasyLi\u{17f}t"] {
            let text = format!("[Adblock Plus 2.0]\n! Title: {title}\n||ads.example^\n");
            assert_eq!(
                checked_text("easylist", text.as_bytes(), false),
                Err(ListError::WrongTitle)
            );
        }
        let kelvin = "[Adblock Plus 2.0]\n! Title: \u{212a}\n||ads.example^\n";
        assert_eq!(
            checked_text("k", kelvin.as_bytes(), false),
            Err(ListError::WrongTitle)
        );
        let fullwidth = "[Adblock Plus 2.0]\n! Title: ＥａｓｙＬｉｓｔ\n||ads.example^\n";
        assert_eq!(
            checked_text("easylist", fullwidth.as_bytes(), false),
            Err(ListError::WrongTitle)
        );
        // A trailing space is part of the line.
        let spaced = "[Adblock Plus 2.0]\n! Title: EasyList \n||ads.example^\n";
        assert_eq!(
            checked_text("easylist", spaced.as_bytes(), false),
            Err(ListError::WrongTitle)
        );
        // U+2028 ends a line for JavaScript's multiline anchors.
        let separated =
            "[Adblock Plus 2.0]\n! x\u{2028}! Title: EasyList\u{2029}\n||ads.example^\n";
        assert!(checked_text("easylist", separated.as_bytes(), false).is_ok());
    }

    #[test]
    fn bom_is_dropped_and_invalid_utf8_refused() {
        let with_bom = format!("\u{feff}{}", list("||ads.example^"));
        assert_eq!(
            checked_text("easylist", with_bom.as_bytes(), false).unwrap(),
            list("||ads.example^")
        );
        let mut bytes = list("||ads.example^").into_bytes();
        bytes.push(0xff);
        assert_eq!(
            checked_text("easylist", &bytes, false),
            Err(ListError::InvalidUtf8)
        );
    }

    #[test]
    fn checksum_matches_abp_normalization() {
        // From build-adblock.test.mjs: the same text and checksum.
        let signed = "[Adblock Plus 2.0]\r\n! Checksum: z2RKQ5QVHnDHGokvvaKKdg\r\n! Title: EasyList\r\n\r\n||ads.example^\r\n";
        assert!(checked_text("easylist", signed.as_bytes(), true).is_ok());
        let tampered = signed.replace("ads.", "adz.");
        assert_eq!(
            checked_text("easylist", tampered.as_bytes(), true),
            Err(ListError::ChecksumMismatch)
        );
        assert_eq!(
            checked_text("easylist", list("||ads.example^").as_bytes(), true),
            Err(ListError::Unverifiable)
        );
        // U+0085 is not JavaScript whitespace, so it stays part of the stated checksum.
        let nel = signed.replace("dg\r\n", "dg\u{85}\r\n");
        assert_eq!(
            checked_text("easylist", nel.as_bytes(), true),
            Err(ListError::ChecksumMismatch)
        );
        // U+FEFF is, so trim removes it.
        let zwnbsp = signed.replace("dg\r\n", "dg\u{feff}\r\n");
        assert!(checked_text("easylist", zwnbsp.as_bytes(), true).is_ok());
    }

    #[test]
    fn js_trim_differs_from_str_trim() {
        assert_eq!(js_trim("\u{feff} a \u{3000}"), "a");
        assert_eq!(js_trim("\u{85}a\u{85}"), "\u{85}a\u{85}");
        assert_eq!("\u{85}a".trim(), "a");
    }

    #[test]
    fn a_list_needs_one_network_rule() {
        assert_eq!(
            checked_text(
                "easylist",
                list("! only a comment\n##.ad").as_bytes(),
                false
            ),
            Err(ListError::NoNetworkRules)
        );
        assert_eq!(
            checked_text(
                "easylist",
                list("||a.example^$redirect=noopjs").as_bytes(),
                false
            ),
            Err(ListError::NoNetworkRules)
        );
        assert!(checked_text("easylist", list("##.ad\n/banner").as_bytes(), false).is_ok());
    }
}
