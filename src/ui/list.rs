//! Rendering of the history list, its rows, empty states and the macro column.

use std::collections::HashMap;

use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::widget::text::Style as TextStyle;
use cosmic::iced::{self, Alignment, Border, Length};
use cosmic::widget::{self, button, column, container, mouse_area, row, text, tooltip};
use cosmic::{Element, Theme, theme};

use crate::ui::app::{App, Item, MACRO_COLUMN_WIDTH, Message, RowAction};
use crate::config::Macro;
use crate::ui::rows::{Kind, Ocr, Row};
use crate::ui::strings;

/// Rows a Page Up / Page Down moves by; roughly what fits (design 2).
pub const ROWS_PER_PAGE: usize = 7;
const ROW_PADDING: u16 = 12;
const ACTION_SIZE: f32 = 28.0;
const THUMB_SIZE: f32 = 96.0;
const SELECTION_BAR: f32 = 3.0;

// ---- theme helpers -------------------------------------------------------------------------

fn with_alpha(c: cosmic::cosmic_theme::palette::Srgba, alpha: f32) -> iced::Color {
    let mut c = c;
    c.alpha = alpha;
    iced::Color::from(c)
}

type Txt<'a> = widget::Text<'a, Theme, cosmic::Renderer>;

pub fn muted_text(t: &Theme) -> TextStyle {
    TextStyle {
        color: Some(with_alpha(t.cosmic().on_bg_color(), 0.6)),
        ..Default::default()
    }
}

pub fn warning_text(t: &Theme) -> TextStyle {
    TextStyle {
        color: Some(iced::Color::from(t.cosmic().warning_color())),
        ..Default::default()
    }
}

/// The overlay's own background and rounded border; non-main windows are not wrapped by
/// libcosmic.
pub fn window_style(t: &Theme) -> iced::widget::container::Style {
    let c = t.cosmic();
    iced::widget::container::Style {
        background: Some(iced::Background::Color(iced::Color::from(c.bg_color()))),
        border: Border {
            radius: c.corner_radii.radius_m.into(),
            width: 1.0,
            color: iced::Color::from(c.bg_divider()),
        },
        ..Default::default()
    }
}

fn row_style(selected: bool, hovered: bool) -> theme::Container<'static> {
    theme::Container::custom(move |t| {
        let c = t.cosmic();
        let bg = if selected {
            Some(with_alpha(c.accent_color(), 0.2))
        } else if hovered {
            Some(with_alpha(c.on_bg_color(), 0.08))
        } else {
            None
        };
        iced::widget::container::Style {
            background: bg.map(iced::Background::Color),
            ..Default::default()
        }
    })
}

fn accent_bar() -> theme::Container<'static> {
    theme::Container::custom(|t| iced::widget::container::Style {
        background: Some(iced::Background::Color(iced::Color::from(
            t.cosmic().accent_color(),
        ))),
        ..Default::default()
    })
}

fn banner_style() -> theme::Container<'static> {
    theme::Container::custom(|t| iced::widget::container::Style {
        background: Some(iced::Background::Color(with_alpha(
            t.cosmic().warning_color(),
            0.2,
        ))),
        border: Border {
            radius: t.cosmic().corner_radii.radius_s.into(),
            ..Default::default()
        },
        ..Default::default()
    })
}

fn muted<'a>(s: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Txt<'a> {
    text(s).class(theme::Text::Custom(muted_text))
}

fn one_line<'a>(s: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Txt<'a> {
    text(s)
        .wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
}

// ---- list ------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn view<'a>(
    app: &'a App,
    rows: &'a [Row],
    items: &[Item],
    selected: usize,
    hovered: Option<usize>,
    thumbs: &'a HashMap<i64, widget::image::Handle>,
    banner: Option<&'static str>,
) -> Element<'a, Message> {
    let mut col = column![].width(Length::Fill);

    if let Some(b) = banner {
        col = col.push(
            container(text::caption(b))
                .padding([6, 10])
                .width(Length::Fill)
                .class(banner_style()),
        );
    } else if app.is_top_clipboard() && matches!(items.first(), Some(Item::Entry(_))) {
        col = col.push(
            container(text::caption(strings::ON_CLIPBOARD_NOW).class(theme::Text::Custom(muted_text)))
                .padding([4, ROW_PADDING]),
        );
    }

    let entries = items.iter().filter(|i| matches!(i, Item::Entry(_))).count();
    if entries == 0 {
        col = col.push(empty_state(app, items));
    }

    for (pos, item) in items.iter().enumerate() {
        let is_selected = pos == selected;
        let is_hovered = hovered == Some(pos);
        let el: Element<'a, Message> = match *item {
            Item::Entry(i) => {
                let r = &rows[i];
                entry(pos, i, r, is_selected, is_hovered, thumbs.get(&r.id), app.ocr_engine_missing())
            }
            Item::OlderFold => older_fold(pos, app, is_selected, is_hovered),
            Item::Divider => container(muted(strings::OLDER_DIVIDER).size(12))
                .padding([6, ROW_PADDING])
                .width(Length::Fill)
                .into(),
        };
        col = col.push(el);
        col = col.push(widget::divider::horizontal::default());
    }

    widget::scrollable(col)
        .id(crate::ui::app::SCROLL_ID.clone())
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
}

