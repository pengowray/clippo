//! Markdown detection and removal (design doc sections 3.5 and 12).
//!
//! The goal is readable prose, not a Markdown parser: rules are line-based and applied in the
//! order the design lists them. Code blocks (fenced or indented) are kept verbatim.

use std::sync::LazyLock;

use regex::Regex;

macro_rules! re {
    ($name:ident, $pat:literal) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("valid regex"));
    };
}

re!(FENCE, r"^ {0,3}(`{3,}|~{3,})");
re!(HEADING, r"^ {0,3}#{1,6} +(.*?)(?: +#+)? *$");
re!(EMPTY_HEADING, r"^ {0,3}#{1,6} *$");
re!(SETEXT, r"^ {0,3}(?:={3,}|-{3,}) *$");
re!(HRULE, r"^ {0,3}(?:(?:\* *){3,}|(?:- *){3,}|(?:_ *){3,})$");
re!(QUOTE, r"^ {0,3}> ?");
re!(LIST, r"^(\s*)[-*+] (?:\[[ xX]\] )?");
re!(TABLE_SEP_CELL, r"^:?-+:?$");
re!(REF_DEF, r"^ {0,3}\[[^\]]+\]:\s+\S");
re!(IMAGE_INLINE, r"!\[([^\]]*)\]\([^)]*\)");
re!(IMAGE_REF, r"!\[([^\]]*)\]\[[^\]]*\]");
re!(LINK_INLINE, r"\[([^\]]+)\]\([^)]*\)");
re!(LINK_REF, r"\[([^\]]+)\]\[[^\]]*\]");
re!(AUTOLINK, r"<((?:https?://|mailto:)[^>\s]+)>");
re!(CODE_DOUBLE, r"``(.+?)``");
re!(CODE_SINGLE, r"`([^`]+)`");
re!(ESCAPE, r"\\([\\`*_{}\[\]()#+\-.!|>~])");

// Detection only.
re!(DETECT_HEADING, r"(?m)^#{1,6} ");
re!(DETECT_FENCE, r"(?m)^(?:```|~~~)");
re!(DETECT_LINK, r"!?\[[^\]]*\]\([^)]+\)");
re!(DETECT_LIST, r"(?m)^\s*(?:[-*+]|\d+\.) ");
re!(DETECT_QUOTE, r"(?m)^> ");
re!(DETECT_TABLE_SEP, r"(?m)^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)+\|?\s*$");

/// Remove Markdown syntax, keeping the text.
pub fn strip(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // 1. Fenced code: contents verbatim, fence lines dropped. Unclosed runs to the end.
        if let Some(m) = FENCE.captures(line) {
            let fence = m.get(1).expect("group").as_str();
            let marker = &fence[..3];
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(marker) {
                out.push(lines[i].to_string());
                i += 1;
            }
            i += 1; // the closing fence, if any
            continue;
        }

        // 2. Indented code after a blank line: verbatim until the next non-indented line.
        let prev_blank = i == 0 || lines[i - 1].trim().is_empty();
        if prev_blank && is_indented(line) {
            while i < lines.len() && (is_indented(lines[i]) || lines[i].trim().is_empty()) {
                // A blank line only stays inside the block if indented code follows it.
                if lines[i].trim().is_empty()
                    && !lines[i + 1..]
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .is_some_and(|l| is_indented(l))
                {
                    break;
                }
                out.push(lines[i].to_string());
                i += 1;
            }
            continue;
        }

        // 3. Setext heading underline, 4. horizontal rule, 7. table separator, 8. reference
        // definition: whole lines that go away.
        let prev_text = i > 0 && !lines[i - 1].trim().is_empty();
        if (prev_text && SETEXT.is_match(line))
            || HRULE.is_match(line)
            || is_table_separator(line)
            || REF_DEF.is_match(line)
        {
            i += 1;
            continue;
        }

        out.push(strip_line(line));
        i += 1;
    }

    // 15. Runs of three or more blank lines collapse to two.
    let mut result = String::with_capacity(input.len());
    let mut blank_run = 0;
    for line in out {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 2 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        result.push_str(&line);
        result.push('\n');
    }
    if !input.ends_with('\n') {
        result.pop();
    }
    result
}

fn is_indented(line: &str) -> bool {
    !line.trim().is_empty() && (line.starts_with("    ") || line.starts_with('\t'))
}

fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.starts_with('|') && t.ends_with('|')
}

fn is_table_separator(line: &str) -> bool {
    is_table_row(line)
        && table_cells(line)
            .iter()
            .all(|c| c.is_empty() || TABLE_SEP_CELL.is_match(c))
}

fn table_cells(line: &str) -> Vec<&str> {
    let t = line.trim();
    t[1..t.len() - 1].split('|').map(str::trim).collect()
}

