use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub max_items: usize,
    /// Delete entries not used for this many days. 0 keeps everything up to `max_items`.
    pub expire_days: u64,
    pub ocr: OcrConfig,
    pub paste: PasteConfig,
    pub plain: PlainConfig,
    pub macros: MacrosConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_items: 1000,
            expire_days: 7,
            ocr: OcrConfig::default(),
            paste: PasteConfig::default(),
            plain: PlainConfig::default(),
            macros: MacrosConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    #[default]
    Auto,
    Ocrs,
    Tesseract,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct OcrConfig {
    pub engine: OcrEngineKind,
    pub tesseract_lang: String,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            engine: OcrEngineKind::Auto,
            tesseract_lang: "eng".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PasteKeys {
    /// Pastes in most GTK, Qt, browser and terminal apps.
    #[default]
    ShiftInsert,
    CtrlV,
    CtrlShiftV,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PasteMethod {
    /// Wayland virtual keyboard, falling back to uinput.
    Auto,
    /// Wayland virtual keyboard only.
    #[default]
    Wayland,
    Uinput,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PasteConfig {
    pub method: PasteMethod,
    /// Paste straight away after `clippo plain`, unless `--no-paste` is given.
    pub auto_paste: bool,
    /// Paste straight away after picking an entry in the menu.
    pub paste_on_select: bool,
    pub keys: PasteKeys,
    /// Wait before pressing the paste keys, so the shortcut's keys can be let go.
    pub delay_ms: u64,
    /// Tell the compositor Super and Alt are up before pasting.
    pub release_modifiers: bool,
}

impl Default for PasteConfig {
    fn default() -> Self {
        Self {
            method: PasteMethod::Wayland,
            auto_paste: true,
            paste_on_select: true,
            keys: PasteKeys::ShiftInsert,
            delay_ms: 100,
            release_modifiers: true,
        }
    }
}

/// `clippo plain` (Super+Alt+V).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PlainConfig {
    /// Also remove Markdown syntax, when the text looks like Markdown.
    pub strip_markdown: bool,
}

impl Default for PlainConfig {
    fn default() -> Self {
        Self {
            strip_markdown: true,
        }
    }
}

/// A `clippo macro` entry: text built from the current time with a strftime format.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Macro {
    pub format: String,
    /// Shown instead of the live value in the menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MacrosConfig {
    /// Put the previous clipboard back after pasting a macro.
    pub restore_clipboard: bool,
    /// `[[macros.items]]`; replaces the defaults entirely when set.
    pub items: Vec<Macro>,
}

impl Default for MacrosConfig {
    fn default() -> Self {
        let m = |format: &str| Macro {
            format: format.into(),
            label: None,
        };
        Self {
            restore_clipboard: true,
            items: vec![m("%H:%M"), m("%Y-%m-%d"), m("%Y-%m-%d %H:%M:%S")],
        }
    }
}

/// Filesystem locations, resolved from XDG variables.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub db: PathBuf,
    pub thumbs_dir: PathBuf,
    pub ocrs_models_dir: PathBuf,
    /// Where the ocrs CLI keeps its models; reused if present.
    pub ocrs_cli_cache_dir: Option<PathBuf>,
    /// Per-session files: the service socket and the ingest skip list.
    pub runtime_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "clippo").context("could not find home directory")?;
        let data_dir = dirs.data_dir().to_path_buf();
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join("clippo");
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            db: data_dir.join("history.db"),
            thumbs_dir: dirs.cache_dir().join("thumbs"),
            ocrs_models_dir: data_dir.join("ocrs"),
            ocrs_cli_cache_dir: BaseDirs::new().map(|b| b.cache_dir().join("ocrs")),
            runtime_dir,
        })
    }
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        match std::fs::read_to_string(&paths.config_file) {
            Ok(s) => toml::from_str(&s)
                .with_context(|| format!("invalid config file {}", paths.config_file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => {
                Err(e).with_context(|| format!("could not read {}", paths.config_file.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_uses_defaults() {
        let c: Config = toml::from_str("[ocr]\nengine = \"tesseract\"\n").unwrap();
        assert_eq!(c.max_items, 1000);
        assert_eq!(c.expire_days, 7);
        assert_eq!(c.ocr.engine, OcrEngineKind::Tesseract);
        assert_eq!(c.ocr.tesseract_lang, "eng");
        assert!(c.paste.auto_paste);
        assert_eq!(c.paste.keys, PasteKeys::ShiftInsert);
        assert_eq!(c.paste.delay_ms, 100);
        assert!(c.plain.strip_markdown);
        assert!(c.macros.restore_clipboard);
        assert_eq!(c.macros.items.len(), 3);
        assert_eq!(c.macros.items[1].format, "%Y-%m-%d");
    }

    #[test]
    fn macros_replace_defaults() {
        let c: Config = toml::from_str(
            "expire_days = 0\n[plain]\nstrip_markdown = false\n[macros]\nrestore_clipboard = false\n\
             [[macros.items]]\nformat = \"%d/%m\"\nlabel = \"Today\"\n",
        )
        .unwrap();
        assert_eq!(c.expire_days, 0);
        assert!(!c.plain.strip_markdown);
        assert!(!c.macros.restore_clipboard);
        assert_eq!(
            c.macros.items,
            vec![Macro {
                format: "%d/%m".into(),
                label: Some("Today".into())
            }]
        );
    }

    #[test]
    fn config_round_trips_through_toml() {
        let text = toml::to_string(&Config::default()).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.macros.items, Config::default().macros.items);
        assert_eq!(back.expire_days, 7);
    }
}
