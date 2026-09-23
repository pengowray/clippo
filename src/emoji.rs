//! Data for the emoji and symbol picker (`clippo emoji`): the catalogue of everything the
//! picker can insert, its tabs, name search, presentation variants, and the user's
//! favourites, recent picks and skin tone (`emoji.toml`).
//!
//! Emoji come from the `emojis` crate (Unicode 17 emoji-test order, names, GitHub
//! shortcodes, skin tones). Letters and text symbols are read from Unicode's names through
//! `unicode_names2` for a fixed set of blocks. Whether a character defaults to text or
//! emoji presentation comes from `unicode-properties`; which characters have text and
//! emoji variation sequences at all comes from the bundled `emoji-variation-sequences.txt`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use emojis::{Emoji, SkinTone};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_properties::{EmojiStatus, GeneralCategory, UnicodeEmoji, UnicodeGeneralCategory};

const VARIATION_SEQUENCES: &str = include_str!("../data/emoji-variation-sequences.txt");

/// Most recent picks kept in `emoji.toml`.
const MAX_RECENT: usize = 40;

pub const VS15_TEXT: char = '\u{FE0E}';
pub const VS16_EMOJI: char = '\u{FE0F}';

// ---- catalogue -------------------------------------------------------------------------------

/// One thing the picker can insert.
#[derive(Debug, Clone)]
pub struct Glyph {
    /// What Enter inserts: the fully qualified emoji, or the character.
    pub text: String,
    /// Display name: emoji name from emoji-test, or the Unicode name in lower case.
    pub name: String,
    /// Lower-case words of the name, matched by search.
    words: Vec<String>,
    /// Lower-case shortcode words (`:red_heart:` gives `red_heart`, `red`, `heart`);
    /// matched after the name so `pi` finds π before `man_with_gua_pi_mao`.
    codes: Vec<String>,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Emoji(&'static Emoji),
    Char(char),
}

impl Glyph {
    fn from_emoji(e: &'static Emoji) -> Self {
        let name = e.name().to_string();
        let mut words: Vec<String> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .collect();
        words.sort();
        words.dedup();
        let mut codes = Vec::new();
        for sc in e.shortcodes() {
            codes.push(sc.to_lowercase());
            codes.extend(sc.split('_').map(str::to_lowercase));
        }
        codes.sort();
        codes.dedup();
        codes.retain(|c| !words.contains(c));
        Self {
            text: e.as_str().to_string(),
            name,
            words,
            codes,
            kind: Kind::Emoji(e),
        }
    }

    fn from_char(c: char) -> Option<Self> {
        let name = unicode_names2::name(c)?.to_string().to_lowercase();
        let mut words: Vec<String> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        words.sort();
        words.dedup();
        Some(Self {
            text: c.to_string(),
            name,
            words,
            codes: Vec::new(),
            kind: Kind::Char(c),
        })
    }

    pub fn emoji(&self) -> Option<&'static Emoji> {
        match self.kind {
            Kind::Emoji(e) => Some(e),
            Kind::Char(_) => None,
        }
    }

    /// `U+1F600 U+FE0F` for the tooltip and detail strip.
    pub fn code_points(&self) -> String {
        self.text
            .chars()
            .map(|c| format!("U+{:04X}", c as u32))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The emoji as shown with `tone` applied, when it takes a skin tone.
    pub fn with_tone(&self, tone: SkinTone) -> &str {
        match self.kind {
            Kind::Emoji(e) if tone != SkinTone::Default => {
                e.with_skin_tone(tone).map_or(e.as_str(), Emoji::as_str)
            }
            Kind::Emoji(e) => e.as_str(),
            Kind::Char(_) => &self.text,
        }
    }

    /// The one-tone variants (base first) for the detail strip; `None` for emoji that
    /// take no skin tone. Multi-person emoji have mixed-tone forms too; those are left out.
    pub fn skin_tones(&self) -> Option<Vec<&'static Emoji>> {
        let e = self.emoji()?;
        e.skin_tone()?;
        Some(
            [
                SkinTone::Default,
                SkinTone::Light,
                SkinTone::MediumLight,
                SkinTone::Medium,
                SkinTone::MediumDark,
                SkinTone::Dark,
            ]
            .into_iter()
            .filter_map(|t| e.with_skin_tone(t))
            .collect(),
        )
    }
}

