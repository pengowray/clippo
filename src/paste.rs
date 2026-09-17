//! Auto-paste: press the paste keys through a virtual uinput keyboard.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};

use crate::config::{PasteConfig, PasteKeys};

/// libinput ignores a new device's first events for a short while after it appears.
const MIN_DEVICE_SETTLE_MS: u64 = 200;

const MODIFIERS: [KeyCode; 4] = [
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_RIGHTALT,
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

pub fn send(cfg: &PasteConfig) -> Result<()> {
    let seq = combo(cfg.keys);
    let mut keys = AttributeSet::<KeyCode>::new();
    for k in MODIFIERS.iter().chain(seq) {
        keys.insert(*k);
    }
    let mut dev = VirtualDevice::builder()
        .and_then(|b| b.name("clippo").with_keys(&keys))
        .and_then(|b| b.build())
        .context("could not create a virtual keyboard (is /dev/uinput writable?)")?;

    // Also gives the shortcut's own keys, or a closing menu, time to let go of focus.
    sleep(Duration::from_millis(cfg.delay_ms.max(MIN_DEVICE_SETTLE_MS)));

    // The shortcut's Super/Alt may still be held, which would turn the paste into another shortcut.
    if cfg.release_modifiers {
        dev.emit(&MODIFIERS.map(|m| key(m, false)))?;
    }
    for k in seq {
        dev.emit(&[key(*k, true)])?;
    }
    for k in seq.iter().rev() {
        dev.emit(&[key(*k, false)])?;
    }
    // Keep the device alive long enough for the events to be read.
    sleep(Duration::from_millis(50));
    Ok(())
}
