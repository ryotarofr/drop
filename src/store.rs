//! Persistence layer: SQLite (WAL) + in-memory cache.
//!
//! Design notes:
//!   - `id` is `INTEGER PRIMARY KEY AUTOINCREMENT` so user-typeable IDs
//!     (`/del 42`) stay short and are never recycled after deletion.
//!   - Deletes are soft (`deleted_at` timestamp). Rows stay in the file so
//!     a user can recover them via SQL even though the UI offers no undo.
//!   - All read queries filter `deleted_at IS NULL`.
//!   - The `entries_fts` virtual table is kept for future full-text search;
//!     today's `/search` command runs against the in-memory cache.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

use crate::classifier;

/// Maximum length of a single entry, in characters. Anything longer is
/// silently truncated at insert time.
pub const MAX_LEN: usize = 140;

/// One captured thought.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub due_today: bool,
    pub last_resurfaced_at: Option<DateTime<Utc>>,
}

impl Entry {
    /// `H:MM` in local time, no leading zero on the hour (`9:14`, `17:42`).
    pub fn time_label(&self) -> String {
        self.created_at
            .with_timezone(&Local)
            .format("%-H:%M")
            .to_string()
    }

    /// Does this entry belong in the `今日中` section right now?
    pub fn is_due_today_now(&self, now: DateTime<Local>) -> bool {
        if !self.due_today {
            return false;
        }
        self.created_at.with_timezone(&Local).date_naive() == now.date_naive()
    }
}

