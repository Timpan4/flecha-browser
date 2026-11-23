//! Browsing history and local command-bar candidates.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, Row, Transaction, TransactionBehavior, params};
use url::Url;

/// The number of entries requested per read by Flecha's History page.
pub const PAGE: usize = 100;

const SCHEMA_VERSION: i32 = 1;
const SCHEMA: &str = "
    CREATE TABLE urls (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL UNIQUE,
        url_folded TEXT NOT NULL,
        title TEXT NOT NULL,
        title_folded TEXT NOT NULL,
        visit_count INTEGER NOT NULL CHECK (visit_count > 0),
        last_visit_ms INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX urls_by_recency ON urls(last_visit_ms DESC, url);

    CREATE TABLE visits (
        id INTEGER PRIMARY KEY,
        url_id INTEGER NOT NULL REFERENCES urls(id) ON DELETE CASCADE,
        visited_at_ms INTEGER NOT NULL,
        title TEXT NOT NULL,
        title_folded TEXT NOT NULL
    ) STRICT;
    CREATE INDEX visits_by_url_and_time ON visits(url_id, visited_at_ms);
    CREATE INDEX visits_by_time ON visits(visited_at_ms);
";

/// A single visit to import. The timestamp is milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryVisit<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub visited_at_ms: i64,
}

/// A URL's current entry in history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: u64,
    pub last_visit_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    /// True when this read returned fewer than [`PAGE`] entries.
    pub exhausted: bool,
}

#[derive(Debug)]
pub enum HistoryError {
    Database(rusqlite::Error),
    UnsupportedSchema { found: i32 },
    UnrecognizedDatabase,
    WalUnavailable,
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(f, "history database: {error}"),
            Self::UnsupportedSchema { found } => {
                write!(
                    f,
                    "unsupported history schema version {found} (expected {SCHEMA_VERSION})"
                )
            }
            Self::UnrecognizedDatabase => f.write_str("history database has an unversioned schema"),
            Self::WalUnavailable => f.write_str("history database could not enable WAL mode"),
        }
    }
}

impl std::error::Error for HistoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for HistoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// Owns one SQLite connection. A private session can use [`Self::in_memory`].
pub struct HistoryStore {
    db: Connection,
}

impl HistoryStore {
    /// Opens or creates a history database at a filesystem path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let mut db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        initialize_schema(&mut db)?;
        let mode: String = db.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if mode != "wal" {
            return Err(HistoryError::WalUnavailable);
        }
        Ok(Self { db })
    }

    /// Creates an empty store with both its main database and temporary data in memory.
    pub fn in_memory() -> Result<Self, HistoryError> {
        let mut db = Connection::open_in_memory()?;
        db.pragma_update(None, "temp_store", "MEMORY")?;
        initialize_schema(&mut db)?;
        Ok(Self { db })
    }

    /// Records one HTTP or HTTPS navigation at a Unix timestamp in milliseconds.
    /// Returns false for invalid or ineligible URLs. Credentials are removed before storage.
    pub fn record_visit(
        &mut self,
        url: &str,
        title: &str,
        visited_at_ms: i64,
    ) -> Result<bool, HistoryError> {
        let Some(clean_url) = history_url(url) else {
            return Ok(false);
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cleaned_title = clean_title(url, &clean_url, title);
        insert_visit(&tx, &clean_url, cleaned_title.as_ref(), visited_at_ms)?;
        tx.commit()?;
        Ok(true)
    }

    /// Imports visits atomically and returns (imported, skipped). The canonical URL and
    /// timestamp identify a visit for import purposes, including visits already recorded locally.
    /// Supply actual visit timestamps when importing from a browser that also stores URL totals.
    pub fn import_visits(
        &mut self,
        visits: &[HistoryVisit<'_>],
    ) -> Result<(usize, usize), HistoryError> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut existing = tx.prepare_cached(
            "SELECT EXISTS(
                SELECT 1 FROM urls JOIN visits ON visits.url_id = urls.id
                WHERE urls.url = ?1 AND visits.visited_at_ms = ?2
            )",
        )?;
        let mut imported = 0;
        for visit in visits {
            let Some(url) = history_url(visit.url) else {
                continue;
            };
            if existing.query_row(params![url.as_str(), visit.visited_at_ms], |row| row.get(0))? {
                continue;
            }
            let cleaned_title = clean_title(visit.url, &url, visit.title);
            insert_visit(&tx, &url, cleaned_title.as_ref(), visit.visited_at_ms)?;
            imported += 1;
        }
        drop(existing);
        tx.commit()?;
        Ok((imported, visits.len() - imported))
    }

    /// Returns a page of unique URLs, newest first, matching a case-insensitive substring in
    /// the URL or title. Start at offset zero and advance by [`PAGE`]. Restart paging when
    /// the search text or the history changes, as the History page does.
    pub fn query(&self, text: &str, offset: usize) -> Result<HistoryPage, HistoryError> {
        let entries = self.find(text, PAGE, offset)?;
        let exhausted = entries.len() < PAGE;
        Ok(HistoryPage { entries, exhausted })
    }

    /// Returns up to `limit` recent matches with their visit counts for command-bar ranking.
    pub fn suggest(&self, text: &str, limit: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
        self.find(text, limit, 0)
    }

    /// Removes the URL and all of its visits. Returns whether the URL existed.
    pub fn remove_url(&mut self, url: &str) -> Result<bool, HistoryError> {
        let Some(url) = history_url(url) else {
            return Ok(false);
        };
        Ok(self
            .db
            .execute("DELETE FROM urls WHERE url = ?1", [url.as_str()])?
            != 0)
    }

    /// Removes visits at or after the given Unix timestamp in milliseconds.
    pub fn clear_since(&mut self, cutoff_ms: i64) -> Result<(), HistoryError> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM visits WHERE visited_at_ms >= ?1", [cutoff_ms])?;
        tx.execute(
            "DELETE FROM urls WHERE last_visit_ms >= ?1
             AND NOT EXISTS (SELECT 1 FROM visits WHERE url_id = urls.id)",
            [cutoff_ms],
        )?;
        tx.execute(
            "UPDATE urls SET
                visit_count = (SELECT COUNT(*) FROM visits WHERE url_id = urls.id),
                last_visit_ms = (SELECT MAX(visited_at_ms) FROM visits WHERE url_id = urls.id),
                title = (
                    SELECT title FROM visits
                    WHERE url_id = urls.id
                    ORDER BY visited_at_ms DESC, id DESC
                    LIMIT 1
                ),
                title_folded = (
                    SELECT title_folded FROM visits
                    WHERE url_id = urls.id
                    ORDER BY visited_at_ms DESC, id DESC
                    LIMIT 1
                )
             WHERE last_visit_ms >= ?1",
            [cutoff_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes every URL and visit.
    pub fn clear_all(&mut self) -> Result<(), HistoryError> {
        self.db.execute("DELETE FROM urls", [])?;
        Ok(())
    }

    fn find(
        &self,
        text: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<HistoryEntry>, HistoryError> {
        let text = text.trim().to_lowercase();
        let mut stmt = self.db.prepare_cached(
            "SELECT url, title, visit_count, last_visit_ms FROM urls
             WHERE ?1 = '' OR instr(url_folded, ?1) > 0 OR instr(title_folded, ?1) > 0
             ORDER BY last_visit_ms DESC, url LIMIT ?2 OFFSET ?3",
        )?;
        stmt.query_map(
            params![
                text,
                i64::try_from(limit).unwrap_or(i64::MAX),
                i64::try_from(offset).unwrap_or(i64::MAX)
            ],
            read_entry,
        )?
        .collect::<rusqlite::Result<_>>()
        .map_err(HistoryError::from)
    }
}

fn initialize_schema(db: &mut Connection) -> Result<(), HistoryError> {
    db.pragma_update(None, "foreign_keys", true)?;
    let version = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        SCHEMA_VERSION => return Ok(()),
        0 => {}
        found => return Err(HistoryError::UnsupportedSchema { found }),
    }

    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Another connection may have initialized the database before we acquired the write lock.
    match tx.pragma_query_value(None, "user_version", |row| row.get(0))? {
        0 => {
            let has_schema =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get(0)
                })?;
            if has_schema {
                return Err(HistoryError::UnrecognizedDatabase);
            }
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        SCHEMA_VERSION => {}
        found => return Err(HistoryError::UnsupportedSchema { found }),
    }
    tx.commit()?;
    Ok(())
}

