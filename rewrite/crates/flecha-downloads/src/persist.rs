//! The saved downloads list and its recovery, shaped like `flecha_core::storage`: the
//! application reads and writes the slots; this module decides what goes in them.
//!
//! Each slot holds the snapshot JSON as a string, as `flecha-services`' `StateStore` writes
//! core's. A damaged current slot is set aside before anything replaces it, the last-good copy
//! takes over if it reads, and a newer schema is an error so an older binary can't overwrite
//! it. Downloads in progress at shutdown come back interrupted with reason `CRASH`, as
//! `DownloadDatabase::EnsureInProgressEntriesCleanedUp` does.
//!
//! Schema 1:
//!
//! ```json
//! {
//!   "version": 1,
//!   "nextId": 2,
//!   "lastPromptFolder": "/home/me/Pictures",
//!   "downloads": [{
//!     "id": 2, "url": "https://example.com/a.pdf", "path": "/home/me/Downloads/a.pdf",
//!     "received": 10, "total": 10, "state": "complete", "startedAt": 1764000000000
//!   }]
//! }
//! ```
//!
//! `state` is `complete`, `cancelled` or `interrupted` (with `interruptReason`, Chromium's
//! name such as `NETWORK_FAILED`); `inProgress` and `paused` are read as interrupted. Paths are
//! stored as UTF-8; a non-UTF-8 path is stored lossily.

use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

use flecha_core::EpochMillis;
pub use flecha_core::storage::Recovery;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::list::{Download, DownloadId, DownloadList, InterruptReason, State};

pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    InvalidShape,
    /// The saved list names a newer schema, spelled as it was saved.
    UnsupportedVersion(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid downloads JSON: {error}"),
            Self::InvalidShape => f.write_str("downloads list has duplicate or unknown ids"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported downloads schema {version}")
            }
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    version: u64,
    next_id: DownloadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_prompt_folder: Option<String>,
    downloads: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: DownloadId,
    url: String,
    path: String,
    received: u64,
    total: u64,
    state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interrupt_reason: Option<InterruptReason>,
    started_at: EpochMillis,
}

pub fn encode(list: &DownloadList) -> serde_json::Result<String> {
    let snapshot = Snapshot {
        version: SCHEMA_VERSION,
        next_id: list.next_id,
        last_prompt_folder: list
            .last_prompt_folder
            .as_ref()
            .map(|folder| folder.to_string_lossy().into_owned()),
        downloads: list
            .entries
            .iter()
            .map(|entry| Record {
                id: entry.id,
                url: entry.url.clone(),
                path: entry.path.to_string_lossy().into_owned(),
                received: entry.received,
                total: entry.total,
                state: entry.state,
                interrupt_reason: entry.interrupt_reason,
                started_at: entry.started_at,
            })
            .collect(),
    };
    serde_json::to_string_pretty(&snapshot)
}

pub fn decode(json: &str) -> Result<DownloadList, DecodeError> {
    // Core's check, so every spelling core refuses as newer is refused here too.
    if let Some(version) = flecha_core::storage::newer_schema_version(json, SCHEMA_VERSION) {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    let snapshot: Snapshot = serde_json::from_str(json).map_err(DecodeError::Json)?;
    let mut ids = HashSet::new();
    if !snapshot
        .downloads
        .iter()
        .all(|record| record.id != 0 && record.id <= snapshot.next_id && ids.insert(record.id))
    {
        return Err(DecodeError::InvalidShape);
    }
    let entries = snapshot
        .downloads
        .into_iter()
        .map(|record| {
            let (state, interrupt_reason) = match record.state {
                State::InProgress | State::Paused => {
                    (State::Interrupted, Some(InterruptReason::Crash))
                }
                state => (state, record.interrupt_reason),
            };
            Download {
                id: record.id,
                url: record.url,
                path: PathBuf::from(record.path),
                received: record.received,
                total: record.total,
                state,
                interrupt_reason,
                started_at: record.started_at,
                warning: None,
                prompted: false,
            }
        })
        .collect();
    Ok(DownloadList::restored(
        entries,
        snapshot.next_id,
        snapshot.last_prompt_folder.map(PathBuf::from),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    Current,
    LastGood,
    Damaged,
}

/// A value to store in a slot, exactly as given.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotWrite {
    slot: Slot,
    value: Value,
}

impl SlotWrite {
    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// The startup list, with its current-slot writer gated on recovery.
#[derive(Debug)]
pub struct Loaded {
    ready: Ready,
    damaged_copy: Option<SlotWrite>,
    recovered_current: Option<SlotWrite>,
}

#[derive(Debug)]
pub struct Ready {
    pub list: DownloadList,
    pub recovered: Option<Recovery>,
    last_good_after_start: Option<SlotWrite>,
}

impl Loaded {
    /// The writer, once no recovery write is pending.
    pub fn ready(&mut self) -> Option<&mut Ready> {
        if self.damaged_copy.is_some() || self.recovered_current.is_some() {
            return None;
        }
        Some(&mut self.ready)
    }

    /// Persists recovery writes: the damaged copy first, then the restored current slot.
    /// `write` returns success only once the value is saved; after a failure another call
    /// resumes at the failed write.
    pub fn apply_recovery<E>(
        &mut self,
        mut write: impl FnMut(&SlotWrite) -> Result<(), E>,
    ) -> Result<&mut Ready, E> {
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

impl Ready {
    pub fn save(&self, list: &DownloadList) -> serde_json::Result<SlotWrite> {
        Ok(SlotWrite {
            slot: Slot::Current,
            value: Value::String(encode(list)?),
        })
    }

    /// The last-good write, once the application has started with the loaded list.
    pub fn started(&mut self) -> Option<SlotWrite> {
        self.last_good_after_start.take()
    }
}

pub fn load(current: Option<&Value>, last_good: Option<&Value>) -> Result<Loaded, DecodeError> {
    let mut loaded = Loaded {
        ready: Ready {
            list: DownloadList::new(),
            recovered: None,
            last_good_after_start: None,
        },
        damaged_copy: None,
        recovered_current: None,
    };
    let Some(current) = current else {
        return Ok(loaded);
    };
    if let Some(list) = readable(current)? {
        let saved = Value::String(encode(&list).map_err(DecodeError::Json)?);
        if last_good != Some(&saved) && !last_good.is_some_and(needs_newer_reader) {
            loaded.ready.last_good_after_start = Some(SlotWrite {
                slot: Slot::LastGood,
                value: saved,
            });
        }
        loaded.ready.list = list;
        return Ok(loaded);
    }

    loaded.damaged_copy = Some(SlotWrite {
        slot: Slot::Damaged,
        value: current.clone(),
    });
    match last_good.map_or(Ok(None), readable) {
        Ok(Some(list)) => {
            loaded.ready.list = list;
            loaded.ready.recovered = Some(Recovery::LastGood);
            loaded.recovered_current = last_good.map(|value| SlotWrite {
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

fn readable(value: &Value) -> Result<Option<DownloadList>, DecodeError> {
    match value.as_str().map(decode) {
        Some(Ok(list)) => Ok(Some(list)),
        Some(Err(error @ DecodeError::UnsupportedVersion(_))) => Err(error),
        _ => Ok(None),
    }
}

fn needs_newer_reader(value: &Value) -> bool {
    matches!(readable(value), Err(DecodeError::UnsupportedVersion(_)))
}
