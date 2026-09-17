use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::markdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrStatus {
    /// Not applicable (text entries, or OCR turned off).
    None,
    Pending,
    Done,
    Failed,
}

impl OcrStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::None,
        }
    }
}

/// Entry metadata without the content blob.
#[derive(Debug, Clone)]
#[allow(dead_code)] // the length, Markdown and rich-format fields are for the history window
pub struct Summary {
    pub id: i64,
    pub mime: String,
    pub preview: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub ocr_status: OcrStatus,
    pub ocr_text: Option<String>,
    /// Milliseconds since the Unix epoch; bumped on every re-copy.
    pub last_used: i64,
    /// Chars for text, bytes for images.
    pub content_len: usize,
    /// Text entries only.
    pub line_count: Option<usize>,
    pub is_markdown: bool,
    /// An extra format (HTML or RTF) is stored alongside the primary content.
    pub has_rich: bool,
}

impl Summary {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// Used at or after `cutoff` (see [`Store::recent_cutoff`]).
    pub fn is_recent(&self, cutoff: i64) -> bool {
        self.last_used >= cutoff
    }
}

/// An extra clipboard type stored next to an entry's primary content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    pub mime: String,
    pub content: Vec<u8>,
}

pub struct NewEntry<'a> {
    pub mime: &'a str,
    pub content: &'a [u8],
    pub dims: Option<(u32, u32)>,
    pub ocr_status: OcrStatus,
    /// Extra formats the copy offered (already filtered by `ingest`).
    pub formats: &'a [Format],
}

pub struct Store {
    conn: Connection,
}

const PREVIEW_CHARS: usize = 400;

/// Entries used within this window count as recent; older ones fold away in the menu.
pub const RECENT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Deleted entries can be undone for this long before they are purged.
pub const UNDO_GRACE: Duration = Duration::from_secs(60);

/// Bumped whenever `migrate` learns a new step. Stored in `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    hash        TEXT NOT NULL UNIQUE,
    mime        TEXT NOT NULL,
    content     BLOB NOT NULL,
    preview     TEXT,
    width       INTEGER,
    height      INTEGER,
    created_at  INTEGER NOT NULL,
    last_used   INTEGER NOT NULL,
    ocr_status  TEXT NOT NULL DEFAULT 'none',
    ocr_text    TEXT,
    content_len INTEGER NOT NULL DEFAULT 0,
    line_count  INTEGER,
    is_markdown INTEGER NOT NULL DEFAULT 0,
    deleted_at  INTEGER
);
CREATE INDEX IF NOT EXISTS entries_last_used ON entries(last_used);
CREATE INDEX IF NOT EXISTS entries_ocr_status ON entries(ocr_status);
CREATE TABLE IF NOT EXISTS formats (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    mime     TEXT NOT NULL,
    content  BLOB NOT NULL,
    PRIMARY KEY (entry_id, mime)
);
";

/// Columns added by schema version 1, for databases created before it.
const V1_COLUMNS: [(&str, &str); 4] = [
    ("content_len", "INTEGER NOT NULL DEFAULT 0"),
    ("line_count", "INTEGER"),
    ("is_markdown", "INTEGER NOT NULL DEFAULT 0"),
    ("deleted_at", "INTEGER"),
];

const SUMMARY_COLS: &str = "id, mime, preview, width, height, ocr_status, ocr_text, last_used, \
     content_len, line_count, is_markdown, \
     EXISTS(SELECT 1 FROM formats WHERE formats.entry_id = entries.id)";

