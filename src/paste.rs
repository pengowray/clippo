//! Auto-paste: press the paste keys through a virtual keyboard.
//!
//! The Wayland virtual keyboard (`vkbd.rs`) is the default. The uinput fallback takes the
//! compositor a noticeable moment to start listening to a new device, so `clippo watch` keeps
//! one open and serves paste requests over its socket (`service.rs`). Without the service, a
//! keyboard is created on the spot and clippo waits for it to settle.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};

use crate::config::{PasteConfig, PasteKeys, PasteMethod, Paths};
use crate::service;

/// How long a freshly created keyboard needs before the compositor reads its keys.
pub const NEW_DEVICE_SETTLE: Duration = Duration::from_millis(800);

const MODIFIERS: [KeyCode; 4] = [
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_RIGHTALT,
];

const PASTE_KEYS: [KeyCode; 4] = [
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_INSERT,
    KeyCode::KEY_V,
];

fn combo(keys: PasteKeys) -> &'static [KeyCode] {
    match keys {
        PasteKeys::ShiftInsert => &[KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_INSERT],
        PasteKeys::CtrlV => &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V],
        PasteKeys::CtrlShiftV => &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_V],
    }
}

fn key(code: KeyCode, down: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, down as i32)
}

pub struct Keyboard {
    dev: VirtualDevice,
}

impl Keyboard {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for k in MODIFIERS.iter().chain(&PASTE_KEYS) {
            keys.insert(*k);
        }
        let dev = VirtualDevice::builder()
            .and_then(|b| b.name("clippo").with_keys(&keys))
            .and_then(|b| b.build())
            .context("could not create a virtual keyboard (is /dev/uinput writable?)")?;
        Ok(Self { dev })
    }

    pub fn paste(&mut self, cfg: &PasteConfig) -> Result<()> {
        // Gives the shortcut's own keys, or a closing menu, time to let go.
        sleep(Duration::from_millis(cfg.delay_ms));
        // Super/Alt may still be held, which would turn the paste into another shortcut.
        if cfg.release_modifiers {
            self.dev.emit(&MODIFIERS.map(|m| key(m, false)))?;
        }
        let seq = combo(cfg.keys);
        for k in seq {
            self.dev.emit(&[key(*k, true)])?;
        }
        for k in seq.iter().rev() {
            self.dev.emit(&[key(*k, false)])?;
        }
        Ok(())
    }
}

/// The socket request for a uinput paste with these settings.
pub fn encode(cfg: &PasteConfig) -> String {
    let keys = match cfg.keys {
        PasteKeys::ShiftInsert => "shift-insert",
        PasteKeys::CtrlV => "ctrl-v",
        PasteKeys::CtrlShiftV => "ctrl-shift-v",
    };
    format!("paste {keys} {} {}", cfg.delay_ms, cfg.release_modifiers as u8)
}

pub fn decode(line: &str) -> Option<PasteConfig> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "paste" {
        return None;
    }
    let keys = match parts.next()? {
        "shift-insert" => PasteKeys::ShiftInsert,
        "ctrl-v" => PasteKeys::CtrlV,
        "ctrl-shift-v" => PasteKeys::CtrlShiftV,
        _ => return None,
    };
    Some(PasteConfig {
        keys,
        delay_ms: parts.next()?.parse().ok()?,
        release_modifiers: parts.next()? == "1",
        ..PasteConfig::default()
    })
}

/// Focus needs a moment to return to the app after the menu closes.
const AFTER_MENU_WAIT: Duration = Duration::from_millis(50);

/// Press the paste keys with the configured method.
pub fn send(paths: &Paths, cfg: &PasteConfig, after_menu: bool) -> Result<()> {
    if cfg.method != PasteMethod::Uinput {
        // Held shortcut keys don't affect the virtual keyboard, so `delay_ms` isn't needed here.
        if after_menu {
            sleep(AFTER_MENU_WAIT);
        }
        match crate::vkbd::paste(cfg.keys) {
            Ok(()) => return Ok(()),
            Err(e) if cfg.method == PasteMethod::Wayland => return Err(e),
            Err(e) => crate::log(&format!(
                "paste: Wayland virtual keyboard failed ({e:#}), trying uinput"
            )),
        }
    }
    send_uinput(paths, cfg)
}

/// Press the paste keys through uinput, using the running `clippo watch`'s keyboard if there is one.
fn send_uinput(paths: &Paths, cfg: &PasteConfig) -> Result<()> {
    match service::paste(paths, cfg) {
        Ok(()) => Ok(()),
        Err(e) if e.is::<service::Unavailable>() => {
            crate::log(&format!("paste: {e:#}, using a new keyboard"));
            let mut kb = Keyboard::new()?;
            sleep(NEW_DEVICE_SETTLE.saturating_sub(Duration::from_millis(cfg.delay_ms)));
            kb.paste(cfg)?;
            // Keep the device alive long enough for the events to be read.
            sleep(Duration::from_millis(50));
            Ok(())
        }
        Err(e) => Err(e.context("paste failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let cfg = PasteConfig {
            keys: PasteKeys::CtrlShiftV,
            delay_ms: 75,
            release_modifiers: false,
            ..PasteConfig::default()
        };
        let back = decode(&encode(&cfg)).unwrap();
        assert_eq!(back.keys, PasteKeys::CtrlShiftV);
        assert_eq!(back.delay_ms, 75);
        assert!(!back.release_modifiers);
        assert!(decode("nonsense").is_none());
    }
}
