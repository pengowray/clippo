//! Settings page, shown in place of the list (design 10).
//!
//! Writes go through `toml_edit` so comments and unknown keys in the user's file survive.
//! Reads come from a `Config` re-parsed from the document after every write, so the page
//! always shows what the file now says. The app re-reads its own `Config` on close.

use std::path::PathBuf;

use cosmic::app::Task;
use cosmic::iced::{self, Alignment, Length};
use cosmic::widget::{self, button, column, container, row, settings, text, toggler};
use cosmic::{Element, theme};
use toml_edit::{DocumentMut, Item, value};

use crate::config::{Config, Macro, OcrEngineKind, PasteKeys, PasteMethod, Paths};
use crate::ocr;
use crate::store::Store;
use crate::thumbs;
use crate::ui::app::{Footer, Message as AppMessage};
use crate::ui::list::{muted_text, warning_text};
use crate::ui::strings;
use crate::{macros, service};

/// Alt+1 to Alt+9.
const MAX_MACROS: usize = 9;
const DEFAULT_EXPIRE_DAYS: u64 = 7;
const STRFTIME_REFERENCE: &str = "https://docs.rs/chrono/latest/chrono/format/strftime/index.html";

#[derive(Debug, Clone)]
pub enum Message {
    Back,
    ResetAsk,
    ResetConfirm,
    ResetCancel,
    MaxItems(String),
    ExpireOn(bool),
    ExpireDays(String),
    DeleteAllAsk,
    DeleteAllConfirm,
    DeleteAllCancel,
    PasteOnSelect(bool),
    AutoPaste(bool),
    PlainStripsMarkdown(bool),
    PasteKeys(usize),
    RestoreClipboard(bool),
    ToggleAdvanced,
    Method(usize),
    DelayMs(String),
    ReleaseModifiers(bool),
    OcrEngine(usize),
    SetupOcr,
    SetupOcrDone(Result<(), String>),
    /// Turn a tesseract language on or off.
    Lang(String, bool),
    MacroFormat(usize, String),
    MacroLabel(usize, String),
    MacroUp(usize),
    MacroDown(usize),
    MacroRemove(usize),
    MacroAdd,
    FormatCodes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirm {
    None,
    Reset,
    DeleteAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OcrSetup {
    Idle,
    Downloading,
    Installed,
    Failed(String),
}

pub struct State {
    config_file: PathBuf,
    paths: Paths,
    doc: DocumentMut,
    /// What the document currently parses to.
    cfg: Config,
    /// Something was written since the page opened; the app re-reads `Config` on close.
    pub changed: bool,
    item_count: usize,
    confirm: Confirm,
    advanced_open: bool,
    ocr_setup: OcrSetup,
    // Text field buffers: the file is only written when they parse.
    max_items: String,
    expire_days: String,
    delay_ms: String,
    /// Languages tesseract has data for; `None` when it is not installed. Read once per open.
    tesseract_langs: Option<Vec<String>>,
    macro_formats: Vec<String>,
    macro_labels: Vec<String>,
}

const PASTE_KEYS_OPTIONS: [&str; 3] = [
    "Shift+Insert (works in most apps and terminals)",
    "Ctrl+V",
    "Ctrl+Shift+V",
];
const PASTE_KEYS: [(PasteKeys, &str); 3] = [
    (PasteKeys::ShiftInsert, "shift-insert"),
    (PasteKeys::CtrlV, "ctrl-v"),
    (PasteKeys::CtrlShiftV, "ctrl-shift-v"),
];
const METHOD_OPTIONS: [&str; 3] = [
    "Wayland virtual keyboard",
    "Virtual input device (uinput)",
    "Wayland first, then uinput",
];
const METHODS: [(PasteMethod, &str); 3] = [
    (PasteMethod::Wayland, "wayland"),
    (PasteMethod::Uinput, "uinput"),
    (PasteMethod::Auto, "auto"),
];
const ENGINE_OPTIONS: [&str; 4] = [
    "Automatic (built-in if set up, else Tesseract)",
    "Built-in (ocrs)",
    "Tesseract",
    "Off",
];
const ENGINES: [(OcrEngineKind, &str); 4] = [
    (OcrEngineKind::Auto, "auto"),
    (OcrEngineKind::Ocrs, "ocrs"),
    (OcrEngineKind::Tesseract, "tesseract"),
    (OcrEngineKind::Off, "off"),
];

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl State {
    pub fn new(cfg: &Config, paths: &Paths, item_count: usize) -> Self {
        let doc = std::fs::read_to_string(&paths.config_file)
            .ok()
            .and_then(|s| s.parse::<DocumentMut>().ok())
            .unwrap_or_default();
        let mut st = Self {
            config_file: paths.config_file.clone(),
            paths: paths.clone(),
            doc,
            cfg: cfg.clone(),
            changed: false,
            item_count,
            confirm: Confirm::None,
            advanced_open: false,
            ocr_setup: OcrSetup::Idle,
            max_items: String::new(),
            expire_days: String::new(),
            delay_ms: String::new(),
            tesseract_langs: ocr::tesseract_langs(),
            macro_formats: Vec::new(),
            macro_labels: Vec::new(),
        };
        st.sync_buffers();
        st
    }

    /// Refresh the text buffers from `cfg` (on open and after a reset).
    fn sync_buffers(&mut self) {
        let c = &self.cfg;
        self.max_items = c.max_items.to_string();
        self.expire_days = if c.expire_days > 0 {
            c.expire_days.to_string()
        } else {
            DEFAULT_EXPIRE_DAYS.to_string()
        };
        self.delay_ms = c.paste.delay_ms.to_string();
        self.macro_formats = c.macros.items.iter().map(|m| m.format.clone()).collect();
        self.macro_labels = c
            .macros
            .items
            .iter()
            .map(|m| m.label.clone().unwrap_or_default())
            .collect();
    }

    fn uinput_in_use(&self) -> bool {
        self.cfg.paste.method != PasteMethod::Wayland
    }

    /// What OCR is really doing, which the config alone does not say.
    fn ocr_status(&self) -> String {
        if self.cfg.ocr.engine == OcrEngineKind::Off {
            return strings::OCR_STATUS_OFF.into();
        }
        match ocr::available(&self.cfg.ocr, &self.paths) {
            Some("ocrs") => strings::OCR_STATUS_BUILTIN.into(),
            Some(_) => strings::ocr_status_tesseract(&self.cfg.ocr.tesseract_lang),
            None => strings::OCR_STATUS_NONE.into(),
        }
    }

    fn macros(&self) -> Vec<Macro> {
        self.macro_formats
            .iter()
            .zip(&self.macro_labels)
            .map(|(f, l)| Macro {
                format: f.clone(),
                label: (!l.trim().is_empty()).then(|| l.trim().to_string()),
            })
            .collect()
    }

    // ---- writing --------------------------------------------------------------------------

    fn set(&mut self, path: &[&str], item: Item) -> Option<Footer> {
        set_path(&mut self.doc, path, item);
        self.save()
    }

    fn save(&mut self) -> Option<Footer> {
        let text = self.doc.to_string();
        match toml::from_str::<Config>(&text) {
            Ok(cfg) => self.cfg = cfg,
            Err(e) => return Some(Footer::Error(strings::footer_save_failed(&e.to_string()))),
        }
        if let Some(dir) = self.config_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&self.config_file, text) {
            return Some(Footer::Error(strings::footer_save_failed(&e.to_string())));
        }
        self.changed = true;
        Some(if service::reload(&self.paths).is_ok() {
            Footer::Info(strings::FOOTER_SAVED.into())
        } else {
            Footer::Info(strings::FOOTER_SAVED_NO_SERVICE.into())
        })
    }

    fn save_macros(&mut self) -> Option<Footer> {
        let mut aot = toml_edit::ArrayOfTables::new();
        for m in self.macros() {
            let mut t = toml_edit::Table::new();
            t["format"] = value(m.format);
            if let Some(l) = m.label {
                t["label"] = value(l);
            }
            aot.push(t);
        }
        self.set(&["macros", "items"], Item::ArrayOfTables(aot))
    }

    pub fn update(
        &mut self,
        m: Message,
        store: Option<&Store>,
        paths: &Paths,
    ) -> (Task<AppMessage>, Option<Footer>) {
        let footer = match m {
            Message::Back => {
                return (
                    cosmic::task::message(cosmic::Action::App(AppMessage::CloseSettings)),
                    None,
                );
            }
            Message::ResetAsk => {
                self.confirm = Confirm::Reset;
                None
            }
            Message::ResetCancel | Message::DeleteAllCancel => {
                self.confirm = Confirm::None;
                None
            }
            Message::ResetConfirm => {
                self.confirm = Confirm::None;
                // Defaults are the absence of keys (design 10).
                self.doc = DocumentMut::new();
                let f = self.save();
                self.sync_buffers();
                f
            }
            Message::MaxItems(s) => {
                self.max_items = s;
                match self.max_items.trim().parse::<i64>() {
                    Ok(n) if (10..=100_000).contains(&n) => self.set(&["max_items"], value(n)),
                    _ => None,
                }
            }
            Message::ExpireOn(on) => {
                let days = if on {
                    self.expire_days
                        .trim()
                        .parse::<i64>()
                        .ok()
                        .filter(|d| *d > 0)
                        .unwrap_or(DEFAULT_EXPIRE_DAYS as i64)
                } else {
                    0
                };
                self.set(&["expire_days"], value(days))
            }
            Message::ExpireDays(s) => {
                self.expire_days = s;
                match self.expire_days.trim().parse::<i64>() {
                    Ok(n) if n > 0 && self.cfg.expire_days > 0 => {
                        self.set(&["expire_days"], value(n))
                    }
                    _ => None,
                }
            }
            Message::DeleteAllAsk => {
                self.confirm = Confirm::DeleteAll;
                None
            }
            Message::DeleteAllConfirm => {
                self.confirm = Confirm::None;
                match store.map(Store::clear) {
                    Some(Ok(_)) => {
                        thumbs::remove_all(&paths.thumbs_dir);
                        self.item_count = 0;
                        self.changed = true;
                        None
                    }
                    Some(Err(e)) => Some(Footer::Error(format!("{e:#}"))),
                    None => None,
                }
            }
            Message::PasteOnSelect(v) => self.set(&["paste", "paste_on_select"], value(v)),
            Message::AutoPaste(v) => self.set(&["paste", "auto_paste"], value(v)),
            Message::PlainStripsMarkdown(v) => self.set(&["plain", "strip_markdown"], value(v)),
            Message::PasteKeys(i) => self.set(&["paste", "keys"], value(PASTE_KEYS[i].1)),
            Message::RestoreClipboard(v) => self.set(&["macros", "restore_clipboard"], value(v)),
            Message::ToggleAdvanced => {
                self.advanced_open = !self.advanced_open;
                None
            }
            Message::Method(i) => self.set(&["paste", "method"], value(METHODS[i].1)),
            Message::DelayMs(s) => {
                self.delay_ms = s;
                match self.delay_ms.trim().parse::<i64>() {
                    Ok(n) if (0..=10_000).contains(&n) => {
                        self.set(&["paste", "delay_ms"], value(n))
                    }
                    _ => None,
                }
            }
            Message::ReleaseModifiers(v) => self.set(&["paste", "release_modifiers"], value(v)),
            Message::OcrEngine(i) => self.set(&["ocr", "engine"], value(ENGINES[i].1)),
            Message::SetupOcr => {
                self.ocr_setup = OcrSetup::Downloading;
                let task = iced::Task::future(async {
                    tokio::task::spawn_blocking(run_setup_ocr)
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                })
                .map(|r| cosmic::Action::App(AppMessage::Settings(Message::SetupOcrDone(r))));
                return (task, None);
            }
            Message::SetupOcrDone(r) => {
                self.ocr_setup = match r {
                    Ok(()) => OcrSetup::Installed,
                    Err(e) => OcrSetup::Failed(e),
                };
                None
            }
            Message::Lang(code, on) => {
                let mut langs = self.selected_langs();
                if on && !langs.contains(&code) {
                    langs.push(code);
                } else if !on {
                    langs.retain(|l| *l != code);
                }
                if langs.is_empty() {
                    None
                } else {
                    self.set(&["ocr", "tesseract_lang"], value(langs.join("+")))
                }
            }
            Message::MacroFormat(i, s) => {
                if let Some(f) = self.macro_formats.get_mut(i) {
                    *f = s;
                }
                self.save_macros()
            }
            Message::MacroLabel(i, s) => {
                if let Some(l) = self.macro_labels.get_mut(i) {
                    *l = s;
                }
                self.save_macros()
            }
            Message::MacroUp(i) => {
                if i > 0 && i < self.macro_formats.len() {
                    self.macro_formats.swap(i, i - 1);
                    self.macro_labels.swap(i, i - 1);
                    self.save_macros()
                } else {
                    None
                }
            }
            Message::MacroDown(i) => {
                if i + 1 < self.macro_formats.len() {
                    self.macro_formats.swap(i, i + 1);
                    self.macro_labels.swap(i, i + 1);
                    self.save_macros()
                } else {
                    None
                }
            }
            Message::MacroRemove(i) => {
                if i < self.macro_formats.len() {
                    self.macro_formats.remove(i);
                    self.macro_labels.remove(i);
                    self.save_macros()
                } else {
                    None
                }
            }
            Message::MacroAdd => {
                if self.macro_formats.len() < MAX_MACROS {
                    self.macro_formats.push("%Y-%m-%d".into());
                    self.macro_labels.push(String::new());
                    self.save_macros()
                } else {
                    None
                }
            }
            Message::FormatCodes => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(STRFTIME_REFERENCE)
                    .spawn();
                None
            }
        };
        (Task::none(), footer)
    }

