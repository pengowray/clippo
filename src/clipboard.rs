//! The live clipboard: reading it, and putting entries on it, through the backend for this
//! session (`backend/`).

use anyhow::{Result, bail};

use crate::backend;
use crate::config::Paths;
use crate::ingest::TEXT_MIME;
use crate::markdown;
use crate::service;
use crate::store::{Format, OcrStatus, Store};

/// The clipboard's data as `mime`, exactly as offered; `None` if it isn't offered.
pub fn read(mime: &str) -> Option<Vec<u8>> {
    backend::current().read(mime)
}

/// The clipboard's text, from whichever text type is offered.
pub fn read_text() -> Option<Vec<u8>> {
    backend::current().read_text()
}

/// Types offered by the current clipboard owner.
pub fn list_types() -> Vec<String> {
    backend::current().list_types()
}

/// Put `data` on the clipboard. With no `mime`, the usual set of text types is offered.
pub fn copy(mime: Option<&str>, data: &[u8]) -> Result<()> {
    backend::current().copy(mime, data)
}

/// Offer every format at once; the first is the primary. Returns once the new content is on
/// the clipboard, so a paste keystroke sent afterwards gets it.
pub fn copy_formats(formats: &[Format]) -> Result<()> {
    backend::current().copy_formats(formats)
}

/// Does this offered type hold plain text?
pub fn is_text_type(t: &str) -> bool {
    t.starts_with("text/plain") || t == "UTF8_STRING" || t == "STRING"
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
/// only the primary type is offered (formatting is lost for that paste).
pub fn copy_entry(paths: &Paths, store: &Store, id: i64, mode: CopyMode) -> Result<()> {
    match service::copy_entry(paths, id, mode) {
        Ok(()) => return Ok(()),
        Err(e) if e.is::<service::Unavailable>() => {
            crate::log(&format!("copy: {e:#}, offering the main format only"));
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
