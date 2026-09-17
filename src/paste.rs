//! Auto-paste: press the paste keys through a virtual uinput keyboard.
//!
//! A new virtual keyboard takes the compositor a noticeable moment to start listening to, so
//! `clippo watch` keeps one open and serves paste requests over a Unix socket. Without the
//! service, a keyboard is created on the spot and clippo waits for it to settle.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};

use crate::config::{PasteConfig, PasteKeys, PasteMethod};

/// How long a freshly created keyboard needs before the compositor reads its keys.
const NEW_DEVICE_SETTLE: Duration = Duration::from_millis(800);

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

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("clippo.sock")
}

fn encode(cfg: &PasteConfig) -> String {
    let keys = match cfg.keys {
        PasteKeys::ShiftInsert => "shift-insert",
        PasteKeys::CtrlV => "ctrl-v",
        PasteKeys::CtrlShiftV => "ctrl-shift-v",
    };
    format!("paste {keys} {} {}\n", cfg.delay_ms, cfg.release_modifiers as u8)
}

fn decode(line: &str) -> Option<PasteConfig> {
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
pub fn send(cfg: &PasteConfig, after_menu: bool) -> Result<()> {
    if cfg.method != PasteMethod::Uinput {
        // Held shortcut keys don't affect the virtual keyboard, so `delay_ms` isn't needed here.
        if after_menu {
            sleep(AFTER_MENU_WAIT);
        }
        match crate::vkbd::paste(cfg.keys) {
            Ok(()) => return Ok(()),
            Err(e) if cfg.method == PasteMethod::Wayland => return Err(e),
            Err(e) => crate::log(&format!("paste: Wayland virtual keyboard failed ({e:#}), trying uinput")),
        }
    }
    send_uinput(cfg)
}

/// Press the paste keys through uinput, using the running `clippo watch`'s keyboard if there is one.
fn send_uinput(cfg: &PasteConfig) -> Result<()> {
    match send_via_service(cfg) {
        Ok(()) => Ok(()),
        Err(e) => {
            crate::log(&format!("paste: service unavailable ({e:#}), using a new keyboard"));
            let mut kb = Keyboard::new()?;
            sleep(NEW_DEVICE_SETTLE.saturating_sub(Duration::from_millis(cfg.delay_ms)));
            kb.paste(cfg)?;
            // Keep the device alive long enough for the events to be read.
            sleep(Duration::from_millis(50));
            Ok(())
        }
    }
}

fn send_via_service(cfg: &PasteConfig) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(encode(cfg).as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    match reply.trim() {
        "ok" => Ok(()),
        other => bail!("paste failed: {other}"),
    }
}

/// Tell the running `clippo watch` there is a new image to OCR. Does nothing if it isn't running.
pub fn notify_ocr() {
    if let Ok(mut stream) = UnixStream::connect(socket_path()) {
        let _ = stream.write_all(b"ocr\n");
    }
}

/// Serve requests from other clippo commands: `ocr` wakes the OCR worker, `paste ...` presses
/// the paste keys through uinput. Runs until the process exits.
pub fn serve(wake_ocr: std::sync::mpsc::Sender<()>) -> Result<()> {
    // Only created when a uinput paste is first requested.
    let mut kb: Option<Keyboard> = None;
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("could not listen on {}", path.display()))?;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut line = String::new();
        let mut reader = BufReader::new(&stream);
        let reply = match reader.read_line(&mut line) {
            Ok(_) if line.trim() == "ocr" => {
                let _ = wake_ocr.send(());
                "ok".to_string()
            }
            Ok(_) => match decode(&line) {
                Some(cfg) => {
                    let result = match &mut kb {
                        Some(kb) => kb.paste(&cfg),
                        None => Keyboard::new().and_then(|mut new| {
                            // A new keyboard needs time before the compositor reads it.
                            sleep(NEW_DEVICE_SETTLE);
                            let r = new.paste(&cfg);
                            kb = Some(new);
                            r
                        }),
                    };
                    result.map_or_else(|e| format!("{e:#}"), |()| "ok".into())
                }
                None => "bad request".into(),
            },
            Err(e) => e.to_string(),
        };
        let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
    }
    Err(anyhow!("clippo socket closed"))
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
