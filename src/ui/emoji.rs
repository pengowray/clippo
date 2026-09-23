//! The emoji and symbol picker page (`clippo emoji`), shown in the same overlay as the
//! history list. Tabs across the top, a grid of characters, and a detail strip that names
//! the hovered or selected character and offers its presentation and skin-tone variants.
//! See docs/menu-design.md section 16.

use std::path::PathBuf;

use cosmic::app::Task;
use cosmic::iced::keyboard::key::Named;
use cosmic::iced::keyboard::{Key, Modifiers};
use cosmic::iced::widget::scrollable::{RelativeOffset, snap_to};
use cosmic::iced::{self, Alignment, Border, Length};
use cosmic::widget::{self, button, column, container, mouse_area, row, text, tooltip};
use cosmic::{Element, Theme, theme};
use emojis::SkinTone;

use crate::config::Paths;
use crate::emoji::{Catalog, Glyph, Prefs, Section, TONES, Tab, catalog};
use crate::ui::app::Message as AppMessage;
use crate::ui::list::muted_text;
use crate::ui::strings;

/// Cells per grid row: 19 × (36 + 4) fits the 784 px inside the window padding.
pub const COLS: usize = 19;
const CELL: f32 = 36.0;
const CELL_SPACING: u16 = 4;
const GLYPH_SIZE: u16 = 20;
const HEADING_HEIGHT: f32 = 26.0;
const TAB_SIZE: f32 = 32.0;
const TONE_SIZE: f32 = 26.0;
/// Grid rows a Page Up / Page Down moves by.
const ROWS_PER_PAGE: usize = 6;
/// Grid lines built into the widget tree at once; the rest is a placeholder until scrolled to.
const RENDER_CHUNK: usize = 14;
const RENDER_MARGIN: usize = 3;
/// Search results beyond this are not shown; the count says so.
const MAX_RESULTS: usize = 600;

pub static SCROLL_ID: std::sync::LazyLock<widget::Id> =
    std::sync::LazyLock::new(|| widget::Id::new("clippo-emoji-grid"));

/// Colour emoji need the emoji font asked for by name; the default fallback picks a
/// monochrome font for many of them.
pub const EMOJI_FONT: cosmic::font::Font = cosmic::font::Font {
    family: cosmic::iced::font::Family::Name("Noto Color Emoji"),
    weight: cosmic::iced::font::Weight::Normal,
    stretch: cosmic::iced::font::Stretch::Normal,
    style: cosmic::iced::font::Style::Normal,
};

#[derive(Debug, Clone)]
pub enum Message {
    Query(String),
    ClearSearch,
    Tab(Tab),
    Tone(SkinTone),
    Hover(Option<usize>),
    /// A cell was clicked (flat index). With Alt held it is favourited instead of pasted.
    Click(usize),
    /// A chip in the detail strip: paste exactly this text.
    Insert(String),
    /// The star in the detail strip.
    ToggleFavourite(String),
    Scrolled(f32),
}

/// One line of the grid.
enum Line {
    Heading(String),
    /// (flat cell index, glyph index) per cell.
    Row(Vec<(usize, usize)>),
}

/// The grid laid out for the current tab or search.
struct Layout {
    lines: Vec<Line>,
    /// Glyph index per flat cell.
    cells: Vec<usize>,
    /// (line, column) per flat cell.
    place: Vec<(usize, usize)>,
    /// Search results beyond `MAX_RESULTS`.
    truncated: usize,
}

