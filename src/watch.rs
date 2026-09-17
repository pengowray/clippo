use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::{Config, OcrConfig, Paths};
use crate::ocr::{self, OcrBackend};
use crate::service::{self, Shared};
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

    // The history window lives in this process (design 1). Its toolkit needs the main
    // thread, so wl-paste is then watched from another one.
    let window = std::env::var_os("WAYLAND_DISPLAY")
        .is_some()
        .then(crate::ui::toggle_channel);
    let shared = Arc::new(Shared {
        cfg: RwLock::new(cfg.clone()),
        paths: paths.clone(),
        menu_toggle: window.as_ref().map(|(tx, _)| {
            let tx = tx.clone();
            Box::new(move || tx.toggle()) as Box<dyn Fn() + Send + Sync>
        }),
    });
    let (wake_ocr, wakeups) = std::sync::mpsc::channel();
    let worker_shared = Arc::clone(&shared);
    thread::spawn(move || ocr_worker(&worker_shared, &wakeups));
    thread::spawn(move || {
        if let Err(e) = service::serve(shared, wake_ocr) {
            eprintln!("clippo: {e:#}");
        }
    });

    let Some((_, toggles)) = window else {
        let status = child.wait()?;
        bail!("clipboard watching stopped: wl-paste exited ({status})");
    };
    thread::spawn(move || {
        let msg = match child.wait() {
            Ok(s) => format!("clipboard watching stopped: wl-paste exited ({s})"),
            Err(e) => format!("clipboard watching stopped: {e}"),
        };
        eprintln!("clippo: {msg}");
        crate::log(&msg);
        std::process::exit(1);
    });
    crate::ui::run_resident(cfg, paths, toggles)
}

/// The OCR engine, plus the settings it was built from so a `reload` can replace it.
struct Engine {
    backend: Box<dyn OcrBackend>,
    cfg: OcrConfig,
}

fn ocr_worker(shared: &Shared, wakeups: &std::sync::mpsc::Receiver<()>) {
    let store = match Store::open(&shared.paths.db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("clippo: OCR worker stopped: {e:#}");
            return;
        }
    };
    let mut engine: Option<Engine> = None;
    let mut reported_missing = false;
    loop {
        let cfg = shared.config();
        if engine.as_ref().is_some_and(|e| e.cfg != cfg.ocr) {
            engine = None;
            reported_missing = false;
        }
        if let Err(e) = ocr_step(&cfg, &shared.paths, &store, &mut engine, &mut reported_missing) {
            eprintln!("clippo: {e:#}");
        }
        // Ingest wakes us as soon as an image arrives; the poll catches anything missed.
        let _ = wakeups.recv_timeout(POLL_INTERVAL);
    }
}

/// Process pending entries until none remain.
fn ocr_step(
    cfg: &Config,
    paths: &Paths,
    store: &Store,
    engine: &mut Option<Engine>,
    reported_missing: &mut bool,
) -> Result<()> {
    while let Some(id) = store.next_pending_ocr()? {
        if engine.is_none() {
            // Engines can appear later (setup-ocr, installing tesseract), so keep checking.
            match ocr::backend(&cfg.ocr, paths) {
                Ok(Some(b)) => {
                    eprintln!("clippo: OCR engine: {}", b.name());
                    *engine = Some(Engine {
                        backend: b,
                        cfg: cfg.ocr.clone(),
                    });
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
        let b = engine.as_ref().map(|e| e.backend.as_ref()).expect("engine set above");
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
