use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_items: usize,
    pub ocr: OcrConfig,
    pub paste: PasteConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_items: 1000,
            ocr: OcrConfig::default(),
            paste: PasteConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineKind {
    #[default]
    Auto,
    Ocrs,
    Tesseract,
    Off,
}

#[derive(Debug, Clone, Deserialize)]
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

/// Auto-paste settings. Parsed only; clippo core doesn't act on them yet.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PasteConfig {
    pub auto_paste: bool,
    pub keys: String,
    pub delay_ms: u64,
}

impl Default for PasteConfig {
    fn default() -> Self {
        Self {
            auto_paste: true,
            keys: "shift-insert".into(),
            delay_ms: 150,
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
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "clippo").context("could not find home directory")?;
        let data_dir = dirs.data_dir().to_path_buf();
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            db: data_dir.join("history.db"),
            thumbs_dir: dirs.cache_dir().join("thumbs"),
            ocrs_models_dir: data_dir.join("ocrs"),
            ocrs_cli_cache_dir: BaseDirs::new().map(|b| b.cache_dir().join("ocrs")),
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
        assert_eq!(c.ocr.engine, OcrEngineKind::Tesseract);
        assert_eq!(c.ocr.tesseract_lang, "eng");
        assert!(c.paste.auto_paste);
        assert_eq!(c.paste.keys, "shift-insert");
        assert_eq!(c.paste.delay_ms, 150);
    }
}
