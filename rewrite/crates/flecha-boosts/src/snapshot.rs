//! The rewrite's Boosts file: a versioned JSON snapshot with a last-good copy.
//!
//! This follows `flecha_core::storage`. The app owns reads, writes and startup. It keeps the
//! current and last-good snapshots as JSON strings, and this module decides what to load. A
//! damaged current snapshot is set aside before anything replaces it, and a snapshot from a
//! newer Flecha is refused so an older one can't overwrite it.

use std::cmp::Ordering;
use std::fmt;

pub use flecha_core::storage::Recovery;
use flecha_core::storage::{StoredVersion, read_schema_version};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::compile::CompiledBoost;
use crate::name::BoostName;
use crate::store::{Boosts, Entry};

/// The first Boosts file format. Version 1 is the first rewrite format. The shell's
/// `chrome.storage.local` keys are read by [`Boosts::from_shell`] instead.
pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    InvalidShape,
    UnsupportedVersion(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid Boosts JSON: {error}"),
            Self::InvalidShape => f.write_str("Boosts file has no version or Boost list"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported Boosts schema {version}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

/// What decoding left out of a readable snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotRepairs {
    /// Entries without a valid name and string code, or repeating an earlier name.
    pub skipped_boosts: usize,
    /// Builds that weren't an object of strings. The Boost is kept without a build.
    pub dropped_builds: usize,
}

#[derive(Debug)]
pub struct DecodedBoosts {
    pub boosts: Boosts,
    pub repairs: SnapshotRepairs,
}

pub fn encode(boosts: &Boosts) -> String {
    #[derive(Serialize)]
    struct Snapshot<'a> {
        version: u64,
        boosts: Vec<SavedBoost<'a>>,
    }

    #[derive(Serialize)]
    struct SavedBoost<'a> {
        name: &'a BoostName,
        code: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        compiled: Option<&'a CompiledBoost>,
    }

    let boosts = boosts
        .iter()
        .map(|(name, code, compiled)| SavedBoost {
            name,
            code,
            compiled,
        })
        .collect();
    serde_json::to_string_pretty(&Snapshot {
        version: SCHEMA_VERSION,
        boosts,
    })
    .expect("Boosts serialize to JSON")
}

pub fn decode(json: &str) -> Result<DecodedBoosts, DecodeError> {
    // Read the version alone first, so a newer file is refused whatever else it holds.
    match read_schema_version(json, SCHEMA_VERSION).map_err(DecodeError::Json)? {
        Some(StoredVersion {
            order: Ordering::Equal,
            ..
        }) => {}
        Some(StoredVersion {
            order: Ordering::Greater,
            text,
        }) => return Err(DecodeError::UnsupportedVersion(text)),
        _ => return Err(DecodeError::InvalidShape),
    }
    let document: Value = serde_json::from_str(json).map_err(DecodeError::Json)?;
    let fields = document.as_object().ok_or(DecodeError::InvalidShape)?;
    let saved = fields
        .get("boosts")
        .and_then(Value::as_array)
        .ok_or(DecodeError::InvalidShape)?;
    let mut decoded = DecodedBoosts {
        boosts: Boosts::new(),
        repairs: SnapshotRepairs::default(),
    };
    for entry in saved {
        let Some((name, entry)) = saved_entry(entry, &mut decoded.repairs) else {
            decoded.repairs.skipped_boosts += 1;
            continue;
        };
        if decoded.boosts.entries.contains_key(&name) {
            decoded.repairs.skipped_boosts += 1;
            continue;
        }
        decoded.boosts.entries.insert(name, entry);
    }
    Ok(decoded)
}

fn saved_entry(value: &Value, repairs: &mut SnapshotRepairs) -> Option<(BoostName, Entry)> {
    let fields = value.as_object()?;
    let name = BoostName::parse(fields.get("name")?.as_str()?)?;
    let code = fields.get("code")?.as_str()?.to_owned();
    let compiled = match fields.get("compiled") {
        None => None,
        Some(build) => {
            let build = compiled_boost(build);
            repairs.dropped_builds += usize::from(build.is_none());
            build
        }
    };
    Some((name, Entry { code, compiled }))
}

