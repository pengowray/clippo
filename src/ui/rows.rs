//! Per-entry view model for the history list. Built once per load from `Summary`, so the
//! view never re-parses previews and search runs over precomputed lowercase text.

use crate::store::{OcrStatus, Summary};
use crate::ui::strings;

/// Preview lines shown per text row (design 3.1).
pub const PREVIEW_LINES: usize = 3;
/// Above this many preview chars the overflow hint also shows the length (design 3.1).
const CHARS_HINT_ABOVE: usize = 200;
/// `Store` keeps this many preview chars; a preview this long means the text was cut.
const STORE_PREVIEW_CHARS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ocr {
    /// Text entry, or OCR turned off.
    None,
    Pending,
    Text,
    Empty,
    Failed,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Text {
        /// Display lines, already tab-expanded, leading blank lines skipped.
        lines: Vec<String>,
        /// Lines in the stored preview beyond `lines`, if the preview held the whole text.
        more_lines: Option<usize>,
        /// Whole-text length in chars when known.
        chars: Option<usize>,
    },
    Image {
        label: String,
        ocr: Ocr,
        /// First lines of recognised text, for the row.
        ocr_lines: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub id: i64,
    pub mime: String,
    pub kind: Kind,
    /// Lowercase text searched by the filter: preview, OCR text, image label.
    haystack: String,
    pub is_markdown: bool,
    pub has_rich: bool,
    /// Milliseconds since the Unix epoch.
    pub last_used: i64,
    /// Images only.
    pub size_bytes: Option<usize>,
}

impl Row {
    pub fn from_summary(s: &Summary) -> Self {
        let (kind, haystack) = if s.is_image() {
            let format = s
                .mime
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase();
            let label = strings::image_label(s.width, s.height, &format);
            let (ocr, ocr_lines) = match s.ocr_status {
                OcrStatus::None => (Ocr::None, Vec::new()),
                OcrStatus::Pending => (Ocr::Pending, Vec::new()),
                OcrStatus::Failed => (Ocr::Failed, Vec::new()),
                OcrStatus::Done => {
                    let text = s.ocr_text.as_deref().unwrap_or("");
                    if text.trim().is_empty() {
                        (Ocr::Empty, Vec::new())
                    } else {
                        (Ocr::Text, preview_lines(text).0)
                    }
                }
            };
            let mut hay = label.to_lowercase();
            if let Some(t) = &s.ocr_text {
                hay.push('\n');
                hay.push_str(&t.to_lowercase());
            }
            (
                Kind::Image {
                    label,
                    ocr,
                    ocr_lines,
                },
                hay,
            )
        } else {
            let preview = s.preview.as_deref().unwrap_or("");
            let (lines, hidden_in_preview) = preview_lines(preview);
            // The store's line count covers the whole text; the preview alone only knows
            // about lines inside its first 400 chars.
            let more_lines = match s.line_count {
                Some(total) => Some(total.saturating_sub(lines.len())),
                None => (preview.chars().count() < STORE_PREVIEW_CHARS).then_some(hidden_in_preview),
            };
            (
                Kind::Text {
                    lines,
                    more_lines,
                    chars: (s.content_len > CHARS_HINT_ABOVE).then_some(s.content_len),
                },
                preview.to_lowercase(),
            )
        };
        Self {
            id: s.id,
            mime: s.mime.clone(),
            kind,
            haystack,
            is_markdown: s.is_markdown,
            has_rich: s.has_rich,
            last_used: s.last_used,
            size_bytes: s.is_image().then_some(s.content_len),
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self.kind, Kind::Image { .. })
    }

    pub fn ocr(&self) -> Ocr {
        match &self.kind {
            Kind::Image { ocr, .. } => *ocr,
            Kind::Text { .. } => Ocr::None,
        }
    }

    /// Every word of `query` (lowercase) appears somewhere in the row's text.
    pub fn matches(&self, words: &[String]) -> bool {
        words.iter().all(|w| self.haystack.contains(w.as_str()))
    }

    /// Used at or after `cutoff` (see `Store::recent_cutoff`).
    pub fn is_recent(&self, cutoff: i64) -> bool {
        self.last_used >= cutoff
    }

    /// Why the `Paste as plain text` button is greyed, or `None` when it is enabled: every
    /// text entry, and images whose recognised text is ready.
    pub fn plain_disabled_reason(&self) -> Option<&'static str> {
        match &self.kind {
            Kind::Text { .. } => None,
            Kind::Image { ocr, .. } => match ocr {
                Ocr::Text => None,
                Ocr::Pending => Some(strings::T_OCR_PENDING),
                Ocr::Empty => Some(strings::T_OCR_EMPTY),
                Ocr::Failed => Some(strings::T_OCR_FAILED),
                Ocr::None => Some(strings::T_OCR_OFF),
            },
        }
    }

    /// Bottom-right overflow hint for text rows.
    pub fn overflow_hint(&self) -> Option<String> {
        match &self.kind {
            Kind::Text {
                more_lines, chars, ..
            } => strings::overflow(more_lines.unwrap_or(0), *chars),
            Kind::Image { .. } => None,
        }
    }
}

