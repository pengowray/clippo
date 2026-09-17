//! Settings page, shown in place of the list (design 10).

use cosmic::app::Task;
use cosmic::iced::Length;
use cosmic::widget::{self, button, column, container, row, text};
use cosmic::{Element, theme};

use crate::config::{Config, Paths};
use crate::store::Store;
use crate::ui::app::{Footer, Message as AppMessage};
use crate::ui::strings;

#[derive(Debug, Clone)]
pub enum Message {
    Back,
}

pub struct State {
    cfg: Config,
    changed: bool,
    item_count: usize,
}

impl State {
    pub fn new(cfg: &Config, _paths: &Paths, item_count: usize) -> Self {
        Self {
            cfg: cfg.clone(),
            changed: false,
            item_count,
        }
    }

    /// The edited config, if anything changed since the page opened.
    pub fn take_config(&mut self) -> Option<Config> {
        self.changed.then(|| self.cfg.clone())
    }

    pub fn update(
        &mut self,
        m: Message,
        _store: Option<&Store>,
        _paths: &Paths,
    ) -> (Task<AppMessage>, Option<Footer>) {
        match m {
            Message::Back => (
                cosmic::task::message(cosmic::Action::App(AppMessage::CloseSettings)),
                None,
            ),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let header = row![
            widget::tooltip(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::Back),
                text(strings::SETTINGS_BACK),
                widget::tooltip::Position::Bottom,
            ),
            text::title4(strings::SETTINGS_TITLE),
            widget::Space::new().width(Length::Fill),
        ]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center);
        // TODO: sections (deliverable 4).
        let _ = (&self.cfg, self.item_count, theme::Container::Card);
        container(column![header].spacing(12))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
