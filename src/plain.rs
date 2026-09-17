use anyhow::{Result, bail};

use crate::clipboard;
use crate::config::{Config, Paths};
use crate::ingest::{preferred_image_type, sniff_image};
use crate::ocr;
use crate::paste;
use crate::store::{OcrStatus, Store};

/// Replace the clipboard with its plain text (recognised text for images), then optionally paste.
pub fn run(cfg: &Config, paths: &Paths, auto_paste: bool) -> Result<()> {
    let types = clipboard::list_types();
    let types: Vec<&str> = types.iter().map(String::as_str).collect();

    let text = if types
        .iter()
        .any(|t| t.starts_with("text/plain") || *t == "UTF8_STRING")
    {
        let data = clipboard::paste(&["--no-newline", "--type", "text"]).unwrap_or_default();
        String::from_utf8_lossy(&data).into_owned()
    } else if let Some(t) = preferred_image_type(&types)
        && let Some(img) = clipboard::paste(&["--type", &t])
    {
        image_text(cfg, paths, &img)?
    } else {
        bail!("the clipboard has no text or image");
    };
    if text.trim().is_empty() {
        bail!("no text found");
    }

    clipboard::copy(None, text.as_bytes())?;
    if auto_paste {
        paste::send(&cfg.paste)?;
    }
    Ok(())
}

/// Recognised text for an image, reusing the history's OCR result when it has one.
fn image_text(cfg: &Config, paths: &Paths, img: &[u8]) -> Result<String> {
    let store = Store::open(&paths.db)?;
    let entry = match sniff_image(img) {
        Some(mime) => store.find(mime, img)?,
        None => None,
    };
    if let Some(e) = &entry
        && e.ocr_status == OcrStatus::Done
        && let Some(text) = &e.ocr_text
    {
        return Ok(text.clone());
    }
    let backend = ocr::backend(&cfg.ocr, paths)?.ok_or_else(ocr::no_engine_error)?;
    match entry {
        Some(e) => ocr::ocr_entry(&store, backend.as_ref(), e.id),
        None => backend.recognize(img),
    }
}
