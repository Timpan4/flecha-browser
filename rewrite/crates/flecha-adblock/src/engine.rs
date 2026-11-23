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
    filters::network::{FilterPart, NetworkFilter, NetworkFilterMask},
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
    b"flecha-adblock recipe 6: parse errors and redirects left out, ||host:port as regex, exceptions naming document kept as page exceptions with lower-case domains and hashed, page exceptions matching every page left out\n";

const NO_REDIRECTS: &str = "redirect and rewrite filters need resources this engine doesn't load";
const MATCHES_EVERY_PAGE: &str =
    "a page exception with no pattern and no listed domain would exempt every page";

/// A list line the engine left out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedLine {
    pub list: String,
    /// 1-based, counting CR, LF and CRLF as one break each.
    pub line: usize,
    pub text: String,
    pub error: String,
}

/// The document that loaded a page, which party and `$domain=` options match against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Initiator<'a> {
    /// A tab's top-level page. Chromium's declarativeNetRequest decides a main frame's
    /// `allowAllRequests` rules against an opaque initiator, so the shell's rules saw it as
    /// third-party with no domain: a third-party option and an excluded-only `$domain=~` hold,
    /// a first-party option and a listed domain don't.
    None,
    /// A frame whose embedder the caller didn't supply. Only page exceptions without party or
    /// `$domain=` options cover it, so a missing embedder never exempts more.
    Unknown,
    /// The embedding document's URL or origin. One without a host (`about:blank`,
    /// `about:srcdoc`, `blob:`, `data:`, or not a URL) counts as [`Initiator::Unknown`]: such a
    /// document takes its origin from the document that created it, which the URL doesn't name.
    Url(&'a str),
}

/// The engine, and the pages the lists exempt from all blocking.
pub struct Filters {
    engine: Engine,
    /// `@@…$document` exceptions without their `@@`, so they match as `$document` filters.
    page_exception_lines: String,
    /// Every page exception, matched against a known or an opaque initiator.
    with_initiator: Engine,
    /// The page exceptions that have no party or `$domain=` option.
    whatever_initiator: Engine,
}

