use std::cmp::Ordering as CmpOrdering;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flecha_core::storage::{StoredVersion, newer_schema_version, read_schema_version};
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
    /// The document with every object's keys in sorted order. `serde_json::Map` keeps insertion
    /// order when another crate in the build enables `preserve_order`, so the keys are sorted
    /// here rather than by the map.
    pub fn encode(&self) -> String {
        let partitions = sorted_object(self.partitions.iter().filter(|(_, g)| !g.is_empty()).map(
            |(partition, grants)| {
                let key = match partition {
                    StoredPartition::Shared => SHARED_KEY.to_owned(),
                    StoredPartition::Isolated(id) => format!("{ISOLATED_PREFIX}{id}"),
                };
                let origins = sorted_object(grants.iter().map(|(origin, decided)| {
                    let decided = sorted_object(decided.iter().map(|(kind, verdict)| {
                        let verdict = match verdict {
                            Verdict::Allow => "allow",
                            Verdict::Block => "block",
                        };
                        (kind.name().to_owned(), Value::from(verdict))
                    }));
                    (origin.as_str().to_owned(), decided)
                }));
                (key, origins)
            },
        ));
        let document = sorted_object([
            ("partitions".to_owned(), partitions),
            ("version".to_owned(), Value::from(SCHEMA_VERSION)),
        ]);
        let mut json = serde_json::to_string_pretty(&document)
            .expect("JSON values with string keys always serialize");
        json.push('\n');
        json
    }

    /// Reads a document. Only the top-level `version` is read first, so a newer document is
    /// reported as newer whatever the rest of it holds.
    pub fn decode(json: &str) -> Result<Decoded, DecodeError> {
        match read_schema_version(json, SCHEMA_VERSION)
            .map_err(|error| DecodeError::Damaged(error.to_string()))?
        {
            Some(StoredVersion {
                order: CmpOrdering::Equal,
                ..
            }) => {}
            Some(StoredVersion {
                order: CmpOrdering::Greater,
                text,
            }) => return Err(DecodeError::NewerSchema(text)),
            _ => return Err(DecodeError::Damaged("no supported version".to_owned())),
        }
        let document: Value =
            serde_json::from_str(json).map_err(|error| DecodeError::Damaged(error.to_string()))?;
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
pub enum SnapshotError {
    Io(io::Error),
    /// A newer build wrote the file. It is left in place: `open` returns no handle, and `save`
    /// refuses to replace it.
    NewerSchema(String),
}

impl From<io::Error> for SnapshotError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "permission snapshot I/O: {error}"),
            Self::NewerSchema(found) => write!(f, "permission snapshot schema {found} is newer"),
        }
    }
}

impl std::error::Error for SnapshotError {
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
    pub fn open(path: impl Into<PathBuf>) -> Result<(Self, Loaded), SnapshotError> {
        let path = path.into();
        let bytes = read_if_present(&path)?;
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
                Err(DecodeError::NewerSchema(found)) => {
                    return Err(SnapshotError::NewerSchema(found));
                }
                Err(DecodeError::Damaged(_)) => {
                    let backup = sibling(&path, "damaged")?;
                    fs::rename(&path, &backup)?;
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
    ///
    /// A newer build may have written the file since `open`, so its version is read again first
    /// and a newer schema is refused. A writer that replaces the file between that read and the
    /// rename is not detected.
    pub fn save(&self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        if let Some(bytes) = read_if_present(&self.path)?
            && let Some(found) = std::str::from_utf8(&bytes)
                .ok()
                .and_then(|json| newer_schema_version(json, SCHEMA_VERSION))
        {
            return Err(SnapshotError::NewerSchema(found));
        }
        let name = file_name(&self.path)?;
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        // `tempfile` creates the file exclusively, retries on a name collision, and deletes only
        // the file it created when a later step fails.
        let mut temp = tempfile::Builder::new()
            .prefix(&format!(".{}.", name.to_string_lossy()))
            .suffix(".tmp")
            .tempfile_in(dir)?;
        temp.write_all(snapshot.encode().as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path).map_err(|error| error.error)?;
        sync_dir(dir)?;
        Ok(())
    }
}

fn sorted_object(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    let sorted: BTreeMap<_, _> = entries.into_iter().collect();
    Value::Object(sorted.into_iter().collect::<Map<_, _>>())
}

fn read_if_present(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn file_name(path: &Path) -> io::Result<&std::ffi::OsStr> {
    path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot path has no file name",
        )
    })
}

fn sibling(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let mut name = file_name(path)?.to_owned();
    name.push(".");
    name.push(suffix);
    Ok(path.with_file_name(name))
}

/// Makes the rename itself durable.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}
