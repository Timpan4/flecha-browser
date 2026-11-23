//! The file name in a `Content-Disposition` header, as `net::HttpContentDisposition` reads it
//! (`net/http/http_content_disposition.cc`): `filename*` (RFC 5987) wins over `filename`.
//!
//! Chromium converts any charset ICU knows. This port decodes UTF-8, US-ASCII and ISO-8859-1,
//! the charsets RFC 5987 requires; another charset gives no name, so the caller falls back to
//! the URL. The header arrives as UTF-8 text from CEF, so Chromium's referrer-charset guess
//! for raw non-UTF-8 bytes has nothing to do.

use percent_encoding::percent_decode_str;

/// The decoded file name, or `None` when the header names none Chromium would accept.
pub fn content_disposition_filename(header: &str) -> Option<String> {
    let params = after_disposition_type(header);
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
    let mut parts = split_outside_quotes(params, ';').into_iter();
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

/// `HttpUtil::ValuesIterator`: splits outside double quotes and skips empty values.
fn split_outside_quotes(text: &str, delimiter: char) -> Vec<&str> {
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
        .filter(|part| !part.is_empty())
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
        return Some((decode_charset(charset, &bytes)?, true));
    }
    if word.starts_with("=?") {
        // An encoded word that doesn't close fails the whole value, as in Chromium.
        return None;
    }
    let bytes: Vec<u8> = percent_decode_str(word).collect();
    String::from_utf8(bytes).ok().map(|text| (text, false))
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
    decode_charset(charset, &bytes)
}

fn decode_charset(charset: &str, bytes: &[u8]) -> Option<String> {
    let charset = charset.to_ascii_lowercase();
    match charset.as_str() {
        "utf-8" | "utf8" => String::from_utf8(bytes.to_vec()).ok(),
        "us-ascii" | "ascii" => bytes
            .is_ascii()
            .then(|| String::from_utf8(bytes.to_vec()).ok())
            .flatten(),
        "iso-8859-1" | "latin1" | "l1" => Some(bytes.iter().map(|&b| char::from(b)).collect()),
        _ => None,
    }
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
