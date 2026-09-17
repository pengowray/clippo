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

/// Macro settings as read from the config file.
pub struct Loaded {
    pub macros: Vec<Macro>,
    /// Re-copy the previous clipboard entry after a macro paste (design 9).
    pub restore_clipboard: bool,
}

/// `[[macros.items]]` from a parsed config file; `None` when it has none.
pub fn read_from_doc(doc: &toml_edit::DocumentMut) -> Option<Vec<Macro>> {
    let items = doc
        .get("macros")?
        .get("items")?
        .as_array_of_tables()?;
    Some(
        items
            .iter()
            .filter_map(|t| {
                Some(Macro {
                    format: t.get("format")?.as_str()?.to_string(),
                    label: t
                        .get("label")
                        .and_then(toml_edit::Item::as_str)
                        .map(str::to_string),
                })
            })
            .collect(),
    )
}

// TODO(backend): read these from `Config` once main adds `[macros]` to it. Until then the
// settings page writes them with toml_edit and this reads them back the same way.
pub fn load(paths: &crate::config::Paths) -> Loaded {
    let doc = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|s| s.parse::<toml_edit::DocumentMut>().ok());
    let macros = doc
        .as_ref()
        .and_then(read_from_doc)
        .filter(|m| !m.is_empty())
        .unwrap_or_else(defaults);
    let restore_clipboard = doc
        .as_ref()
        .and_then(|d| d.get("macros")?.get("restore_clipboard")?.as_bool())
        .unwrap_or(true);
    Loaded {
        macros: macros.into_iter().take(MAX_MACROS).collect(),
        restore_clipboard,
    }
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
