//! The Super+V window: a layer-shell overlay hosting the history list, macro column and
//! settings page. See docs/menu-design.md.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cosmic::app::{Core, Task};
use cosmic::iced::futures::{self, Stream, StreamExt};
use cosmic::iced::event::Status;
use cosmic::iced::keyboard::key::Named;
use cosmic::iced::keyboard::{Key, Modifiers};
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{
    IcedOutput, SctkLayerSurfaceSettings,
};
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer, destroy_layer_surface,
};
use cosmic::iced::runtime::core::event::wayland::LayerEvent;
use cosmic::iced::runtime::core::event::{PlatformSpecific, wayland};
use cosmic::iced::runtime::core::layout::Limits;
use cosmic::iced::widget::scrollable::{RelativeOffset, snap_to};
use cosmic::iced::{self, Length, Subscription, window};
use cosmic::surface::action::{LiveSettings, simple_layer_shell};
use cosmic::widget::{self, column, container, row, text, text_input};
use cosmic::{Element, theme};

use crate::clipboard::{self, CopyMode};
use crate::config::{Config, OcrEngineKind, Paths};
use crate::store::{OcrStatus, Store};
use crate::thumbs;
use crate::ui::list;
use crate::ui::rows::{self, Ocr, Row};
use crate::ui::strings;
use crate::{macros, paste, service};

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 500;
/// Width of the macro column (design 2).
pub const MACRO_COLUMN_WIDTH: f32 = 170.0;

/// After unmapping the surface, before copying and pasting. `paste::send` adds its own
/// `AFTER_MENU_WAIT` on top. Tune upwards if cosmic-comp has not returned focus in time.
const CLOSE_WAIT: Duration = Duration::from_millis(60);
/// How long footer feedback stays (design 7).
const FOOTER_TTL: Duration = Duration::from_secs(6);
/// Shift+Enter on a pending image waits this long for OCR (design 5).
const OCR_WAIT: Duration = Duration::from_secs(3);
const OCR_POLL: Duration = Duration::from_millis(500);

pub static SEARCH_ID: std::sync::LazyLock<widget::Id> =
    std::sync::LazyLock::new(|| widget::Id::new("clippo-search"));
pub static SCROLL_ID: std::sync::LazyLock<widget::Id> =
    std::sync::LazyLock::new(|| widget::Id::new("clippo-list"));

pub struct Flags {
    pub cfg: Config,
    pub paths: Paths,
    /// Resident mode: stay hidden until the service asks, and hide instead of exiting.
    pub toggles: Option<crate::ui::ToggleReceiver>,
}

/// The toggle receiver, handed to the subscription once. Hashes as a constant so iced keeps
/// one stream alive for the app's lifetime.
struct ToggleSource(Arc<Mutex<Option<crate::ui::ToggleReceiver>>>);

impl std::hash::Hash for ToggleSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "clippo-menu-toggle".hash(state);
    }
}

fn toggle_stream(src: &ToggleSource) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    match src.0.lock().ok().and_then(|mut rx| rx.take()) {
        Some(rx) => Box::pin(rx.map(|()| Message::Toggle)),
        None => Box::pin(futures::stream::pending()),
    }
}

/// One line of the list: an entry, the fold row, or the search-results divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Entry(usize),
    OlderFold,
    Divider,
}