impl Layout {
    fn from_sections<'a>(sections: impl Iterator<Item = (Option<&'a str>, &'a [usize])>) -> Self {
        let mut lines = Vec::new();
        let mut cells = Vec::new();
        let mut place = Vec::new();
        for (title, glyphs) in sections {
            if let Some(t) = title {
                lines.push(Line::Heading(t.to_string()));
            }
            for chunk in glyphs.chunks(COLS) {
                let line = lines.len();
                let row = chunk
                    .iter()
                    .enumerate()
                    .map(|(col, &g)| {
                        let flat = cells.len();
                        cells.push(g);
                        place.push((line, col));
                        (flat, g)
                    })
                    .collect();
                lines.push(Line::Row(row));
            }
        }
        Self {
            lines,
            cells,
            place,
            truncated: 0,
        }
    }

    /// The flat index of the cell `delta` rows away in the same column, staying inside
    /// grid rows (headings are skipped).
    fn vertical(&self, from: usize, delta: isize) -> usize {
        let Some(&(line, col)) = self.place.get(from) else {
            return 0;
        };
        let rows: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| matches!(l, Line::Row(_)).then_some(i))
            .collect();
        let Some(pos) = rows.iter().position(|&r| r == line) else {
            return from;
        };
        let target = (pos as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        match &self.lines[rows[target]] {
            Line::Row(cells) => cells[col.min(cells.len() - 1)].0,
            Line::Heading(_) => from,
        }
    }
}

pub struct State {
    catalog: &'static Catalog,
    prefs: Prefs,
    prefs_path: PathBuf,
    tab: Tab,
    query: String,
    /// Flat cell index in the current layout.
    selected: usize,
    hovered: Option<usize>,
    render_lines: usize,
}

impl State {
    pub fn new(paths: &Paths) -> Self {
        Self {
            catalog: catalog(),
            prefs: Prefs::load(&paths.emoji_prefs),
            prefs_path: paths.emoji_prefs.clone(),
            tab: Tab::Smileys,
            query: String::new(),
            selected: 0,
            hovered: None,
            render_lines: RENDER_CHUNK,
        }
    }

    /// Fresh state for a new open: favourites and recents re-read, search cleared. The
    /// first tab is the recent one when it has anything, else smileys.
    pub fn reset(&mut self) {
        self.prefs = Prefs::load(&self.prefs_path);
        self.query.clear();
        self.selected = 0;
        self.hovered = None;
        self.render_lines = RENDER_CHUNK;
        self.tab = if self.prefs.favourites.is_empty() && self.prefs.recent.is_empty() {
            Tab::Smileys
        } else {
            Tab::Recent
        };
    }

    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn clear_search(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.render_lines = RENDER_CHUNK;
    }

    fn layout(&self) -> Layout {
        if self.searching() {
            let hits = self.catalog.search(&self.query);
            let truncated = hits.len().saturating_sub(MAX_RESULTS);
            let shown = &hits[..hits.len() - truncated];
            let mut l = Layout::from_sections(std::iter::once((None, shown)));
            l.truncated = truncated;
            return l;
        }
        if self.tab == Tab::Recent {
            let favs: Vec<usize> = self
                .prefs
                .favourites
                .iter()
                .filter_map(|t| self.catalog.index_of(t))
                .collect();
            let recent: Vec<usize> = self
                .prefs
                .recent
                .iter()
                .filter_map(|t| self.catalog.index_of(t))
                .filter(|i| !favs.contains(i))
                .collect();
            let mut sections: Vec<(Option<&str>, &[usize])> = Vec::new();
            if !favs.is_empty() {
                sections.push((Some(strings::EMOJI_FAVOURITES), &favs));
            }
            if !recent.is_empty() {
                sections.push((Some(strings::EMOJI_RECENT), &recent));
            }
            return Layout::from_sections(sections.into_iter());
        }
        Layout::from_sections(
            self.catalog
                .sections(self.tab)
                .iter()
                .map(|s: &Section| (s.title.as_deref(), s.glyphs.as_slice())),
        )
    }

