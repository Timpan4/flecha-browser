//! Building the adblock-rust engine from list text, and the serialized copy that lets a launch
//! skip parsing.
//!
//! `engine.dat` holds a SHA-256 key over the list texts, the page exception lines and their
//! SHA-256, then adblock-rust's own serialization (which carries its crate version and a
//! checksum). A key that doesn't match the cached lists, damaged page exception lines, or a file
//! adblock-rust refuses means a rebuild from the text.

use std::{fs, io, path::Path};

use adblock::{
    Engine, FilterSet,
    filters::network::{NetworkFilter, NetworkFilterMask},
    lists::{FilterParseError, ParseOptions, ParsedLine, parse_filter},
    request::Request,
};
use sha2::{Digest, Sha256};

use crate::fetch::write_atomically;

const ENGINE_FILE: &str = "engine.dat";
const MAGIC: &[u8] = b"flecha-adblock engine\n";

/// The hash a serialized engine is filed under.
pub type EngineKey = [u8; 32];

/// What [`build_engine`] keeps from a list, and the `engine.dat` layout. Change it whenever
/// either changes, so an `engine.dat` built the old way is rebuilt.
const RECIPE: &[u8] =
    b"flecha-adblock recipe 5: parse errors and redirects left out, ||host:port as regex, exceptions naming document kept as page exceptions and hashed\n";

const NO_REDIRECTS: &str = "redirect and rewrite filters need resources this engine doesn't load";

/// A list line the engine left out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedLine {
    pub list: String,
    /// 1-based, counting CR, LF and CRLF as one break each.
    pub line: usize,
    pub text: String,
    pub error: String,
}

/// The engine, and the pages the lists exempt from all blocking.
pub struct Filters {
    engine: Engine,
    /// `@@…$document` exceptions without their `@@`, so they match as `$document` filters.
    page_exception_lines: String,
    page_exceptions: Engine,
}