impl Item {
    pub fn selectable(self) -> bool {
        !matches!(self, Item::Divider)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Paste,
    PastePlain,
    PasteNoMarkdown,
    CopyOnly,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    List,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Footer {
    Undo,
    Info(String),
    Error(String),
}

/// Work done after the surface is unmapped, on a blocking thread.
#[derive(Debug, Clone)]
pub enum Job {
    /// Copy an entry (with the service, every stored format) and paste it if `paste`.
    Entry {
        id: i64,
        mode: CopyMode,
        paste: bool,
    },
    /// `clippo macro n`: copy, paste, restore the previous clipboard (design 9).
    Macro(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    Copy(String),
    Paste(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Key(Key, Modifiers, Status),
    Layer(LayerEvent, window::Id),
    Query(String),
    ClearSearch,
    Hover(Option<usize>),
    Activate(usize),
    Act(usize, RowAction),
    ToggleOlder,
    MacroPressed(usize),
    Undo,
    Tick,
    Reload,
    ThumbReady(i64, Option<PathBuf>),
    OcrWaited(i64, bool),
    /// `menu toggle` from the service (resident mode).
    Toggle,
    Done(Result<(), JobError>),
    OpenSettings,
    CloseSettings,
    Settings(crate::ui::settings::Message),
}

pub struct App {
    core: Core,
    surface: window::Id,
    cfg: Config,
    paths: Paths,
    store: Option<Store>,
    load_error: Option<String>,
    rows: Vec<Row>,
    query: String,
    /// Index into `items()`.
    selected: usize,
    hovered: Option<usize>,
    older_expanded: bool,
    /// The top entry is what the clipboard holds right now (design 8.1).
    top_is_clipboard: bool,
    thumbs: HashMap<i64, widget::image::Handle>,
    footer: Option<(Footer, Instant)>,
    page: Page,
    /// Entry the footer's Undo would bring back.
    deleted: Option<i64>,
    service_running: bool,
    ocr_engine_missing: bool,
    closing: bool,
    /// Resident mode: the surface comes and goes; the process stays.
    toggles: Option<ToggleSource>,
    mapped: bool,
    pub settings: crate::ui::settings::State,
}

impl App {
    // ---- list model -------------------------------------------------------------------

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn older_count(&self) -> usize {
        let cutoff = Store::recent_cutoff();
        self.rows.iter().filter(|r| !r.is_recent(cutoff)).count()
    }

    /// Rows currently laid out, in order.
    pub fn items(&self) -> Vec<Item> {
        let cutoff = Store::recent_cutoff();
        let mut out = Vec::with_capacity(self.rows.len() + 2);
        if self.searching() {
            let words = rows::query_words(&self.query);
            let (recent, older): (Vec<_>, Vec<_>) = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.matches(&words))
                .partition(|(_, r)| r.is_recent(cutoff));
            out.extend(recent.iter().map(|(i, _)| Item::Entry(*i)));
            if !older.is_empty() {
                out.push(Item::Divider);
                out.extend(older.iter().map(|(i, _)| Item::Entry(*i)));
            }
        } else {
            out.extend(
                self.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.is_recent(cutoff))
                    .map(|(i, _)| Item::Entry(i)),
            );
            let older: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.is_recent(cutoff))
                .map(|(i, _)| i)
                .collect();
            if !older.is_empty() {
                out.push(Item::OlderFold);
                if self.older_expanded {
                    out.extend(older.into_iter().map(Item::Entry));
                }
            }
        }
        out
    }

    fn match_count(&self) -> usize {
        self.items()
            .iter()
            .filter(|i| matches!(i, Item::Entry(_)))
            .count()
    }

    fn selected_item(&self) -> Option<Item> {
        self.items().get(self.selected).copied()
    }

    fn selected_row(&self) -> Option<usize> {
        match self.selected_item() {
            Some(Item::Entry(i)) => Some(i),
            _ => None,
        }
    }

    /// Move the selection by `delta` rows, skipping non-selectable rows, without wrapping.
    fn move_selection(&mut self, delta: isize) -> Task<Message> {
        let items = self.items();
        if items.is_empty() {
            self.selected = 0;
            return Task::none();
        }
        let last = items.len() as isize - 1;
        let mut target = (self.selected as isize + delta).clamp(0, last);
        let step = if delta < 0 { -1 } else { 1 };
        while !items[target as usize].selectable() {
            let next = target + step;
            if next < 0 || next > last {
                return Task::none();
            }
            target = next;
        }
        self.selected = target as usize;
        self.scroll_to_selection()
    }

    fn scroll_to_selection(&self) -> Task<Message> {
        let n = self.items().len();
        let y = if n <= 1 {
            0.0
        } else {
            self.selected as f32 / (n - 1) as f32
        };
        snap_to(SCROLL_ID.clone(), RelativeOffset { x: None, y: Some(y) })
    }

    fn clamp_selection(&mut self) {
        let items = self.items();
        if items.is_empty() {
            self.selected = 0;
        } else if self.selected >= items.len() {
            self.selected = items.len() - 1;
        }
        if let Some(item) = items.get(self.selected)
            && !item.selectable()
        {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    // ---- data -------------------------------------------------------------------------

    fn reload(&mut self) -> Task<Message> {
        let Some(store) = &self.store else {
            return Task::none();
        };
        match store.list() {
            Ok(list) => {
                self.rows = list.iter().map(Row::from_summary).collect();
                self.load_error = None;
                self.top_is_clipboard = self.check_clipboard_now();
                self.clamp_selection();
                self.request_thumbs()
            }
            Err(e) => {
                self.load_error = Some(format!("{e:#}"));
                self.footer = Some((
                    Footer::Error(strings::footer_read_failed(&format!("{e:#}"))),
                    Instant::now(),
                ));
                Task::none()
            }
        }
    }

    /// Does the live clipboard hold the top entry? One `wl-paste` spawn (design 8.1).
    fn check_clipboard_now(&self) -> bool {
        let (Some(store), Some(top)) = (&self.store, self.rows.first()) else {
            return false;
        };
        let types = clipboard::list_types();
        let types: Vec<&str> = types.iter().map(String::as_str).collect();
        let data = if top.is_image() {
            crate::ingest::preferred_image_type(&types)
                .and_then(|t| clipboard::paste(&["--no-newline", "--type", &t]))
        } else if types
            .iter()
            .any(|t| t.starts_with("text/plain") || *t == "UTF8_STRING")
        {
            clipboard::paste(&["--type", "text"])
        } else {
            None
        };
        let Some(data) = data else { return false };
        // `ingest` stored whatever `wl-paste --watch` piped, which may or may not have had a
        // trailing newline, so try both shapes.
        let mut candidates = vec![data.clone()];
        if data.last() == Some(&b'\n') {
            candidates.push(data[..data.len() - 1].to_vec());
        } else {
            let mut with = data.clone();
            with.push(b'\n');
            candidates.push(with);
        }
        candidates
            .iter()
            .any(|c| matches!(store.find(&top.mime, c), Ok(Some(s)) if s.id == top.id))
    }

    fn request_thumbs(&mut self) -> Task<Message> {
        let Some(store) = &self.store else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for r in self.rows.iter().filter(|r| r.is_image()) {
            if self.thumbs.contains_key(&r.id) {
                continue;
            }
            let path = thumbs::path(&self.paths.thumbs_dir, r.id);
            if path.is_file() {
                self.thumbs
                    .insert(r.id, widget::image::Handle::from_path(path));
                continue;
            }
            let Ok(Some(content)) = store.content(r.id) else {
                continue;
            };
            let dir = self.paths.thumbs_dir.clone();
            let id = r.id;
            tasks.push(
                iced::Task::future(async move {
                    tokio::task::spawn_blocking(move || thumbs::ensure(&dir, id, &content).ok())
                        .await
                        .ok()
                        .flatten()
                })
                .map(move |p| cosmic::Action::App(Message::ThumbReady(id, p))),
            );
        }
        Task::batch(tasks)
    }

    fn set_footer(&mut self, f: Footer) {
        self.footer = Some((f, Instant::now()));
    }

    fn error(&mut self, msg: impl Into<String>) -> Task<Message> {
        self.set_footer(Footer::Error(msg.into()));
        Task::none()
    }

    // ---- showing, closing and pasting -----------------------------------------------------

    fn resident(&self) -> bool {
        self.toggles.is_some()
    }

    /// Map the overlay with fresh state: top row selected, no search, older rows folded.
    fn show(&mut self) -> Task<Message> {
        if self.mapped {
            return Task::none();
        }
        self.mapped = true;
        self.closing = false;
        self.query.clear();
        self.selected = 0;
        self.hovered = None;
        self.older_expanded = false;
        self.footer = None;
        self.deleted = None;
        self.page = Page::List;
        if self.resident() {
            // The service may have reloaded the config since the last open.
            if let Ok(cfg) = Config::load(&self.paths) {
                self.cfg = cfg;
            }
            let (running, missing) = service_status(&self.cfg, &self.paths);
            self.service_running = running;
            self.ocr_engine_missing = missing;
        }
        let load = self.reload();
        let surface = self.surface;
        let show = cosmic::surface::surface_task(simple_layer_shell::<Message>(
            LiveSettings::default,
            move || SctkLayerSurfaceSettings {
                id: surface,
                layer: Layer::Overlay,
                keyboard_interactivity: KeyboardInteractivity::Exclusive,
                anchor: Anchor::empty(),
                output: IcedOutput::Active,
                namespace: "clippo".into(),
                size: Some((Some(WIDTH), Some(HEIGHT))),
                size_limits: Limits::NONE
                    .min_width(WIDTH as f32)
                    .max_width(WIDTH as f32)
                    .min_height(HEIGHT as f32)
                    .max_height(HEIGHT as f32),
                exclusive_zone: -1,
                ..Default::default()
            },
            None::<fn() -> Element<'static, cosmic::Action<Message>>>,
        ));
        Task::batch([show, load])
    }

    /// Unmap the overlay; in resident mode the process stays, otherwise it exits.
    fn close(&mut self) -> Task<Message> {
        self.closing = true;
        self.mapped = false;
        if self.resident() {
            destroy_layer_surface(self.surface)
        } else {
            Task::batch([destroy_layer_surface(self.surface), iced::exit()])
        }
    }

    /// Unmap the surface, then run `job` on a blocking thread, then exit (or, resident, stay).
    fn close_and(&mut self, job: Job) -> Task<Message> {
        self.closing = true;
        self.mapped = false;
        let cfg = self.cfg.clone();
        let paths = self.paths.clone();
        let run = iced::Task::future(async move {
            tokio::task::spawn_blocking(move || run_job(job, &cfg, &paths))
                .await
                .unwrap_or_else(|e| Err(JobError::Paste(e.to_string())))
        })
        .map(|r| cosmic::Action::App(Message::Done(r)));
        Task::batch([destroy_layer_surface(self.surface), run])
    }

    fn act(&mut self, idx: usize, action: RowAction) -> Task<Message> {
        let Some(row) = self.rows.get(idx) else {
            return Task::none();
        };
        let Some(store) = &self.store else {
            return Task::none();
        };
        let id = row.id;
        let paste = self.cfg.paste.paste_on_select;
        match action {
            RowAction::Paste => self.close_and(Job::Entry {
                id,
                mode: CopyMode::Full,
                paste,
            }),
            RowAction::CopyOnly => {
                if let Err(e) = clipboard::copy_entry(&self.paths, store, id, CopyMode::Full) {
                    return self.error(format!("{e:#}"));
                }
                // `ingest` bumps the entry to the top; reflect that shortly.
                iced::Task::future(async {
                    tokio::time::sleep(Duration::from_millis(300)).await
                })
                .map(|()| cosmic::Action::App(Message::Reload))
            }
            RowAction::PastePlain => {
                if row.is_image() && row.ocr() == Ocr::Pending {
                    if self.ocr_engine_missing {
                        return self.error(strings::FOOTER_NO_ENGINE);
                    }
                    self.set_footer(Footer::Info(strings::OCR_PENDING.into()));
                    let db = self.paths.db.clone();
                    return iced::Task::future(async move {
                        tokio::task::spawn_blocking(move || wait_for_ocr(&db, id))
                            .await
                            .unwrap_or(false)
                    })
                    .map(move |ok| cosmic::Action::App(Message::OcrWaited(id, ok)));
                }
                if let Some(reason) = row.plain_disabled_reason() {
                    return self.error(reason);
                }
                self.close_and(Job::Entry {
                    id,
                    mode: CopyMode::Plain,
                    paste,
                })
            }
            RowAction::PasteNoMarkdown => {
                if row.is_image() {
                    return self.error(strings::FOOTER_MARKDOWN_IMAGE);
                }
                self.close_and(Job::Entry {
                    id,
                    mode: CopyMode::NoMarkdown,
                    paste,
                })
            }
            RowAction::Delete => {
                match store.delete(id) {
                    Ok(true) => {}
                    Ok(false) => return Task::none(),
                    Err(e) => return self.error(format!("{e:#}")),
                }
                self.deleted = Some(id);
                self.set_footer(Footer::Undo);
                let t = self.reload();
                self.clamp_selection();
                t
            }
        }
    }

    fn undo(&mut self) -> Task<Message> {
        let Some(id) = self.deleted.take() else {
            return Task::none();
        };
        let Some(store) = &self.store else {
            return Task::none();
        };
        if let Err(e) = store.undelete(id) {
            return self.error(format!("{e:#}"));
        }
        self.footer = None;
        self.reload()
    }

    fn paste_macro(&mut self, n: usize) -> Task<Message> {
        if n >= self.cfg.macros.items.len() {
            return Task::none();
        }
        self.close_and(Job::Macro(n + 1))
    }

    // ---- keys ---------------------------------------------------------------------------

    fn on_key(&mut self, key: Key, mods: Modifiers, _status: Status) -> Task<Message> {
        if self.closing {
            return Task::none();
        }
        // Errors clear on the next keypress (design 7).
        if matches!(self.footer, Some((Footer::Error(_), _))) {
            self.footer = None;
        }
        let ch = match &key {
            Key::Character(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        // Super+V toggles the window even while it has exclusive focus (design 1).
        if mods.logo() && ch.as_deref() == Some("v") {
            return self.close();
        }
        if mods.control() && ch.as_deref() == Some(",") {
            return self.open_settings();
        }
        if self.page == Page::Settings {
            return match key {
                Key::Named(Named::Escape) => self.close_settings(),
                _ => Task::none(),
            };
        }
        if mods.alt()
            && let Some(c) = &ch
            && let Some(n) = c.chars().next().and_then(|d| d.to_digit(10))
            && (1..=9).contains(&n)
        {
            return self.paste_macro(n as usize - 1);
        }
        if mods.control() && ch.as_deref() == Some("z") {
            return self.undo();
        }
        match key {
            Key::Named(Named::Escape) => {
                if self.searching() {
                    self.query.clear();
                    self.selected = 0;
                    Task::none()
                } else {
                    self.close()
                }
            }
            Key::Named(Named::ArrowUp) => self.move_selection(-1),
            Key::Named(Named::ArrowDown) => self.move_selection(1),
            Key::Named(Named::PageUp) => self.move_selection(-(list::ROWS_PER_PAGE as isize)),
            Key::Named(Named::PageDown) => self.move_selection(list::ROWS_PER_PAGE as isize),
            Key::Named(Named::Home) if mods.control() => self.move_selection(isize::MIN / 2),
            Key::Named(Named::End) if mods.control() => self.move_selection(isize::MAX / 2),
            // The search field never submits, so every Enter arrives here exactly once.
            Key::Named(Named::Enter) => match self.selected_item() {
                Some(Item::OlderFold) => {
                    self.older_expanded = !self.older_expanded;
                    Task::none()
                }
                Some(Item::Entry(i)) => {
                    let action = if mods.control() {
                        RowAction::CopyOnly
                    } else if mods.shift() {
                        RowAction::PastePlain
                    } else if mods.alt() {
                        RowAction::PasteNoMarkdown
                    } else {
                        RowAction::Paste
                    };
                    self.act(i, action)
                }
                _ => Task::none(),
            },
            Key::Named(Named::Delete) if mods.shift() => match self.selected_row() {
                Some(i) => self.act(i, RowAction::Delete),
                None => Task::none(),
            },
            _ => Task::none(),
        }
    }

    fn open_settings(&mut self) -> Task<Message> {
        self.page = Page::Settings;
        self.settings = crate::ui::settings::State::new(&self.cfg, &self.paths, self.rows.len());
        Task::none()
    }

    fn close_settings(&mut self) -> Task<Message> {
        self.page = Page::List;
        // Settings may have changed the list (Delete all) or paste behaviour.
        if self.settings.changed {
            match Config::load(&self.paths) {
                Ok(cfg) => self.cfg = cfg,
                Err(e) => crate::log(&format!("window: {e:#}")),
            }
        }
        let t = self.reload();
        Task::batch([t, text_input::focus(SEARCH_ID.clone())])
    }

    // ---- view ---------------------------------------------------------------------------

    fn view_list(&self) -> Element<'_, Message> {
        let count = if self.searching() {
            strings::match_count(self.match_count(), self.rows.len())
        } else {
            strings::item_count(self.rows.len())
        };
        let search = widget::search_input(strings::SEARCH_PLACEHOLDER, &self.query)
            .id(SEARCH_ID.clone())
            .always_active()
            .on_input(Message::Query)
            .on_clear(Message::ClearSearch)
            .width(Length::Fill);
        let gear = widget::tooltip(
            widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
                .on_press(Message::OpenSettings),
            text(strings::SETTINGS_TOOLTIP),
            widget::tooltip::Position::Bottom,
        );
        let header = row![search, text::caption(count), gear]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .height(40);

        let banner = if !self.service_running {
            Some(strings::BANNER_NOT_RECORDING)
        } else if self.ocr_engine_missing {
            Some(strings::BANNER_NO_ENGINE)
        } else {
            None
        };

        let body = row![
            list::view(
                self,
                &self.rows,
                &self.items(),
                self.selected,
                self.hovered,
                &self.thumbs,
                banner,
            ),
            list::macro_column(&self.cfg.macros.items),
        ]
        .spacing(8)
        .height(Length::Fill);

        column![header, body, self.view_footer()]
            .spacing(4)
            .into()
    }

    fn view_footer(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.footer {
            Some((Footer::Undo, _)) => row![
                text::caption(strings::FOOTER_DELETED),
                widget::button::link(strings::FOOTER_UNDO)
                    .on_press(Message::Undo)
                    .padding(0)
                    .font_size(12),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .into(),
            Some((Footer::Info(s), _)) => text::caption(s.clone()).into(),
            Some((Footer::Error(s), _)) => text::caption(s.clone())
                .class(theme::Text::Custom(list::warning_text))
                .into(),
            None => {
                let hints = if self.cfg.paste.paste_on_select {
                    strings::FOOTER_HINTS
                } else {
                    strings::FOOTER_HINTS_COPY
                };
                text::caption(hints)
                    .class(theme::Text::Custom(list::muted_text))
                    .into()
            }
        };
        container(content)
            .height(24)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center)
            .padding([0, 4])
            .into()
    }

    pub fn is_top_clipboard(&self) -> bool {
        self.top_is_clipboard && !self.searching()
    }

    pub fn older_expanded(&self) -> bool {
        self.older_expanded
    }

    pub fn older_total(&self) -> usize {
        self.older_count()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn ocr_engine_missing(&self) -> bool {
        self.ocr_engine_missing
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }
}

/// The blocking half of close-and-paste: wait for the compositor to give focus back,
/// then copy and paste. Opens its own `Store`; the app's stays on the UI thread.
fn run_job(job: Job, cfg: &Config, paths: &Paths) -> Result<(), JobError> {
    std::thread::sleep(CLOSE_WAIT);
    match job {
        Job::Entry { id, mode, paste } => {
            let store = Store::open(&paths.db).map_err(|e| JobError::Copy(format!("{e:#}")))?;
            clipboard::copy_entry(paths, &store, id, mode)
                .map_err(|e| JobError::Copy(format!("{e:#}")))?;
            if paste {
                paste::send(paths, &cfg.paste, true)
                    .map_err(|e| JobError::Paste(format!("{e:#}")))?;
            }
            Ok(())
        }
        // `macros::run` pastes without the after-menu wait, so give it the same head start.
        Job::Macro(n) => {
            std::thread::sleep(paste::AFTER_MENU_WAIT);
            macros::run(cfg, paths, n).map_err(|e| JobError::Paste(format!("{e:#}")))
        }
    }
}

/// Poll the store until the entry's OCR has finished, up to `OCR_WAIT`. True if text was found.
fn wait_for_ocr(db: &std::path::Path, id: i64) -> bool {
    let deadline = Instant::now() + OCR_WAIT;
    let Ok(store) = Store::open(db) else {
        return false;
    };
    loop {
        if let Ok(Some(s)) = store.summary(id)
            && s.ocr_status != OcrStatus::Pending
        {
            return s.ocr_status == OcrStatus::Done
                && s.ocr_text.is_some_and(|t| !t.trim().is_empty());
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(OCR_POLL);
    }
}

/// (service running, OCR on but no engine) for the banners (design 7.3).
fn service_status(cfg: &Config, paths: &Paths) -> (bool, bool) {
    match service::status(paths) {
        Ok(s) => (true, s.ocr.is_none() && cfg.ocr.engine != OcrEngineKind::Off),
        Err(_) => (false, false),
    }
}

fn map_event(e: iced::Event, status: Status, _id: window::Id) -> Option<Message> {
    match e {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Message::Key(key, modifiers, status))
        }
        iced::Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Layer(
            e,
            _,
            id,
        ))) => Some(Message::Layer(e, id)),
        _ => None,
    }
}

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;

    const APP_ID: &'static str = "io.github.pengowray.clippo";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, flags: Flags) -> (Self, Task<Message>) {
        core.set_keyboard_nav(false);
        let surface = window::Id::unique();
        let (store, load_error) = match Store::open(&flags.paths.db) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        let resident = flags.toggles.is_some();
        let (service_running, ocr_engine_missing) = if resident {
            (true, false)
        } else {
            service_status(&flags.cfg, &flags.paths)
        };
        let settings = crate::ui::settings::State::new(&flags.cfg, &flags.paths, 0);
        let mut app = App {
            core,
            surface,
            cfg: flags.cfg,
            paths: flags.paths,
            store,
            load_error: load_error.clone(),
            rows: Vec::new(),
            query: String::new(),
            selected: 0,
            hovered: None,
            older_expanded: false,
            top_is_clipboard: false,
            thumbs: HashMap::new(),
            footer: load_error.map(|e| {
                (
                    Footer::Error(strings::footer_read_failed(&e)),
                    Instant::now(),
                )
            }),
            page: Page::List,
            deleted: None,
            service_running,
            ocr_engine_missing,
            closing: false,
            toggles: flags
                .toggles
                .map(|rx| ToggleSource(Arc::new(Mutex::new(Some(rx))))),
            mapped: false,
            settings,
        };
        // Resident: stay hidden until the first `menu toggle`.
        let task = if resident { Task::none() } else { app.show() };
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Key(key, mods, status) => self.on_key(key, mods, status),
            Message::Layer(LayerEvent::Focused, id) if id == self.surface => Task::batch([
                text_input::focus(SEARCH_ID.clone()),
                // A reused scrollable can keep an old offset; every open starts at the top.
                snap_to(
                    SCROLL_ID.clone(),
                    RelativeOffset {
                        x: None,
                        y: Some(0.0),
                    },
                ),
            ]),
            Message::Layer(_, _) => Task::none(),
            Message::Query(q) => {
                self.query = q;
                self.selected = 0;
                self.scroll_to_selection()
            }
            Message::ClearSearch => {
                self.query.clear();
                self.selected = 0;
                Task::none()
            }
            Message::Hover(h) => {
                self.hovered = h;
                Task::none()
            }
            Message::Activate(i) => {
                self.selected = i;
                match self.selected_item() {
                    Some(Item::Entry(r)) => self.act(r, RowAction::Paste),
                    Some(Item::OlderFold) => {
                        self.older_expanded = !self.older_expanded;
                        Task::none()
                    }
                    _ => Task::none(),
                }
            }
            Message::Act(i, action) => self.act(i, action),
            Message::ToggleOlder => {
                self.older_expanded = !self.older_expanded;
                Task::none()
            }
            Message::MacroPressed(n) => self.paste_macro(n),
            Message::Undo => self.undo(),
            Message::Tick => {
                if let Some((f, at)) = &self.footer
                    && !matches!(f, Footer::Error(_))
                    && at.elapsed() > FOOTER_TTL
                {
                    self.footer = None;
                    self.deleted = None;
                }
                Task::none()
            }
            Message::Reload => self.reload(),
            Message::ThumbReady(id, Some(path)) => {
                self.thumbs
                    .insert(id, widget::image::Handle::from_path(path));
                Task::none()
            }
            Message::ThumbReady(_, None) => Task::none(),
            Message::OcrWaited(id, ok) => {
                if self.closing {
                    return Task::none();
                }
                if ok {
                    let paste = self.cfg.paste.paste_on_select;
                    return self.close_and(Job::Entry {
                        id,
                        mode: CopyMode::Plain,
                        paste,
                    });
                }
                let t = self.reload();
                let reason = self
                    .rows
                    .iter()
                    .find(|r| r.id == id)
                    .and_then(|r| r.plain_disabled_reason())
                    .unwrap_or(strings::T_OCR_FAILED);
                self.set_footer(Footer::Error(reason.into()));
                t
            }
            Message::Toggle => {
                if self.mapped {
                    self.close()
                } else {
                    self.show()
                }
            }
            Message::Done(result) => {
                if let Err(e) = &result {
                    crate::log(&format!("window: {e:?}"));
                }
                let key = self.cfg.paste.keys;
                let notify = crate::ui::notify::after_close(result, key);
                if self.resident() {
                    notify
                } else {
                    Task::batch([notify, iced::exit()])
                }
            }
            Message::OpenSettings => self.open_settings(),
            Message::CloseSettings => self.close_settings(),
            Message::Settings(m) => {
                let store = self.store.as_ref();
                let (task, footer) = self.settings.update(m, store, &self.paths);
                if let Some(f) = footer {
                    self.set_footer(f);
                }
                task
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![iced::event::listen_raw(map_event)];
        if self.mapped {
            // Macro values and footer expiry only matter while the window is visible.
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick));
        }
        if let Some(src) = &self.toggles {
            subs.push(Subscription::run_with(
                ToggleSource(Arc::clone(&src.0)),
                toggle_stream,
            ));
        }
        Subscription::batch(subs)
    }

    fn view(&self) -> Element<'_, Message> {
        // No main window: everything is drawn through `view_window`.
        widget::space::horizontal().into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if id != self.surface {
            return widget::space::horizontal().into();
        }
        let page: Element<'_, Message> = match self.page {
            Page::List => self.view_list(),
            Page::Settings => column![
                self.settings.view().map(Message::Settings),
                self.view_footer()
            ]
            .into(),
        };
        container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(8)
            .class(theme::Container::custom(list::window_style))
            .into()
    }
}