    fn glyph_at(&self, layout: &Layout, flat: usize) -> Option<&'static Glyph> {
        layout.cells.get(flat).map(|&g| &self.catalog.glyphs[g])
    }

    /// The glyph the detail strip describes: hovered first, else selected.
    fn focus(&self, layout: &Layout) -> Option<&'static Glyph> {
        self.hovered
            .and_then(|h| self.glyph_at(layout, h))
            .or_else(|| self.glyph_at(layout, self.selected))
    }

    fn tone(&self) -> SkinTone {
        self.prefs.tone()
    }

    fn save_prefs(&self) {
        if let Err(e) = self.prefs.save(&self.prefs_path) {
            crate::log(&format!("emoji: {e:#}"));
        }
    }

    fn set_selected(&mut self, flat: usize) -> Task<AppMessage> {
        self.selected = flat;
        let layout = self.layout();
        if let Some(&(line, _)) = layout.place.get(flat)
            && line + RENDER_MARGIN >= self.render_lines
        {
            self.render_lines = line + RENDER_CHUNK;
        }
        let n = layout.lines.len();
        let y = match layout.place.get(flat) {
            Some(&(line, _)) if n > 1 => line as f32 / (n - 1) as f32,
            _ => 0.0,
        };
        snap_to(SCROLL_ID.clone(), RelativeOffset { x: None, y: Some(y) })
    }

    fn set_tab(&mut self, tab: Tab) -> Task<AppMessage> {
        self.tab = tab;
        self.query.clear();
        self.hovered = None;
        self.render_lines = RENDER_CHUNK;
        self.set_selected(0)
    }

    /// Paste `text` (or copy it with `copy_only`), remembering it as a recent pick.
    fn insert(&mut self, text: String, copy_only: bool) -> Task<AppMessage> {
        self.prefs.push_recent(&text);
        self.save_prefs();
        let msg = if copy_only {
            AppMessage::CopyText(text)
        } else {
            AppMessage::PasteText(text)
        };
        cosmic::task::message(cosmic::Action::App(msg))
    }

    fn insert_selected(&mut self, copy_only: bool) -> Task<AppMessage> {
        let layout = self.layout();
        let Some(g) = self.glyph_at(&layout, self.selected) else {
            return Task::none();
        };
        let text = g.with_tone(self.tone()).to_string();
        self.insert(text, copy_only)
    }

    fn toggle_favourite(&mut self, text: &str) {
        self.prefs.toggle_favourite(text);
        self.save_prefs();
    }

    pub fn update(&mut self, m: Message, mods: Modifiers) -> Task<AppMessage> {
        match m {
            Message::Query(q) => {
                self.query = q;
                self.hovered = None;
                self.render_lines = RENDER_CHUNK;
                self.set_selected(0)
            }
            Message::ClearSearch => {
                self.clear_search();
                Task::none()
            }
            Message::Tab(t) => self.set_tab(t),
            Message::Tone(t) => {
                self.prefs.set_tone(t);
                self.save_prefs();
                Task::none()
            }
            Message::Hover(h) => {
                self.hovered = h;
                Task::none()
            }
            Message::Click(flat) => {
                let layout = self.layout();
                let Some(g) = self.glyph_at(&layout, flat) else {
                    return Task::none();
                };
                if mods.alt() {
                    self.toggle_favourite(&g.text);
                    return Task::none();
                }
                self.selected = flat;
                let text = g.with_tone(self.tone()).to_string();
                self.insert(text, mods.control())
            }
            Message::Insert(text) => self.insert(text, mods.control()),
            Message::ToggleFavourite(text) => {
                self.toggle_favourite(&text);
                Task::none()
            }
            Message::Scrolled(y) => {
                if y > 0.6 && self.render_lines < self.layout().lines.len() {
                    self.render_lines += RENDER_CHUNK;
                }
                Task::none()
            }
        }
    }

    /// Keys not handled by the window itself (Escape, Super+V, Super+., Ctrl+,).
    pub fn on_key(&mut self, key: &Key, mods: Modifiers) -> Task<AppMessage> {
        let layout = self.layout();
        let last = layout.cells.len().saturating_sub(1);
        match key {
            Key::Named(Named::Tab) if mods.control() => {
                let t = if mods.shift() {
                    self.tab.previous()
                } else {
                    self.tab.next()
                };
                self.set_tab(t)
            }
            Key::Named(Named::ArrowLeft) => self.set_selected(self.selected.saturating_sub(1)),
            Key::Named(Named::ArrowRight) => self.set_selected((self.selected + 1).min(last)),
            Key::Named(Named::ArrowUp) => self.set_selected(layout.vertical(self.selected, -1)),
            Key::Named(Named::ArrowDown) => self.set_selected(layout.vertical(self.selected, 1)),
            Key::Named(Named::PageUp) => {
                self.set_selected(layout.vertical(self.selected, -(ROWS_PER_PAGE as isize)))
            }
            Key::Named(Named::PageDown) => {
                self.set_selected(layout.vertical(self.selected, ROWS_PER_PAGE as isize))
            }
            Key::Named(Named::Home) if mods.control() => self.set_selected(0),
            Key::Named(Named::End) if mods.control() => self.set_selected(last),
            Key::Named(Named::Enter) if mods.alt() => {
                if let Some(g) = self.glyph_at(&layout, self.selected) {
                    self.toggle_favourite(&g.text);
                }
                Task::none()
            }
            Key::Named(Named::Enter) => self.insert_selected(mods.control()),
            _ => Task::none(),
        }
    }

    // ---- view ---------------------------------------------------------------------------

    pub fn view(&self) -> Element<'_, Message> {
        let layout = self.layout();
        let count = if self.searching() {
            strings::emoji_match_count(layout.cells.len(), layout.truncated)
        } else {
            String::new()
        };
        let search = widget::search_input(strings::EMOJI_SEARCH_PLACEHOLDER, &self.query)
            .id(crate::ui::app::SEARCH_ID.clone())
            .always_active()
            .on_input(Message::Query)
            .on_clear(Message::ClearSearch)
            .width(Length::Fill);
        let header = row![search, text::caption(count)]
            .spacing(8)
            .align_y(Alignment::Center)
            .height(40);

        column![
            header,
            self.tab_bar(),
            self.grid(&layout),
            self.detail_strip(&layout),
        ]
        .spacing(6)
        .height(Length::Fill)
        .into()
    }

    fn tab_bar(&self) -> Element<'_, Message> {
        let mut tabs = row![].spacing(2).align_y(Alignment::Center);
        for tab in Tab::ALL {
            let (icon, is_emoji) = tab_icon(tab);
            let mut glyph = text(icon).size(18);
            if is_emoji {
                glyph = glyph.font(EMOJI_FONT);
            }
            let cell = container(glyph)
                .width(TAB_SIZE)
                .height(TAB_SIZE)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .class(cell_style(tab == self.tab && !self.searching(), false, true));
            let area = mouse_area(cell).on_press(Message::Tab(tab));
            tabs = tabs.push(tooltip(
                area,
                text::caption(strings::emoji_tab_name(tab)),
                tooltip::Position::Bottom,
            ));
        }
        let mut tones = row![].spacing(2).align_y(Alignment::Center);
        let hand = emojis::get("✋").expect("raised hand is an emoji");
        for (tone, _) in TONES {
            let sample = hand.with_skin_tone(tone).map_or("✋", |e| e.as_str());
            let cell = container(text(sample).font(EMOJI_FONT).size(14))
                .width(TONE_SIZE)
                .height(TONE_SIZE)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .class(cell_style(tone == self.tone(), false, true));
            tones = tones.push(tooltip(
                mouse_area(cell).on_press(Message::Tone(tone)),
                text::caption(strings::skin_tone_tooltip(tone)),
                tooltip::Position::Bottom,
            ));
        }
        row![tabs, widget::Space::new().width(Length::Fill), tones]
            .align_y(Alignment::Center)
            .into()
    }

    fn grid<'a>(&'a self, layout: &Layout) -> Element<'a, Message> {
        let mut col = column![].spacing(CELL_SPACING).width(Length::Fill);
        if layout.cells.is_empty() {
            col = col.push(self.empty_state());
        }
        for (i, line) in layout.lines.iter().enumerate() {
            if i >= self.render_lines {
                let rest: f32 = layout.lines[i..]
                    .iter()
                    .map(|l| match l {
                        Line::Heading(_) => HEADING_HEIGHT,
                        Line::Row(_) => CELL + f32::from(CELL_SPACING),
                    })
                    .sum();
                col = col.push(widget::Space::new().height(rest));
                break;
            }
            let el: Element<'a, Message> = match line {
                Line::Heading(t) => container(
                    text(t.clone())
                        .size(12)
                        .class(theme::Text::Custom(muted_text)),
                )
                .height(HEADING_HEIGHT)
                .align_y(Alignment::End)
                .padding([0, 4])
                .into(),
                Line::Row(cells) => {
                    let mut r = row![].spacing(CELL_SPACING);
                    for &(flat, g) in cells {
                        r = r.push(self.cell(flat, &self.catalog.glyphs[g]));
                    }
                    r.into()
                }
            };
            col = col.push(el);
        }
        widget::scrollable(col)
            .id(SCROLL_ID.clone())
            .on_scroll(|vp| Message::Scrolled(vp.relative_offset().y))
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn cell<'a>(&self, flat: usize, g: &'static Glyph) -> Element<'a, Message> {
        let shown = g.with_tone(self.tone());
        let mut t = text(shown).size(GLYPH_SIZE);
        if g.emoji().is_some() {
            t = t.font(EMOJI_FONT);
        }
        let selected = flat == self.selected;
        let hovered = self.hovered == Some(flat);
        let c = container(t)
            .width(CELL)
            .height(CELL)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(cell_style(selected, hovered, false));
        mouse_area(c)
            .on_press(Message::Click(flat))
            .on_enter(Message::Hover(Some(flat)))
            .on_exit(Message::Hover(None))
            .into()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = if self.searching() {
            column![
                text(strings::no_matches(&self.query)),
                button::link(strings::CLEAR_SEARCH).on_press(Message::ClearSearch),
            ]
            .spacing(8)
            .align_x(Alignment::Center)
            .into()
        } else {
            column![
                text(strings::EMOJI_EMPTY_TITLE),
                text(strings::EMOJI_EMPTY_BODY).class(theme::Text::Custom(muted_text)),
            ]
            .spacing(4)
            .align_x(Alignment::Center)
            .into()
        };
        container(content)
            .width(Length::Fill)
            .padding(40)
            .align_x(Alignment::Center)
            .into()
    }

    /// Name, code points, presentation variants and skin tones of the focused character.
    fn detail_strip<'a>(&'a self, layout: &Layout) -> Element<'a, Message> {
        let Some(g) = self.focus(layout) else {
            return container(widget::Space::new())
                .height(STRIP_HEIGHT)
                .width(Length::Fill)
                .into();
        };
        let shown = g.with_tone(self.tone());
        let mut big = text(shown).size(28);
        if g.emoji().is_some() {
            big = big.font(EMOJI_FONT);
        }
        let name = column![
            text(g.name.as_str()).size(14),
            text(code_points(shown))
                .size(11)
                .class(theme::Text::Custom(muted_text)),
        ]
        .spacing(2)
        .width(Length::Fixed(230.0));

        let mut chips = row![].spacing(10).align_y(Alignment::End);
        if let Some(v) = self.catalog.variants(g) {
            let default_caption = if v.default_is_emoji {
                strings::VARIANT_DEFAULT_EMOJI
            } else {
                strings::VARIANT_DEFAULT_TEXT
            };
            chips = chips.push(chip(v.bare(), default_caption, false));
            chips = chips.push(chip(v.text(), strings::VARIANT_TEXT, false));
            chips = chips.push(chip(v.emoji(), strings::VARIANT_EMOJI, true));
        }
        if let Some(tones) = g.skin_tones() {
            let mut r = row![].spacing(2);
            for e in tones {
                r = r.push(tone_chip(e.as_str(), shown == e.as_str()));
            }
            chips = chips.push(r);
        }

        let fav = self.prefs.is_favourite(&g.text);
        let (icon, tip) = if fav {
            ("starred-symbolic", strings::REMOVE_FAVOURITE)
        } else {
            ("non-starred-symbolic", strings::ADD_FAVOURITE)
        };
        let star = tooltip(
            button::icon(widget::icon::from_name(icon).size(16))
                .class(theme::Button::Icon)
                .on_press(Message::ToggleFavourite(g.text.clone())),
            text::caption(tip),
            tooltip::Position::Top,
        );

        container(
            row![
                container(big)
                    .width(44)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
                name,
                widget::scrollable(chips)
                    .direction(iced::widget::scrollable::Direction::Horizontal(
                        iced::widget::scrollable::Scrollbar::new()
                            .width(0)
                            .scroller_width(0),
                    ))
                    .width(Length::Fill),
                star,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .height(STRIP_HEIGHT)
        .width(Length::Fill)
        .padding([0, 4])
        .into()
    }
}

