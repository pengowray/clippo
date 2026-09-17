//! Every user-visible string in the window, in one place so the copy can be reviewed
//! against docs/menu-design.md section 15 without reading the layout code.

pub const SEARCH_PLACEHOLDER: &str = "Search history";
pub const SETTINGS_TOOLTIP: &str = "Settings";
pub const ON_CLIPBOARD_NOW: &str = "ON CLIPBOARD NOW";
pub const OLDER_DIVIDER: &str = "Older (not used in the last day)";

pub fn item_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "item" } else { "items" })
}

pub fn match_count(matched: usize, total: usize) -> String {
    format!("{matched} of {total} match")
}

pub fn older_collapsed(n: usize) -> String {
    format!("{n} older {} (not used in the last day)", items(n))
}

pub fn older_expanded(n: usize) -> String {
    format!("Hide {n} older {}", items(n))
}

fn items(n: usize) -> &'static str {
    if n == 1 { "item" } else { "items" }
}

pub fn image_label(width: Option<u32>, height: Option<u32>, format: &str) -> String {
    match (width, height) {
        (Some(w), Some(h)) => format!("Image {w}×{h} · {format}"),
        _ => format!("Image · {format}"),
    }
}

pub const OCR_PENDING: &str = "Reading text...";
pub const OCR_PENDING_NO_ENGINE: &str = "Waiting for a text engine";
pub const OCR_EMPTY: &str = "No text found";
pub const OCR_FAILED: &str = "Couldn't read text";

pub fn overflow(more_lines: usize, chars: Option<usize>) -> Option<String> {
    let lines = (more_lines > 0).then(|| {
        format!(
            "{more_lines} more {}",
            if more_lines == 1 { "line" } else { "lines" }
        )
    });
    let chars = chars.map(|c| format!("{c} chars"));
    match (lines, chars) {
        (Some(l), Some(c)) => Some(format!("{l} · {c}")),
        (Some(l), None) => Some(l),
        (None, Some(c)) => Some(format!("· {c}")),
        (None, None) => None,
    }
}

pub const PASTE_WITHOUT_MARKDOWN: &str = "Paste without Markdown";
pub const PASTE_AS_PLAIN_TEXT: &str = "Paste as plain text";
pub const ALREADY_PLAIN: &str = "Already plain text";
pub const T_OCR_PENDING: &str = "Reading text, try again in a moment";
pub const T_OCR_EMPTY: &str = "No text in this image";
pub const T_OCR_FAILED: &str = "Couldn't read text in this image";
pub const T_OCR_OFF: &str = "Text recognition is off. Turn it on in Settings";
pub const DELETE: &str = "Delete";

pub const MENU_PASTE: &str = "Paste";
pub const MENU_COPY_ONLY: &str = "Copy only";

pub const MACRO_HEADING: &str = "Paste";
pub fn macro_tooltip(format: &str) -> String {
    format!("{format} · Change in Settings")
}
pub fn macro_key(n: usize) -> String {
    format!("Alt+{n}")
}

pub const FOOTER_HINTS: &str =
    "Enter paste · Shift+Enter paste as plain text · Alt+Enter without Markdown · Shift+Del delete";
pub const FOOTER_HINTS_COPY: &str =
    "Enter copy · Shift+Enter copy as plain text · Alt+Enter copy without Markdown · Shift+Del delete";
pub const FOOTER_DELETED: &str = "Deleted.";
pub const FOOTER_UNDO: &str = "Undo (Ctrl+Z)";
pub const FOOTER_NO_ENGINE: &str = "No text recognition engine. Set one up in Settings";
pub const FOOTER_MARKDOWN_IMAGE: &str = "Only text entries can have Markdown removed";
pub fn footer_read_failed(err: &str) -> String {
    format!("Couldn't read history: {err}")
}
/// Not in the design: shown until the backend's Markdown stripper lands.
pub const FOOTER_MARKDOWN_UNAVAILABLE: &str = "Paste without Markdown isn't available yet";
pub const SETTINGS_TITLE: &str = "Settings";
pub const SETTINGS_BACK: &str = "Back";
pub const FOOTER_SAVED: &str = "Saved";
pub const FOOTER_SAVED_NO_SERVICE: &str =
    "Saved. Restart the clippo service to apply paste and OCR settings";

pub const BANNER_NOT_RECORDING: &str = "Not recording. Start the clippo service to save new copies";
pub const BANNER_NO_ENGINE: &str = "Text in images is not being read. Set up an engine in Settings";

pub const EMPTY_TITLE: &str = "Nothing copied yet";
pub const EMPTY_BODY: &str = "Text and images you copy will show up here";
pub const EMPTY_RECENT: &str = "Nothing copied in the last day";
pub fn no_matches(query: &str) -> String {
    format!("No matches for \"{query}\"")
}
pub const CLEAR_SEARCH: &str = "Clear search";

pub const NOTIFY_PASTE_FAILED_TITLE: &str = "Copied, but couldn't paste";
pub fn notify_paste_failed_body(err: &str, key: &str) -> String {
    format!("{err}. Press {key} to paste it yourself.")
}
pub const NOTIFY_COPY_FAILED_TITLE: &str = "Couldn't copy";

/// `Copied 3 min ago` style relative time for the row tooltip. `ms` is a unix time in
/// milliseconds; `now` likewise.
pub fn copied_ago(ms: i64, now: i64) -> String {
    use chrono::{Local, TimeZone};
    let secs = (now - ms).max(0) / 1000;
    let when = if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 24 * 3600 {
        format!("{} h ago", secs / 3600)
    } else {
        let t = Local.timestamp_millis_opt(ms).single();
        let today = Local.timestamp_millis_opt(now).single();
        match (t, today) {
            (Some(t), Some(today)) if (today.date_naive() - t.date_naive()).num_days() == 1 => {
                format!("Yesterday {}", t.format("%H:%M"))
            }
            (Some(t), _) => t.format("%-d %b %H:%M").to_string(),
            _ => format!("{} days ago", secs / 86400),
        }
    };
    format!("Copied {when}")
}

pub fn size_label(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{} kB", bytes / 1_000)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_parts() {
        assert_eq!(overflow(3, Some(412)).as_deref(), Some("3 more lines · 412 chars"));
        assert_eq!(overflow(6, None).as_deref(), Some("6 more lines"));
        assert_eq!(overflow(1, None).as_deref(), Some("1 more line"));
        assert_eq!(overflow(0, Some(300)).as_deref(), Some("· 300 chars"));
        assert_eq!(overflow(0, None), None);
    }

    #[test]
    fn relative_times() {
        let now = 1_700_000_000_000;
        assert_eq!(copied_ago(now - 5_000, now), "Copied just now");
        assert_eq!(copied_ago(now - 3 * 60_000, now), "Copied 3 min ago");
        assert_eq!(copied_ago(now - 5 * 3_600_000, now), "Copied 5 h ago");
    }
}