/// The picker's tabs, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Recent,
    Smileys,
    People,
    Animals,
    Food,
    Travel,
    Activities,
    Objects,
    Symbols,
    Flags,
    Letters,
    TextSymbols,
}

impl Tab {
    pub const ALL: [Tab; 12] = [
        Tab::Recent,
        Tab::Smileys,
        Tab::People,
        Tab::Animals,
        Tab::Food,
        Tab::Travel,
        Tab::Activities,
        Tab::Objects,
        Tab::Symbols,
        Tab::Flags,
        Tab::Letters,
        Tab::TextSymbols,
    ];

    fn group(self) -> Option<emojis::Group> {
        use emojis::Group as G;
        Some(match self {
            Tab::Smileys => G::SmileysAndEmotion,
            Tab::People => G::PeopleAndBody,
            Tab::Animals => G::AnimalsAndNature,
            Tab::Food => G::FoodAndDrink,
            Tab::Travel => G::TravelAndPlaces,
            Tab::Activities => G::Activities,
            Tab::Objects => G::Objects,
            Tab::Symbols => G::Symbols,
            Tab::Flags => G::Flags,
            Tab::Recent | Tab::Letters | Tab::TextSymbols => return None,
        })
    }

    pub fn next(self) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + 1) % Tab::ALL.len()]
    }

    pub fn previous(self) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// A run of glyphs under one heading within a tab.
#[derive(Debug, Clone)]
pub struct Section {
    pub title: Option<String>,
    /// Indexes into `Catalog::glyphs`.
    pub glyphs: Vec<usize>,
}

pub struct Catalog {
    pub glyphs: Vec<Glyph>,
    tabs: HashMap<Tab, Vec<Section>>,
    /// Index by inserted text, for favourites and recents.
    by_text: HashMap<String, usize>,
    /// Characters with both a text-style and an emoji-style variation sequence.
    variation: HashSet<char>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// The catalogue, built on first use (about 5 ms in a release build). Call it off the UI
/// thread once at start-up so the first open never pays for it.
pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(Catalog::build)
}

impl Catalog {
    fn build() -> Self {
        let mut glyphs = Vec::with_capacity(4000);
        let mut tabs = HashMap::new();
        let mut by_text = HashMap::new();
        let mut push = |glyphs: &mut Vec<Glyph>, g: Glyph| -> usize {
            if let Some(&i) = by_text.get(&g.text) {
                return i;
            }
            let i = glyphs.len();
            by_text.insert(g.text.clone(), i);
            glyphs.push(g);
            i
        };

        for tab in Tab::ALL {
            let Some(group) = tab.group() else { continue };
            let idx: Vec<usize> = group
                .emojis()
                // `iter()` lists the base form only, but be safe about tone variants.
                .filter(|e| e.skin_tone().is_none_or(|t| t == SkinTone::Default))
                .map(|e| push(&mut glyphs, Glyph::from_emoji(e)))
                .collect();
            tabs.insert(
                tab,
                vec![Section {
                    title: None,
                    glyphs: idx,
                }],
            );
        }

        let letters = letter_sections(&mut glyphs, &mut push);
        tabs.insert(Tab::Letters, letters);
        let symbols = symbol_sections(&mut glyphs, &mut push);
        tabs.insert(Tab::TextSymbols, symbols);
        tabs.insert(Tab::Recent, Vec::new());

        Self {
            glyphs,
            tabs,
            by_text,
            variation: parse_variation_sequences(VARIATION_SEQUENCES),
        }
    }

