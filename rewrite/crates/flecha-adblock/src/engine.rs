//! Building the adblock-rust engine from list text, and the serialized copy that lets a launch
//! skip parsing.
//!
//! `engine.dat` holds a SHA-256 key over the list texts, then adblock-rust's own serialization
//! (which carries its crate version and a checksum). A key that doesn't match the cached lists,
//! or a file adblock-rust refuses, means a rebuild from the text.

use std::{fs, io, path::Path};

use adblock::{
    Engine, FilterSet,
    lists::{FilterParseError, ParseOptions, ParsedLine, parse_filter},
};
use sha2::{Digest, Sha256};

use crate::fetch::write_atomically;

const ENGINE_FILE: &str = "engine.dat";
const MAGIC: &[u8] = b"flecha-adblock engine\n";

/// The hash a serialized engine is filed under.
pub type EngineKey = [u8; 32];

/// What [`build_engine`] keeps from a list. Change it whenever that changes, so an `engine.dat`
/// built the old way is rebuilt.
const RECIPE: &[u8] = b"flecha-adblock recipe 1: parse errors and redirects left out\n";

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
) -> (Engine, Vec<SkippedLine>) {
    let mut set = FilterSet::new(false);
    let mut skipped = Vec::new();
    for (id, text) in lists {
        set.add_filter_list(kept_lines(id, text, &mut skipped), ParseOptions::default());
    }
    (Engine::new_with_filter_set(set), skipped)
}

/// The list without its redirect filters. Comments, blank lines and the header neither parse
/// nor count as errors; adblock-rust calls syntax it doesn't take at all `Unsupported`.
fn kept_lines(id: &str, text: &str, skipped: &mut Vec<SkippedLine>) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut kept = String::with_capacity(normalized.len());
    for (index, line) in normalized.split(['\n', '\r']).enumerate() {
        let error = match parse_filter(line, false, ParseOptions::default()) {
            Ok(ParsedLine::Network(filter)) if filter.is_redirect() => NO_REDIRECTS.to_owned(),
            Ok(_) | Err(FilterParseError::Empty | FilterParseError::Unsupported) => {
                kept.push_str(line);
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

pub fn load_engine(cache: &Path, key: &EngineKey) -> Result<Engine, CachedEngineError> {
    let bytes = match fs::read(cache.join(ENGINE_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CachedEngineError::Missing);
        }
        Err(error) => return Err(CachedEngineError::Read(error)),
    };
    let payload = bytes
        .strip_prefix(MAGIC)
        .and_then(|rest| rest.strip_prefix(key.as_slice()))
        .ok_or(CachedEngineError::StaleKey)?;
    let mut engine = Engine::default();
    engine
        .deserialize(payload)
        .map_err(|error| CachedEngineError::Deserialize(format!("{error:?}")))?;
    Ok(engine)
}

pub fn save_engine(cache: &Path, key: &EngineKey, engine: &Engine) -> io::Result<()> {
    let serialized = engine.serialize();
    let mut bytes = Vec::with_capacity(MAGIC.len() + key.len() + serialized.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&serialized);
    fs::create_dir_all(cache)?;
    write_atomically(&cache.join(ENGINE_FILE), &bytes)
}
