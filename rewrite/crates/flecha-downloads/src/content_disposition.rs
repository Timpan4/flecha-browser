//! The file name in a `Content-Disposition` header, as `net::HttpContentDisposition` reads it
//! (`net/http/http_content_disposition.cc`): `filename*` (RFC 5987) wins over `filename`.
//!
//! Chromium converts any charset its ICU knows. This port decodes the two charsets RFC 5987
//! requires, under every label Chromium's ICU gives them (`third_party/icu/source/data/
//! mappings/convrtrs.txt`): UTF-8, and `windows-1252-html`, which also answers to ISO-8859-1
//! and US-ASCII. Another charset gives no name, so the caller falls back to the URL. The
//! header arrives as UTF-8 text from CEF, so Chromium's referrer-charset guess for raw
//! non-UTF-8 bytes has nothing to do.

use percent_encoding::percent_decode_str;
use unicode_normalization::UnicodeNormalization;

/// The decoded file name, or `None` when the header names none Chromium would accept.
pub fn content_disposition_filename(header: &str) -> Option<String> {
    // The string constructor reads only the first comma-separated value.
    let first = split_outside_quotes(header, ',', false)
        .into_iter()
        .next()
        .unwrap_or_default();
    let params = after_disposition_type(first);
    let mut filename = String::new();
    let mut ext_filename = String::new();
    for (name, value) in pairs(params) {
        if filename.is_empty() && name.eq_ignore_ascii_case("filename") {
            if let Some(decoded) = decode_filename_value(&value.unquoted()) {
                filename = decoded;
            }
        } else if ext_filename.is_empty()
            && name.eq_ignore_ascii_case("filename*")
            && let Some(decoded) = decode_ext_value(value.raw)
        {
            ext_filename = decoded;
        }
    }
    let chosen = if ext_filename.is_empty() {
        filename
    } else {
        ext_filename
    };
    (!chosen.is_empty()).then_some(chosen)
}

/// `ConsumeDispositionType`: without a token before the first `;`, every part is a parameter.
fn after_disposition_type(header: &str) -> &str {
    let (kind, rest) = header.split_once(';').unwrap_or((header, ""));
    let kind = trim_lws(kind);
    if kind.is_empty() || !kind.bytes().all(is_token_byte) {
        return header;
    }
    rest
}

struct Value<'a> {
    raw: &'a str,
}

impl Value<'_> {
    /// `HttpUtil::NameValuePairsIterator`'s lenient unquoting.
    fn unquoted(&self) -> String {
        let raw = self.raw;
        if !raw.starts_with('"') {
            return raw.into();
        }
        if raw.len() == 1 || !raw.ends_with('"') {
            // A missing closing quote is forgiven (crbug.com/39836).
            return raw[1..].into();
        }
        let mut out = String::new();
        let mut escaped = false;
        for c in raw[1..raw.len() - 1].chars() {
            if c == '\\' && !escaped {
                escaped = true;
                continue;
            }
            escaped = false;
            out.push(c);
        }
        out
    }
}

/// `NameValuePairsIterator(params, ';')`: stops at the first malformed pair.
fn pairs(params: &str) -> impl Iterator<Item = (&str, Value<'_>)> {
    let mut parts = split_outside_quotes(params, ';', true).into_iter();
    std::iter::from_fn(move || {
        let part = parts.next()?;
        let equals = part.find('=')?;
        if equals == 0 {
            return None;
        }
        let name = trim_lws(&part[..equals]);
        if name.contains('"') {
            return None;
        }
        let value = trim_lws(&part[equals + 1..]);
        if value.is_empty() {
            return None;
        }
        Some((name, Value { raw: value }))
    })
}

/// `HttpUtil::ValuesIterator`: splits outside double quotes and trims each value.
fn split_outside_quotes(text: &str, delimiter: char, ignore_empty: bool) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        if in_quotes {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_quotes = false;
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delimiter {
            parts.push(&text[start..index]);
            start = index + c.len_utf8();
        }
    }
    parts.push(&text[start..]);
    parts
        .into_iter()
        .map(trim_lws)
        .filter(|part| !ignore_empty || !part.is_empty())
        .collect()
}

/// `DecodeFilenameValue`: whitespace-separated words, each RFC 2047 or percent-encoded.
fn decode_filename_value(input: &str) -> Option<String> {
    let mut decoded = String::new();
    let mut previous_rfc2047 = true;
    let mut rest = input;
    while !rest.is_empty() {
        let delimiter = rest.find([' ', '\t', '\n', '\r']).unwrap_or(rest.len());
        if delimiter == 0 {
            if !previous_rfc2047 {
                decoded.push(' ');
            }
            rest = &rest[1..];
            continue;
        }
        let (word, after) = rest.split_at(delimiter);
        let (text, rfc2047) = decode_word(word)?;
        previous_rfc2047 = rfc2047;
        decoded.push_str(&text);
        rest = after;
    }
    Some(decoded)
}