    pub fn sections(&self, tab: Tab) -> &[Section] {
        self.tabs.get(&tab).map_or(&[], Vec::as_slice)
    }

    pub fn index_of(&self, text: &str) -> Option<usize> {
        self.by_text.get(text).copied().or_else(|| {
            // A recent pick with a skin tone applied maps back to its base emoji.
            let e = emojis::get(text)?;
            let base = e.skin_tones()?.next()?;
            self.by_text.get(base.as_str()).copied()
        })
    }

    /// The three presentation forms of a single-code-point emoji, in the order
    /// bare, text style, emoji style; `None` when Unicode defines no variation sequence.
    pub fn variants(&self, g: &Glyph) -> Option<Variants> {
        let mut chars = g.text.chars();
        let base = chars.next()?;
        let rest: Vec<char> = chars.collect();
        if !(rest.is_empty() || rest == [VS16_EMOJI]) || !self.variation.contains(&base) {
            return None;
        }
        Some(Variants {
            base,
            default_is_emoji: matches!(
                base.emoji_status(),
                EmojiStatus::EmojiPresentation
                    | EmojiStatus::EmojiPresentationAndModifierBase
                    | EmojiStatus::EmojiPresentationAndEmojiComponent
                    | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
            ),
        })
    }

    /// Ranked search over names, shortcodes, the characters themselves and `U+XXXX` codes.
    /// Every word of the query must match; exact words rank above prefixes above substrings.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        // (rank, name length in words, index): at equal rank a shorter name is the
        // closer match, so 'heart' lists 'red heart' before 'smiling face with heart-eyes'.
        let mut hits: Vec<(u8, usize, usize)> = Vec::new();
        if let Some(i) = self.index_of(query.trim()) {
            hits.push((0, 0, i));
        }
        // `:heart:` or `heart` as a GitHub shortcode names one emoji exactly.
        if let Some(i) = emojis::get_by_shortcode(q.trim_matches(':'))
            .and_then(|e| self.index_of(e.as_str()))
        {
            hits.push((0, 0, i));
        }
        if let Some(c) = parse_code_point(&q)
            && let Some(&i) = self
                .by_text
                .get(&c.to_string())
                .or_else(|| self.by_text.get(&format!("{c}{VS16_EMOJI}")))
        {
            hits.push((0, 0, i));
        }
        let words: Vec<&str> = q
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        if !words.is_empty() {
            for (i, g) in self.glyphs.iter().enumerate() {
                let mut worst = 0u8;
                let mut all = true;
                for w in &words {
                    let rank = if g.words.iter().any(|gw| gw == w) {
                        1
                    } else if g.codes.iter().any(|gw| gw == w) {
                        2
                    } else if g.words.iter().any(|gw| gw.starts_with(w)) {
                        3
                    } else if g.words.iter().chain(&g.codes).any(|gw| gw.contains(w)) {
                        4
                    } else {
                        all = false;
                        break;
                    };
                    worst = worst.max(rank);
                }
                if all {
                    // Emoji ahead of plain characters at the same rank: 'heart' should
                    // list ❤️ before ♥.
                    let tie = u8::from(g.emoji().is_none());
                    hits.push((worst * 2 + tie, g.words.len(), i));
                }
            }
        }
        hits.sort();
        let mut seen = HashSet::new();
        hits.into_iter()
            .filter(|&(_, _, i)| seen.insert(i))
            .map(|(_, _, i)| i)
            .collect()
    }
}

/// `u+1f600`, `1f600`, `03c4`: a code point typed into the search field.
fn parse_code_point(q: &str) -> Option<char> {
    let hex = q.strip_prefix("u+").unwrap_or(q);
    if !(2..=6).contains(&hex.len()) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // Plain hex is ambiguous with words like 'cafe' or 'bead'; require u+ below 4 digits.
    if hex.len() < 4 && !q.starts_with("u+") {
        return None;
    }
    char::from_u32(u32::from_str_radix(hex, 16).ok()?)
}

