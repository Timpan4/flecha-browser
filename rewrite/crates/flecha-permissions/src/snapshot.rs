use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use std::{fmt, process};

use serde_json::{Map, Value};

use crate::kind::{PermissionKind, Verdict};
use crate::manager::{Grants, StoredPartition};
use crate::origin::Origin;

/// The snapshot format this build reads and writes.
pub const SCHEMA_VERSION: u64 = 1;

const SHARED_KEY: &str = "shared";
const ISOLATED_PREFIX: &str = "isolated:";

/// Saved decisions of the persistent partitions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub(crate) partitions: BTreeMap<StoredPartition, Grants>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub snapshot: Snapshot,
    /// Entries dropped because their partition, origin, permission or decision is invalid, or
    /// their origin is not a secure context.
    pub skipped_entries: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The document is not a snapshot of any version.
    Damaged(String),
    /// A newer build wrote the document. `found` is its version as written.
    NewerSchema(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Damaged(why) => write!(f, "damaged permission snapshot: {why}"),
            Self::NewerSchema(found) => write!(f, "permission snapshot schema {found} is newer"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Snapshot {
    /// The document in sorted key order.
    pub fn encode(&self) -> String {
        let mut partitions = Map::new();
        for (partition, grants) in &self.partitions {
            if grants.is_empty() {
                continue;
            }
            let key = match partition {
                StoredPartition::Shared => SHARED_KEY.to_owned(),
                StoredPartition::Isolated(id) => format!("{ISOLATED_PREFIX}{id}"),
            };
            let origins = grants
                .iter()
                .map(|(origin, decided)| {
                    let decided = decided
                        .iter()
                        .map(|(kind, verdict)| {
                            let verdict = match verdict {
                                Verdict::Allow => "allow",
                                Verdict::Block => "block",
                            };
                            (kind.name().to_owned(), Value::from(verdict))
                        })
                        .collect::<Map<_, _>>();
                    (origin.as_str().to_owned(), Value::Object(decided))
                })
                .collect::<Map<_, _>>();
            partitions.insert(key, Value::Object(origins));
        }
        let mut document = Map::new();
        document.insert("partitions".to_owned(), Value::Object(partitions));
        document.insert("version".to_owned(), Value::from(SCHEMA_VERSION));
        let mut json = serde_json::to_string_pretty(&Value::Object(document))
            .expect("JSON values with string keys always serialize");
        json.push('\n');
        json
    }

    /// Reads a document. The version is checked before anything else, so a newer document is
    /// reported as newer even when this build cannot read its shape.
    pub fn decode(json: &str) -> Result<Decoded, DecodeError> {
        let document: Value =
            serde_json::from_str(json).map_err(|error| DecodeError::Damaged(error.to_string()))?;
        let Value::Object(document) = document else {
            return Err(DecodeError::Damaged("not an object".to_owned()));
        };
        match document.get("version") {
            Some(Value::Number(version)) if version.as_u64() == Some(SCHEMA_VERSION) => {}
            Some(Value::Number(version))
                if version
                    .as_f64()
                    .is_some_and(|found| found > SCHEMA_VERSION as f64) =>
            {
                return Err(DecodeError::NewerSchema(version.to_string()));
            }
            _ => return Err(DecodeError::Damaged("no supported version".to_owned())),
        }
        let Some(Value::Object(partitions)) = document.get("partitions") else {
            return Err(DecodeError::Damaged("no partitions object".to_owned()));
        };

        let mut snapshot = Snapshot::default();
        let mut skipped = 0;
        for (key, origins) in partitions {
            let partition = if key == SHARED_KEY {
                Some(StoredPartition::Shared)
            } else {
                key.strip_prefix(ISOLATED_PREFIX)
                    .filter(|id| !id.is_empty())
                    .map(|id| StoredPartition::Isolated(id.to_owned()))
            };
            let (Some(partition), Value::Object(origins)) = (partition, origins) else {
                skipped += 1;
                continue;
            };
            let mut grants = Grants::new();
            for (origin, decided) in origins {
                let origin = Origin::parse_canonical(origin)
                    .ok()
                    .filter(Origin::is_potentially_trustworthy);
                let (Some(origin), Value::Object(decided)) = (origin, decided) else {
                    skipped += 1;
                    continue;
                };
                let mut kinds = BTreeMap::new();
                for (kind, verdict) in decided {
                    let verdict = match verdict.as_str() {
                        Some("allow") => Some(Verdict::Allow),
                        Some("block") => Some(Verdict::Block),
                        _ => None,
                    };
                    match (PermissionKind::from_name(kind), verdict) {
                        (Some(kind), Some(verdict)) => {
                            kinds.insert(kind, verdict);
                        }
                        _ => skipped += 1,
                    }
                }
                if !kinds.is_empty() {
                    grants.insert(origin, kinds);
                }
            }
            if !grants.is_empty() {
                snapshot.partitions.insert(partition, grants);
            }
        }
        Ok(Decoded {
            snapshot,
            skipped_entries: skipped,
        })
    }
}

/// The snapshot file. A handle exists only after a load that this build may overwrite.
#[derive(Debug)]
pub struct SnapshotFile {
    path: PathBuf,
}

#[derive(Debug)]
pub struct Loaded {
    pub snapshot: Snapshot,
    pub skipped_entries: usize,
    /// Where a damaged file was moved before starting empty.
    pub damaged_backup: Option<PathBuf>,
}

#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    /// A newer build wrote the file. It is left in place and no handle is returned, so this
    /// build cannot overwrite it.
    NewerSchema(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "reading permission snapshot: {error}"),
            Self::NewerSchema(found) => write!(f, "permission snapshot schema {found} is newer"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NewerSchema(_) => None,
        }
    }
}