// --- Schema ------------------------------------------------------------

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    due_today INTEGER NOT NULL DEFAULT 0,
    last_resurfaced_at INTEGER,
    deleted_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_created_at ON entries(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_due_today ON entries(due_today, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_deleted_at ON entries(deleted_at);

CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    text,
    content='entries',
    content_rowid='id',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, text) VALUES('delete', old.id, old.text);
END;
"#;

// --- Store -------------------------------------------------------------

pub struct Store {
    conn: Connection,
    /// Live entries (deleted_at IS NULL), sorted by `created_at DESC`.
    entries: Vec<Entry>,
}

impl Store {
    /// Windows: `%LOCALAPPDATA%\drop\drop.sqlite`.
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::data_local_dir()
            .ok_or_else(|| anyhow::anyhow!("could not resolve local data dir"))?;
        Ok(dir.join("drop").join("drop.sqlite"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        migrate_if_needed(&conn).context("migrate")?;
        conn.execute_batch(SCHEMA)?;
        let entries = Self::load_live(&conn)?;
        Ok(Self { conn, entries })
    }

    fn load_live(conn: &Connection) -> Result<Vec<Entry>> {
        let mut stmt = conn.prepare(
            "SELECT id, text, created_at, due_today, last_resurfaced_at
             FROM entries
             WHERE deleted_at IS NULL
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_entry)?;
        let entries = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    /// Read-only view of all live entries (most recent first).
    pub fn all(&self) -> &[Entry] {
        &self.entries
    }

    /// Entries that should appear in the `今日中` section right now.
    pub fn today(&self, now: DateTime<Local>) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| e.is_due_today_now(now))
            .cloned()
            .collect()
    }

    /// Case-insensitive substring filter over the in-memory cache.
    /// Order matches `all()` (most recent first).
    pub fn search(&self, query: &str) -> Vec<Entry> {
        if query.is_empty() {
            return self.entries.clone();
        }
        let needle = query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| e.text.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    }

    /// Insert a new entry. Returns the persisted Entry.
    pub fn insert(&mut self, raw_text: &str) -> Result<Entry> {
        let trimmed = raw_text.trim();
        if trimmed.is_empty() {
            anyhow::bail!("empty entry");
        }
        let text: String = trimmed.chars().take(MAX_LEN).collect();
        let created_at = Utc::now();
        let due_today = classifier::is_due_today(&text);

        let id: i64 = self.conn.query_row(
            "INSERT INTO entries (text, created_at, due_today, last_resurfaced_at, deleted_at)
             VALUES (?1, ?2, ?3, NULL, NULL)
             RETURNING id",
            params![text, created_at.timestamp_millis(), due_today as i64],
            |row| row.get(0),
        )?;

        let entry = Entry {
            id,
            text,
            created_at,
            due_today,
            last_resurfaced_at: None,
        };
        self.entries.insert(0, entry.clone());
        Ok(entry)
    }

    /// Soft-delete an entry by its short ID. Returns the deleted Entry on
    /// success, `None` if the ID is unknown or already deleted.
    pub fn delete(&mut self, id: i64, now: DateTime<Utc>) -> Result<Option<Entry>> {
        let updated = self.conn.execute(
            "UPDATE entries SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![now.timestamp_millis(), id],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        let entry = self
            .entries
            .iter()
            .position(|e| e.id == id)
            .map(|pos| self.entries.remove(pos));
        Ok(entry)
    }

    /// Pick one entry for the `過去から` section, then stamp
    /// `last_resurfaced_at` so it is not chosen again soon.
    ///
    /// Candidates are: created over 7 days ago, not marked `due_today`,
    /// not deleted, and either never resurfaced or last resurfaced more
    /// than 30 days ago. The pick is random among matches.
    pub fn pick_resurface(&mut self, now: DateTime<Utc>) -> Result<Option<Entry>> {
        let seven_days_ago = (now - Duration::days(7)).timestamp_millis();
        let thirty_days_ago = (now - Duration::days(30)).timestamp_millis();

        let picked = self
            .conn
            .query_row(
                "SELECT id, text, created_at, due_today, last_resurfaced_at
                 FROM entries
                 WHERE created_at <= ?1
                   AND due_today = 0
                   AND deleted_at IS NULL
                   AND (last_resurfaced_at IS NULL OR last_resurfaced_at <= ?2)
                 ORDER BY RANDOM() LIMIT 1",
                params![seven_days_ago, thirty_days_ago],
                row_to_entry,
            )
            .optional()?;

        if let Some(ref e) = picked {
            self.conn.execute(
                "UPDATE entries SET last_resurfaced_at = ?1 WHERE id = ?2",
                params![now.timestamp_millis(), e.id],
            )?;
            if let Some(cached) = self.entries.iter_mut().find(|c| c.id == e.id) {
                cached.last_resurfaced_at = Some(now);
            }
        }

        Ok(picked)
    }
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<Entry> {
    use rusqlite::Error::FromSqlConversionFailure;
    use rusqlite::types::Type;

    let id: i64 = row.get(0)?;
    let text: String = row.get(1)?;
    let created_ms: i64 = row.get(2)?;
    let due_today: i64 = row.get(3)?;
    let last_ms: Option<i64> = row.get(4)?;

    let created_at = DateTime::<Utc>::from_timestamp_millis(created_ms).ok_or_else(|| {
        FromSqlConversionFailure(
            2,
            Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad created_at",
            )),
        )
    })?;
    let last_resurfaced_at = last_ms.and_then(DateTime::<Utc>::from_timestamp_millis);

    Ok(Entry {
        id,
        text,
        created_at,
        due_today: due_today != 0,
        last_resurfaced_at,
    })
}

/// One-time destructive migration from the original `TEXT id` (UUID) schema
/// to the current `INTEGER PRIMARY KEY AUTOINCREMENT` shape. Any existing
/// rows are dropped — only test data ever lived under that schema.
fn migrate_if_needed(conn: &Connection) -> Result<()> {
    let needs_rebuild: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('entries') WHERE name='id' AND upper(type) != 'INTEGER'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if needs_rebuild > 0 {
        eprintln!("[drop] old schema detected; rebuilding entries table");
        conn.execute_batch(
            r#"
            DROP TRIGGER IF EXISTS entries_ai;
            DROP TRIGGER IF EXISTS entries_ad;
            DROP TABLE IF EXISTS entries_fts;
            DROP TABLE IF EXISTS entries;
            "#,
        )?;
    }
    Ok(())
}

// --- Tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_rejected() {
        let mut s = Store::open_in_memory().unwrap();
        assert!(s.insert("   ").is_err());
        assert!(s.insert("").is_err());
    }

    #[test]
    fn insert_prepends_to_cache_and_assigns_id() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.insert("first").unwrap();
        let b = s.insert("second").unwrap();

        assert!(a.id >= 1);
        assert!(b.id > a.id);
        assert_eq!(s.all()[0].text, "second");
        assert_eq!(s.all()[1].text, "first");
    }

    #[test]
    fn classifier_runs_on_insert() {
        let mut s = Store::open_in_memory().unwrap();
        assert!(s.insert("明日 PR レビュー").unwrap().due_today);
        assert!(!s.insert("ベルトの斜め配置できたら面白そう").unwrap().due_today);
    }

    #[test]
    fn truncates_at_max_len() {
        let mut s = Store::open_in_memory().unwrap();
        let long: String = "あ".repeat(200);
        let e = s.insert(&long).unwrap();
        assert_eq!(e.text.chars().count(), MAX_LEN);
    }

    #[test]
    fn delete_removes_from_cache_and_filters_reads() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.insert("first").unwrap();
        let _b = s.insert("second").unwrap();

        let deleted = s.delete(a.id, Utc::now()).unwrap();
        assert_eq!(deleted.unwrap().id, a.id);

        // Gone from cache.
        assert_eq!(s.all().len(), 1);
        assert_eq!(s.all()[0].text, "second");

        // Gone from a fresh load too (deleted_at filter).
        let reloaded = Store::load_live(&s.conn).unwrap();
        assert_eq!(reloaded.len(), 1);
    }

    #[test]
    fn delete_returns_none_for_unknown_id() {
        let mut s = Store::open_in_memory().unwrap();
        assert!(s.delete(9999, Utc::now()).unwrap().is_none());
    }

    #[test]
    fn deleted_id_is_not_recycled_for_new_inserts() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.insert("first").unwrap();
        s.delete(a.id, Utc::now()).unwrap();
        let c = s.insert("third").unwrap();
        assert!(c.id > a.id, "AUTOINCREMENT must skip reused ids");
    }

    #[test]
    fn search_matches_substring_case_insensitive() {
        let mut s = Store::open_in_memory().unwrap();
        s.insert("牛乳を買う").unwrap();
        s.insert("PR レビュー").unwrap();
        s.insert("夕方 散歩").unwrap();

        let hits = s.search("牛乳");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "牛乳を買う");

        let hits = s.search("pr"); // case-insensitive
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "PR レビュー");

        let hits = s.search("");
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn search_skips_deleted_entries() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.insert("牛乳を買う").unwrap();
        s.insert("牛乳プリン").unwrap();
        s.delete(a.id, Utc::now()).unwrap();
        let hits = s.search("牛乳");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "牛乳プリン");
    }

    #[test]
    fn resurface_skips_recent_entries() {
        let mut s = Store::open_in_memory().unwrap();
        s.insert("brand new thought").unwrap();
        assert!(s.pick_resurface(Utc::now()).unwrap().is_none());
    }

    #[test]
    fn resurface_skips_deleted_entries() {
        let mut s = Store::open_in_memory().unwrap();
        // Back-date an entry by 30 days.
        let backdated = (Utc::now() - Duration::days(30)).timestamp_millis();
        s.conn
            .execute(
                "INSERT INTO entries (text, created_at, due_today)
                 VALUES (?1, ?2, 0)",
                params!["old thought", backdated],
            )
            .unwrap();
        s.entries = Store::load_live(&s.conn).unwrap();
        let id = s.entries[0].id;

        // Delete it, then try to surface — should pick nothing.
        s.delete(id, Utc::now()).unwrap();
        assert!(s.pick_resurface(Utc::now()).unwrap().is_none());
    }
}
