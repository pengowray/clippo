//! The Super+V clipboard history window (libcosmic, layer-shell).

pub mod app;
pub mod list;
pub mod notify;
pub mod rows;
pub mod settings;
pub mod strings;

use std::path::PathBuf;

use anyhow::{Context, Result};

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

/// Open the window in this process, or close an already open one.
pub fn run(cfg: &Config, paths: &Paths) -> Result<()> {
    if toggle_existing(paths) {
        return Ok(());
    }
    let path = pid_file(paths);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, std::process::id().to_string())?;
    let _guard = PidGuard(path);

    let settings = cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(false)
        .client_decorations(true);
    cosmic::app::run::<app::App>(
        settings,
        app::Flags {
            cfg: cfg.clone(),
            paths: paths.clone(),
        },
    )
    .context("could not open the clipboard window")
}