    // ---- view -------------------------------------------------------------------------------

    pub fn view(&self) -> Element<'_, Message> {
        let header_end: Element<'_, Message> = match self.confirm {
            Confirm::Reset => row![
                text(strings::RESET_CONFIRM),
                button::destructive(strings::RESET).on_press(Message::ResetConfirm),
                button::standard(strings::CANCEL).on_press(Message::ResetCancel),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
            _ => button::standard(strings::RESET_TO_DEFAULTS)
                .on_press(Message::ResetAsk)
                .into(),
        };
        let header = row![
            widget::tooltip(
                button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::Back),
                text(strings::SETTINGS_BACK),
                widget::tooltip::Position::Bottom,
            ),
            text::title4(strings::SETTINGS_TITLE),
            widget::Space::new().width(Length::Fill),
            header_end,
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let sections = settings::view_column(vec![
            self.history_section(),
            self.paste_section(),
            self.ocr_section(),
            self.macros_section(),
        ]);

        column![
            header,
            widget::scrollable(sections).height(Length::Fill),
        ]
        .spacing(8)
        .height(Length::Fill)
        .into()
    }

    fn history_section(&self) -> Element<'_, Message> {
        let expire_on = self.cfg.expire_days > 0;
        let delete_all: Element<'_, Message> = if self.confirm == Confirm::DeleteAll {
            row![
                text(strings::delete_all_confirm(&thousands(self.item_count))),
                button::destructive(strings::DELETE_ALL).on_press(Message::DeleteAllConfirm),
                button::standard(strings::CANCEL).on_press(Message::DeleteAllCancel),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            button::destructive(strings::DELETE_ALL_HISTORY)
                .on_press(Message::DeleteAllAsk)
                .into()
        };
        settings::section()
            .title(strings::SECTION_HISTORY)
            .add(settings::item(
                strings::KEEP_UP_TO,
                number_field(&self.max_items, strings::SUFFIX_ITEMS, Message::MaxItems, true),
            ))
            .add(
                settings::item::builder(strings::DELETE_NOT_USED_FOR)
                    .description(strings::EXPIRE_HELP)
                    .control(
                        row![
                            toggler(expire_on).on_toggle(Message::ExpireOn),
                            number_field(
                                &self.expire_days,
                                strings::SUFFIX_DAYS,
                                Message::ExpireDays,
                                expire_on,
                            ),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    ),
            )
            .add(container(delete_all).width(Length::Fill))
            .into()
    }

    fn paste_section(&self) -> Element<'_, Message> {
        let uinput = self.uinput_in_use();
        let p = &self.cfg.paste;
        let mut advanced = column![
            button::text(strings::ADVANCED)
                .leading_icon(widget::icon::from_name(if self.advanced_open {
                    "go-down-symbolic"
                } else {
                    "go-next-symbolic"
                }))
                .on_press(Message::ToggleAdvanced)
        ]
        .spacing(8);
        if self.advanced_open {
            let method = METHODS.iter().position(|(m, _)| *m == p.method).unwrap_or(0);
            advanced = advanced
                .push(
                    settings::item::builder(strings::HOW_KEYS_ARE_SENT)
                        .description(strings::UINPUT_HELP)
                        .control(widget::dropdown(
                            &METHOD_OPTIONS[..],
                            Some(method),
                            Message::Method,
                        )),
                )
                .push(settings::item(
                    strings::WAIT_BEFORE_PASTING,
                    number_field(&self.delay_ms, strings::SUFFIX_MS, Message::DelayMs, uinput),
                ))
                .push(settings::item(
                    strings::RELEASE_MODIFIERS,
                    toggler(p.release_modifiers)
                        .on_toggle_maybe(uinput.then_some(Message::ReleaseModifiers)),
                ));
        }
        let keys = PASTE_KEYS.iter().position(|(k, _)| *k == p.keys).unwrap_or(0);
        settings::section()
            .title(strings::SECTION_PASTE)
            .add(item_with_help(
                strings::PASTE_AFTER_PICKING,
                // The consequence of "off" is spelled out only when it applies.
                (!p.paste_on_select).then_some(strings::PASTE_AFTER_PICKING_OFF),
                toggler(p.paste_on_select).on_toggle(Message::PasteOnSelect),
            ))
            .add(item_with_help(
                strings::PASTE_AFTER_PLAIN,
                (!p.auto_paste).then_some(strings::PASTE_AFTER_PLAIN_OFF),
                toggler(p.auto_paste).on_toggle(Message::AutoPaste),
            ))
            .add(
                settings::item::builder(strings::PLAIN_STRIPS_MARKDOWN)
                    .description(strings::PLAIN_STRIPS_MARKDOWN_HELP)
                    .control(
                        toggler(self.cfg.plain.strip_markdown)
                            .on_toggle(Message::PlainStripsMarkdown),
                    ),
            )
            .add(settings::item(
                strings::PASTE_BY_PRESSING,
                widget::dropdown(&PASTE_KEYS_OPTIONS[..], Some(keys), Message::PasteKeys),
            ))
            .add(settings::item(
                strings::RESTORE_CLIPBOARD,
                toggler(self.cfg.macros.restore_clipboard).on_toggle(Message::RestoreClipboard),
            ))
            .add(advanced)
            .into()
    }

    fn ocr_section(&self) -> Element<'_, Message> {
        let models_missing = ocr::models_dir(&self.paths).is_none();
        let engine = ENGINES
            .iter()
            .position(|(e, _)| *e == self.cfg.ocr.engine)
            .unwrap_or(0);
        let mut section = settings::section()
            .title(strings::SECTION_OCR)
            .add(settings::item(
                strings::READ_TEXT_IN_IMAGES,
                widget::dropdown(&ENGINE_OPTIONS[..], Some(engine), Message::OcrEngine),
            ))
            .add(text::caption(self.ocr_status()).class(theme::Text::Custom(muted_text)));
        if models_missing || self.ocr_setup != OcrSetup::Idle {
            let setup: Element<'_, Message> = match &self.ocr_setup {
                OcrSetup::Idle => button::standard(strings::SETUP_BUILTIN)
                    .on_press(Message::SetupOcr)
                    .into(),
                OcrSetup::Downloading => button::standard(strings::DOWNLOADING).into(),
                OcrSetup::Installed => button::standard(strings::INSTALLED).into(),
                OcrSetup::Failed(e) => column![
                    button::standard(strings::SETUP_BUILTIN).on_press(Message::SetupOcr),
                    text::caption(e.clone()).class(theme::Text::Custom(warning_text)),
                ]
                .spacing(4)
                .into(),
            };
            section = section.add(container(setup).width(Length::Fill));
        }
        section = section.add(text::body(strings::TESSERACT_LANGUAGES));
        let Some(installed) = &self.tesseract_langs else {
            return section
                .add(text::caption(strings::TESSERACT_MISSING).class(theme::Text::Custom(muted_text)))
                .into();
        };
        let selected = self.selected_langs();
        // Installed languages, then any the config names that are not installed (greyed).
        let mut codes: Vec<(String, bool)> = installed.iter().map(|l| (l.clone(), true)).collect();
        for l in &selected {
            if !installed.contains(l) {
                codes.push((l.clone(), false));
            }
        }
        for (code, present) in codes {
            let on = selected.contains(&code);
            // The last selected language stays on: tesseract needs at least one.
            let can_toggle = present && !(on && selected.len() == 1);
            let label = if present {
                code.clone()
            } else {
                strings::lang_not_installed(&code)
            };
            let c = code.clone();
            section = section.add(settings::item(
                label,
                toggler(on).on_toggle_maybe(can_toggle.then_some(move |v| Message::Lang(c.clone(), v))),
            ));
        }
        section
            .add(text::caption(strings::TESSERACT_MORE).class(theme::Text::Custom(muted_text)))
            .into()
    }

    /// Languages named in `ocr.tesseract_lang` (`eng+deu`).
    fn selected_langs(&self) -> Vec<String> {
        self.cfg
            .ocr
            .tesseract_lang
            .split('+')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn macros_section(&self) -> Element<'_, Message> {
        let mut section = settings::section().title(strings::SECTION_MACROS);
        let n = self.macro_formats.len();
        let now = chrono::Local::now();
        for (i, (f, l)) in self.macro_formats.iter().zip(&self.macro_labels).enumerate() {
            let preview = macros::render(
                &Macro {
                    format: f.clone(),
                    label: None,
                },
                &now,
            )
            .unwrap_or_else(|e| format!("({e})"));
            let r = row![
                widget::text_input(strings::MACRO_FORMAT, f.as_str())
                    .label(strings::MACRO_FORMAT)
                    .on_input(move |s| Message::MacroFormat(i, s))
                    .width(180),
                container(text(preview).class(theme::Text::Custom(muted_text)))
                    .width(Length::Fill),
                widget::text_input(strings::MACRO_LABEL, l.as_str())
                    .label(strings::MACRO_LABEL)
                    .on_input(move |s| Message::MacroLabel(i, s))
                    .width(140),
                button::icon(widget::icon::from_name("go-up-symbolic"))
                    .on_press_maybe((i > 0).then_some(Message::MacroUp(i))),
                button::icon(widget::icon::from_name("go-down-symbolic"))
                    .on_press_maybe((i + 1 < n).then_some(Message::MacroDown(i))),
                button::icon(widget::icon::from_name("window-close-symbolic"))
                    .on_press(Message::MacroRemove(i)),
            ]
            .spacing(8)
            .align_y(Alignment::End);
            section = section.add(r);
        }
        section = section.add(
            row![
                button::standard(strings::ADD_MACRO)
                    .on_press_maybe((n < MAX_MACROS).then_some(Message::MacroAdd)),
                widget::Space::new().width(Length::Fill),
                button::link(strings::FORMAT_CODES).on_press(Message::FormatCodes),
            ]
            .align_y(Alignment::Center),
        );
        section.into()
    }
}

