//! The ingest skip list: hashes of copies that must not be recorded (macro pastes).
//!
//! `$XDG_RUNTIME_DIR/clippo/skip` holds one `<unix_secs> <hash>` per line. `clippo macro`
//! adds the hash before copying; `ingest` drops a copy whose hash is listed and removes the
//! line. Lines older than `MAX_AGE` are ignored and cleaned up, so a copy that never arrived
//! cannot hide a later identical one. Cross-process, and no socket needed.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

const MAX_AGE_SECS: u64 = 10;

pub fn path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("skip")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn live_lines(file: &Path) -> Vec<(u64, String)> {
    let now = now_secs();
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (ts, hash) = l.split_once(' ')?;
            let ts: u64 = ts.parse().ok()?;
            (now.saturating_sub(ts) <= MAX_AGE_SECS).then(|| (ts, hash.to_string()))
        })
        .collect()
}

fn write_lines(file: &Path, lines: &[(u64, String)]) -> Result<()> {
    if lines.is_empty() {
        let _ = std::fs::remove_file(file);
        return Ok(());
    }
    let body: String = lines.iter().map(|(ts, h)| format!("{ts} {h}\n")).collect();
    // Write then rename so ingest never reads a partial file.
    let tmp = file.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, file)?;
    Ok(())
}

/// Ask `ingest` to drop the next copy with this hash.
pub fn add(runtime_dir: &Path, hash: &str) -> Result<()> {
    std::fs::create_dir_all(runtime_dir)
        .with_context(|| format!("could not create {}", runtime_dir.display()))?;
    let file = path(runtime_dir);
    let mut lines = live_lines(&file);
    lines.push((now_secs(), hash.to_string()));
    write_lines(&file, &lines)
}

/// Whether `hash` is listed; if so, the line is removed. Missing or stale files mean false.
pub fn take(runtime_dir: &Path, hash: &str) -> bool {
    let file = path(runtime_dir);
    if !file.exists() {
        return false;
    }
    let lines = live_lines(&file);
    let kept: Vec<_> = lines.iter().filter(|(_, h)| h != hash).cloned().collect();
    let found = kept.len() != lines.len();
    if let Err(e) = write_lines(&file, &kept) {
        crate::log(&format!("skip list: could not update {}: {e}", file.display()));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_hash_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let rt = dir.path().join("clippo");
        assert!(!take(&rt, "abc"));
        add(&rt, "abc").unwrap();
        add(&rt, "def").unwrap();
        assert!(!take(&rt, "zzz"));
        assert!(take(&rt, "abc"));
        assert!(!take(&rt, "abc"));
        assert!(take(&rt, "def"));
        assert!(!path(&rt).exists());
    }

    #[test]
    fn stale_lines_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let rt = dir.path().to_path_buf();
        std::fs::write(path(&rt), format!("{} old\n{} new\n", now_secs() - 60, now_secs())).unwrap();
        assert!(!take(&rt, "old"));
        assert!(take(&rt, "new"));
    }
}
