use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use cap_std::fs::Dir;
use flecha_core::input_text::trim_input;
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, PrepFlags, Row, Statement};
use tempfile::TempDir;

use crate::bookmarks::BookmarkTree;
use crate::input::{file, io_error};
use crate::{Bookmark, HistoryVisit, ReadError, SavedTab};

// Per profile, from the shipping browserImport.ts.
const HISTORY_ROWS: i64 = 5000;

#[derive(Clone, Copy)]
pub(crate) enum Database {
    Chromium,
    Firefox,
    Safari,
}

pub(crate) struct Snapshot {
    connection: Connection,
    _directory: TempDir,
    pub(crate) wal_warning: Option<ReadError>,
}

pub(crate) struct FirefoxBookmarks {
    pub(crate) nodes: Vec<Bookmark>,
    pub(crate) malformed: usize,
    pub(crate) duplicate_places: usize,
}

impl Snapshot {
    pub(crate) fn open(root: &Dir, name: &str, kind: Database) -> Result<Self, ReadError> {
        let mut temp = tempfile::Builder::new();
        temp.prefix("flecha-import-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = temp.tempdir().map_err(io_error)?;
        let main = directory.path().join("snapshot.sqlite");
        copy(root, name, &main)?;
        let wal = directory.path().join("snapshot.sqlite-wal");
        let wal_warning = match copy(root, &format!("{name}-wal"), &wal) {
            Ok(()) | Err(ReadError::Missing) => None,
            Err(error) => {
                // A partial WAL copy is unusable. Discard only the file we created.
                match std::fs::remove_file(&wal) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(io_error(err)),
                }
                Some(error)
            }
        };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut connection = Connection::open_with_flags(&main, flags).map_err(database_error)?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
            .map_err(database_error)?;
        connection
            .authorizer(Some(move |ctx: AuthContext<'_>| authorize(kind, ctx)))
            .map_err(database_error)?;
        Ok(Self {
            connection,
            _directory: directory,
            wal_warning,
        })
    }

    fn prepare(&self, sql: &str) -> Result<Statement<'_>, ReadError> {
        self.connection
            .prepare_with_flags(sql, PrepFlags::SQLITE_PREPARE_NO_VTAB)
            .map_err(database_error)
    }

    pub(crate) fn history(&self, kind: Database) -> Result<(Vec<HistoryVisit>, usize), ReadError> {
        let sql = match kind {
            Database::Chromium => "SELECT url, title, visit_count, last_visit_time / 1000 - 11644473600000
                FROM urls WHERE hidden = 0 AND last_visit_time > 0 ORDER BY last_visit_time DESC LIMIT ?1",
            Database::Firefox => "SELECT url, title, visit_count, last_visit_date / 1000
                FROM moz_places WHERE hidden = 0 AND visit_count > 0 AND last_visit_date IS NOT NULL
                ORDER BY last_visit_date DESC LIMIT ?1",
            Database::Safari => "SELECT i.url, v.title, i.visit_count,
                CAST((MAX(v.visit_time) + 978307200) * 1000 AS INTEGER) AS last_visit
                FROM history_items i JOIN history_visits v ON v.history_item = i.id
                GROUP BY i.id ORDER BY last_visit DESC LIMIT ?1",
        };
        let mut statement = self.prepare(sql)?;
        let mut rows = statement.query([HISTORY_ROWS]).map_err(database_error)?;
        let mut history = Vec::new();
        let mut malformed = 0;
        while let Some(row) = rows.next().map_err(database_error)? {
            let Some(url) = row
                .get_ref(0)
                .ok()
                .and_then(text)
                .filter(|url| !url.is_empty())
            else {
                malformed += 1;
                continue;
            };
            let Some(last_visit) = row.get_ref(3).ok().and_then(number) else {
                malformed += 1;
                continue;
            };
            let visits = row
                .get_ref(2)
                .ok()
                .and_then(number)
                .filter(|value| *value != 0.0)
                .unwrap_or(1.0);
            let title = row
                .get_ref(1)
                .ok()
                .and_then(title)
                .unwrap_or_else(|| url.clone());
            history.push(HistoryVisit {
                url,
                title,
                visits,
                last_visit,
            });
        }
        Ok((history, malformed))
    }

    pub(crate) fn firefox_bookmarks(&self) -> Result<FirefoxBookmarks, ReadError> {
        let mut places = HashMap::new();
        let mut duplicate_places = 0;
        {
            // Resolve each place once, including in databases with no index on the ID.
            let mut statement = self.prepare("SELECT id, url FROM moz_places")?;
            let mut rows = statement.query([]).map_err(database_error)?;
            while let Some(row) = rows.next().map_err(database_error)? {
                let Some(id) = PlaceId::from_value(row.get_ref(0).map_err(database_error)?) else {
                    continue;
                };
                match places.entry(id) {
                    Entry::Vacant(entry) => {
                        entry.insert(row.get_ref(1).ok().and_then(text));
                    }
                    Entry::Occupied(_) => duplicate_places += 1,
                }
            }
        }
        let mut statement = self.prepare(
            "SELECT b.id, b.parent, b.type, b.title, b.guid, b.fk
             FROM moz_bookmarks b ORDER BY b.parent, b.position",
        )?;
        let mut rows = statement.query([]).map_err(database_error)?;
        let mut roots = HashMap::new();
        let mut parents: HashMap<i64, Vec<FirefoxBookmark>> = HashMap::new();
        let mut malformed = 0;
        while let Some(row) = rows.next().map_err(database_error)? {
            match firefox_record(row, &places) {
                Ok((parent, guid, item)) => {
                    roots.entry(guid).or_insert(item.id);
                    parents.entry(parent).or_default().push(item);
                }
                Err(_) => malformed += 1,
            }
        }
        let mut output = Vec::new();
        let mut expanded = HashSet::new();
        for (guid, name) in [
            ("toolbar_____", None),
            ("menu________", Some("Bookmarks Menu")),
            ("unfiled_____", Some("Other Bookmarks")),
            ("mobile______", Some("Mobile Bookmarks")),
        ] {
            let bookmarks = roots.get(guid).map_or_else(Vec::new, |id| {
                firefox_tree(*id, &parents, &mut expanded, &mut malformed)
            });
            if let Some(name) = name {
                output.push(Bookmark::Folder {
                    name: name.into(),
                    children: bookmarks,
                });
            } else {
                output.extend(bookmarks);
            }
        }
        Ok(FirefoxBookmarks {
            nodes: output,
            malformed,
            duplicate_places,
        })
    }
}

fn copy(root: &Dir, name: &str, destination: &Path) -> Result<(), ReadError> {
    let source = file(root, name)?;
    // Copy through the validated handle, and stop at the size observed when it opened.
    let len = source.metadata().map_err(io_error)?.len();
    let mut destination = File::create_new(destination).map_err(io_error)?;
    io::copy(&mut source.take(len), &mut destination).map_err(io_error)?;
    Ok(())
}

fn authorize(kind: Database, ctx: AuthContext<'_>) -> Authorization {
    if ctx.accessor.is_some() {
        return Authorization::Deny;
    }
    let allowed = match ctx.action {
        AuthAction::Select => true,
        AuthAction::Function { function_name } => {
            matches!(kind, Database::Safari) && function_name == "max"
        }
        AuthAction::Read {
            table_name,
            column_name,
        } if ctx.database_name == Some("main") => match (kind, table_name) {
            (Database::Chromium, "urls") => matches!(
                column_name,
                "url" | "title" | "visit_count" | "last_visit_time" | "hidden"
            ),
            (Database::Firefox, "moz_places") => matches!(
                column_name,
                "id" | "url" | "title" | "visit_count" | "last_visit_date" | "hidden"
            ),
            (Database::Firefox, "moz_bookmarks") => matches!(
                column_name,
                "id" | "parent" | "type" | "title" | "guid" | "fk" | "position"
            ),
            (Database::Safari, "history_items") => {
                matches!(column_name, "id" | "url" | "visit_count")
            }
            (Database::Safari, "history_visits") => {
                matches!(column_name, "title" | "visit_time" | "history_item")
            }
            _ => false,
        },
        _ => false,
    };
    if allowed {
        Authorization::Allow
    } else {
        Authorization::Deny
    }
}

fn database_error(_: rusqlite::Error) -> ReadError {
    ReadError::InvalidDatabase
}

fn text(value: ValueRef<'_>) -> Option<String> {
    match value {
        ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        _ => None,
    }
}

fn title(value: ValueRef<'_>) -> Option<String> {
    match value {
        ValueRef::Integer(value) if value != 0 => Some(value.to_string()),
        ValueRef::Real(value) if value != 0.0 && value.is_finite() => Some(value.to_string()),
        _ => text(value).filter(|value| !value.is_empty()),
    }
}

fn number(value: ValueRef<'_>) -> Option<f64> {
    let number = match value {
        ValueRef::Integer(number) => number as f64,
        ValueRef::Real(number) => number,
        ValueRef::Text(bytes) => {
            let text = trim_input(std::str::from_utf8(bytes).ok()?);
            if text.is_empty() {
                0.0
            } else {
                text.parse().ok()?
            }
        }
        ValueRef::Null => 0.0,
        ValueRef::Blob(_) => return None,
    };
    number.is_finite().then_some(number)
}

#[derive(PartialEq, Eq, Hash)]
enum PlaceId {
    Integer(i64),
    Real(u64),
    Text(Box<[u8]>),
    Blob(Box<[u8]>),
}

impl PlaceId {
    fn from_value(value: ValueRef<'_>) -> Option<Self> {
        Some(match value {
            ValueRef::Integer(value) => Self::Integer(value),
            ValueRef::Real(value) => {
                // i64::MAX rounds up to 2^63 as an f64. Exclude that upper endpoint.
                if value.fract() == 0.0 && (i64::MIN as f64..-(i64::MIN as f64)).contains(&value) {
                    Self::Integer(value as i64)
                } else {
                    Self::Real(value.to_bits())
                }
            }
            ValueRef::Text(bytes) => {
                let integer = std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|value| value.trim_ascii().parse::<i64>().ok());
                match integer {
                    Some(integer) => Self::Integer(integer),
                    None => Self::Text(bytes.into()),
                }
            }
            ValueRef::Blob(bytes) => Self::Blob(bytes.into()),
            ValueRef::Null => return None,
        })
    }
}