fn empty_state<'a>(app: &'a App, items: &[Item]) -> Element<'a, Message> {
    let content: Element<'a, Message> = if let Some(e) = app.load_error() {
        column![text(strings::footer_read_failed(e))].into()
    } else if !app.query().trim().is_empty() {
        column![
            text(strings::no_matches(app.query())),
            button::link(strings::CLEAR_SEARCH).on_press(Message::ClearSearch),
        ]
        .spacing(8)
        .align_x(Alignment::Center)
        .into()
    } else if items.contains(&Item::OlderFold) {
        text(strings::EMPTY_RECENT).into()
    } else {
        column![text(strings::EMPTY_TITLE), muted(strings::EMPTY_BODY)]
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

fn older_fold<'a>(pos: usize, app: &'a App, selected: bool, hovered: bool) -> Element<'a, Message> {
    let n = app.older_total();
    let (icon, label) = if app.older_expanded() {
        ("go-up-symbolic", strings::older_expanded(n))
    } else {
        ("go-down-symbolic", strings::older_collapsed(n))
    };
    let content = row![
        selection_bar(selected),
        widget::icon::from_name(icon).size(16),
        muted(label),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    mouse_area(
        container(content)
            .padding([8, ROW_PADDING])
            .width(Length::Fill)
            .class(row_style(selected, hovered)),
    )
    .on_press(Message::ToggleOlder)
    .on_enter(Message::Hover(Some(pos)))
    .on_exit(Message::Hover(None))
    .into()
}

fn selection_bar<'a>(selected: bool) -> Element<'a, Message> {
    let bar = container(widget::Space::new().width(SELECTION_BAR).height(Length::Fill));
    if selected {
        bar.class(accent_bar()).into()
    } else {
        bar.into()
    }
}

fn entry<'a>(
    pos: usize,
    idx: usize,
    r: &'a Row,
    selected: bool,
    hovered: bool,
    thumb: Option<&'a widget::image::Handle>,
    no_engine: bool,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = match &r.kind {
        Kind::Text { lines, .. } => {
            let mut c = column![].spacing(0);
            for l in lines {
                c = c.push(one_line(l.as_str()));
            }
            c.width(Length::Fill).into()
        }
        Kind::Image {
            label,
            ocr,
            ocr_lines,
        } => {
            let mut info = column![one_line(label.as_str())].spacing(0);
            match ocr {
                Ocr::Text => {
                    for l in ocr_lines {
                        info = info.push(one_line(l.as_str()));
                    }
                }
                Ocr::Empty => info = info.push(muted(strings::OCR_EMPTY)),
                Ocr::Pending if no_engine => info = info.push(muted(strings::OCR_PENDING_NO_ENGINE)),
                Ocr::Pending => info = info.push(muted(strings::OCR_PENDING)),
                Ocr::Failed => {
                    info = info.push(
                        row![
                            widget::icon::from_name("dialog-warning-symbolic").size(14),
                            muted(strings::OCR_FAILED)
                        ]
                        .spacing(4)
                        .align_y(Alignment::Center),
                    )
                }
                Ocr::None => {}
            }
            row![thumbnail(thumb, r), info.width(Length::Fill)]
                .spacing(ROW_PADDING)
                .into()
        }
    };

    let mut main = column![row![body, actions(idx, r, selected || hovered)].spacing(4)]
        .width(Length::Fill);
    if let Some(hint) = r.overflow_hint() {
        main = main.push(
            container(muted(hint).size(12))
                .width(Length::Fill)
                .align_x(Alignment::End),
        );
    }

    let content = row![selection_bar(selected), main]
        .spacing(ROW_PADDING - 3)
        .align_y(Alignment::Start);

    let area = mouse_area(
        container(content)
            .padding([8, ROW_PADDING, 8, 0])
            .width(Length::Fill)
            .class(row_style(selected, hovered)),
    )
    .on_press(Message::Activate(pos))
    .on_enter(Message::Hover(Some(pos)))
    .on_exit(Message::Hover(None));
    let with_menu = widget::context_menu(area, Some(context_items(idx, r)));
    // Relative time lives in the tooltip, not the row (design 3.3).
    let mut when = strings::copied_ago(r.last_used, crate::store::now_ms());
    if let Some(bytes) = r.size_bytes {
        when.push_str(" · ");
        when.push_str(&strings::size_label(bytes));
    }
    tooltip(with_menu, text::caption(when), tooltip::Position::FollowCursor).into()
}

/// Right-click menu action: which row, and what to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowMenu {
    idx: usize,
    action: RowAction,
}

impl widget::menu::Action for RowMenu {
    type Message = Message;
    fn message(&self) -> Message {
        Message::Act(self.idx, self.action)
    }
}