/// The bare, text-style and emoji-style forms of one character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variants {
    pub base: char,
    /// Unicode's default presentation for the bare character.
    pub default_is_emoji: bool,
}

impl Variants {
    pub fn bare(&self) -> String {
        self.base.to_string()
    }
    pub fn text(&self) -> String {
        format!("{}{VS15_TEXT}", self.base)
    }
    pub fn emoji(&self) -> String {
        format!("{}{VS16_EMOJI}", self.base)
    }
}

fn parse_variation_sequences(text: &str) -> HashSet<char> {
    let mut text_style = HashSet::new();
    let mut emoji_style = HashSet::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(';');
        let seq = parts.next().unwrap_or("").trim();
        let style = parts.next().unwrap_or("").trim();
        let Some(first) = seq.split_whitespace().next() else {
            continue;
        };
        let Some(c) = u32::from_str_radix(first, 16).ok().and_then(char::from_u32) else {
            continue;
        };
        match style {
            "text style" => text_style.insert(c),
            "emoji style" => emoji_style.insert(c),
            _ => false,
        };
    }
    text_style.intersection(&emoji_style).copied().collect()
}

// ---- letters and text symbols ------------------------------------------------------------

fn is_letter(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::ModifierLetter
    )
}

/// Accented Latin letters grouped by base letter (`A a`, `B b`, ...), then the Latin
/// letters with no ASCII base, then Greek and Cyrillic.
fn letter_sections(
    glyphs: &mut Vec<Glyph>,
    push: &mut impl FnMut(&mut Vec<Glyph>, Glyph) -> usize,
) -> Vec<Section> {
    const LATIN: [(u32, u32); 4] = [
        (0x00C0, 0x00FF), // Latin-1 Supplement letters
        (0x0100, 0x017F), // Latin Extended-A
        (0x0180, 0x024F), // Latin Extended-B
        (0x1E00, 0x1EFF), // Latin Extended Additional
    ];
    // (base letter, marks after NFD, lowercase, char): sorted so È è É é sit together.
    let mut by_base: Vec<(char, String, bool, char)> = Vec::new();
    let mut other = Vec::new();
    for &(lo, hi) in &LATIN {
        for c in (lo..=hi).filter_map(char::from_u32).filter(|c| is_letter(*c)) {
            let nfd: String = c.nfd().collect();
            let mut it = nfd.chars();
            let base = it.next().unwrap_or(c);
            let marks: String = it.collect();
            if base.is_ascii_alphabetic() && !marks.is_empty() {
                by_base.push((base.to_ascii_lowercase(), marks, c.is_lowercase(), c));
            } else {
                other.push(c);
            }
        }
    }
    by_base.sort();
    let mut sections: Vec<Section> = Vec::new();
    for (base, _, _, c) in by_base {
        let Some(g) = Glyph::from_char(c) else { continue };
        let i = push(glyphs, g);
        let title = format!("{} {base}", base.to_ascii_uppercase());
        match sections.last_mut() {
            Some(s) if s.title.as_deref() == Some(&title) => s.glyphs.push(i),
            _ => sections.push(Section {
                title: Some(title),
                glyphs: vec![i],
            }),
        }
    }
    let mut named = |glyphs: &mut Vec<Glyph>, title: &str, chars: Vec<char>| {
        let idx: Vec<usize> = chars
            .into_iter()
            .filter_map(Glyph::from_char)
            .map(|g| push(glyphs, g))
            .collect();
        if !idx.is_empty() {
            sections.push(Section {
                title: Some(title.to_string()),
                glyphs: idx,
            });
        }
    };
    named(glyphs, "Other Latin", other);
    named(glyphs, "Greek", range_chars(0x0370, 0x03FF, is_letter));
    named(glyphs, "Cyrillic", range_chars(0x0400, 0x04FF, is_letter));
    sections
}

fn range_chars(lo: u32, hi: u32, keep: impl Fn(char) -> bool) -> Vec<char> {
    (lo..=hi).filter_map(char::from_u32).filter(|c| keep(*c)).collect()
}