/// Write `item` at `path` (one or two segments), keeping `[section]` tables as real tables
/// rather than the inline `section = { key = ... }` form that plain indexing produces.
fn set_path(doc: &mut DocumentMut, path: &[&str], item: Item) {
    match path {
        [key] => {
            doc[key] = item;
        }
        [section, key] => {
            let table = match doc.remove(section) {
                Some(Item::Table(t)) => t,
                Some(Item::Value(toml_edit::Value::InlineTable(t))) => t.into_table(),
                _ => toml_edit::Table::new(),
            };
            doc.insert(section, Item::Table(table));
            doc[section][key] = item;
        }
        _ => unreachable!("settings paths have one or two segments"),
    }
}

/// A settings row whose help text is only present in some states.
fn item_with_help<'a>(
    title: &'a str,
    help: Option<&'a str>,
    control: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let b = settings::item::builder(title);
    match help {
        Some(h) => b.description(h).control(control).into(),
        None => b.control(control).into(),
    }
}

fn run_setup_ocr() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let out = std::process::Command::new(exe)
        .arg("setup-ocr")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn number_field<'a>(
    value: &'a str,
    suffix: &'a str,
    on_input: fn(String) -> Message,
    enabled: bool,
) -> Element<'a, Message> {
    let mut input = widget::text_input("", value).width(90);
    if enabled {
        input = input.on_input(on_input);
    }
    row![input, text(suffix)]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separator() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn nested_set_keeps_comments_and_parses_back() {
        let mut doc: DocumentMut =
            "# mine\nmax_items = 5\npaste = { delay_ms = 7 }\n[ocr]\nengine = \"off\" # keep\n"
                .parse()
                .unwrap();
        set_path(&mut doc, &["paste", "keys"], value("ctrl-v"));
        set_path(&mut doc, &["plain", "strip_markdown"], value(false));
        set_path(&mut doc, &["max_items"], value(9));
        let s = doc.to_string();
        assert!(s.contains("# mine"));
        assert!(s.contains("# keep"));
        assert!(s.contains("[paste]"), "{s}");
        assert!(s.contains("[plain]"), "{s}");
        assert!(s.contains("delay_ms = 7"), "{s}");
        let cfg: Config = toml::from_str(&s).unwrap();
        assert_eq!(cfg.paste.keys, PasteKeys::CtrlV);
        assert!(!cfg.plain.strip_markdown);
        assert_eq!(cfg.max_items, 9);
        assert_eq!(cfg.paste.delay_ms, 7);
    }
}
