//! Macro column: date/time buttons whose label is the live value (design 9).

use std::path::PathBuf;

use chrono::Local;

/// Most macros the column holds: Alt+1 to Alt+9.
pub const MAX_MACROS: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    /// strftime format, rendered with `chrono`.
    pub format: String,
    /// Optional fixed label; the live value then shows muted underneath.
    pub label: Option<String>,
}

impl Macro {
    pub fn value(&self) -> String {
        Local::now().format(&self.format).to_string()
    }
}

/// The three defaults from design 9, in order of expected use.
pub fn defaults() -> Vec<Macro> {
    ["%H:%M", "%Y-%m-%d", "%Y-%m-%d %H:%M:%S"]
        .into_iter()
        .map(|f| Macro {
            format: f.to_string(),
            label: None,
        })
        .collect()
}

// TODO(backend): load `[[macros]]` from `Config` once main adds it; `defaults()` is the
// fallback when the config has none.
pub fn from_config(_cfg: &crate::config::Config) -> Vec<Macro> {
    defaults()
}

fn skip_file() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("clippo")
        .join("skip")
}

/// Ask `ingest` not to record the next copy of `text` (design 9). One hash per line;
/// entries older than 10 s are ignored by the reader.
// TODO(backend): use the backend's `content_hash` and skip-file reader once main lands
// them. This writes the same sha256(mime, 0, bytes) hex that store.rs computes today.
pub fn mark_skip(text: &str) -> std::io::Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let mut h = Sha256::new();
    h.update(crate::ingest::TEXT_MIME.as_bytes());
    h.update([0]);
    h.update(text.as_bytes());
    let hash: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let path = skip_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    writeln!(f, "{secs} {hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_render() {
        let m = defaults();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].value().len(), 5); // HH:MM
        assert_eq!(m[1].value().len(), 10); // YYYY-MM-DD
        assert_eq!(m[2].value().len(), 19);
    }
}
