//! Which display system the clipboard and paste keys go through: Wayland (`wl-clipboard`,
//! virtual keyboard protocol) or X11 (`x11.rs`).

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    Wayland,
    X11,
}

/// `CLIPPO_SESSION` (an override), then `XDG_SESSION_TYPE` decide; without either, a Wayland
/// socket, then an X display. Wayland is the fallback, as it was before X11 support.
fn detect(session_type: Option<&str>, wayland_display: bool, x_display: bool) -> Session {
    match session_type {
        Some("x11") => Session::X11,
        Some("wayland") => Session::Wayland,
        _ if wayland_display => Session::Wayland,
        _ if x_display => Session::X11,
        _ => Session::Wayland,
    }
}

pub fn current() -> Session {
    static SESSION: OnceLock<Session> = OnceLock::new();
    *SESSION.get_or_init(|| {
        let set = |var| std::env::var_os(var).is_some_and(|v| !v.is_empty());
        let session_type = std::env::var("CLIPPO_SESSION")
            .or_else(|_| std::env::var("XDG_SESSION_TYPE"))
            .ok();
        detect(
            session_type.as_deref(),
            set("WAYLAND_DISPLAY"),
            set("DISPLAY"),
        )
    })
}

pub fn is_x11() -> bool {
    current() == Session::X11
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_wins() {
        assert_eq!(detect(Some("x11"), true, true), Session::X11);
        assert_eq!(detect(Some("wayland"), false, true), Session::Wayland);
    }

    #[test]
    fn falls_back_to_displays() {
        assert_eq!(detect(None, true, true), Session::Wayland);
        assert_eq!(detect(Some("tty"), false, true), Session::X11);
        assert_eq!(detect(None, false, false), Session::Wayland);
    }
}