impl Filters {
    fn new(engine: Engine, page_exception_lines: String) -> Self {
        let mut set = FilterSet::new(false);
        set.add_filter_list(page_exception_lines.clone(), ParseOptions::default());
        Self {
            engine,
            page_exception_lines,
            page_exceptions: Engine::new_with_filter_set(set),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Whether an `@@…$document` exception covers this page or frame, as the shell's
    /// priority-3 `allowAllRequests` rules did. adblock-rust lets any exception match a document
    /// request (uBlock Origin's rule for letting a page load), so the main engine can't answer
    /// this; only filters that name `$document` count.
    ///
    /// `initiator_url` is the document that loaded the page: the page itself for a tab's
    /// top-level page, and the embedding page for a frame. Party and `$domain=` options match
    /// against it, as the shell's rules matched a frame against its initiator.
    pub fn exempts_page(&self, page_url: &str, initiator_url: &str) -> bool {
        Request::new(page_url, initiator_url, "document", "get").is_ok_and(|request| {
            self.page_exceptions
                .check_network_request(&request)
                .should_block()
        })
    }
}

/// Identifies a set of list texts and the way they're built. Each list contributes its id and
/// length, so moving text from one list to the next changes the key.
pub fn engine_key<'a>(lists: impl IntoIterator<Item = (&'a str, &'a str)>) -> EngineKey {
    let mut hash = Sha256::new();
    hash.update(RECIPE);
    for (id, text) in lists {
        hash.update((id.len() as u64).to_le_bytes());
        hash.update(id.as_bytes());
        hash.update((text.len() as u64).to_le_bytes());
        hash.update(text.as_bytes());
    }
    hash.finalize().into()
}

/// Parses the lists into one engine, in order, and reports the lines it left out: lines that
/// look like filters but don't parse (adblock-rust skips those itself), and `$redirect`,
/// `$redirect-rule` and `$rewrite` filters. Without their resources adblock-rust would cancel
/// a `$redirect` request outright, where the list meant to serve a stub; build-adblock.mjs left
/// them out, and so does this.
pub fn build_engine<'a>(
    lists: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> (Filters, Vec<SkippedLine>) {
    let mut set = FilterSet::new(false);
    let mut skipped = Vec::new();
    let mut page_exceptions = String::new();
    for (id, text) in lists {
        let kept = kept_lines(id, text, &mut skipped, &mut page_exceptions);
        set.add_filter_list(kept, ParseOptions::default());
    }
    let engine = Engine::new_with_filter_set(set);
    (Filters::new(engine, page_exceptions), skipped)
}

/// An exception whose line names `$document`: it exempts whole pages.
///
/// The decision comes from the options the line names, not from adblock-rust's type mask.
/// adblock-rust also gives the document type to an exception shaped `||host^` that names no type
/// (uBlock Origin's implicit page exception) and to `$all`, with or without negated types. The
/// shell made those allow requests to their host and nothing more, and so does the main engine.
/// `$csp` and `$removeparam` filters also carry the document type, but only change what a page
/// gets.
fn is_page_exception(filter: &NetworkFilter, line: &str) -> bool {
    filter.mask.contains(NetworkFilterMask::IS_EXCEPTION)
        && filter.mask.contains(NetworkFilterMask::FROM_DOCUMENT)
        && names_document(line)
        && !filter.is_csp()
        && !filter.is_removeparam()
}

/// Whether the line's options name `document`, or adblock-rust's `doc`, without `~`. adblock-rust
/// takes everything after the last `$` as the options, splits them on `,`, and compares names
/// exactly.
fn names_document(line: &str) -> bool {
    line.trim().rsplit_once('$').is_some_and(|(_, options)| {
        options
            .split(',')
            .any(|option| matches!(option, "document" | "doc"))
    })
}

/// adblock-rust never matches a `||host:port` filter: its hostname anchor ends at the port. Such
/// a filter is rewritten into the regular expression its pattern means in declarativeNetRequest,
/// which the shell matched with: any scheme, the host or a subdomain of it, then the rest of the
/// pattern. `*` matches anything, `^` a separator or the end, and a final `|` the end. The
/// options stay as they were. The expression holds no `$`, because adblock-rust takes the last
/// `$` as the start of the options, and no upper-case escapes, because it lowercases the pattern.
fn port_anchored_as_regex(line: &str) -> Option<String> {
    let (exception, rest) = match line.strip_prefix("@@") {
        Some(rest) => ("@@", rest),
        None => ("", line),
    };
    let rest = rest.strip_prefix("||")?;
    let (pattern, options) = rest
        .rsplit_once('$')
        .map_or((rest, None), |(pattern, options)| (pattern, Some(options)));
    let host = &pattern[..pattern
        .find(['/', '^', '*', '|', '?'])
        .unwrap_or(pattern.len())];
    let (_, port) = host.split_once(':')?;
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (body, end) = pattern
        .strip_suffix('|')
        .map_or((pattern, ""), |body| (body, r"\z"));
    let mut regex = format!("{exception}/^[a-z][a-z0-9+.-]*://(?:[^/?#]*\\.)?");
    for c in body.chars() {
        match c {
            '*' => regex.push_str(".*"),
            '^' => regex.push_str(r"(?:[^0-9a-z_.%-]|\z)"),
            '\\' | '.' | '+' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' => {
                regex.push('\\');
                regex.push(c);
            }
            c => regex.push(c),
        }
    }
    regex.push_str(end);
    regex.push('/');
    if let Some(options) = options {
        regex.push('$');
        regex.push_str(options);
    }
    Some(regex)
}

/// The list without its redirect filters. Comments, blank lines and the header neither parse
/// nor count as errors; adblock-rust calls syntax it doesn't take at all `Unsupported`.
fn kept_lines(
    id: &str,
    text: &str,
    skipped: &mut Vec<SkippedLine>,
    page_exceptions: &mut String,
) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut kept = String::with_capacity(normalized.len());
    for (index, line) in normalized.split(['\n', '\r']).enumerate() {
        let rewritten = port_anchored_as_regex(line);
        let filter_text = rewritten.as_deref().unwrap_or(line);
        let error = match parse_filter(filter_text, false, ParseOptions::default()) {
            Ok(ParsedLine::Network(filter)) if filter.is_redirect() => NO_REDIRECTS.to_owned(),
            Ok(parsed) => {
                if let ParsedLine::Network(filter) = &parsed
                    && is_page_exception(filter, filter_text)
                {
                    page_exceptions.push_str(filter_text.trim().trim_start_matches("@@"));
                    page_exceptions.push('\n');
                }
                kept.push_str(filter_text);
                kept.push('\n');
                continue;
            }
            Err(FilterParseError::Empty | FilterParseError::Unsupported) => {
                kept.push_str(filter_text);
                kept.push('\n');
                continue;
            }
            Err(error) => error.to_string(),
        };
        skipped.push(SkippedLine {
            list: id.to_owned(),
            line: index + 1,
            text: line.to_owned(),
            error,
        });
    }
    kept
}

