//! Settings page, shown in place of the list (design 10).
//!
//! The page edits the config file directly through `toml_edit`, so comments and unknown
//! keys survive, and so keys the backend has not added to `Config` yet (`expire_days`,
//! `paste.plain_strips_markdown`, `[macros]`) still round-trip. The app re-reads `Config`
//! when the page closes.

use std::path::PathBuf;

use cosmic::app::Task;
use cosmic::iced::{self, Alignment, Length};
use cosmic::widget::{self, button, column, container, row, settings, text, toggler};
use cosmic::{Element, theme};
use toml_edit::{DocumentMut, Item, value};

use crate::config::{Config, Paths};
use crate::ocr;
use crate::store::Store;
use crate::thumbs;
use crate::ui::app::{Footer, Message as AppMessage};
use crate::ui::list::muted_text;
use crate::ui::macros::{self, MAX_MACROS, Macro};
use crate::ui::strings;

const DEFAULT_EXPIRE_DAYS: i64 = 7;
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
    TesseractLang(String),
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
    tesseract_lang: String,
    macro_formats: Vec<String>,
    macro_labels: Vec<String>,
}

const PASTE_KEYS_OPTIONS: [&str; 3] = [
    "Shift+Insert (works in most apps and terminals)",
    "Ctrl+V",
    "Ctrl+Shift+V",
];
const PASTE_KEYS_VALUES: [&str; 3] = ["shift-insert", "ctrl-v", "ctrl-shift-v"];
const METHOD_OPTIONS: [&str; 3] = [
    "Wayland virtual keyboard",
    "Virtual input device (uinput)",
    "Wayland first, then uinput",
];
const METHOD_VALUES: [&str; 3] = ["wayland", "uinput", "auto"];
const ENGINE_OPTIONS: [&str; 4] = [
    "Automatic (built-in if set up, else Tesseract)",
    "Built-in (ocrs)",
    "Tesseract",
    "Off",
];
const ENGINE_VALUES: [&str; 4] = ["auto", "ocrs", "tesseract", "off"];

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
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
        let macros = macros::read_from_doc(&doc).unwrap_or_else(macros::defaults);
        let expire = doc
            .get("expire_days")
            .and_then(Item::as_integer)
            .unwrap_or(DEFAULT_EXPIRE_DAYS);
        Self {
            config_file: paths.config_file.clone(),
            paths: paths.clone(),
            doc,
            changed: false,
            item_count,
            confirm: Confirm::None,
            advanced_open: false,
            ocr_setup: OcrSetup::Idle,
            max_items: cfg.max_items.to_string(),
            expire_days: if expire > 0 {
                expire.to_string()
            } else {
                DEFAULT_EXPIRE_DAYS.to_string()
            },
            delay_ms: cfg.paste.delay_ms.to_string(),
            tesseract_lang: cfg.ocr.tesseract_lang.clone(),
            macro_formats: macros.iter().map(|m| m.format.clone()).collect(),
            macro_labels: macros
                .iter()
                .map(|m| m.label.clone().unwrap_or_default())
                .collect(),
        }
    }

    // ---- reading the document ------------------------------------------------------------

    fn get_bool(&self, path: &[&str], default: bool) -> bool {
        lookup(&self.doc, path)
            .and_then(Item::as_bool)
            .unwrap_or(default)
    }

    fn get_str(&self, path: &[&str]) -> Option<String> {
        lookup(&self.doc, path)
            .and_then(Item::as_str)
            .map(str::to_string)
    }

    fn expire_on(&self) -> bool {
        self.doc
            .get("expire_days")
            .and_then(Item::as_integer)
            .unwrap_or(DEFAULT_EXPIRE_DAYS)
            > 0
    }

    fn paste_keys_index(&self) -> usize {
        let v = self
            .get_str(&["paste", "keys"])
            .unwrap_or_else(|| "shift-insert".into());
        PASTE_KEYS_VALUES.iter().position(|k| *k == v).unwrap_or(0)
    }

    fn method_index(&self) -> usize {
        let v = self
            .get_str(&["paste", "method"])
            .unwrap_or_else(|| "wayland".into());
        METHOD_VALUES.iter().position(|k| *k == v).unwrap_or(0)
    }

    fn engine_index(&self) -> usize {
        let v = self
            .get_str(&["ocr", "engine"])
            .unwrap_or_else(|| "auto".into());
        ENGINE_VALUES.iter().position(|k| *k == v).unwrap_or(0)
    }

    fn uinput_in_use(&self) -> bool {
        self.method_index() != 0
    }

    /// What OCR is really doing, which the config alone does not say.
    fn ocr_status(&self) -> String {
        let models = ocr::models_dir(&self.paths).is_some();
        let tess = ocr::tesseract_available();
        let lang = &self.tesseract_lang;
        match ENGINE_VALUES[self.engine_index()] {
            "off" => strings::OCR_STATUS_OFF.into(),
            "ocrs" if models => strings::OCR_STATUS_BUILTIN.into(),
            "tesseract" if tess => strings::ocr_status_tesseract(lang),
            "auto" if models => strings::OCR_STATUS_BUILTIN.into(),
            "auto" if tess => strings::ocr_status_tesseract(lang),
            _ => strings::OCR_STATUS_NONE.into(),
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
        let mut cur = self.doc.as_item_mut();
        for key in &path[..path.len() - 1] {
            cur = &mut cur[key];
        }
        cur[path[path.len() - 1]] = item;
        for key in &path[..path.len() - 1] {
            if let Some(t) = self.doc.get_mut(key).and_then(Item::as_table_mut) {
                t.set_implicit(false);
            }
        }
        self.save()
    }

    fn save(&mut self) -> Option<Footer> {
        if let Some(dir) = self.config_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&self.config_file, self.doc.to_string()) {
            return Some(Footer::Error(strings::footer_save_failed(&e.to_string())));
        }
        self.changed = true;
        Some(if crate::ui::service::reload() {
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
                self.doc = DocumentMut::new();
                let f = self.save();
                let cfg = Config::default();
                self.max_items = cfg.max_items.to_string();
                self.expire_days = DEFAULT_EXPIRE_DAYS.to_string();
                self.delay_ms = cfg.paste.delay_ms.to_string();
                self.tesseract_lang = cfg.ocr.tesseract_lang.clone();
                let d = macros::defaults();
                self.macro_formats = d.iter().map(|m| m.format.clone()).collect();
                self.macro_labels = d.iter().map(|_| String::new()).collect();
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
                        .unwrap_or(DEFAULT_EXPIRE_DAYS)
                } else {
                    0
                };
                self.set(&["expire_days"], value(days))
            }
            Message::ExpireDays(s) => {
                self.expire_days = s;
                match self.expire_days.trim().parse::<i64>() {
                    Ok(n) if n > 0 && self.expire_on() => self.set(&["expire_days"], value(n)),
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
            Message::PlainStripsMarkdown(v) => {
                self.set(&["paste", "plain_strips_markdown"], value(v))
            }
            Message::PasteKeys(i) => self.set(&["paste", "keys"], value(PASTE_KEYS_VALUES[i])),
            Message::RestoreClipboard(v) => self.set(&["macros", "restore_clipboard"], value(v)),
            Message::ToggleAdvanced => {
                self.advanced_open = !self.advanced_open;
                None
            }
            Message::Method(i) => self.set(&["paste", "method"], value(METHOD_VALUES[i])),
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
            Message::OcrEngine(i) => self.set(&["ocr", "engine"], value(ENGINE_VALUES[i])),
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
            Message::TesseractLang(s) => {
                self.tesseract_lang = s.clone();
                if s.trim().is_empty() {
                    None
                } else {
                    self.set(&["ocr", "tesseract_lang"], value(s.trim()))
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
        let expire_on = self.expire_on();
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
                number_field(&self.max_items, strings::SUFFIX_ITEMS, Message::MaxItems),
            ))
            .add(
                settings::item::builder(strings::DELETE_NOT_USED_FOR)
                    .description(strings::EXPIRE_HELP)
                    .control(
                        row![
                            toggler(expire_on).on_toggle(Message::ExpireOn),
                            number_field_enabled(
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
            advanced = advanced
                .push(
                    settings::item::builder(strings::HOW_KEYS_ARE_SENT)
                        .description(strings::UINPUT_HELP)
                        .control(widget::dropdown(
                            &METHOD_OPTIONS[..],
                            Some(self.method_index()),
                            Message::Method,
                        )),
                )
                .push(settings::item(
                    strings::WAIT_BEFORE_PASTING,
                    number_field_enabled(
                        &self.delay_ms,
                        strings::SUFFIX_MS,
                        Message::DelayMs,
                        uinput,
                    ),
                ))
                .push(settings::item(
                    strings::RELEASE_MODIFIERS,
                    toggler(self.get_bool(&["paste", "release_modifiers"], true))
                        .on_toggle_maybe(uinput.then_some(Message::ReleaseModifiers)),
                ));
        }
        settings::section()
            .title(strings::SECTION_PASTE)
            .add(settings::item(
                strings::PASTE_AFTER_PICKING,
                toggler(self.get_bool(&["paste", "paste_on_select"], true))
                    .on_toggle(Message::PasteOnSelect),
            ))
            .add(settings::item(
                strings::PASTE_AFTER_PLAIN,
                toggler(self.get_bool(&["paste", "auto_paste"], true)).on_toggle(Message::AutoPaste),
            ))
            .add(
                settings::item::builder(strings::PLAIN_STRIPS_MARKDOWN)
                    .description(strings::PLAIN_STRIPS_MARKDOWN_HELP)
                    .control(
                        toggler(self.get_bool(&["paste", "plain_strips_markdown"], true))
                            .on_toggle(Message::PlainStripsMarkdown),
                    ),
            )
            .add(settings::item(
                strings::PASTE_BY_PRESSING,
                widget::dropdown(
                    &PASTE_KEYS_OPTIONS[..],
                    Some(self.paste_keys_index()),
                    Message::PasteKeys,
                ),
            ))
            .add(settings::item(
                strings::RESTORE_CLIPBOARD,
                toggler(self.get_bool(&["macros", "restore_clipboard"], true))
                    .on_toggle(Message::RestoreClipboard),
            ))
            .add(advanced)
            .into()
    }

    fn ocr_section(&self) -> Element<'_, Message> {
        let models_missing = ocr::models_dir(&self.paths).is_none();
        let mut section = settings::section()
            .title(strings::SECTION_OCR)
            .add(settings::item(
                strings::READ_TEXT_IN_IMAGES,
                widget::dropdown(
                    &ENGINE_OPTIONS[..],
                    Some(self.engine_index()),
                    Message::OcrEngine,
                ),
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
                    text::caption(e.clone()).class(theme::Text::Custom(crate::ui::list::warning_text)),
                ]
                .spacing(4)
                .into(),
            };
            section = section.add(container(setup).width(Length::Fill));
        }
        section
            .add(settings::item(
                strings::TESSERACT_LANGUAGE,
                widget::text_input("eng", &self.tesseract_lang)
                    .on_input(Message::TesseractLang)
                    .width(120),
            ))
            .into()
    }

    fn macros_section(&self) -> Element<'_, Message> {
        let mut section = settings::section().title(strings::SECTION_MACROS);
        let n = self.macro_formats.len();
        for (i, (f, l)) in self.macro_formats.iter().zip(&self.macro_labels).enumerate() {
            let preview = Macro {
                format: f.clone(),
                label: None,
            }
            .value();
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

fn lookup<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
    let mut cur: &Item = doc.as_item();
    for key in path {
        cur = cur.get(key)?;
    }
    Some(cur)
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
) -> Element<'a, Message> {
    number_field_enabled(value, suffix, on_input, true)
}

fn number_field_enabled<'a>(
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
    fn nested_set_keeps_comments() {
        let mut doc: DocumentMut = "# mine\nmax_items = 5\n[ocr]\nengine = \"off\" # keep\n"
            .parse()
            .unwrap();
        doc["paste"]["keys"] = value("ctrl-v");
        let s = doc.to_string();
        assert!(s.contains("# mine"));
        assert!(s.contains("# keep"));
        assert!(s.contains("[paste]"));
        assert!(s.contains("keys = \"ctrl-v\""));
        assert!(lookup(&doc, &["paste", "keys"]).is_some());
        assert!(lookup(&doc, &["nope", "keys"]).is_none());
    }
}
