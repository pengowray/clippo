use std::io::Read;
use std::process::{Command, Stdio};

use anyhow::Result;

use crate::config::{Config, OcrEngineKind, Paths};
use crate::store::{NewEntry, OcrStatus, Store};
use crate::thumbs;

pub const TEXT_MIME: &str = "text/plain;charset=utf-8";
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

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

fn wl_paste(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Pick the image type to prefer from an offered type list.
fn preferred_image_type(types: &[&str]) -> Option<String> {
    types
        .iter()
        .find(|t| **t == "image/png")
        .or_else(|| types.iter().find(|t| t.starts_with("image/")))
        .map(|t| t.to_string())
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
    if from_watch {
        let types_raw = wl_paste(&["--list-types"]).unwrap_or_default();
        let types_str = String::from_utf8_lossy(&types_raw);
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
            store.upsert(&NewEntry {
                mime: &mime,
                content: &data,
                dims,
                ocr_status,
            })?;
        }
        None => {
            let Ok(text) = std::str::from_utf8(&data) else {
                return Ok(()); // unsupported binary data
            };
            if text.trim().is_empty() {
                return Ok(());
            }
            store.upsert(&NewEntry {
                mime: TEXT_MIME,
                content: &data,
                dims: None,
                ocr_status: OcrStatus::None,
            })?;
        }
    }
    let removed = store.enforce_cap(cfg.max_items)?;
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
}