/// Rules 3 (ATX headings), 5, 6, 7 and the inline rules 9 to 14, on one line.
fn strip_line(line: &str) -> String {
    // 5. Block quotes first: `> # Title` and `> - item` then fall through to their own rules.
    let mut s = line.to_string();
    while let Some(m) = QUOTE.find(&s) {
        s = s[m.end()..].to_string();
    }

    // 3. ATX headings.
    if let Some(m) = HEADING.captures(&s) {
        s = m.get(1).expect("group").as_str().to_string();
    } else if EMPTY_HEADING.is_match(&s) {
        s.clear();
    }

    // 6. Lists: any bullet becomes `- `, task markers go, ordered items stay.
    s = LIST.replace(&s, "${1}- ").into_owned();

    // 7. Table rows: cells separated by two spaces.
    if is_table_row(&s) {
        s = table_cells(&s).join("  ").trim().to_string();
    }

    // 9. Images, 10. links.
    s = IMAGE_INLINE.replace_all(&s, "$1").into_owned();
    s = IMAGE_REF.replace_all(&s, "$1").into_owned();
    s = LINK_INLINE.replace_all(&s, "$1").into_owned();
    s = LINK_REF.replace_all(&s, "$1").into_owned();
    s = AUTOLINK.replace_all(&s, "$1").into_owned();

    // 11. Inline code.
    s = CODE_DOUBLE.replace_all(&s, "$1").into_owned();
    s = CODE_SINGLE.replace_all(&s, "$1").into_owned();

    // 12. Emphasis. Double markers before single so `**` is not seen as two `*`.
    for (marker, word_bound) in [("**", false), ("__", true), ("~~", false), ("*", false), ("_", true)] {
        s = strip_pairs(&s, marker, word_bound);
    }

    // 13. Escapes.
    s = ESCAPE.replace_all(&s, "$1").into_owned();

    // 14. Hard breaks.
    let trimmed = s.trim_end_matches(' ');
    if s.len() - trimmed.len() >= 2 {
        s.truncate(trimmed.len());
    }
    if let Some(stripped) = s.strip_suffix('\\') {
        s.truncate(stripped.len());
    }
    s
}

/// Remove paired `marker`s where the opener is followed by a non-space and the closer is
/// preceded by one. With `word_bound`, the pair must not sit inside a word (`snake_case`).
fn strip_pairs(line: &str, marker: &str, word_bound: bool) -> String {
    let mut s = line.to_string();
    let mut from = 0;
    while let Some(pos) = s[from..].find(marker).map(|p| p + from) {
        let Some(close) = find_closer(&s, pos, marker, word_bound) else {
            from = pos + marker.len();
            continue;
        };
        let inner = s[pos + marker.len()..close].to_string();
        s.replace_range(pos..close + marker.len(), &inner);
        // Continue after the unwrapped text; nested single markers get their own pass.
        from = pos;
        if inner.is_empty() {
            from += marker.len();
        }
    }
    s
}

fn find_closer(s: &str, open: usize, marker: &str, word_bound: bool) -> Option<usize> {
    let after_open = open + marker.len();
    let next = s[after_open..].chars().next()?;
    if next.is_whitespace() || s[after_open..].starts_with(marker) || escaped(s, open) {
        return None;
    }
    if word_bound && s[..open].chars().last().is_some_and(char::is_alphanumeric) {
        return None;
    }
    let mut search = after_open + next.len_utf8();
    while let Some(rel) = s[search..].find(marker) {
        let close = search + rel;
        let before = s[..close].chars().last().expect("non-empty");
        let after = s[close + marker.len()..].chars().next();
        let ok = !before.is_whitespace()
            && !escaped(s, close)
            && (!word_bound || !after.is_some_and(char::is_alphanumeric));
        if ok {
            return Some(close);
        }
        search = close + marker.len();
    }
    None
}

/// A marker at `pos` is escaped (`\*`) and left for rule 13.
fn escaped(s: &str, pos: usize) -> bool {
    pos > 0 && s.as_bytes()[pos - 1] == b'\\'
}

/// Whether `text` looks like Markdown: two or more of the design's seven signals.
pub fn looks_like(text: &str) -> bool {
    let mut hits = 0;
    hits += DETECT_HEADING.is_match(text) as u32;
    hits += DETECT_FENCE.is_match(text) as u32;
    hits += DETECT_LINK.is_match(text) as u32;
    hits += has_paired_emphasis(text) as u32;
    hits += (DETECT_LIST.find_iter(text).count() >= 2) as u32;
    hits += DETECT_QUOTE.is_match(text) as u32;
    hits += DETECT_TABLE_SEP.is_match(text) as u32;
    hits >= 2
}