const STRIP_HEIGHT: f32 = 56.0;

fn code_points(s: &str) -> String {
    s.chars()
        .map(|c| format!("U+{:04X}", c as u32))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A presentation-variant chip: the form above its caption; click pastes exactly that form.
fn chip<'a>(text_form: String, caption: &'static str, emoji_font: bool) -> Element<'a, Message> {
    let mut t = text(text_form.clone()).size(GLYPH_SIZE);
    if emoji_font {
        t = t.font(EMOJI_FONT);
    }
    let inner = column![
        container(t).height(CELL).align_y(Alignment::Center),
        text(caption).size(10).class(theme::Text::Custom(muted_text)),
    ]
    .align_x(Alignment::Center)
    .spacing(0);
    button::custom(inner)
        .class(theme::Button::Text)
        .padding([0, 6])
        .on_press(Message::Insert(text_form))
        .into()
}

fn tone_chip<'a>(text_form: &'static str, current: bool) -> Element<'a, Message> {
    let c = container(text(text_form).font(EMOJI_FONT).size(GLYPH_SIZE))
        .width(CELL)
        .height(CELL)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(cell_style(current, false, true));
    mouse_area(c)
        .on_press(Message::Insert(text_form.to_string()))
        .into()
}

/// The tab's icon and whether it needs the emoji font.
fn tab_icon(tab: Tab) -> (&'static str, bool) {
    match tab {
        Tab::Recent => ("🕘", true),
        Tab::Smileys => ("😀", true),
        Tab::People => ("👋", true),
        Tab::Animals => ("🐻", true),
        Tab::Food => ("🍔", true),
        Tab::Travel => ("✈️", true),
        Tab::Activities => ("⚽", true),
        Tab::Objects => ("💡", true),
        Tab::Symbols => ("🔣", true),
        Tab::Flags => ("🏁", true),
        Tab::Letters => ("Áé", false),
        Tab::TextSymbols => ("∑", false),
    }
}

fn with_alpha(c: cosmic::cosmic_theme::palette::Srgba, alpha: f32) -> iced::Color {
    let mut c = c;
    c.alpha = alpha;
    iced::Color::from(c)
}

/// Grid cell, tab and tone-button background: accent when selected, lightened when
/// hovered. `outline` adds an accent border for selected tabs and tones, which have no
/// other selection mark.
fn cell_style(selected: bool, hovered: bool, outline: bool) -> theme::Container<'static> {
    theme::Container::custom(move |t: &Theme| {
        let c = t.cosmic();
        let bg = if selected {
            Some(with_alpha(c.accent_color(), 0.25))
        } else if hovered {
            Some(with_alpha(c.on_bg_color(), 0.08))
        } else {
            None
        };
        iced::widget::container::Style {
            background: bg.map(iced::Background::Color),
            border: Border {
                radius: c.corner_radii.radius_s.into(),
                width: if selected && outline { 1.0 } else { 0.0 },
                color: iced::Color::from(c.accent_color()),
            },
            ..Default::default()
        }
    })
}