impl Filters {
    fn new(engine: Engine, page_exception_lines: String) -> Self {
        let whatever: String = page_exception_lines
            .lines()
            .filter(|line| !limited_by_initiator(line))
            .flat_map(|line| [line, "\n"])
            .collect();
        let with_initiator = side_engine(page_exception_lines.clone());
        Self {
            engine,
            page_exception_lines,
            with_initiator,
            whatever_initiator: side_engine(whatever),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Whether an `@@…$document` exception covers this page or frame, as the shell's
    /// priority-3 `allowAllRequests` rules did. adblock-rust lets any exception match a document
    /// request (uBlock Origin's rule for letting a page load), so the main engine can't answer
    /// this; only filters that name `$document` count.
    pub fn exempts_page(&self, page_url: &str, initiator: Initiator<'_>) -> bool {
        // adblock-rust reads a source it can't parse as third-party with no hostname: the opaque
        // initiator declarativeNetRequest gives a main frame.
        let source = match initiator {
            Initiator::None => "",
            Initiator::Unknown => page_url,
            Initiator::Url(url) => url,
        };
        let Ok(request) = Request::new(page_url, source, "document", "get") else {
            return false;
        };
        // The page exceptions without party or `$domain=` options don't depend on the source.
        let engine = match initiator {
            Initiator::None => &self.with_initiator,
            Initiator::Url(_) if request.source_hostname_hashes.is_some() => &self.with_initiator,
            Initiator::Url(_) | Initiator::Unknown => &self.whatever_initiator,
        };
        engine.check_network_request(&request).should_block()
    }
}

fn side_engine(lines: String) -> Engine {
    let mut set = FilterSet::new(false);
    set.add_filter_list(lines, ParseOptions::default());
    Engine::new_with_filter_set(set)
}

/// Whether a page exception holds only for some initiators: it has a party option or a
/// `$domain=` (`$from=`) list, as adblock-rust parsed them. A line that doesn't parse counts as
/// limited, so it never covers a frame with an unknown embedder.
fn limited_by_initiator(line: &str) -> bool {
    let Ok(ParsedLine::Network(filter)) = parse_filter(line, false, ParseOptions::default()) else {
        return true;
    };
    filter.opt_domains.is_some()
        || filter.opt_not_domains.is_some()
        || !filter
            .mask
            .contains(NetworkFilterMask::FIRST_PARTY | NetworkFilterMask::THIRD_PARTY)
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
/// them out, and so does this. Page exceptions that would match every page are left out and
/// reported too, as build-adblock.mjs dropped them.
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

/// A page exception with no hostname, no pattern and no listed domain covers every page. The
/// shell dropped such rules ("Would match everything"), and its `||*` patterns, and so does
/// this. adblock-rust parses `||*` and `||` with an empty hostname.
fn matches_every_page(filter: &NetworkFilter) -> bool {
    filter.hostname.as_deref().is_none_or(str::is_empty)
        && matches!(filter.filter, FilterPart::Empty)
        && filter.opt_domains.is_none()
}

/// The page exception with its `$domain=` (`$from=`) values in lower case, as the shell read
/// them. adblock-rust compares those values with the page's lower-case host as written, so an
/// upper-case `domain=~X` would exclude nothing. Options are split as adblock-rust splits them:
/// after the last `$`, on `,`, with every leading `~` and the value after the first `=` set apart.
fn lowercase_domain_values(line: &str) -> String {
    let Some((pattern, options)) = line.rsplit_once('$') else {
        return line.to_owned();
    };
    let options: Vec<String> = options
        .split(',')
        .map(|option| {
            let name = option.trim_start_matches('~');
            if name.starts_with("domain=") || name.starts_with("from=") {
                option.to_ascii_lowercase()
            } else {
                option.to_owned()
            }
        })
        .collect();
    format!("{pattern}${}", options.join(","))
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

/// The list without its redirect filters and the page exceptions that would match every page.
/// The kept page exceptions go to `page_exceptions`. Comments, blank lines and the header neither parse
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
            Ok(ParsedLine::Network(filter))
                if is_page_exception(&filter, filter_text) && matches_every_page(&filter) =>
            {
                MATCHES_EVERY_PAGE.to_owned()
            }
            Ok(parsed) => {
                if let ParsedLine::Network(filter) = &parsed
                    && is_page_exception(filter, filter_text)
                {
                    let line = filter_text.trim().trim_start_matches("@@");
                    page_exceptions.push_str(&lowercase_domain_values(line));
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
            // adblock-rust takes the last `$` as the start of the options.
            ("/a$b$document", true),
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

    fn filters(list: &str) -> (Filters, Vec<SkippedLine>) {
        build_engine([("easylist", list)])
    }

    /// Party and `$domain=` options on page exceptions, for each kind of initiator: a tab's page
    /// (opaque, as declarativeNetRequest sees a main frame), a known embedder, an embedder URL
    /// with no host, and no embedder. The options are read as adblock-rust parses them.
    #[test]
    fn page_exceptions_against_each_initiator() {
        let page = "https://cdn.a.example/f";
        let own = Initiator::Url("https://www.a.example/");
        let news = Initiator::Url("https://news.example/");
        let hostless = [
            Initiator::Unknown,
            Initiator::Url("about:blank"),
            Initiator::Url("about:srcdoc"),
            Initiator::Url("blob:https://www.a.example/0f1e"),
            Initiator::Url("data:text/html,x"),
            Initiator::Url(""),
            Initiator::Url("not a url"),
        ];
        // (options, exempt as the tab's page, embedded by its own site, embedded by news,
        // with no known embedder)
        let cases = [
            ("$document", true, true, true, true),
            ("$document,~third-party", false, true, false, false),
            ("$document,first-party", false, true, false, false),
            ("$document,third-party", true, false, true, false),
            ("$document,~first-party", true, false, true, false),
            // adblock-rust strips every leading `~`.
            ("$document,~~third-party", false, true, false, false),
            ("$document,~~~third-party", false, true, false, false),
            ("$document,~~first-party", true, false, true, false),
            ("$document,domain=a.example", false, true, false, false),
            ("$document,domain=~news.example", true, true, false, false),
            ("$document,from=~news.example", true, true, false, false),
            ("$document,~~domain=a.example", false, true, false, false),
            // Domains compare in lower case, as the shell read them.
            ("$document,domain=~NEWS.Example", true, true, false, false),
            ("$document,domain=A.EXAMPLE", false, true, false, false),
            ("$document,~~domain=~NEWS.Example", true, true, false, false),
            ("$document,from=~NEWS.Example", true, true, false, false),
        ];
        for (options, tab, by_own, by_news, unknown) in cases {
            let (filters, _) = filters(&format!("@@||cdn.a.example^{options}\n"));
            let exempts = |initiator| filters.exempts_page(page, initiator);
            assert_eq!(exempts(Initiator::None), tab, "{options} as the tab's page");
            assert_eq!(exempts(own), by_own, "{options} embedded by its own site");
            assert_eq!(exempts(news), by_news, "{options} embedded by news");
            for initiator in hostless {
                assert_eq!(exempts(initiator), unknown, "{options} {initiator:?}");
            }
        }
        // Options start after the last `$`; the earlier one is part of the pattern.
        let (filters, _) = filters("@@||a.example/$third-party,y$document\n");
        let page = "https://a.example/$third-party,y";
        assert!(filters.exempts_page(page, Initiator::Unknown));
        assert!(filters.exempts_page(page, Initiator::None));
    }

    /// A page exception with no pattern and no listed domain would exempt every page. The shell
    /// dropped it, and so does `build_engine`, with a report.
    #[test]
    fn page_exceptions_that_match_every_page_are_left_out() {
        let dropped = [
            "@@$document",
            "@@*$document",
            "@@||*$document",
            "@@||$document",
            "@@|*$document",
            "@@$document,~third-party",
            "@@$document,first-party",
            "@@$document,third-party",
            "@@$document,domain=~x.example",
            "@@$document,domain=~x.example|~y.example",
            "@@$document,subdocument",
        ];
        for line in dropped {
            let (filters, skipped) = filters(&format!("{line}\n"));
            assert_eq!(skipped.len(), 1, "{line}");
            assert_eq!(skipped[0].error, MATCHES_EVERY_PAGE, "{line}");
            for initiator in [
                Initiator::None,
                Initiator::Unknown,
                Initiator::Url("https://news.example/"),
                Initiator::Url("https://www.x.example/"),
            ] {
                assert!(
                    !filters.exempts_page("https://news.example/", initiator),
                    "{line} {initiator:?}"
                );
            }
        }
        // A listed domain limits it, as `initiatorDomains` did in the shell.
        let (filters, skipped) = filters("@@$document,domain=x.example\n");
        assert!(skipped.is_empty());
        let x = Initiator::Url("https://www.x.example/");
        assert!(filters.exempts_page("https://f.example/", x));
        assert!(!filters.exempts_page("https://f.example/", Initiator::None));
    }
}
