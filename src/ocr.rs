use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{OcrConfig, OcrEngineKind, Paths};

pub const MODEL_BASE_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/";
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";

pub trait OcrBackend {
    fn name(&self) -> &'static str;
    /// Recognise text in an encoded image (PNG, JPEG, ...).
    fn recognize(&self, image: &[u8]) -> Result<String>;
}

/// Build the configured backend. `Ok(None)` means OCR is off, or `auto` found nothing usable.
pub fn backend(cfg: &OcrConfig, paths: &Paths) -> Result<Option<Box<dyn OcrBackend>>> {
    match cfg.engine {
        OcrEngineKind::Off => Ok(None),
        OcrEngineKind::Ocrs => {
            let dir = models_dir(paths)
                .ok_or_else(|| anyhow!("OCR models not found. Run `clippo setup-ocr` first."))?;
            Ok(Some(Box::new(Ocrs::load(&dir)?)))
        }
        OcrEngineKind::Tesseract => {
            if !tesseract_available() {
                bail!("tesseract not found. Install the tesseract-ocr package.");
            }
            Ok(Some(Box::new(Tesseract {
                lang: cfg.tesseract_lang.clone(),
            })))
        }
        OcrEngineKind::Auto => {
            if let Some(dir) = models_dir(paths) {
                Ok(Some(Box::new(Ocrs::load(&dir)?)))
            } else if tesseract_available() {
                Ok(Some(Box::new(Tesseract {
                    lang: cfg.tesseract_lang.clone(),
                })))
            } else {
                Ok(None)
            }
        }
    }
}

/// Which engine `backend` would build, found without loading anything (models on disk,
/// tesseract on PATH). `None` when OCR is off or nothing usable is installed.
pub fn available(cfg: &OcrConfig, paths: &Paths) -> Option<&'static str> {
    let ocrs = || models_dir(paths).is_some();
    match cfg.engine {
        OcrEngineKind::Off => None,
        OcrEngineKind::Ocrs => ocrs().then_some("ocrs"),
        OcrEngineKind::Tesseract => tesseract_available().then_some("tesseract"),
        OcrEngineKind::Auto => {
            if ocrs() {
                Some("ocrs")
            } else {
                tesseract_available().then_some("tesseract")
            }
        }
    }
}

pub fn no_engine_error() -> anyhow::Error {
    anyhow!(
        "no OCR engine available. Run `clippo setup-ocr`, install tesseract-ocr, or check `ocr.engine` in the config."
    )
}

fn models_present(dir: &Path) -> bool {
    dir.join(DETECTION_MODEL).is_file() && dir.join(RECOGNITION_MODEL).is_file()
}

/// Directory holding both ocrs models: clippo's own, else the ocrs CLI cache.
pub fn models_dir(paths: &Paths) -> Option<PathBuf> {
    std::iter::once(&paths.ocrs_models_dir)
        .chain(paths.ocrs_cli_cache_dir.as_ref())
        .find(|d| models_present(d))
        .cloned()
}

pub fn tesseract_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("tesseract").is_file()))
        .unwrap_or(false)
}

pub struct Ocrs {
    engine: ocrs::OcrEngine,
}

impl Ocrs {
    pub fn load(dir: &Path) -> Result<Self> {
        let load = |name: &str| {
            let p = dir.join(name);
            rten::Model::load_file(&p)
                .with_context(|| format!("could not load OCR model {}", p.display()))
        };
        let engine = ocrs::OcrEngine::new(ocrs::OcrEngineParams {
            detection_model: Some(load(DETECTION_MODEL)?),
            recognition_model: Some(load(RECOGNITION_MODEL)?),
            ..Default::default()
        })?;
        Ok(Self { engine })
    }
}

impl OcrBackend for Ocrs {
    fn name(&self) -> &'static str {
        "ocrs"
    }

    fn recognize(&self, image: &[u8]) -> Result<String> {
        let img = image::load_from_memory(image)
            .context("could not decode image")?
            .into_rgb8();
        let source = ocrs::ImageSource::from_bytes(img.as_raw(), img.dimensions())?;
        let input = self.engine.prepare_input(source)?;
        let words = self.engine.detect_words(&input)?;
        let lines = self.engine.find_text_lines(&input, &words);
        let texts = self.engine.recognize_text(&input, &lines)?;
        // Single-character lines are usually spurious detections.
        let out: Vec<String> = texts
            .iter()
            .flatten()
            .map(|l| l.to_string())
            .filter(|l| l.trim().chars().count() > 1)
            .collect();
        Ok(out.join("\n"))
    }
}

pub struct Tesseract {
    lang: String,
}

impl OcrBackend for Tesseract {
    fn name(&self) -> &'static str {
        "tesseract"
    }

    fn recognize(&self, image: &[u8]) -> Result<String> {
        let mut child = Command::new("tesseract")
            .args(["stdin", "stdout", "-l", &self.lang])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => {
                    anyhow!("tesseract not found. Install the tesseract-ocr package.")
                }
                _ => anyhow!(e).context("could not run tesseract"),
            })?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let data = image.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&data));
        let output = child.wait_with_output()?;
        // A write error just means tesseract exited early; its status explains why.
        let _ = writer.join();
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            bail!("tesseract failed: {}", err.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

/// Download the ocrs models into clippo's data dir, unless usable copies already exist.
pub fn setup(paths: &Paths) -> Result<()> {
    if models_present(&paths.ocrs_models_dir) {
        println!(
            "OCR models already installed in {}",
            paths.ocrs_models_dir.display()
        );
        return Ok(());
    }
    if let Some(dir) = paths
        .ocrs_cli_cache_dir
        .as_ref()
        .filter(|d| models_present(d))
    {
        println!("Using existing OCR models in {}", dir.display());
        return Ok(());
    }
    let dir = &paths.ocrs_models_dir;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    for name in [DETECTION_MODEL, RECOGNITION_MODEL] {
        let dest = dir.join(name);
        if dest.is_file() {
            continue;
        }
        let url = format!("{MODEL_BASE_URL}{name}");
        let tmp = dir.join(format!("{name}.part"));
        println!("Downloading {url}");
        let status = Command::new("curl")
            .args(["--fail", "--location", "--progress-bar", "--output"])
            .arg(&tmp)
            .arg(&url)
            .status()
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => {
                    anyhow!("curl not found. It is needed to download the models.")
                }
                _ => anyhow!(e).context("could not run curl"),
            })?;
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            bail!("download failed: {url}");
        }
        std::fs::rename(&tmp, &dest)?;
    }
    println!("OCR models installed in {}", dir.display());
    Ok(())
}

/// Run OCR on a stored image entry and record the result. Returns the text.
pub fn ocr_entry(store: &crate::store::Store, backend: &dyn OcrBackend, id: i64) -> Result<String> {
    use crate::store::OcrStatus;
    let content = store
        .content(id)?
        .ok_or_else(|| anyhow!("no entry with id {id}"))?;
    match backend.recognize(&content) {
        Ok(text) => {
            store.set_ocr(id, OcrStatus::Done, Some(&text))?;
            Ok(text)
        }
        Err(e) => {
            store.set_ocr(id, OcrStatus::Failed, None)?;
            Err(e)
        }
    }
}