/// Design 6: a stable shape; unavailable items are greyed, not hidden.
fn context_items(idx: usize, r: &Row) -> Vec<widget::menu::Tree<Message>> {
    use widget::menu::{Item as MenuItem, items};
    let act = |action| RowMenu { idx, action };
    let plain_ok = r.plain_disabled_reason().is_none();
    let mut list = vec![MenuItem::Button(
        strings::MENU_PASTE,
        None,
        act(RowAction::Paste),
    )];
    list.push(if plain_ok {
        MenuItem::Button(strings::PASTE_AS_PLAIN_TEXT, None, act(RowAction::PastePlain))
    } else {
        MenuItem::ButtonDisabled(strings::PASTE_AS_PLAIN_TEXT, None, act(RowAction::PastePlain))
    });
    list.push(if r.is_image() {
        MenuItem::ButtonDisabled(
            strings::PASTE_WITHOUT_MARKDOWN,
            None,
            act(RowAction::PasteNoMarkdown),
        )
    } else {
        MenuItem::Button(
            strings::PASTE_WITHOUT_MARKDOWN,
            None,
            act(RowAction::PasteNoMarkdown),
        )
    });
    list.push(MenuItem::Button(
        strings::MENU_COPY_ONLY,
        None,
        act(RowAction::CopyOnly),
    ));
    list.push(MenuItem::Divider);
    list.push(MenuItem::Button(strings::DELETE, None, act(RowAction::Delete)));
    items(&HashMap::new(), list)
}

fn thumbnail<'a>(thumb: Option<&'a widget::image::Handle>, r: &'a Row) -> Element<'a, Message> {
    let inner: Element<'a, Message> = match thumb {
        Some(h) => widget::image(h.clone())
            .content_fit(iced::ContentFit::Contain)
            .width(THUMB_SIZE)
            .height(THUMB_SIZE)
            .into(),
        None => {
            let fmt = r.mime.rsplit('/').next().unwrap_or("").to_ascii_uppercase();
            muted(fmt).size(12).into()
        }
    };
    container(inner)
        .width(THUMB_SIZE)
        .height(THUMB_SIZE)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(theme::Container::Secondary)
        .into()
}

fn action_button<'a>(
    glyph: &'a str,
    tip: String,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let b = button::custom(
        container(text(glyph).size(14))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .class(theme::Button::Icon)
    .width(ACTION_SIZE)
    .height(ACTION_SIZE)
    .padding(0)
    .on_press_maybe(on_press);
    tooltip(b, text::caption(tip), tooltip::Position::Bottom).into()
}

fn icon_action<'a>(name: &'a str, tip: &'a str, on_press: Message) -> Element<'a, Message> {
    let b = button::icon(widget::icon::from_name(name).size(16))
        .class(theme::Button::Icon)
        .width(ACTION_SIZE)
        .height(ACTION_SIZE)
        .on_press(on_press);
    tooltip(b, text::caption(tip), tooltip::Position::Bottom).into()
}

fn actions<'a>(idx: usize, r: &'a Row, show_delete: bool) -> Element<'a, Message> {
    let mut strip = row![].spacing(2).align_y(Alignment::Center);
    if r.is_markdown {
        strip = strip.push(action_button(
            "M",
            strings::PASTE_WITHOUT_MARKDOWN.into(),
            Some(Message::Act(idx, RowAction::PasteNoMarkdown)),
        ));
    }
    let (tip, on_press) = match r.plain_disabled_reason() {
        None => (
            strings::PASTE_AS_PLAIN_TEXT.to_string(),
            Some(Message::Act(idx, RowAction::PastePlain)),
        ),
        Some(reason) => (reason.to_string(), None),
    };
    strip = strip.push(action_button("T", tip, on_press));
    if show_delete {
        strip = strip.push(icon_action(
            "window-close-symbolic",
            strings::DELETE,
            Message::Act(idx, RowAction::Delete),
        ));
    } else {
        strip = strip.push(widget::Space::new().width(ACTION_SIZE));
    }
    strip.into()
}

// ---- macro column ----------------------------------------------------------------------------

pub fn macro_column<'a>(macros: &'a [Macro]) -> Element<'a, Message> {
    let mut col = column![text::heading(strings::MACRO_HEADING)]
        .spacing(8)
        .width(MACRO_COLUMN_WIDTH);
    let now = chrono::Local::now();
    for (i, m) in macros.iter().enumerate() {
        let value = crate::macros::render(m, &now).unwrap_or_else(|e| format!("({e})"));
        let key = strings::macro_key(i + 1);
        let label: Element<'a, Message> = match &m.label {
            Some(l) => column![text(l.clone()), muted(value).size(12)].into(),
            None => text(value).into(),
        };
        let inner = row![
            container(label).width(Length::Fill),
            text::caption(key).class(theme::Text::Custom(muted_text)),
        ]
        .spacing(4)
        .align_y(Alignment::End);
        let b = button::custom(inner)
            .class(theme::Button::Standard)
            .width(Length::Fill)
            .padding([8, 10])
            .on_press(Message::MacroPressed(i));
        col = col.push(tooltip(
            b,
            text::caption(strings::macro_tooltip(&m.format)),
            tooltip::Position::Left,
        ));
    }
    container(col).height(Length::Fill).into()
}