fn insert_visit(
    tx: &Transaction<'_>,
    url: &Url,
    title: &str,
    visited_at_ms: i64,
) -> rusqlite::Result<()> {
    let url = url.as_str();
    let title_folded = title.to_lowercase();
    let id: i64 = tx.prepare_cached(
        "INSERT INTO urls (url, url_folded, title, title_folded, visit_count, last_visit_ms)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)
         ON CONFLICT(url) DO UPDATE SET
            title = CASE WHEN excluded.last_visit_ms >= last_visit_ms THEN excluded.title ELSE title END,
            title_folded = CASE WHEN excluded.last_visit_ms >= last_visit_ms THEN excluded.title_folded ELSE title_folded END,
            visit_count = visit_count + 1,
            last_visit_ms = MAX(last_visit_ms, excluded.last_visit_ms)
         RETURNING id",
    )?.query_row(
        params![url, url.to_lowercase(), title, title_folded.as_str(), visited_at_ms],
        |row| row.get(0),
    )?;
    tx.prepare_cached(
        "INSERT INTO visits (url_id, visited_at_ms, title, title_folded) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![id, visited_at_ms, title, title_folded.as_str()])?;
    Ok(())
}

fn read_entry(row: &Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let count: i64 = row.get(2)?;
    Ok(HistoryEntry {
        url: row.get(0)?,
        title: row.get(1)?,
        visit_count: u64::try_from(count)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, count))?,
        last_visit_ms: row.get(3)?,
    })
}

fn history_url(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.password().is_some() || !url.username().is_empty() {
        url.set_password(None).ok()?;
        url.set_username("").ok()?;
    }
    Some(url)
}

fn clean_title<'a>(raw_url: &str, url: &'a Url, title: &'a str) -> Cow<'a, str> {
    // Importers sometimes use the original URL as the title when none was saved.
    if title == raw_url {
        return Cow::Borrowed(url.as_str());
    }
    if let Ok(mut parsed) = Url::parse(title.trim())
        && matches!(parsed.scheme(), "http" | "https")
        && (parsed.password().is_some() || !parsed.username().is_empty())
        && parsed.set_password(None).is_ok()
        && parsed.set_username("").is_ok()
    {
        return Cow::Owned(parsed.into());
    }
    Cow::Borrowed(title)
}
