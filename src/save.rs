//! Saving an image entry to a file (Super+V context menu, design 6).
//!
//! Files go to the user's Pictures folder, named `clippo-YYYYMMDD-HHMMSS.<ext>`. The window
//! closes when another surface takes focus, so a file chooser is not an option; the footer
//! names the path instead.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::ImageFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    /// The bytes as stored, with the extension the mime implies.
    Original,
    Png,
    Jpeg,
}

/// File extension for a stored mime type, as the user sees it in the menu and the file name.
pub fn extension_for_mime(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        other => other.rsplit('/').next().unwrap_or("img"),
    }
}

/// Upper-case label for the menu: `PNG`, `JPG`, `WEBP`.
pub fn format_label(mime: &str) -> String {
    extension_for_mime(mime).to_ascii_uppercase()
}

/// The format the stored bytes already are, if it is one we offer a conversion to.
pub fn stored_format(mime: &str) -> Option<SaveFormat> {
    match mime {
        "image/png" => Some(SaveFormat::Png),
        "image/jpeg" => Some(SaveFormat::Jpeg),
        _ => None,
    }
}

/// Where saved images go: the Pictures folder, else the home directory.
pub fn default_dir() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    Some(
        dirs.picture_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dirs.home_dir().to_path_buf()),
    )
}

/// Encode `content` (stored as `mime`) in `format`. `Original`, or a format equal to the
/// stored one, returns the bytes untouched.
pub fn encode(mime: &str, content: &[u8], format: SaveFormat) -> Result<Vec<u8>> {
    if format == SaveFormat::Original || stored_format(mime) == Some(format) {
        return Ok(content.to_vec());
    }
    let img = image::load_from_memory(content).context("could not decode image")?;
    let mut out = Cursor::new(Vec::new());
    match format {
        SaveFormat::Png => img
            .write_to(&mut out, ImageFormat::Png)
            .context("could not encode PNG")?,
        // JPEG has no alpha channel: flatten onto white, as image viewers show it.
        SaveFormat::Jpeg => flatten_on_white(&img)
            .write_to(&mut out, ImageFormat::Jpeg)
            .context("could not encode JPG")?,
        SaveFormat::Original => unreachable!(),
    }
    Ok(out.into_inner())
}

fn flatten_on_white(img: &image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let mix = |c: u8| ((c as u16 * a as u16 + 255 * (255 - a as u16)) / 255) as u8;
        image::Rgb([mix(r), mix(g), mix(b)])
    })
}

fn extension(mime: &str, format: SaveFormat) -> &str {
    match format {
        SaveFormat::Original => extension_for_mime(mime),
        SaveFormat::Png => "png",
        SaveFormat::Jpeg => "jpg",
    }
}

/// A path in `dir` that does not exist yet: `clippo-<stamp>.<ext>`, then `-2`, `-3`, ...
fn unique_path(dir: &Path, stamp: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("clippo-{stamp}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("clippo-{stamp}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// Write the image to a new file in `dir` and return its path.
pub fn save(
    dir: &Path,
    mime: &str,
    content: &[u8],
    format: SaveFormat,
    now: &chrono::DateTime<chrono::Local>,
) -> Result<PathBuf> {
    let bytes = encode(mime, content, format)?;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let out = unique_path(dir, &stamp, extension(mime, format));
    // Write then rename so a half-written file never sits under the final name.
    let tmp = dir.join(format!(".clippo-{stamp}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &bytes).with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, &out).with_context(|| format!("could not write {}", out.display()))?;
    Ok(out)
}

/// `~/Pictures/clippo-....png` for the footer: the home directory shortened to `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn rgba_png() -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_fn(4, 3, |x, _| Rgba([x as u8 * 60, 10, 20, 128]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn now() -> chrono::DateTime<chrono::Local> {
        chrono::Local::now()
    }

    #[test]
    fn same_format_is_byte_identical() {
        let png = rgba_png();
        assert_eq!(encode("image/png", &png, SaveFormat::Png).unwrap(), png);
        assert_eq!(
            encode("image/png", &png, SaveFormat::Original).unwrap(),
            png
        );
        assert_eq!(
            encode("image/webp", b"not decoded", SaveFormat::Original).unwrap(),
            b"not decoded"
        );
    }

    #[test]
    fn rgba_converts_to_jpg() {
        let jpg = encode("image/png", &rgba_png(), SaveFormat::Jpeg).unwrap();
        assert_eq!(image::guess_format(&jpg).unwrap(), ImageFormat::Jpeg);
        let back = image::load_from_memory(&jpg).unwrap();
        assert_eq!((back.width(), back.height()), (4, 3));
    }

    #[test]
    fn transparent_becomes_white_in_jpg() {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        let flat = flatten_on_white(&image::DynamicImage::ImageRgba8(img));
        assert_eq!(flat.get_pixel(3, 3).0, [255, 255, 255]);
    }

    #[test]
    fn undecodable_input_is_an_error() {
        assert!(encode("image/png", b"junk", SaveFormat::Jpeg).is_err());
    }

    #[test]
    fn saves_with_extension_and_avoids_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let png = rgba_png();
        let t = now();
        let a = save(dir.path(), "image/png", &png, SaveFormat::Original, &t).unwrap();
        let b = save(dir.path(), "image/png", &png, SaveFormat::Original, &t).unwrap();
        let c = save(dir.path(), "image/png", &png, SaveFormat::Jpeg, &t).unwrap();
        assert_eq!(a.extension().unwrap(), "png");
        assert_ne!(a, b);
        assert!(b.file_name().unwrap().to_str().unwrap().ends_with("-2.png"));
        assert_eq!(c.extension().unwrap(), "jpg");
        assert_eq!(std::fs::read(&a).unwrap(), png);
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .all(|e| { !e.unwrap().file_name().to_str().unwrap().ends_with(".tmp") })
        );
    }

    #[test]
    fn labels() {
        assert_eq!(format_label("image/jpeg"), "JPG");
        assert_eq!(format_label("image/webp"), "WEBP");
        assert_eq!(extension_for_mime("image/x-icon"), "x-icon");
    }
}