/// Why a serialized engine was not used.
#[derive(Debug)]
pub enum CachedEngineError {
    Missing,
    Read(io::Error),
    /// Built from other list text, or not written by this crate.
    StaleKey,
    Deserialize(String),
}

impl std::fmt::Display for CachedEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("no serialized engine"),
            Self::Read(error) => write!(f, "could not read the serialized engine: {error}"),
            Self::StaleKey => f.write_str("the serialized engine was built from other lists"),
            Self::Deserialize(error) => write!(f, "could not deserialize the engine: {error}"),
        }
    }
}

impl std::error::Error for CachedEngineError {}

pub fn load_engine(cache: &Path, key: &EngineKey) -> Result<Filters, CachedEngineError> {
    let bytes = match fs::read(cache.join(ENGINE_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CachedEngineError::Missing);
        }
        Err(error) => return Err(CachedEngineError::Read(error)),
    };
    let rest = bytes
        .strip_prefix(MAGIC)
        .and_then(|rest| rest.strip_prefix(key.as_slice()))
        .ok_or(CachedEngineError::StaleKey)?;
    let damaged = |what: &str| CachedEngineError::Deserialize(format!("damaged {what}"));
    let (length, rest) = rest
        .split_first_chunk::<8>()
        .ok_or_else(|| damaged("page exception length"))?;
    let length = usize::try_from(u64::from_le_bytes(*length))
        .ok()
        .filter(|&length| length <= rest.len())
        .ok_or_else(|| damaged("page exception length"))?;
    let (lines, rest) = rest.split_at(length);
    // adblock-rust's checksum covers only its own payload.
    let (digest, payload) = rest
        .split_first_chunk::<32>()
        .ok_or_else(|| damaged("page exceptions"))?;
    if Sha256::digest(lines).as_slice() != digest {
        return Err(damaged("page exceptions"));
    }
    let lines = std::str::from_utf8(lines).map_err(|_| damaged("page exceptions"))?;
    let mut engine = Engine::default();
    engine
        .deserialize(payload)
        .map_err(|error| CachedEngineError::Deserialize(format!("{error:?}")))?;
    Ok(Filters::new(engine, lines.to_owned()))
}

pub fn save_engine(cache: &Path, key: &EngineKey, filters: &Filters) -> io::Result<()> {
    let serialized = filters.engine.serialize();
    let lines = filters.page_exception_lines.as_bytes();
    let digest = Sha256::digest(lines);
    let mut bytes = Vec::with_capacity(
        MAGIC.len() + key.len() + 8 + lines.len() + digest.len() + serialized.len(),
    );
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&(lines.len() as u64).to_le_bytes());
    bytes.extend_from_slice(lines);
    bytes.extend_from_slice(&digest);
    bytes.extend_from_slice(&serialized);
    fs::create_dir_all(cache)?;
    write_atomically(&cache.join(ENGINE_FILE), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which exceptions go into the page-exception engine and `engine.dat`: only those that
    /// name `$document` among their types.
    #[test]
    fn only_exceptions_naming_document_are_page_exceptions() {
        let options = [
            ("", false),
            ("$script", false),
            ("$~script", false),
            ("$all", false),
            ("$subdocument", false),
            ("$document", true),
            ("$document,subdocument", true),
            ("$image,document", true),
            ("$generichide", false),
            ("$third-party", false),
            ("$~third-party", false),
            ("$1p", false),
            ("$domain=news.example", false),
            ("$important", false),
            ("$xmlhttprequest", false),
            ("$csp=script-src 'none'", false),
            // `$all` gives the document type, also with a type negated; the shell ignored it.
            ("$all,~script", false),
            ("$all,~image", false),
            ("$all,document", true),
            ("$doc", true),
            ("$document,~third-party", true),
        ];
        for (option, expected) in options {
            let line = format!("@@||page.example^{option}");
            let Ok(ParsedLine::Network(filter)) =
                parse_filter(&line, false, ParseOptions::default())
            else {
                panic!("{line} did not parse");
            };
            assert_eq!(is_page_exception(&filter, &line), expected, "{line}");
        }
        // A blocking filter never is, `$document` or not.
        let line = "||page.example^$document";
        let Ok(ParsedLine::Network(filter)) = parse_filter(line, false, ParseOptions::default())
        else {
            panic!();
        };
        assert!(!is_page_exception(&filter, line));
    }
}
