use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::clipboard;
use crate::config::{Config, Paths};
use crate::paste;
use crate::store::{Store, Summary};
use crate::thumbs;

const TEXT_LABEL_CHARS: usize = 80;
const OCR_LABEL_CHARS: usize = 200;
const PROMPT: &str = "Clipboard: ";

/// Collapse runs of whitespace (and control chars) to single spaces, then
/// truncate to `max` chars with an ellipsis.
pub fn collapse(s: &str, max: usize) -> String {
    let words: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect();
    let joined = words.join(" ");
    if joined.chars().count() <= max {
        joined
    } else {
        let mut t: String = joined.chars().take(max.saturating_sub(1)).collect();
        t.truncate(t.trim_end().len());
        t.push('…');
        t
    }
}

/// First non-blank line of a text entry, tidied for display.
pub fn text_label(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    collapse(line, TEXT_LABEL_CHARS)
}

pub fn image_label(width: Option<u32>, height: Option<u32>, ocr_text: Option<&str>) -> String {
    let mut label = match (width, height) {
        (Some(w), Some(h)) => format!("Image {w}×{h}"),
        _ => "Image".to_string(),
    };
    if let Some(ocr) = ocr_text
        .map(|t| collapse(t, OCR_LABEL_CHARS))
        .filter(|t| !t.is_empty())
    {
        label.push_str("  ");
        label.push_str(&ocr);
    }
    label
}

pub fn label(entry: &Summary) -> String {
    if entry.is_image() {
        image_label(entry.width, entry.height, entry.ocr_text.as_deref())
    } else {
        text_label(entry.preview.as_deref().unwrap_or(""))
    }
}

/// Entry id from a selected menu line (`"{id}\t{label}"`).
pub fn parse_selection(line: &str) -> Option<i64> {
    let (id, _) = line.split_once('\t')?;
    id.trim().parse().ok()
}

fn fuzzel_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "fuzzel"])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn run(cfg: &Config, store: &Store, paths: &Paths) -> Result<()> {
    // Pressing the shortcut again closes an open picker.
    if fuzzel_running() {
        let _ = Command::new("pkill").args(["-x", "fuzzel"]).status();
        return Ok(());
    }

    let mut input = String::new();
    for entry in store.list()? {
        input.push_str(&format!("{}\t{}", entry.id, label(&entry)));
        if entry.is_image() {
            let cached = thumbs::path(&paths.thumbs_dir, entry.id);
            let thumb = if cached.exists() {
                Some(cached)
            } else {
                store
                    .content(entry.id)?
                    .and_then(|c| thumbs::ensure(&paths.thumbs_dir, entry.id, &c).ok())
            };
            if let Some(path) = thumb {
                input.push_str(&format!("\0icon\x1f{}", path.display()));
            }
        }
        input.push('\n');
    }

    let mut child = Command::new("fuzzel")
        .args(["--dmenu", "--prompt", PROMPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("could not run fuzzel")?;
    // Dropping stdin at the end of this statement closes it so fuzzel shows the list.
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Ok(()); // cancelled
    }
    let selected = String::from_utf8_lossy(&output.stdout);
    let Some(id) = parse_selection(selected.trim_end_matches('\n')) else {
        return Ok(());
    };
    copy_entry(store, id)?;
    if cfg.paste.paste_on_select {
        paste::send(&cfg.paste)?;
    }
    Ok(())
}

/// Put an entry back on the clipboard with its original type.
pub fn copy_entry(store: &Store, id: i64) -> Result<()> {
    let (Some(entry), Some(content)) = (store.summary(id)?, store.content(id)?) else {
        bail!("no entry with id {id}");
    };
    clipboard::copy(Some(&entry.mime), &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_label_uses_first_nonblank_line_collapsed() {
        assert_eq!(
            text_label("\n  \n  hello \t  world  \nsecond"),
            "hello world"
        );
        assert_eq!(text_label("   "), "");
    }

    #[test]
    fn text_label_truncates() {
        let long = "x".repeat(200);
        let l = text_label(&long);
        assert_eq!(l.chars().count(), TEXT_LABEL_CHARS);
        assert!(l.ends_with('…'));
        assert_eq!(
            text_label(&"y".repeat(TEXT_LABEL_CHARS)),
            "y".repeat(TEXT_LABEL_CHARS)
        );
    }

    #[test]
    fn collapse_strips_control_chars() {
        assert_eq!(collapse("a\0b\x1fc", 80), "a b c");
    }

    #[test]
    fn image_labels() {
        assert_eq!(image_label(Some(640), Some(480), None), "Image 640×480");
        assert_eq!(image_label(Some(1), Some(2), Some("  ")), "Image 1×2");
        assert_eq!(
            image_label(Some(10), Some(20), Some("Hello\n  world")),
            "Image 10×20  Hello world"
        );
        assert_eq!(image_label(None, None, None), "Image");
    }

    #[test]
    fn parses_selection() {
        assert_eq!(parse_selection("42\tsome text\twith tab"), Some(42));
        assert_eq!(parse_selection("7\t"), Some(7));
        assert_eq!(parse_selection("typed text"), None);
        assert_eq!(parse_selection("abc\tlabel"), None);
        assert_eq!(parse_selection(""), None);
    }
}
