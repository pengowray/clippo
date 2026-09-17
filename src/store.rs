use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

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
pub struct Summary {
    pub id: i64,
    pub mime: String,
    pub preview: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub ocr_status: OcrStatus,
    pub ocr_text: Option<String>,
}

impl Summary {
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }
}

pub struct NewEntry<'a> {
    pub mime: &'a str,
    pub content: &'a [u8],
    pub dims: Option<(u32, u32)>,
    pub ocr_status: OcrStatus,
}

pub struct Store {
    conn: Connection,
}

const PREVIEW_CHARS: usize = 400;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    hash       TEXT NOT NULL UNIQUE,
    mime       TEXT NOT NULL,
    content    BLOB NOT NULL,
    preview    TEXT,
    width      INTEGER,
    height     INTEGER,
    created_at INTEGER NOT NULL,
    last_used  INTEGER NOT NULL,
    ocr_status TEXT NOT NULL DEFAULT 'none',
    ocr_text   TEXT
);
CREATE INDEX IF NOT EXISTS entries_last_used ON entries(last_used);
CREATE INDEX IF NOT EXISTS entries_ocr_status ON entries(ocr_status);
";

const SUMMARY_COLS: &str = "id, mime, preview, width, height, ocr_status, ocr_text";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn content_hash(mime: &str, content: &[u8]) -> String {
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
    })
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
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Insert an entry, or bump an identical existing one to the top.
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
                "UPDATE entries SET last_used = ?1 WHERE id = ?2",
                params![ts, id],
            )?;
            (id, false)
        } else {
            let preview = e.mime.starts_with("text/").then(|| {
                String::from_utf8_lossy(e.content)
                    .chars()
                    .take(PREVIEW_CHARS)
                    .collect::<String>()
            });
            tx.execute(
                "INSERT INTO entries (hash, mime, content, preview, width, height, created_at, last_used, ocr_status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
                params![
                    hash,
                    e.mime,
                    e.content,
                    preview,
                    e.dims.map(|d| d.0),
                    e.dims.map(|d| d.1),
                    ts,
                    e.ocr_status.as_str()
                ],
            )?;
            (tx.last_insert_rowid(), true)
        };
        tx.commit()?;
        Ok(result)
    }

    /// The stored entry with exactly this type and content, if any.
    pub fn find(&self, mime: &str, content: &[u8]) -> Result<Option<Summary>> {
        let hash = content_hash(mime, content);
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS} FROM entries WHERE hash = ?1"),
                [&hash],
                summary_from_row,
            )
            .optional()?)
    }

    /// Delete all but the `max_items` most recently used entries. Returns removed ids.
    pub fn enforce_cap(&self, max_items: usize) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM entries ORDER BY last_used DESC, id DESC LIMIT -1 OFFSET ?1",
        )?;
        let ids: Vec<i64> = stmt
            .query_map([max_items as i64], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for id in &ids {
            self.conn
                .execute("DELETE FROM entries WHERE id = ?1", [id])?;
        }
        Ok(ids)
    }

    /// All entries, most recently used first.
    pub fn list(&self) -> Result<Vec<Summary>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SUMMARY_COLS} FROM entries ORDER BY last_used DESC, id DESC"
        ))?;
        let rows = stmt
            .query_map([], summary_from_row)?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn summary(&self, id: i64) -> Result<Option<Summary>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS} FROM entries WHERE id = ?1"),
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

    /// Most recently used entry still waiting for OCR.
    pub fn next_pending_ocr(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM entries WHERE ocr_status = 'pending' ORDER BY last_used DESC LIMIT 1",
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

    pub fn delete(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM entries WHERE id = ?1", [id])?
            > 0)
    }

    pub fn clear(&self) -> Result<usize> {
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
        };
        let (id, _) = s.upsert(&img).unwrap();
        assert_eq!(s.next_pending_ocr().unwrap(), Some(id));
        s.set_ocr(id, OcrStatus::Done, Some("hello")).unwrap();
        let sum = s.summary(id).unwrap().unwrap();
        assert_eq!(sum.ocr_status, OcrStatus::Done);
        assert_eq!(sum.ocr_text.as_deref(), Some("hello"));
        assert_eq!((sum.width, sum.height), (Some(10), Some(20)));
        assert_eq!(s.next_pending_ocr().unwrap(), None);
    }
}