/// `DecodeWord`: returns the text and whether it was an RFC 2047 encoded word.
fn decode_word(word: &str) -> Option<(String, bool)> {
    if !word.is_ascii() {
        return Some((word.into(), false));
    }
    if let Some(body) = word.strip_prefix("=?").and_then(|w| w.strip_suffix("?=")) {
        let mut parts = body.splitn(3, '?');
        let (Some(charset), Some(encoding), Some(text)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        if text.contains('?') {
            return None;
        }
        let bytes = match encoding {
            "b" | "B" => decode_base64(text)?,
            "q" | "Q" => decode_q(text)?,
            _ => return None,
        };
        if bytes.is_empty() {
            // DecodeBQEncoding returns an empty word before looking at the charset.
            return Some((String::new(), true));
        }
        // ConvertToUtf8 keeps ICU's default substitution for malformed input.
        return Some((
            decode_charset(charset, &bytes, Malformed::Substitute)?,
            true,
        ));
    }
    if word.starts_with("=?") {
        // An encoded word that doesn't close fails the whole value, as in Chromium.
        return None;
    }
    let bytes: Vec<u8> = percent_decode_str(word).collect();
    String::from_utf8(bytes)
        .ok()
        .filter(|text| text.chars().all(is_valid_character))
        .map(|text| (text, false))
}

/// `base::IsValidCharacter`, which `base::IsStringUTF8` applies: no noncharacters.
fn is_valid_character(c: char) -> bool {
    let code = u32::from(c);
    !(0xFDD0..=0xFDEF).contains(&code) && code & 0xFFFE != 0xFFFE
}

/// `DecodeExtValue`: `charset'language'percent-encoded`.
fn decode_ext_value(raw: &str) -> Option<String> {
    if raw.contains('"') {
        return None;
    }
    let mut parts = raw.split('\'');
    let (Some(charset), Some(_language), Some(value), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if charset.is_empty() || value.is_empty() {
        return None;
    }
    if !value.is_ascii() {
        return Some(String::new());
    }
    let bytes: Vec<u8> = percent_decode_str(value).collect();
    // ConvertToUtf8AndNormalize: fail on malformed input, then NFC.
    decode_charset(charset, &bytes, Malformed::Fail).map(|text| text.nfc().collect())
}

#[derive(Clone, Copy)]
enum Malformed {
    Fail,
    Substitute,
}

/// The `UTF-8` entry in Chromium's `convrtrs.txt`.
const UTF_8_LABELS: &[&str] = &["UTF-8", "unicode-1-1-utf-8", "utf8"];

/// The `windows-1252-html` entry in Chromium's `convrtrs.txt`.
const WINDOWS_1252_LABELS: &[&str] = &[
    "windows-1252-html",
    "windows-1252",
    "ansi_x3.4-1968",
    "ascii",
    "cp1252",
    "cp819",
    "csisolatin1",
    "ibm819",
    "iso-8859-1",
    "iso-ir-100",
    "iso8859-1",
    "iso88591",
    "iso_8859-1",
    "iso_8859-1:1987",
    "l1",
    "latin1",
    "us-ascii",
    "x-cp1252",
];

/// `windows-1252-html.ucm` for bytes 0x80 to 0x9F; every other byte is its own code point.
const WINDOWS_1252_80_9F: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

fn decode_charset(label: &str, bytes: &[u8], malformed: Malformed) -> Option<String> {
    let key = icu_alias_key(label);
    let is = |labels: &[&str]| labels.iter().any(|known| icu_alias_key(known) == key);
    if is(UTF_8_LABELS) {
        return match malformed {
            Malformed::Fail => String::from_utf8(bytes.to_vec()).ok(),
            Malformed::Substitute => Some(String::from_utf8_lossy(bytes).into_owned()),
        };
    }
    if is(WINDOWS_1252_LABELS) {
        return Some(
            bytes
                .iter()
                .map(|&byte| match byte {
                    0x80..=0x9F => WINDOWS_1252_80_9F[usize::from(byte - 0x80)],
                    _ => char::from(byte),
                })
                .collect(),
        );
    }
    None
}

/// `ucnv_io_stripASCIIForCompare`: ICU matches converter names ignoring case, everything but
/// letters and digits, and a zero that starts a number.
fn icu_alias_key(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut key = String::with_capacity(bytes.len());
    let mut after_digit = false;
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'0' => {
                let next_is_digit = bytes.get(index + 1).is_some_and(u8::is_ascii_digit);
                if !after_digit && next_is_digit {
                    continue;
                }
                key.push('0');
            }
            b'1'..=b'9' => {
                after_digit = true;
                key.push(char::from(byte));
            }
            _ if byte.is_ascii_alphabetic() => {
                after_digit = false;
                key.push(char::from(byte.to_ascii_lowercase()));
            }
            _ => after_digit = false,
        }
    }
    key
}

fn decode_q(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'_' => out.push(b' '),
            b'=' => {
                let hex = text.get(index + 1..index + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 2;
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    Some(out)
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let trimmed = text.trim_end_matches('=');
    if !text.len().is_multiple_of(4) || text.len() - trimmed.len() > 2 {
        return None;
    }
    let mut out = Vec::new();
    for chunk in trimmed.as_bytes().chunks(4) {
        let mut value = 0u32;
        for (position, &byte) in chunk.iter().enumerate() {
            value |= sextet(byte)? << (18 - 6 * position);
        }
        let produced = chunk.len() * 6 / 8;
        out.extend_from_slice(&value.to_be_bytes()[1..=produced]);
    }
    Some(out)
}

fn trim_lws(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

/// RFC 7230 `tchar`.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}
