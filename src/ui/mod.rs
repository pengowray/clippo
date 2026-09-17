//! The Super+V clipboard history window (libcosmic, layer-shell).

pub mod app;
pub mod list;
pub mod notify;
pub mod rows;
pub mod settings;
pub mod strings;

use std::path::PathBuf;

use anyhow::{Context, Result};
use cosmic::iced::futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::config::{Config, Paths};

fn pid_file(paths: &Paths) -> PathBuf {
    paths.runtime_dir.join("window.pid")
}

/// If another `clippo window` is open, close it (toggle) and return `true`.
// TODO(backend): replace with the socket `menu toggle` once `clippo watch` hosts the window.
fn toggle_existing(paths: &Paths) -> bool {
    let path = pid_file(paths);
    let Ok(s) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(pid) = s.trim().parse::<i32>() else {
        let _ = std::fs::remove_file(&path);
        return false;
    };
    // SAFETY: kill with a pid is a plain syscall with no memory effects.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if alive {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let _ = std::fs::remove_file(&path);
        true
    } else {
        let _ = std::fs::remove_file(&path);
        false
    }
}

struct PidGuard(PathBuf);

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Show-or-hide requests from the service socket to the resident window.
#[derive(Clone)]
pub struct ToggleSender(UnboundedSender<()>);

impl ToggleSender {
    pub fn toggle(&self) {
        let _ = self.0.unbounded_send(());
    }
}

pub type ToggleReceiver = UnboundedReceiver<()>;

pub fn toggle_channel() -> (ToggleSender, ToggleReceiver) {
    let (tx, rx) = unbounded();
    (ToggleSender(tx), rx)
}

fn settings() -> cosmic::app::Settings {
    cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(false)
        .client_decorations(true)
}

/// Toggle the window: through the running service when it hosts one, else in this process.
pub fn run(cfg: &Config, paths: &Paths) -> Result<()> {
    match crate::service::menu_toggle(paths) {
        Ok(()) => return Ok(()),
        // Not running, or an older service without a window: open one here instead.
        Err(e) => crate::log(&format!("window: {e:#}; opening in-process")),
    }
    if toggle_existing(paths) {
        return Ok(());
    }
    let path = pid_file(paths);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, std::process::id().to_string())?;
    let _guard = PidGuard(path);
    cosmic::app::run::<app::App>(
        settings(),
        app::Flags {
            cfg: cfg.clone(),
            paths: paths.clone(),
            toggles: None,
        },
    )
    .context("could not open the clipboard window")
}

/// Host the window inside `clippo watch`: hidden until a `menu toggle` arrives. Runs until
/// the process exits.
pub fn run_resident(cfg: &Config, paths: &Paths, toggles: ToggleReceiver) -> Result<()> {
    cosmic::app::run::<app::App>(
        settings(),
        app::Flags {
            cfg: cfg.clone(),
            paths: paths.clone(),
            toggles: Some(toggles),
        },
    )
    .context("could not start the clipboard window")
}