const LIVE: &str = "deleted_at IS NULL";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// The dedupe key. Only the primary content counts; extra formats never change it.
pub fn content_hash(mime: &str, content: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(mime.as_bytes());
    h.update([0]);
    h.update(content);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn summary_from_row(row: &rusqlite::Row) -> rusqlite::Result<Summary> {
    Ok(Summary {
        id: row.get(0)?,
        mime: row.get(1)?,
        preview: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        ocr_status: OcrStatus::parse(&row.get::<_, String>(5)?),
        ocr_text: row.get(6)?,
        last_used: row.get(7)?,
        content_len: row.get::<_, i64>(8)? as usize,
        line_count: row.get::<_, Option<i64>>(9)?.map(|n| n as usize),
        is_markdown: row.get(10)?,
        has_rich: row.get(11)?,
    })
}

/// What `upsert` derives from a text entry's content.
struct TextMeta {
    preview: String,
    len: usize,
    lines: usize,
    is_markdown: bool,
}

fn text_meta(content: &[u8]) -> TextMeta {
    let text = String::from_utf8_lossy(content);
    TextMeta {
        // Kept verbatim (no trimming) so the menu can show the real shape.
        preview: text.chars().take(PREVIEW_CHARS).collect(),
        len: text.chars().count(),
        lines: text.lines().count(),
        is_markdown: markdown::looks_like(&text),
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("could not open database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Bring a database from an earlier clippo up to the current schema.
    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version >= SCHEMA_VERSION {
            return Ok(());
        }
        let existing: Vec<String> = self
            .conn
            .prepare("PRAGMA table_info(entries)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<_, _>>()?;
        let mut added = false;
        for (name, decl) in V1_COLUMNS {
            if !existing.iter().any(|c| c == name) {
                self.conn
                    .execute(&format!("ALTER TABLE entries ADD COLUMN {name} {decl}"), [])?;
                added = true;
            }
        }
        if added {
            self.conn.execute(
                "UPDATE entries SET content_len = length(content) WHERE mime NOT LIKE 'text/%'",
                [],
            )?;
            // Text needs char counts and Markdown detection, which SQL can't do.
            let rows: Vec<(i64, Vec<u8>)> = self
                .conn
                .prepare("SELECT id, content FROM entries WHERE mime LIKE 'text/%'")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            for (id, content) in rows {
                let meta = text_meta(&content);
                self.conn.execute(
                    "UPDATE entries SET content_len = ?1, line_count = ?2, is_markdown = ?3 WHERE id = ?4",
                    params![meta.len as i64, meta.lines as i64, meta.is_markdown, id],
                )?;
            }
        }
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// Insert an entry, or bump an identical existing one to the top (undeleting it if needed).
    /// Returns the entry id and whether it was newly inserted.
    pub fn upsert(&mut self, e: &NewEntry) -> Result<(i64, bool)> {
        let hash = content_hash(e.mime, e.content);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Strictly increasing so ordering is stable even within one millisecond.
        let max: Option<i64> =
            tx.query_row("SELECT MAX(last_used) FROM entries", [], |r| r.get(0))?;
        let ts = now_ms().max(max.map_or(0, |m| m + 1));

        let existing: Option<i64> = tx
            .query_row("SELECT id FROM entries WHERE hash = ?1", [&hash], |r| {
                r.get(0)
            })
            .optional()?;
        let result = if let Some(id) = existing {
            tx.execute(
                "UPDATE entries SET last_used = ?1, deleted_at = NULL WHERE id = ?2",
                params![ts, id],
            )?;
            // The newest copy's formats win, but a copy with none (e.g. `clippo plain`
            // re-copying the text) keeps what was stored rather than losing the formatting.
            if !e.formats.is_empty() {
                tx.execute("DELETE FROM formats WHERE entry_id = ?1", [id])?;
            }
            (id, false)
        } else {
            let meta = e.mime.starts_with("text/").then(|| text_meta(e.content));
            tx.execute(
                "INSERT INTO entries (hash, mime, content, preview, width, height, created_at, last_used,
                                      ocr_status, content_len, line_count, is_markdown)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10, ?11)",
                params![
                    hash,
                    e.mime,
                    e.content,
                    meta.as_ref().map(|m| m.preview.as_str()),
                    e.dims.map(|d| d.0),
                    e.dims.map(|d| d.1),
                    ts,
                    e.ocr_status.as_str(),
                    meta.as_ref().map_or(e.content.len(), |m| m.len) as i64,
                    meta.as_ref().map(|m| m.lines as i64),
                    meta.as_ref().is_some_and(|m| m.is_markdown),
                ],
            )?;
            (tx.last_insert_rowid(), true)
        };
        for f in e.formats {
            tx.execute(
                "INSERT OR REPLACE INTO formats (entry_id, mime, content) VALUES (?1, ?2, ?3)",
                params![result.0, f.mime, f.content],
            )?;
        }
        tx.commit()?;
        Ok(result)
    }

    /// The stored entry with exactly this type and content, if any.
    pub fn find(&self, mime: &str, content: &[u8]) -> Result<Option<Summary>> {
        let hash = content_hash(mime, content);
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS} FROM entries WHERE hash = ?1 AND {LIVE}"),
                [&hash],
                summary_from_row,
            )
            .optional()?)
    }

    fn remove_ids(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.conn
                .execute("DELETE FROM formats WHERE entry_id = ?1", [id])?;
            self.conn
                .execute("DELETE FROM entries WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    fn select_ids(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(sql)?;
        let ids = stmt
            .query_map(p, |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
    }

    /// Delete all but the `max_items` most recently used entries. Returns removed ids.
    pub fn enforce_cap(&self, max_items: usize) -> Result<Vec<i64>> {
        let ids = self.select_ids(
            "SELECT id FROM entries ORDER BY last_used DESC, id DESC LIMIT -1 OFFSET ?1",
            [max_items as i64],
        )?;
        self.remove_ids(&ids)?;
        Ok(ids)
    }

    /// Delete entries not used for `days` days. `0` turns expiry off. Returns removed ids.
    pub fn expire(&self, days: u64) -> Result<Vec<i64>> {
        if days == 0 {
            return Ok(Vec::new());
        }
        let cutoff = now_ms() - (days as i64) * 86_400_000;
        let ids = self.select_ids("SELECT id FROM entries WHERE last_used < ?1", [cutoff])?;
        self.remove_ids(&ids)?;
        Ok(ids)
    }

    /// Hard-delete entries whose undo window has passed. Returns removed ids.
    pub fn purge_deleted(&self) -> Result<Vec<i64>> {
        let cutoff = now_ms() - UNDO_GRACE.as_millis() as i64;
        let ids = self.select_ids(
            "SELECT id FROM entries WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            [cutoff],
        )?;
        self.remove_ids(&ids)?;
        Ok(ids)
    }

    /// All entries, most recently used first.
    pub fn list(&self) -> Result<Vec<Summary>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SUMMARY_COLS} FROM entries WHERE {LIVE} ORDER BY last_used DESC, id DESC"
        ))?;
        let rows = stmt
            .query_map([], summary_from_row)?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// `last_used` boundary between recent and older entries, as of now.
    pub fn recent_cutoff() -> i64 {
        now_ms() - RECENT_WINDOW.as_millis() as i64
    }

    /// `list`, split into entries used since `cutoff` and the rest. Both keep list order.
    pub fn list_split(&self, cutoff: i64) -> Result<(Vec<Summary>, Vec<Summary>)> {
        Ok(self
            .list()?
            .into_iter()
            .partition(|s| s.is_recent(cutoff)))
    }

    /// Entries used since `cutoff`, most recently used first.
    pub fn list_since(&self, cutoff: i64) -> Result<Vec<Summary>> {
        Ok(self.list_split(cutoff)?.0)
    }

    pub fn summary(&self, id: i64) -> Result<Option<Summary>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS} FROM entries WHERE id = ?1 AND {LIVE}"),
                [id],
                summary_from_row,
            )
            .optional()?)
    }

    pub fn content(&self, id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT content FROM entries WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Extra formats stored for an entry, in a stable order.
    pub fn formats(&self, id: i64) -> Result<Vec<Format>> {
        let mut stmt = self
            .conn
            .prepare("SELECT mime, content FROM formats WHERE entry_id = ?1 ORDER BY mime")?;
        let rows = stmt
            .query_map([id], |r| {
                Ok(Format {
                    mime: r.get(0)?,
                    content: r.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Most recently used entry still waiting for OCR.
    pub fn next_pending_ocr(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT id FROM entries WHERE ocr_status = 'pending' AND {LIVE} \
                     ORDER BY last_used DESC LIMIT 1"
                ),
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_ocr(&self, id: i64, status: OcrStatus, text: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE entries SET ocr_status = ?1, ocr_text = ?2 WHERE id = ?3",
            params![status.as_str(), text, id],
        )?;
        Ok(())
    }

    /// Number of entries, and how many images still wait for OCR.
    pub fn counts(&self) -> Result<(usize, usize)> {
        let total: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM entries WHERE {LIVE}"),
            [],
            |r| r.get(0),
        )?;
        let pending: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM entries WHERE ocr_status = 'pending' AND {LIVE}"),
            [],
            |r| r.get(0),
        )?;
        Ok((total as usize, pending as usize))
    }

    /// Hide an entry. It can be brought back with `undelete` until the next purge.
    pub fn delete(&self, id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            &format!("UPDATE entries SET deleted_at = ?1 WHERE id = ?2 AND {LIVE}"),
            params![now_ms(), id],
        )? > 0)
    }

    /// Bring back a deleted entry at its old position. False if there is nothing to restore.
    pub fn undelete(&self, id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE entries SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        )? > 0)
    }

    pub fn clear(&self) -> Result<usize> {
        self.conn.execute("DELETE FROM formats", [])?;
        Ok(self.conn.execute("DELETE FROM entries", [])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> NewEntry<'_> {
        NewEntry {
            mime: "text/plain",
            content: s.as_bytes(),
            dims: None,
            ocr_status: OcrStatus::None,
            formats: &[],
        }
    }

    fn fmt(mime: &str, body: &str) -> Format {
        Format {
            mime: mime.into(),
            content: body.as_bytes().to_vec(),
        }
    }

    fn ids(store: &Store) -> Vec<i64> {
        store.list().unwrap().iter().map(|s| s.id).collect()
    }

    #[test]
    fn dedupe_bumps_existing_entry() {
        let mut s = Store::open_in_memory().unwrap();
        let (a, new_a) = s.upsert(&text("alpha")).unwrap();
        let (b, _) = s.upsert(&text("beta")).unwrap();
        assert!(new_a);
        assert_eq!(ids(&s), vec![b, a]);

        let (a2, new_a2) = s.upsert(&text("alpha")).unwrap();
        assert_eq!(a2, a);
        assert!(!new_a2);
        assert_eq!(ids(&s), vec![a, b]);
    }

    #[test]
    fn same_bytes_different_mime_are_distinct() {
        let mut s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert(&text("x")).unwrap();
        let other = NewEntry {
            mime: "text/html",
            ..text("x")
        };
        let (b, _) = s.upsert(&other).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn cap_removes_least_recently_used() {
        let mut s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert(&text("a")).unwrap();
        let (b, _) = s.upsert(&text("b")).unwrap();
        let (c, _) = s.upsert(&text("c")).unwrap();
        s.upsert(&text("a")).unwrap(); // a is now newest; b is oldest
        let removed = s.enforce_cap(2).unwrap();
        assert_eq!(removed, vec![b]);
        assert_eq!(ids(&s), vec![a, c]);
        assert!(s.enforce_cap(2).unwrap().is_empty());
    }

    #[test]
    fn ocr_status_roundtrip() {
        let mut s = Store::open_in_memory().unwrap();
        let img = NewEntry {
            mime: "image/png",
            content: b"fake",
            dims: Some((10, 20)),
            ocr_status: OcrStatus::Pending,
            formats: &[],
        };
        let (id, _) = s.upsert(&img).unwrap();
        assert_eq!(s.next_pending_ocr().unwrap(), Some(id));
        s.set_ocr(id, OcrStatus::Done, Some("hello")).unwrap();
        let sum = s.summary(id).unwrap().unwrap();
        assert_eq!(sum.ocr_status, OcrStatus::Done);
        assert_eq!(sum.ocr_text.as_deref(), Some("hello"));
        assert_eq!((sum.width, sum.height), (Some(10), Some(20)));
        assert_eq!(sum.content_len, 4);
        assert_eq!(sum.line_count, None);
        assert_eq!(s.next_pending_ocr().unwrap(), None);
    }

    #[test]
    fn text_metadata_is_stored() {
        let mut s = Store::open_in_memory().unwrap();
        let body = "# Title\n\n**bold** and [a](http://x)\nñ";
        let (id, _) = s.upsert(&text(body)).unwrap();
        let sum = s.summary(id).unwrap().unwrap();
        assert_eq!(sum.content_len, body.chars().count());
        assert_eq!(sum.line_count, Some(4));
        assert!(sum.is_markdown);
        assert!(!sum.has_rich);
        assert_eq!(sum.preview.as_deref(), Some(body));

        let (plain, _) = s.upsert(&text("  just words\n")).unwrap();
        let sum = s.summary(plain).unwrap().unwrap();
        assert!(!sum.is_markdown);
        assert_eq!(sum.preview.as_deref(), Some("  just words\n"));
    }

    #[test]
    fn formats_are_stored_and_replaced_by_richer_copies() {
        let mut s = Store::open_in_memory().unwrap();
        let rich = [fmt("text/html", "<b>hi</b>"), fmt("text/rtf", "{\\rtf1 hi}")];
        let (id, _) = s
            .upsert(&NewEntry {
                formats: &rich,
                ..text("hi")
            })
            .unwrap();
        assert!(s.summary(id).unwrap().unwrap().has_rich);
        assert_eq!(s.formats(id).unwrap(), rich.to_vec());

        // Same text without formats: a bump that keeps the stored ones.
        s.upsert(&text("hi")).unwrap();
        assert_eq!(s.formats(id).unwrap().len(), 2);

        // Same text with new formats: newest wins.
        let newer = [fmt("text/html", "<i>hi</i>")];
        s.upsert(&NewEntry {
            formats: &newer,
            ..text("hi")
        })
        .unwrap();
        assert_eq!(s.formats(id).unwrap(), newer.to_vec());

        // A hard delete takes the formats with it.
        s.enforce_cap(0).unwrap();
        assert!(s.formats(id).unwrap().is_empty());
    }

    #[test]
    fn delete_is_undoable_until_purged() {
        let mut s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert(&text("a")).unwrap();
        let (b, _) = s.upsert(&text("b")).unwrap();
        assert!(s.delete(a).unwrap());
        assert!(!s.delete(a).unwrap());
        assert_eq!(ids(&s), vec![b]);
        assert!(s.summary(a).unwrap().is_none());
        assert!(s.find("text/plain", b"a").unwrap().is_none());
        assert_eq!(s.counts().unwrap(), (1, 0));

        assert!(s.undelete(a).unwrap());
        assert!(!s.undelete(a).unwrap());
        assert_eq!(ids(&s), vec![b, a]); // back at its original position

        s.delete(a).unwrap();
        assert!(s.purge_deleted().unwrap().is_empty()); // still inside the undo window
        s.conn
            .execute(
                "UPDATE entries SET deleted_at = deleted_at - 120000 WHERE id = ?1",
                [a],
            )
            .unwrap();
        assert_eq!(s.purge_deleted().unwrap(), vec![a]);
        assert!(s.content(a).unwrap().is_none());
    }

    #[test]
    fn recopy_revives_deleted_entry() {
        let mut s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert(&text("a")).unwrap();
        s.delete(a).unwrap();
        let (a2, new) = s.upsert(&text("a")).unwrap();
        assert_eq!((a2, new), (a, false));
        assert_eq!(ids(&s), vec![a]);
    }

    #[test]
    fn expiry_and_recent_split() {
        let mut s = Store::open_in_memory().unwrap();
        let (old, _) = s.upsert(&text("old")).unwrap();
        let (fresh, _) = s.upsert(&text("fresh")).unwrap();
        let two_days: i64 = 2 * 86_400_000;
        s.conn
            .execute(
                "UPDATE entries SET last_used = last_used - ?1 WHERE id = ?2",
                params![two_days, old],
            )
            .unwrap();
        let (recent, older) = s.list_split(Store::recent_cutoff()).unwrap();
        assert_eq!(recent.iter().map(|e| e.id).collect::<Vec<_>>(), vec![fresh]);
        assert_eq!(older.iter().map(|e| e.id).collect::<Vec<_>>(), vec![old]);
        assert_eq!(s.list_since(Store::recent_cutoff()).unwrap().len(), 1);

        assert!(s.expire(0).unwrap().is_empty());
        assert!(s.expire(7).unwrap().is_empty());
        assert_eq!(s.expire(1).unwrap(), vec![old]);
        assert_eq!(ids(&s), vec![fresh]);
    }

    #[test]
    fn migrates_a_version_zero_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT, hash TEXT NOT NULL UNIQUE, mime TEXT NOT NULL,
                content BLOB NOT NULL, preview TEXT, width INTEGER, height INTEGER,
                created_at INTEGER NOT NULL, last_used INTEGER NOT NULL,
                ocr_status TEXT NOT NULL DEFAULT 'none', ocr_text TEXT);
             INSERT INTO entries (hash, mime, content, preview, created_at, last_used)
                VALUES ('h1', 'text/plain;charset=utf-8', CAST('# Hi' || char(10) || '- a' || char(10) || '- b' || char(10) || 'ñ' AS BLOB), '# Hi', 1, 1);
             INSERT INTO entries (hash, mime, content, created_at, last_used, ocr_status)
                VALUES ('h2', 'image/png', X'00010203', 2, 2, 'pending');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let v: i64 = s
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let list = s.list().unwrap();
        assert_eq!(list.len(), 2);
        let img = &list[0];
        assert_eq!((img.content_len, img.line_count, img.is_markdown), (4, None, false));
        let txt = &list[1];
        assert_eq!((txt.content_len, txt.line_count, txt.is_markdown), (14, Some(4), true));
        assert!(!txt.has_rich);
        assert!(s.formats(txt.id).unwrap().is_empty());
        assert!(s.delete(txt.id).unwrap());
        assert!(s.undelete(txt.id).unwrap());
    }
}
