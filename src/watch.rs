use std::os::unix::process::CommandExt;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::{Config, Paths};
use crate::ocr::{self, OcrBackend};
use crate::store::Store;
use crate::thumbs;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub fn run(cfg: &Config, paths: &Paths) -> Result<()> {
    // Create the database up front so ingest and the worker never race to do it.
    Store::open(&paths.db)?;

    let exe = std::env::current_exe().context("could not find clippo executable")?;
    let mut cmd = Command::new("wl-paste");
    cmd.arg("--watch").arg(&exe).arg("ingest");
    // SAFETY: prctl is async-signal-safe. It makes wl-paste exit when we do,
    // however we die.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("could not run wl-paste")?;

    let (cfg, paths) = (cfg.clone(), paths.clone());
    thread::spawn(move || ocr_worker(&cfg, &paths));

    let status = child.wait()?;
    bail!("clipboard watching stopped: wl-paste exited ({status})");
}

fn ocr_worker(cfg: &Config, paths: &Paths) {
    let store = match Store::open(&paths.db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clippo: OCR worker stopped: {e:#}");
            return;
        }
    };
    let mut backend: Option<Box<dyn OcrBackend>> = None;
    let mut reported_missing = false;
    loop {
        if let Err(e) = ocr_step(cfg, paths, &store, &mut backend, &mut reported_missing) {
            eprintln!("clippo: {e:#}");
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Process pending entries until none remain.
fn ocr_step(
    cfg: &Config,
    paths: &Paths,
    store: &Store,
    backend: &mut Option<Box<dyn OcrBackend>>,
    reported_missing: &mut bool,
) -> Result<()> {
    while let Some(id) = store.next_pending_ocr()? {
        if backend.is_none() {
            // Engines can appear later (setup-ocr, installing tesseract), so keep checking.
            match ocr::backend(&cfg.ocr, paths) {
                Ok(Some(b)) => {
                    eprintln!("clippo: OCR engine: {}", b.name());
                    *backend = Some(b);
                }
                Ok(None) | Err(_) if *reported_missing => return Ok(()),
                Ok(None) => {
                    *reported_missing = true;
                    eprintln!("clippo: {}", ocr::no_engine_error());
                    return Ok(());
                }
                Err(e) => {
                    *reported_missing = true;
                    return Err(e);
                }
            }
        }
        let b = backend.as_deref().expect("backend set above");
        if let Some(content) = store.content(id)? {
            let _ = thumbs::ensure(&paths.thumbs_dir, id, &content);
        }
        if let Err(e) = ocr::ocr_entry(store, b, id) {
            // Pick up where we left off on the next tick.
            eprintln!("clippo: OCR failed for entry {id}: {e:#}");
            return Ok(());
        }
    }
    Ok(())
}