fn has_paired_emphasis(text: &str) -> bool {
    text.lines().any(|line| {
        [("**", false), ("__", true), ("~~", false), ("*", false), ("_", true)]
            .iter()
            .any(|(m, wb)| {
                line.match_indices(m)
                    .any(|(pos, _)| find_closer(line, pos, m, *wb).is_some())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_code_is_kept_verbatim_and_protects_headings() {
        assert_eq!(strip("```rust\n# not a heading\n**x**\n```\nafter"), "# not a heading\n**x**\nafter");
        assert_eq!(strip("~~~\ncode\n~~~"), "code");
        assert_eq!(strip("```\nunclosed\n# still code"), "unclosed\n# still code");
    }

    #[test]
    fn indented_code_is_kept_verbatim() {
        assert_eq!(strip("text\n\n    # code\n    *x*\n\nafter"), "text\n\n    # code\n    *x*\n\nafter");
        // Indented without a blank line before it: not code.
        assert_eq!(strip("- item\n    **cont**"), "- item\n    cont");
    }

    #[test]
    fn headings() {
        assert_eq!(strip("# Title"), "Title");
        assert_eq!(strip("### Sub ###  "), "Sub");
        assert_eq!(strip("###### six"), "six");
        assert_eq!(strip("####### seven"), "####### seven");
        assert_eq!(strip("#nospace"), "#nospace");
        assert_eq!(strip("Title\n=====\nbody"), "Title\nbody");
        assert_eq!(strip("Title\n---\nbody"), "Title\nbody");
    }

    #[test]
    fn horizontal_rules() {
        assert_eq!(strip("a\n***\nb"), "a\nb");
        // Two blank lines remain: rule 15 only collapses runs of three or more.
        assert_eq!(strip("a\n\n- - -\n\nb"), "a\n\n\nb");
        assert_eq!(strip("a\n\n___\n\nb"), "a\n\n\nb");
    }

    #[test]
    fn block_quotes() {
        assert_eq!(strip("> quoted\n>> nested\n>no space"), "quoted\nnested\nno space");
        assert_eq!(strip("> # Title"), "Title");
    }

    #[test]
    fn lists() {
        assert_eq!(strip("* a\n+ b\n- c\n  * nested"), "- a\n- b\n- c\n  - nested");
        assert_eq!(strip("- [ ] todo\n- [x] done"), "- todo\n- done");
        assert_eq!(strip("1. one\n2) two"), "1. one\n2) two");
    }

    #[test]
    fn tables_with_separator() {
        assert_eq!(
            strip("| Name | Age |\n|---|:--:|\n| Bob | 42 |"),
            "Name  Age\nBob  42"
        );
        assert_eq!(strip("|a|b|"), "a  b");
    }

    #[test]
    fn reference_definitions() {
        assert_eq!(strip("see [x][ref]\n\n[ref]: http://example.com \"Title\""), "see x\n");
    }

    #[test]
    fn images_and_links() {
        assert_eq!(strip("![alt text](img.png) and ![a][r]"), "alt text and a");
        assert_eq!(strip("[text](http://x) [t][r] <http://a.b> <mailto:m@x.y>"), "text t http://a.b mailto:m@x.y");
        assert_eq!(strip("bare http://example.com stays"), "bare http://example.com stays");
    }

    #[test]
    fn link_inside_bold() {
        assert_eq!(strip("**[the link](http://x)**"), "the link");
        assert_eq!(strip("[**bold** link](http://x)"), "bold link");
    }

    #[test]
    fn inline_code() {
        assert_eq!(strip("use `x` and ``a`b``"), "use x and a`b");
    }

    #[test]
    fn emphasis() {
        assert_eq!(strip("**b** __b__ *i* _i_ ~~s~~"), "b b i i s");
        assert_eq!(strip("**bold *and italic* text**"), "bold and italic text");
        assert_eq!(strip("5 * 3 * 2"), "5 * 3 * 2");
        assert_eq!(strip("a_b_c and snake_case_name"), "a_b_c and snake_case_name");
        assert_eq!(strip("unpaired *star and __under"), "unpaired *star and __under");
        assert_eq!(strip("** not bold **"), "** not bold **");
        assert_eq!(strip("*a* *b*"), "a b");
    }

    #[test]
    fn escapes_and_hard_breaks() {
        assert_eq!(strip(r"\* not a list \# \_x\_"), "* not a list # _x_");
        assert_eq!(strip("line  \nnext\\\nlast "), "line\nnext\nlast ");
    }

    #[test]
    fn blank_lines_collapse() {
        assert_eq!(strip("a\n\n\n\n\nb"), "a\n\n\nb");
        assert_eq!(strip("a\n\nb"), "a\n\nb");
    }

    #[test]
    fn plain_text_is_unchanged() {
        let s = "Tuesday meeting notes\nship 0.2, fix the paste delay\n";
        assert_eq!(strip(s), s);
    }

    #[test]
    fn detection_needs_two_signals() {
        assert!(!looks_like("just a line with *one* emphasis"));
        assert!(!looks_like("- a lone\n- list"));
        assert!(looks_like("# Title\n- a\n- b"));
        assert!(looks_like("see [x](http://y)\n\n> quote"));
        assert!(looks_like("```\ncode\n```\n**bold**"));
        assert!(looks_like("| a | b |\n|---|---|\n1. x\n2. y"));
        assert!(!looks_like("5 * 3 * 2 = 30\nsnake_case_name and a_b_c\n# not heading? yes it is"));
        assert!(!looks_like("plain text\nwith nothing\n"));
    }
}