/// Punctuation, currency, maths, arrows and the rest, leaving out anything that is an
/// emoji (those live in the emoji tabs) and invisible characters.
fn symbol_sections(
    glyphs: &mut Vec<Glyph>,
    push: &mut impl FnMut(&mut Vec<Glyph>, Glyph) -> usize,
) -> Vec<Section> {
    let visible = |c: char| {
        !matches!(
            c.general_category(),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::SpaceSeparator
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
                | GeneralCategory::NonspacingMark
                | GeneralCategory::EnclosingMark
                | GeneralCategory::Unassigned
                | GeneralCategory::PrivateUse
                | GeneralCategory::Surrogate
        ) && matches!(
            c.emoji_status(),
            EmojiStatus::NonEmoji | EmojiStatus::NonEmojiButEmojiComponent
        )
    };
    let blocks: [(&str, Vec<(u32, u32)>); 9] = [
        (
            "Punctuation",
            vec![(0x00A1, 0x00BF), (0x2010, 0x2027), (0x2030, 0x205E)],
        ),
        ("Currency", vec![(0x20A0, 0x20C0)]),
        ("Letterlike and numbers", vec![(0x2100, 0x218F)]),
        (
            "Arrows",
            vec![(0x2190, 0x21FF), (0x27F0, 0x27FF), (0x2900, 0x297F), (0x2B00, 0x2B4F)],
        ),
        (
            "Maths",
            vec![(0x2200, 0x22FF), (0x27C0, 0x27EF), (0x2980, 0x2AFF), (0x00D7, 0x00D7), (0x00F7, 0x00F7)],
        ),
        ("Technical", vec![(0x2300, 0x23FF)]),
        ("Shapes", vec![(0x25A0, 0x25FF), (0x2B50, 0x2BFF)]),
        ("Boxes and blocks", vec![(0x2500, 0x259F)]),
        ("Other symbols", vec![(0x2600, 0x27BF), (0x2460, 0x24FF)]),
    ];
    let mut sections = Vec::new();
    for (title, ranges) in blocks {
        let mut idx = Vec::new();
        for (lo, hi) in ranges {
            for c in range_chars(lo, hi, visible) {
                if is_letter(c) && !matches!(title, "Letterlike and numbers") {
                    continue;
                }
                if let Some(g) = Glyph::from_char(c) {
                    idx.push(push(glyphs, g));
                }
            }
        }
        if !idx.is_empty() {
            sections.push(Section {
                title: Some(title.to_string()),
                glyphs: idx,
            });
        }
    }
    sections
}

// ---- user preferences --------------------------------------------------------------------

/// `emoji.toml`: favourites, recent picks and the chosen skin tone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Inserted text, most recently favourited last.
    pub favourites: Vec<String>,
    /// Inserted text, most recent first.
    pub recent: Vec<String>,
    /// `default`, `light`, `medium-light`, `medium`, `medium-dark` or `dark`.
    pub skin_tone: String,
}

pub const TONES: [(SkinTone, &str); 6] = [
    (SkinTone::Default, "default"),
    (SkinTone::Light, "light"),
    (SkinTone::MediumLight, "medium-light"),
    (SkinTone::Medium, "medium"),
    (SkinTone::MediumDark, "medium-dark"),
    (SkinTone::Dark, "dark"),
];

