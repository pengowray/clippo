//! The live clipboard: reading it with `wl-paste`, and putting entries on it.
//!
//! Single-type copies go through `wl-copy`, which forks a process that serves the clipboard
//! until someone else copies. A copy that offers several types (the primary plus stored HTML
//! or RTF) uses `wl-clipboard-rs` instead: `wl-copy` serves one type per process and running
//! one per type does not work, as each takes ownership from the last. `wl-clipboard-rs` 0.9
//! binds `ext_data_control_v1` when the compositor has it and falls back to
//! `zwlr_data_control_v1`, so it works on COSMIC either way. Its source lives in a thread of
//! the calling process, so the resident `clippo watch` does the serving (see `service::serve`)
//! and the clipboard is lost if the service restarts, unlike `wl-copy`'s fork.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

use crate::config::Paths;
use crate::ingest::TEXT_MIME;
use crate::markdown;
use crate::service;
use crate::store::{Format, OcrStatus, Store};

/// How long to wait for the compositor to report our multi-type source as the owner.
const OWNERSHIP_WAIT: Duration = Duration::from_millis(500);

/// Run `wl-paste` with `args`; `None` if it fails (e.g. empty clipboard or type not offered).
pub fn paste(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Types offered by the current clipboard owner.
pub fn list_types() -> Vec<String> {
    let raw = paste(&["--list-types"]).unwrap_or_default();
    String::from_utf8_lossy(&raw)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Put `data` on the clipboard. With no `mime`, wl-copy offers the usual set of text types.
pub fn copy(mime: Option<&str>, data: &[u8]) -> Result<()> {
    let mut cmd = Command::new("wl-copy");
    if let Some(mime) = mime {
        cmd.args(["--type", mime]);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .spawn()
        .context("could not run wl-copy")?;
    child.stdin.take().expect("piped stdin").write_all(data)?;
    // wl-copy forks to serve the clipboard, so this returns once it has taken ownership.
    if !child.wait()?.success() {
        bail!("wl-copy failed");
    }
    Ok(())
}

/// Offer every format at once, from a thread of this process that serves until another
/// client takes the clipboard. The first format is the primary. Returns once the compositor
/// reports the new offer, so a paste keystroke sent afterwards gets this content.
pub fn copy_formats(formats: &[Format]) -> Result<()> {
    let primary = formats.first().ok_or_else(|| anyhow!("nothing to copy"))?;
    let sources = formats
        .iter()
        .map(|f| MimeSource {
            source: Source::Bytes(f.content.clone().into_boxed_slice()),
            mime_type: MimeType::Specific(f.mime.clone()),
        })
        .collect();
    Options::new()
        .copy_multi(sources)
        .map_err(|e| anyhow!("could not take the clipboard: {e}"))?;
    // copy_multi returns before its thread flushes set_selection; confirm ownership so "ok"
    // means what wl-copy's exit means.
    let start = Instant::now();
    loop {
        if list_types().contains(&primary.mime) {
            return Ok(());
        }
        if start.elapsed() > OWNERSHIP_WAIT {
            bail!("the compositor did not accept the clipboard offer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Which form of an entry to put on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMode {
    /// The primary content plus every stored format.
    Full,
    /// Only plain text (an image's recognised text).
    Plain,
    /// Plain text with Markdown syntax removed.
    NoMarkdown,
}

impl CopyMode {
    /// The socket protocol word, if any (`copy <id> [plain|nomd]`).
    pub fn as_word(self) -> Option<&'static str> {
        match self {
            Self::Full => None,
            Self::Plain => Some("plain"),
            Self::NoMarkdown => Some("nomd"),
        }
    }

    pub fn parse(word: Option<&str>) -> Option<Self> {
        match word {
            None => Some(Self::Full),
            Some("plain") => Some(Self::Plain),
            Some("nomd") => Some(Self::NoMarkdown),
            Some(_) => None,
        }
    }
}

/// What a copy of `id` in `mode` offers, primary first.
pub fn entry_formats(store: &Store, id: i64, mode: CopyMode) -> Result<Vec<Format>> {
    let (Some(entry), Some(content)) = (store.summary(id)?, store.content(id)?) else {
        bail!("no entry with id {id}");
    };
    let text = |content: Vec<u8>| -> Result<String> {
        if entry.is_image() {
            match (entry.ocr_status, &entry.ocr_text) {
                (OcrStatus::Done, Some(t)) if !t.trim().is_empty() => Ok(t.clone()),
                (OcrStatus::Done, _) => bail!("no text in this image"),
                (OcrStatus::Pending, _) => bail!("text in this image has not been read yet"),
                (OcrStatus::Failed, _) => bail!("couldn't read text in this image"),
                (OcrStatus::None, _) => bail!("text recognition is off"),
            }
        } else {
            Ok(String::from_utf8_lossy(&content).into_owned())
        }
    };
    let plain = |s: String| {
        vec![Format {
            mime: TEXT_MIME.into(),
            content: s.into_bytes(),
        }]
    };
    Ok(match mode {
        CopyMode::Full => {
            let mut formats = vec![Format {
                mime: entry.mime.clone(),
                content,
            }];
            formats.extend(store.formats(id)?);
            formats
        }
        CopyMode::Plain => plain(text(content)?),
        CopyMode::NoMarkdown => {
            if entry.is_image() {
                bail!("only text entries can have Markdown removed");
            }
            plain(markdown::strip(&text(content)?))
        }
    })
}

/// Put an entry on the clipboard. The running service serves every stored format; without it,
/// `wl-copy` serves the primary type only (formatting is lost for that paste).
pub fn copy_entry(paths: &Paths, store: &Store, id: i64, mode: CopyMode) -> Result<()> {
    match service::copy_entry(paths, id, mode) {
        Ok(()) => return Ok(()),
        Err(e) if e.is::<service::Unavailable>() => {
            crate::log(&format!("copy: {e:#}, using wl-copy"));
        }
        Err(e) => return Err(e),
    }
    let formats = entry_formats(store, id, mode)?;
    let primary = &formats[0];
    if formats.len() > 1 {
        crate::log(&format!(
            "copy: entry {id} has {} stored formats; only {} offered without the service",
            formats.len() - 1,
            primary.mime
        ));
    }
    copy(Some(&primary.mime), &primary.content)
}