struct FirefoxBookmark {
    id: i64,
    kind: i64,
    title: Option<String>,
    url: Option<String>,
}

fn firefox_record(
    row: &Row<'_>,
    places: &HashMap<PlaceId, Option<String>>,
) -> rusqlite::Result<(i64, String, FirefoxBookmark)> {
    Ok((
        row.get(1)?,
        row.get(4)?,
        FirefoxBookmark {
            id: row.get(0)?,
            kind: row.get(2)?,
            title: title(row.get_ref(3)?),
            url: PlaceId::from_value(row.get_ref(5)?)
                .and_then(|id| places.get(&id))
                .and_then(Clone::clone),
        },
    ))
}

fn firefox_tree(
    id: i64,
    parents: &HashMap<i64, Vec<FirefoxBookmark>>,
    expanded: &mut HashSet<i64>,
    malformed: &mut usize,
) -> Vec<Bookmark> {
    if !expanded.insert(id) {
        *malformed += 1;
        return Vec::new();
    }
    let children = |id| parents.get(&id).map_or(&[][..], Vec::as_slice).iter();
    let mut stack = vec![children(id)];
    let mut tree = BookmarkTree::default();
    while let Some(rows) = stack.last_mut() {
        match rows.next() {
            Some(row) if row.kind == 1 => {
                if let Some(url) = row.url.as_ref().filter(|url| !url.is_empty()) {
                    let title = row.title.as_ref().unwrap_or(url);
                    tree.push(Bookmark::Tab(SavedTab {
                        url: url.clone(),
                        title: title.clone(),
                    }));
                } else {
                    *malformed += 1;
                }
            }
            Some(row) if row.kind == 2 => {
                if expanded.insert(row.id) {
                    tree.open(
                        row.title
                            .clone()
                            .unwrap_or_else(|| "Untitled folder".into()),
                    );
                    stack.push(children(row.id));
                } else {
                    *malformed += 1;
                }
            }
            Some(_) => {}
            None => {
                stack.pop();
                if !stack.is_empty() {
                    tree.close(true);
                }
            }
        }
    }
    tree.finish()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{Bookmark, Connection, Database, Snapshot};
    use crate::input::open_root;

    #[test]
    fn firefox_late_place_lookups_scale_with_rows() {
        let mut instructions = Vec::new();
        for count in [500_i64, 1000] {
            let profile = tempfile::tempdir().unwrap();
            let name = "places.sqlite";
            let mut db = Connection::open(profile.path().join(name)).unwrap();
            db.execute_batch(
                "CREATE TABLE moz_places (id, url);
                 CREATE TABLE moz_bookmarks (id, parent, type, title, guid, fk, position);
                 INSERT INTO moz_bookmarks VALUES (100, 0, 2, 'Toolbar', 'toolbar_____', NULL, 0);",
            )
            .unwrap();
            let rows = db.transaction().unwrap();
            for index in 0..count {
                rows.execute(
                    "INSERT INTO moz_places VALUES (2, ?1)",
                    [format!("https://example.org/{index}")],
                )
                .unwrap();
                rows.execute(
                    "INSERT INTO moz_bookmarks VALUES (?1, 100, 1, 'Bookmark', ?2, 1, ?1)",
                    (1000 + index, format!("bookmark-{index}")),
                )
                .unwrap();
            }
            rows.execute(
                "INSERT INTO moz_places (rowid, id, url) VALUES
                    (?2, 1, 'https://example.org/duplicate'), (?1, 1, 'https://example.org/target')",
                (count + 1, count + 2),
            ).unwrap();
            rows.commit().unwrap();
            drop(db);

            let root = open_root(profile.path()).unwrap();
            let mut snapshot = Snapshot::open(&root, name, Database::Firefox).unwrap();
            let steps = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&steps);
            snapshot
                .connection
                .progress_handler(
                    1,
                    Some(move || {
                        counter.fetch_add(1, Ordering::Relaxed);
                        false
                    }),
                )
                .unwrap();

            let bookmarks = snapshot.firefox_bookmarks().unwrap();
            instructions.push(steps.load(Ordering::Relaxed));
            let tabs: Vec<_> = bookmarks
                .nodes
                .iter()
                .filter_map(|node| match node {
                    Bookmark::Tab(tab) => Some(tab),
                    Bookmark::Folder { .. } => None,
                })
                .collect();
            assert_eq!(bookmarks.malformed, 0);
            assert_eq!(bookmarks.duplicate_places, usize::try_from(count).unwrap());
            assert_eq!(tabs.len(), usize::try_from(count).unwrap());
            assert!(
                tabs.iter()
                    .all(|tab| tab.url == "https://example.org/target")
            );
        }
        // Doubling both tables can at most double the per-row instructions of the two scans.
        assert!(instructions[0] > 0);
        assert!(instructions[1] <= 2 * instructions[0], "{instructions:?}");
    }
}
