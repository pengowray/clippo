//! Throwaway test for auto-paste through a uinput keyboard.
//! Usage: paste_spike <shift-insert|ctrl-v|ctrl-shift-v> <release-mods: 0|1> <delay-ms>
use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode};
use std::{thread::sleep, time::Duration};

fn key(code: KeyCode, down: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, down as i32)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let combo = args.get(1).map(String::as_str).unwrap_or("shift-insert");
    let release_mods = args.get(2).map(String::as_str) == Some("1");
    let delay: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(150);

    let mods = [
        KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTMETA,
        KeyCode::KEY_LEFTALT, KeyCode::KEY_RIGHTALT,
    ];
    let mut keys = AttributeSet::<KeyCode>::new();
    for k in mods.iter().chain(&[
        KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_LEFTCTRL, KeyCode::KEY_INSERT, KeyCode::KEY_V,
    ]) {
        keys.insert(*k);
    }
    let mut dev = VirtualDevice::builder()?.name("clippo paste spike").with_keys(&keys)?.build()?;

    // New input devices take a moment for libinput to pick up
    sleep(Duration::from_millis(delay.max(200)));

    if release_mods {
        dev.emit(&mods.map(|m| key(m, false)))?;
    }
    let seq: &[KeyCode] = match combo {
        "ctrl-v" => &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V],
        "ctrl-shift-v" => &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_V],
        _ => &[KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_INSERT],
    };
    for k in seq {
        dev.emit(&[key(*k, true)])?;
    }
    for k in seq.iter().rev() {
        dev.emit(&[key(*k, false)])?;
    }
    sleep(Duration::from_millis(100));
    Ok(())
}
