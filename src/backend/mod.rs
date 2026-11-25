//! The display system clippo works through: the clipboard, paste keys, watching for copies,
//! and what kind of window the history window is. Chosen once per process; `clippo watch`
//! passes its choice to the processes it starts.

pub mod vkbd;
pub mod wayland;
pub mod x11;

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::Result;

use crate::config::PasteKeys;
use crate::ingest::TEXT_MIME;
use crate::store::Format;

/// Set to `wayland` or `x11` to choose the backend; otherwise `XDG_SESSION_TYPE` decides.
pub const OVERRIDE_VAR: &str = "CLIPPO_SESSION";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A compositor with data-control and virtual keyboard protocols, such as COSMIC.
    Wayland,
    X11,
}

/// Blocks until clipboard watching stops, and says why.
pub type Watcher = Box<dyn FnOnce() -> String + Send>;

/// What the history window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// A layer-shell overlay, placed by the compositor on the active output.
    LayerShell,
    /// An ordinary undecorated window, centred on the monitor under the pointer.
    Plain,
}

/// `CLIPPO_SESSION`, then `XDG_SESSION_TYPE` decide (`session_type`); without either, a
/// Wayland socket, then an X display. Wayland is the fallback, as it was before X11 support.
fn detect(session_type: Option<&str>, wayland_display: bool, x_display: bool) -> Backend {
    match session_type {
        Some("x11") => Backend::X11,
        Some("wayland") => Backend::Wayland,
        _ if wayland_display => Backend::Wayland,
        _ if x_display => Backend::X11,
        _ => Backend::Wayland,
    }
}

pub fn current() -> Backend {
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    *BACKEND.get_or_init(|| {
        let set = |var| std::env::var_os(var).is_some_and(|v| !v.is_empty());
        let session_type = std::env::var(OVERRIDE_VAR)
            .or_else(|_| std::env::var("XDG_SESSION_TYPE"))
            .ok();
        detect(
            session_type.as_deref(),
            set("WAYLAND_DISPLAY"),
            set("DISPLAY"),
        )
    })
}

/// Run `clippo ingest` for a copy the watcher saw, and wait for it. Ingest reads the clipboard
/// itself, through the same backend.
fn run_ingest(exe: &Path, backend: Backend) {
    let status = Command::new(exe)
        .arg("ingest")
        .env("CLIPBOARD_STATE", "data")
        .env(OVERRIDE_VAR, backend.word())
        .stdin(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("clippo: ingest failed ({s})"),
        Err(e) => eprintln!("clippo: could not run ingest: {e}"),
    }
}

impl Backend {
    /// Name for people: logs and the settings page.
    pub fn name(self) -> &'static str {
        match self {
            Self::Wayland => "Wayland",
            Self::X11 => "X11",
        }
    }

    /// The `CLIPPO_SESSION` value that selects this backend.
    pub fn word(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::X11 => "x11",
        }
    }

    /// Is there a display to open the history window on?
    pub fn has_display(self) -> bool {
        let var = match self {
            Self::Wayland => "WAYLAND_DISPLAY",
            Self::X11 => "DISPLAY",
        };
        std::env::var_os(var).is_some()
    }

    /// The clipboard's data as `mime`, exactly as offered; `None` if it isn't offered.
    pub fn read(self, mime: &str) -> Option<Vec<u8>> {
        match self {
            Self::Wayland => wayland::read(mime),
            Self::X11 => x11::read(mime),
        }
    }

    /// The clipboard's text, from whichever text type is offered.
    pub fn read_text(self) -> Option<Vec<u8>> {
        match self {
            Self::Wayland => wayland::read_text(),
            Self::X11 => x11::read_text(),
        }
    }

    /// Types offered by the current clipboard owner.
    pub fn list_types(self) -> Vec<String> {
        match self {
            Self::Wayland => wayland::list_types(),
            Self::X11 => x11::list_types(),
        }
    }

    /// Put `data` on the clipboard. With no `mime`, the usual set of text types is offered.
    pub fn copy(self, mime: Option<&str>, data: &[u8]) -> Result<()> {
        match self {
            Self::Wayland => wayland::copy(mime, data),
            Self::X11 => x11::copy(&[Format {
                mime: mime.unwrap_or(TEXT_MIME).to_string(),
                content: data.to_vec(),
            }]),
        }
    }

    /// Offer every format at once; the first is the primary.
    pub fn copy_formats(self, formats: &[Format]) -> Result<()> {
        match self {
            Self::Wayland => wayland::copy_formats(formats),
            Self::X11 => x11::copy(formats),
        }
    }

    /// Press the paste keys through the display's virtual keyboard.
    pub fn paste_keys(self, keys: PasteKeys, release_modifiers: bool) -> Result<()> {
        match self {
            Self::Wayland => vkbd::paste(keys),
            Self::X11 => x11::paste(keys, release_modifiers),
        }
    }

    /// Name of the virtual keyboard, for logs.
    pub fn keys_name(self) -> &'static str {
        match self {
            Self::Wayland => "Wayland virtual keyboard",
            Self::X11 => "XTEST",
        }
    }

    /// Start watching for copies; each one is stored by a `clippo ingest` process.
    pub fn start_watch(self, exe: &Path) -> Result<Watcher> {
        match self {
            Self::Wayland => wayland::start_watch(exe),
            Self::X11 => {
                let exe = exe.to_path_buf();
                Ok(Box::new(move || {
                    match x11::watch(|| run_ingest(&exe, Self::X11)) {
                        Err(e) => format!("clipboard watching stopped: {e:#}"),
                    }
                }))
            }
        }
    }

    pub fn window_kind(self) -> WindowKind {
        match self {
            Self::Wayland => WindowKind::LayerShell,
            Self::X11 => WindowKind::Plain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_wins() {
        assert_eq!(detect(Some("x11"), true, true), Backend::X11);
        assert_eq!(detect(Some("wayland"), false, true), Backend::Wayland);
    }

    #[test]
    fn falls_back_to_displays() {
        assert_eq!(detect(None, true, true), Backend::Wayland);
        assert_eq!(detect(Some("tty"), false, true), Backend::X11);
        assert_eq!(detect(None, false, false), Backend::Wayland);
    }

    #[test]
    fn words_round_trip() {
        for b in [Backend::Wayland, Backend::X11] {
            assert_eq!(detect(Some(b.word()), false, false), b);
        }
    }
}