impl Prefs {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(self)?;
        std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
    }

    pub fn tone(&self) -> SkinTone {
        TONES
            .iter()
            .find(|(_, name)| *name == self.skin_tone)
            .map_or(SkinTone::Default, |(t, _)| *t)
    }

    pub fn set_tone(&mut self, tone: SkinTone) {
        self.skin_tone = TONES
            .iter()
            .find(|(t, _)| *t == tone)
            .map_or("default", |(_, n)| *n)
            .to_string();
    }

    pub fn is_favourite(&self, text: &str) -> bool {
        self.favourites.iter().any(|f| f == text)
    }

    /// Returns whether it is now a favourite.
    pub fn toggle_favourite(&mut self, text: &str) -> bool {
        if let Some(i) = self.favourites.iter().position(|f| f == text) {
            self.favourites.remove(i);
            false
        } else {
            self.favourites.push(text.to_string());
            true
        }
    }

    pub fn push_recent(&mut self, text: &str) {
        self.recent.retain(|r| r != text);
        self.recent.insert(0, text.to_string());
        self.recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variation_sequences_cover_hearts_but_not_grinning_face() {
        let c = catalog();
        let heart = &c.glyphs[c.index_of("❤️").unwrap()];
        let v = c.variants(heart).unwrap();
        assert!(!v.default_is_emoji);
        assert_eq!(v.text(), "❤\u{FE0E}");
        assert_eq!(v.emoji(), "❤\u{FE0F}");
        let grin = &c.glyphs[c.index_of("😀").unwrap()];
        assert!(c.variants(grin).is_none());
        let watch = &c.glyphs[c.index_of("⌚").unwrap()];
        assert!(c.variants(watch).unwrap().default_is_emoji);
    }

    #[test]
    fn search_ranks_words_first() {
        let c = catalog();
        let pi = c.search("pi");
        let top: Vec<&str> = pi.iter().take(5).map(|&i| c.glyphs[i].name.as_str()).collect();
        assert!(top.iter().all(|n| n.split_whitespace().any(|w| w == "pi")), "{top:?}");
        assert!(top.contains(&"greek small letter pi"), "{top:?}");
        assert!(pi.iter().any(|&i| c.glyphs[i].name == "pineapple"));
        let tau = c.search("tau");
        assert!(tau.iter().take(2).all(|&i| c.glyphs[i].name.contains("tau")));
        let e = c.search("e acute");
        assert!(["é", "É"].contains(&c.glyphs[e[0]].text.as_str()));
        assert_eq!(c.glyphs[c.search("u+03c4")[0]].text, "τ");
        assert_eq!(c.glyphs[c.search("τ")[0]].text, "τ");
        assert_eq!(c.glyphs[c.search(":heart:")[0]].text, "❤️");
        assert!(c.search("").is_empty());
    }

    #[test]
    fn tabs_and_letters() {
        let c = catalog();
        assert!(c.sections(Tab::Smileys)[0].glyphs.len() > 100);
        let letters = c.sections(Tab::Letters);
        assert_eq!(letters[0].title.as_deref(), Some("A a"));
        assert!(letters.iter().any(|s| s.title.as_deref() == Some("Greek")));
        let e = letters.iter().find(|s| s.title.as_deref() == Some("E e")).unwrap();
        let first: Vec<&str> = e.glyphs[..4].iter().map(|&i| c.glyphs[i].text.as_str()).collect();
        assert_eq!(first, ["È", "è", "É", "é"]);
        // Emoji are not repeated in the text symbols tab.
        for s in c.sections(Tab::TextSymbols) {
            assert!(s.glyphs.iter().all(|&i| c.glyphs[i].emoji().is_none()));
        }
    }

    #[test]
    fn skin_tones_and_prefs() {
        let c = catalog();
        let wave = &c.glyphs[c.index_of("👋").unwrap()];
        assert_eq!(wave.skin_tones().unwrap().len(), 6);
        assert_eq!(wave.with_tone(SkinTone::Dark), "👋🏿");
        assert_eq!(c.index_of("👋🏿"), c.index_of("👋"));
        let mut p = Prefs::default();
        assert!(p.toggle_favourite("👋"));
        assert!(!p.toggle_favourite("👋"));
        p.set_tone(SkinTone::Medium);
        assert_eq!(p.tone(), SkinTone::Medium);
        p.push_recent("a");
        p.push_recent("b");
        p.push_recent("a");
        assert_eq!(p.recent, ["a", "b"]);
    }
}