/// Up to `PREVIEW_LINES` display lines (tabs as 4 spaces, leading blank lines skipped) and
/// how many further non-empty-or-not lines followed them in `text`.
pub fn preview_lines(text: &str) -> (Vec<String>, usize) {
    let mut lines = text.lines().skip_while(|l| l.trim().is_empty()).peekable();
    let mut shown = Vec::with_capacity(PREVIEW_LINES);
    while shown.len() < PREVIEW_LINES {
        match lines.next() {
            Some(l) => shown.push(l.replace('\t', "    ")),
            None => break,
        }
    }
    // Trailing blank lines are not "more lines" worth announcing.
    let rest: Vec<&str> = lines.collect();
    let hidden = rest
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    (shown, hidden)
}

/// Lowercase search words; empty when the query is blank.
pub fn query_words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_skips_leading_blanks_and_counts_rest() {
        let (lines, hidden) = preview_lines("\n\n  a\tb\nc\nd\ne\nf\n\n");
        assert_eq!(lines, vec!["  a    b", "c", "d"]);
        assert_eq!(hidden, 2);
        assert_eq!(preview_lines("one").1, 0);
        assert_eq!(preview_lines("").0.len(), 0);
    }

    fn summary(mime: &str, preview: Option<&str>) -> Summary {
        Summary {
            id: 1,
            mime: mime.into(),
            preview: preview.map(str::to_string),
            width: None,
            height: None,
            ocr_status: OcrStatus::None,
            ocr_text: None,
            last_used: 0,
            content_len: preview.map_or(0, |p| p.chars().count()),
            line_count: preview.map(|p| p.lines().count()),
            is_markdown: false,
            has_rich: false,
        }
    }

    fn text_row(preview: &str) -> Row {
        Row::from_summary(&summary("text/plain;charset=utf-8", Some(preview)))
    }

    #[test]
    fn text_row_hints() {
        assert_eq!(text_row("short").overflow_hint(), None);
        let long: String = (0..6).map(|i| format!("line {i}\n")).collect();
        assert_eq!(text_row(&long).overflow_hint().as_deref(), Some("3 more lines"));
        let wide = "x".repeat(250);
        assert_eq!(text_row(&wide).overflow_hint().as_deref(), Some("· 250 chars"));
        // Whole-text counts come from the store, not the 400-char preview.
        let mut s = summary("text/plain;charset=utf-8", Some(&"y\n".repeat(200)));
        s.content_len = 5000;
        s.line_count = Some(2500);
        assert_eq!(
            Row::from_summary(&s).overflow_hint().as_deref(),
            Some("2497 more lines · 5000 chars")
        );
    }

    #[test]
    fn search_requires_every_word() {
        let r = text_row("Hello wide World");
        assert!(r.matches(&query_words("world hello")));
        assert!(!r.matches(&query_words("world mars")));
        assert!(r.matches(&query_words("")));
    }

    #[test]
    fn image_row_label_and_state() {
        let mut s = summary("image/png", None);
        s.width = Some(640);
        s.height = Some(480);
        s.ocr_status = OcrStatus::Done;
        s.ocr_text = Some("Error 1002\nsecond".into());
        let r = Row::from_summary(&s);
        match &r.kind {
            Kind::Image { label, ocr, ocr_lines } => {
                assert_eq!(label, "Image 640×480 · PNG");
                assert_eq!(*ocr, Ocr::Text);
                assert_eq!(ocr_lines.len(), 2);
            }
            _ => panic!("expected image"),
        }
        assert!(r.matches(&query_words("png 1002")));
        assert_eq!(r.plain_disabled_reason(), None);
    }
}