/// A build object whose `code` and `error`, where present, are strings.
pub(crate) fn compiled_boost(value: &Value) -> Option<CompiledBoost> {
    let fields: &Map<String, Value> = value.as_object()?;
    let text = |key| match fields.get(key) {
        None => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    };
    Some(CompiledBoost {
        code: text("code")?,
        error: text("error")?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    Current,
    LastGood,
    Damaged,
}

/// A value for the app to store in `slot`.
#[derive(Clone, Debug, PartialEq)]
pub struct BackupWrite {
    slot: Slot,
    value: Value,
}

impl BackupWrite {
    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// Startup Boosts whose current-slot writer is gated on recovery.
///
/// ```compile_fail
/// # use flecha_boosts::snapshot::load;
/// # use flecha_boosts::Boosts;
/// let loaded = load(None, None).unwrap();
/// loaded.save_current(&Boosts::new());
/// ```
#[derive(Debug)]
pub struct LoadedBoosts {
    ready: ReadyBoosts,
    damaged_copy: Option<BackupWrite>,
    recovered_current: Option<BackupWrite>,
}

#[derive(Debug)]
pub struct ReadyBoosts {
    pub boosts: Boosts,
    pub recovered: Option<Recovery>,
    pub repairs: SnapshotRepairs,
    last_good_after_start: Option<BackupWrite>,
}

impl LoadedBoosts {
    /// The current-slot writer, once no recovery write is pending.
    pub fn ready(&mut self) -> Option<&mut ReadyBoosts> {
        if self.damaged_copy.is_some() || self.recovered_current.is_some() {
            return None;
        }
        Some(&mut self.ready)
    }

    /// Persist recovery writes. Return success from the writer only after the value is saved.
    /// On failure, another call resumes at the failed write.
    pub fn apply_recovery<E>(
        &mut self,
        mut write: impl FnMut(&BackupWrite) -> Result<(), E>,
    ) -> Result<&mut ReadyBoosts, E> {
        if let Some(damaged) = &self.damaged_copy {
            write(damaged)?;
            self.damaged_copy = None;
        }
        if let Some(current) = &self.recovered_current {
            write(current)?;
            self.recovered_current = None;
        }
        Ok(&mut self.ready)
    }
}

impl ReadyBoosts {
    pub fn save_current(&self, boosts: &Boosts) -> BackupWrite {
        BackupWrite {
            slot: Slot::Current,
            value: Value::String(encode(boosts)),
        }
    }

    /// Promote the repaired startup snapshot only after the app starts successfully.
    pub fn started(&mut self) -> Option<BackupWrite> {
        self.last_good_after_start.take()
    }
}

/// Read the two stored JSON strings, or keep the exact damaged value if it wasn't readable.
/// A newer schema is an error, so an older Flecha can't overwrite it.
pub fn load(saved: Option<&Value>, last_good: Option<&Value>) -> Result<LoadedBoosts, DecodeError> {
    let mut loaded = LoadedBoosts {
        ready: ReadyBoosts {
            boosts: Boosts::new(),
            recovered: None,
            repairs: SnapshotRepairs::default(),
            last_good_after_start: None,
        },
        damaged_copy: None,
        recovered_current: None,
    };
    let Some(saved) = saved else {
        return Ok(loaded);
    };
    if let Some(decoded) = readable(saved)? {
        let repaired = Value::String(encode(&decoded.boosts));
        if last_good != Some(&repaired) && !last_good.is_some_and(needs_newer_reader) {
            loaded.ready.last_good_after_start = Some(BackupWrite {
                slot: Slot::LastGood,
                value: repaired,
            });
        }
        loaded.ready.boosts = decoded.boosts;
        loaded.ready.repairs = decoded.repairs;
        return Ok(loaded);
    }

    loaded.damaged_copy = Some(BackupWrite {
        slot: Slot::Damaged,
        value: saved.clone(),
    });
    match last_good.map_or(Ok(None), readable) {
        Ok(Some(fallback)) => {
            loaded.ready.boosts = fallback.boosts;
            loaded.ready.repairs = fallback.repairs;
            loaded.ready.recovered = Some(Recovery::LastGood);
            loaded.recovered_current = last_good.map(|value| BackupWrite {
                slot: Slot::Current,
                value: value.clone(),
            });
        }
        Ok(None) => loaded.ready.recovered = Some(Recovery::Fresh),
        Err(DecodeError::UnsupportedVersion(_)) => {
            loaded.ready.recovered = Some(Recovery::UnsupportedBackup)
        }
        Err(error) => return Err(error),
    }
    Ok(loaded)
}

fn readable(value: &Value) -> Result<Option<DecodedBoosts>, DecodeError> {
    match value.as_str().map(decode) {
        Some(Ok(decoded)) => Ok(Some(decoded)),
        Some(Err(error @ DecodeError::UnsupportedVersion(_))) => Err(error),
        _ => Ok(None),
    }
}

fn needs_newer_reader(value: &Value) -> bool {
    matches!(
        value.as_str().map(decode),
        Some(Err(DecodeError::UnsupportedVersion(_)))
    )
}