impl SnapshotFile {
    /// Reads the file at `path`. A missing file loads empty. A damaged one is renamed to
    /// `<name>.damaged`, replacing an older backup, and loads empty.
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, Loaded), LoadError> {
        let path = path.into();
        let bytes = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(LoadError::Io(error)),
        };
        let mut loaded = Loaded {
            snapshot: Snapshot::default(),
            skipped_entries: 0,
            damaged_backup: None,
        };
        if let Some(bytes) = bytes {
            let decoded = String::from_utf8(bytes)
                .map_err(|error| DecodeError::Damaged(error.to_string()))
                .and_then(|json| Snapshot::decode(&json));
            match decoded {
                Ok(decoded) => {
                    loaded.snapshot = decoded.snapshot;
                    loaded.skipped_entries = decoded.skipped_entries;
                }
                Err(DecodeError::NewerSchema(found)) => return Err(LoadError::NewerSchema(found)),
                Err(DecodeError::Damaged(_)) => {
                    let backup = sibling(&path, "damaged").map_err(LoadError::Io)?;
                    fs::rename(&path, &backup).map_err(LoadError::Io)?;
                    loaded.damaged_backup = Some(backup);
                }
            }
        }
        Ok((Self { path }, loaded))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes through a new file beside the snapshot and renames it into place, so a reader sees
    /// the old or the new contents, never part of either.
    pub fn save(&self, snapshot: &Snapshot) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "{}-{}-{}.tmp",
            process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let temp = sibling(&self.path, &unique)?;
        let result = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .and_then(|mut file| {
                file.write_all(snapshot.encode().as_bytes())?;
                file.sync_all()
            })
            .and_then(|()| fs::rename(&temp, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        sync_parent(&self.path)
    }
}

fn sibling(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot path has no file name",
        )
    })?;
    let mut name = name.to_owned();
    name.push(".");
    name.push(suffix);
    Ok(path.with_file_name(name))
}

/// Makes the rename itself durable.
#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => fs::File::open(dir)?.sync_all(),
        _ => fs::File::open(".")?.sync_all(),
    }
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}
