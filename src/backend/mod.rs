//! The display system clippo works through: the clipboard, paste keys, watching for copies,
//! and what kind of window the history window is. Chosen once per process; `clippo watch`
//! passes its choice to the processes it starts.

pub mod gnome;
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

/// Set to `wayland`, `x11` or `gnome` to choose the backend; otherwise `XDG_SESSION_TYPE` and
/// `XDG_CURRENT_DESKTOP` decide.
pub const OVERRIDE_VAR: &str = "CLIPPO_SESSION";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A compositor with data-control and virtual keyboard protocols, such as COSMIC.
    Wayland,
    X11,
    /// GNOME on Wayland, through clippo's GNOME Shell extension.
    Gnome,
}

/// Blocks until clipboard watching stops, and says why.
pub type Watcher = Box<dyn FnOnce() -> String + Send>;

/// What the history window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// A layer-shell overlay, placed by the compositor on the active output.
    LayerShell,
    /// An ordinary undecorated window, centred on the monitor under the pointer (by clippo on
    /// X11, by the Shell extension on GNOME).
    Plain,
}

/// `CLIPPO_SESSION`, then `XDG_SESSION_TYPE` decide (`session_type`); without either, a
/// Wayland socket, then an X display. A Wayland session on GNOME (`gnome`: `XDG_CURRENT_DESKTOP`
/// lists GNOME) goes through the Shell extension. Wayland is the fallback, as it was before
/// X11 support.
fn detect(session_type: Option<&str>, gnome: bool, wayland_display: bool, x_display: bool) -> Backend {
    let wayland = if gnome { Backend::Gnome } else { Backend::Wayland };
    match session_type {
        Some("x11") => Backend::X11,
        Some("gnome") => Backend::Gnome,
        Some("wayland") => wayland,
        _ if wayland_display => wayland,
        _ if x_display => Backend::X11,
        _ => Backend::Wayland,
    }
}

pub fn current() -> Backend {
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    *BACKEND.get_or_init(|| {
        let set = |var| std::env::var_os(var).is_some_and(|v| !v.is_empty());
        // The override names the backend outright; `wayland` there means the plain one.
        if let Ok(word) = std::env::var(OVERRIDE_VAR)
            && let Some(b) = [Backend::Wayland, Backend::X11, Backend::Gnome]
                .into_iter()
                .find(|b| b.word() == word)
        {
            return b;
        }
        let gnome = std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case("GNOME")));
        detect(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            gnome,
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
            Self::Gnome => "GNOME Shell",
        }
    }

    /// The `CLIPPO_SESSION` value that selects this backend.
    pub fn word(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::X11 => "x11",
            Self::Gnome => "gnome",
        }
    }

    /// Is there a display to open the history window on?
    pub fn has_display(self) -> bool {
        let var = match self {
            Self::Wayland | Self::Gnome => "WAYLAND_DISPLAY",
            Self::X11 => "DISPLAY",
        };
        std::env::var_os(var).is_some()
    }

    /// The clipboard's data as `mime`, exactly as offered; `None` if it isn't offered.
    pub fn read(self, mime: &str) -> Option<Vec<u8>> {
        match self {
            Self::Wayland => wayland::read(mime),
            Self::X11 => x11::read(mime),
            Self::Gnome => gnome::read(mime),
        }
    }

    /// The clipboard's text, from whichever text type is offered.
    pub fn read_text(self) -> Option<Vec<u8>> {
        match self {
            Self::Wayland => wayland::read_text(),
            Self::X11 => x11::read_text(),
            Self::Gnome => gnome::read_text(),
        }
    }

    /// Types offered by the current clipboard owner.
    pub fn list_types(self) -> Vec<String> {
        match self {
            Self::Wayland => wayland::list_types(),
            Self::X11 => x11::list_types(),
            Self::Gnome => gnome::list_types(),
        }
    }

    /// Put `data` on the clipboard. With no `mime`, the usual set of text types is offered.
    pub fn copy(self, mime: Option<&str>, data: &[u8]) -> Result<()> {
        match self {
            Self::Wayland => wayland::copy(mime, data),
            Self::X11 | Self::Gnome => self.copy_formats(&[Format {
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
            Self::Gnome => gnome::copy_formats(formats),
        }
    }

    /// Press the paste keys through the display's virtual keyboard.
    pub fn paste_keys(self, keys: PasteKeys, release_modifiers: bool) -> Result<()> {
        match self {
            Self::Wayland => vkbd::paste(keys),
            Self::X11 => x11::paste(keys, release_modifiers),
            // The extension waits for Super and Alt to be released itself.
            Self::Gnome => gnome::paste(keys),
        }
    }

    /// Name of the virtual keyboard, for logs.
    pub fn keys_name(self) -> &'static str {
        match self {
            Self::Wayland => "Wayland virtual keyboard",
            Self::X11 => "XTEST",
            Self::Gnome => "GNOME Shell virtual keyboard",
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
            Self::Gnome => gnome::start_watch(exe),
        }
    }

    pub fn window_kind(self) -> WindowKind {
        match self {
            Self::Wayland => WindowKind::LayerShell,
            Self::X11 | Self::Gnome => WindowKind::Plain,
        }
    }

    /// Where to put a plain window: the monitor under the pointer, as (x, y, width, height).
    /// `None` leaves it to the compositor (on GNOME the extension places it).
    pub fn window_area(self) -> Option<(i16, i16, u16, u16)> {
        match self {
            Self::X11 => x11::pointer_monitor(),
            Self::Wayland | Self::Gnome => None,
        }
    }

    /// Raise and focus a plain window just opened; `raw_id` is the display system's window id.
    /// On GNOME the extension does this when the window appears.
    pub fn activate_window(self, raw_id: u64) -> Result<()> {
        match self {
            Self::X11 => x11::activate(raw_id as u32),
            Self::Wayland | Self::Gnome => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_wins() {
        assert_eq!(detect(Some("x11"), false, true, true), Backend::X11);
        assert_eq!(detect(Some("wayland"), false, false, true), Backend::Wayland);
    }

    #[test]
    fn gnome_on_wayland_uses_the_extension() {
        assert_eq!(detect(Some("wayland"), true, true, true), Backend::Gnome);
        assert_eq!(detect(Some("x11"), true, false, true), Backend::X11);
        assert_eq!(detect(None, true, true, false), Backend::Gnome);
    }

    #[test]
    fn falls_back_to_displays() {
        assert_eq!(detect(None, false, true, true), Backend::Wayland);
        assert_eq!(detect(Some("tty"), false, false, true), Backend::X11);
        assert_eq!(detect(None, false, false, false), Backend::Wayland);
    }

    #[test]
    fn words_round_trip() {
        for b in [Backend::Wayland, Backend::X11, Backend::Gnome] {
            assert_eq!(detect(Some(b.word()), false, false, false), b);
        }
    }
}
