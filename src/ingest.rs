use std::io::Read;

use anyhow::Result;

use crate::clipboard::paste as wl_paste;
use crate::config::{Config, OcrEngineKind, Paths};
use crate::service;
use crate::skip;
use crate::store::{Format, NewEntry, OcrStatus, Store, content_hash};
use crate::thumbs;

pub const TEXT_MIME: &str = "text/plain;charset=utf-8";
const HTML_MIME: &str = "text/html";
const RTF_MIMES: [&str; 2] = ["text/rtf", "application/rtf"];
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// Extra formats bigger than this are dropped and the copy is stored as plain.
const MAX_FORMAT_BYTES: usize = 1024 * 1024;

/// Tags whose presence means HTML carries formatting worth keeping (design 11.2).
const RICH_HTML_TAGS: [&str; 20] = [
    "b", "strong", "i", "em", "u", "s", "a", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol",
    "table", "img", "code", "pre", "blockquote",
];

/// Identify common image formats from their magic bytes.
pub fn sniff_image(data: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| data.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && starts(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if starts(b"BM")
        && data.len() > 14
        && u32::from_le_bytes([data[2], data[3], data[4], data[5]]) as usize == data.len()
    {
        Some("image/bmp")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some("image/tiff")
    } else {
        None
    }
}

/// Pick the image type to prefer from an offered type list.
pub fn preferred_image_type(types: &[&str]) -> Option<String> {
    types
        .iter()
        .find(|t| **t == "image/png")
        .or_else(|| types.iter().find(|t| t.starts_with("image/")))
        .map(|t| t.to_string())
}

/// Browsers wrap every copy in HTML; only HTML with real formatting is worth storing.
pub fn html_has_formatting(html: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(html).to_ascii_lowercase();
    if lower.contains("style=") {
        return true;
    }
    // Whole tag names only: `<b` must not match `<br>` or `<body>`, nor `<s` match `<span>`.
    lower.match_indices('<').any(|(i, _)| {
        let rest = &lower[i + 1..];
        RICH_HTML_TAGS.iter().any(|tag| {
            rest.strip_prefix(tag)
                .and_then(|after| after.chars().next())
                .is_some_and(|c| c == '>' || c == '/' || c.is_whitespace())
        })
    })
}

/// The extra formats to keep from what the clipboard owner offers, for a text or image primary.
pub fn extra_formats(types: &[&str], image: bool) -> Vec<Format> {
    let fetch = |mime: &str| -> Option<Format> {
        let content = wl_paste(&["--no-newline", "--type", mime])?;
        (!content.is_empty() && content.len() <= MAX_FORMAT_BYTES).then(|| Format {
            mime: mime.to_string(),
            content,
        })
    };
    let mut out = Vec::new();
    if types.contains(&HTML_MIME)
        && let Some(html) = fetch(HTML_MIME)
        && (image || html_has_formatting(&html.content))
    {
        out.push(html);
    }
    if !image
        && let Some(mime) = RTF_MIMES.iter().find(|m| types.contains(m))
        && let Some(rtf) = fetch(mime)
    {
        out.push(rtf);
    }
    out
}

pub fn run(cfg: &Config, paths: &Paths) -> Result<()> {
    // wl-paste --watch sets CLIPBOARD_STATE; only then is it safe to query the
    // live clipboard. A manual `clippo ingest < file` just stores stdin.
    let from_watch = match std::env::var("CLIPBOARD_STATE").ok().as_deref() {
        None => false,
        Some("data") => true,
        Some(_) => return Ok(()), // nil, clear, sensitive
    };

    let mut data = Vec::new();
    std::io::stdin().read_to_end(&mut data)?;

    let mut mime = sniff_image(&data).map(str::to_string);
    let mut types_str = String::new();
    if from_watch {
        let types_raw = wl_paste(&["--list-types"]).unwrap_or_default();
        types_str = String::from_utf8_lossy(&types_raw).into_owned();
        let types: Vec<&str> = types_str.lines().map(str::trim).collect();
        if types.contains(&PASSWORD_HINT) {
            let hint = wl_paste(&["--no-newline", "--type", PASSWORD_HINT]).unwrap_or_default();
            if String::from_utf8_lossy(&hint).trim() == "secret" {
                return Ok(());
            }
        }
        // wl-paste --watch hands us its own pick (often text); prefer an image if offered.
        if mime.is_none()
            && let Some(t) = preferred_image_type(&types)
            && let Some(img) = wl_paste(&["--no-newline", "--type", &t])
            && let Some(m) = sniff_image(&img)
        {
            data = img;
            mime = Some(m.to_string());
        }
    }
    let types: Vec<&str> = types_str.lines().map(str::trim).collect();

    let mut store = Store::open(&paths.db)?;
    match mime {
        Some(mime) => {
            let dims = image::ImageReader::new(std::io::Cursor::new(&data))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok());
            let ocr_status = if cfg.ocr.engine == OcrEngineKind::Off {
                OcrStatus::None
            } else {
                OcrStatus::Pending
            };
            let formats = if from_watch {
                extra_formats(&types, true)
            } else {
                Vec::new()
            };
            store.upsert(&NewEntry {
                mime: &mime,
                content: &data,
                dims,
                ocr_status,
                formats: &formats,
            })?;
            if ocr_status == OcrStatus::Pending {
                service::notify_ocr(paths);
            }
        }
        None => {
            let Ok(text) = std::str::from_utf8(&data) else {
                return Ok(()); // unsupported binary data
            };
            if text.trim().is_empty() {
                return Ok(());
            }
            // A macro paste asked not to be recorded (see skip.rs).
            if skip::take(&paths.runtime_dir, &content_hash(TEXT_MIME, &data)) {
                return Ok(());
            }
            let formats = if from_watch {
                extra_formats(&types, false)
            } else {
                Vec::new()
            };
            store.upsert(&NewEntry {
                mime: TEXT_MIME,
                content: &data,
                dims: None,
                ocr_status: OcrStatus::None,
                formats: &formats,
            })?;
        }
    }
    let mut removed = store.enforce_cap(cfg.max_items)?;
    removed.extend(store.expire(cfg.expire_days)?);
    removed.extend(store.purge_deleted()?);
    thumbs::remove(&paths.thumbs_dir, &removed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_images() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_image(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"hello"), None);
        assert_eq!(sniff_image(b"BMW makes cars, lots of them"), None);
    }

    #[test]
    fn prefers_png() {
        let types = ["text/html", "image/jpeg", "image/png"];
        assert_eq!(preferred_image_type(&types).as_deref(), Some("image/png"));
        assert_eq!(
            preferred_image_type(&["image/bmp"]).as_deref(),
            Some("image/bmp")
        );
        assert_eq!(preferred_image_type(&["text/plain"]), None);
    }

    #[test]
    fn html_formatting_test() {
        // A browser's plain wrapper: no formatting.
        assert!(!html_has_formatting(
            b"<meta charset=\"utf-8\"><span>just words</span>"
        ));
        assert!(!html_has_formatting(b"<script>x</script><style>y</style>"));
        assert!(!html_has_formatting(b"<html><body>line<br>two</body></html>"));
        assert!(html_has_formatting(b"<p>a <B>bold</B> word</p>"));
        assert!(html_has_formatting(b"<a href=\"x\">link</a>"));
        assert!(html_has_formatting(b"<span style=\"color:red\">x</span>"));
        assert!(html_has_formatting(b"<img src=\"x.png\" alt=\"pic\">"));
        assert!(html_has_formatting(b"<s>struck</s>"));
        assert!(html_has_formatting(b"<h2>Title</h2>"));
    }
}
