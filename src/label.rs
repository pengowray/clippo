//! One-line labels for history entries, as `clippo list` prints them.

use crate::store::Summary;

const TEXT_LABEL_CHARS: usize = 80;
const OCR_LABEL_CHARS: usize = 200;

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
}
