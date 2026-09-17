//! Macros: paste the current time or date in a configured strftime format (design 9).
//!
//! The text is copied, pasted, and not recorded in history (see `skip.rs`). With
//! `macros.restore_clipboard`, the entry that was on the clipboard before is put back
//! afterwards, so a macro does not cost the user what they had copied.

use std::fmt::Write as _;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use crate::clipboard::{self, CopyMode};
use crate::config::{Config, Macro, Paths};
use crate::ingest::{TEXT_MIME, preferred_image_type, sniff_image};
use crate::paste;
use crate::skip;
use crate::store::{Store, content_hash};

/// Time for the pasted text to be read by the target app before the clipboard changes again.
const RESTORE_DELAY: Duration = Duration::from_millis(300);

/// The macro's text for `now`. Errors name the macro, since chrono only says "invalid".
pub fn render(m: &Macro, now: &chrono::DateTime<chrono::Local>) -> Result<String> {
    let mut s = String::new();
    write!(s, "{}", now.format(&m.format))
        .map_err(|_| anyhow!("invalid macro format {:?}", m.format))?;
    Ok(s)
}

/// The text macro `n` (1-based) would paste right now.
pub fn value(cfg: &Config, n: usize) -> Result<String> {
    let m = cfg
        .macros
        .items
        .get(n.wrapping_sub(1))
        .ok_or_else(|| match cfg.macros.items.len() {
            0 => anyhow!("no macros are configured"),
            1 => anyhow!("there is only 1 macro"),
            count => anyhow!("there are only {count} macros"),
        })?;
    render(m, &chrono::Local::now())
}

/// Lines for `clippo macro --list`: `n<TAB>value<TAB>format[<TAB>label]`.
pub fn list(cfg: &Config) -> Vec<String> {
    let now = chrono::Local::now();
    cfg.macros
        .items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let value = render(m, &now).unwrap_or_else(|e| format!("({e})"));
            let mut line = format!("{}\t{value}\t{}", i + 1, m.format);
            if let Some(label) = &m.label {
                line.push('\t');
                line.push_str(label);
            }
            line
        })
        .collect()
}

/// Copy macro `n`'s text, paste it, and put the previous clipboard back.
pub fn run(cfg: &Config, paths: &Paths, n: usize) -> Result<()> {
    let text = value(cfg, n)?;
    if text.is_empty() {
        bail!("macro {n} produces no text");
    }
    let store = Store::open(&paths.db)?;
    let previous = if cfg.macros.restore_clipboard {
        current_entry(&store)
    } else {
        None
    };

    skip::add(&paths.runtime_dir, &content_hash(TEXT_MIME, text.as_bytes()))?;
    clipboard::copy(Some(TEXT_MIME), text.as_bytes())?;
    paste::send(paths, &cfg.paste, false)?;

    if let Some(id) = previous {
        sleep(RESTORE_DELAY);
        // Re-copying the same content is a dedupe bump, so history is unchanged.
        clipboard::copy_entry(paths, &store, id, CopyMode::Full)?;
    }
    Ok(())
}

/// The history entry that matches what is on the clipboard now, if clippo recorded it.
fn current_entry(store: &Store) -> Option<i64> {
    let types = clipboard::list_types();
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    let (mime, data) = if let Some(t) = preferred_image_type(&types) {
        let img = clipboard::paste(&["--no-newline", "--type", &t])?;
        (sniff_image(&img)?, img)
    } else if types.iter().any(|t| t.starts_with("text/plain")) {
        (TEXT_MIME, clipboard::paste(&["--no-newline", "--type", "text"])?)
    } else {
        return None;
    };
    store.find(mime, &data).ok().flatten().map(|e| e.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_defaults_and_rejects_bad_formats() {
        let cfg = Config::default();
        let now = chrono::Local::now();
        let date = render(&cfg.macros.items[1], &now).unwrap();
        assert_eq!(date, now.format("%Y-%m-%d").to_string());
        assert_eq!(render(&cfg.macros.items[0], &now).unwrap().len(), 5);
        let bad = Macro {
            format: "%Q".into(),
            label: None,
        };
        assert!(render(&bad, &now).unwrap_err().to_string().contains("%Q"));
    }

    #[test]
    fn value_is_one_based() {
        let cfg = Config::default();
        assert!(value(&cfg, 1).is_ok());
        assert!(value(&cfg, 3).is_ok());
        assert_eq!(value(&cfg, 0).unwrap_err().to_string(), "there are only 3 macros");
        assert_eq!(value(&cfg, 4).unwrap_err().to_string(), "there are only 3 macros");
        assert_eq!(list(&cfg).len(), 3);
        assert!(list(&cfg)[2].ends_with("\t%Y-%m-%d %H:%M:%S"));
    }
}
