//! Paste keys through the Wayland virtual keyboard protocol (`zwp_virtual_keyboard_v1`).
//!
//! The compositor delivers these keys straight to the focused window with the modifiers we
//! state, so keys the user is still holding from the shortcut don't get in the way.

use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

use crate::config::PasteKeys;

/// Compiled by the compositor, so the includes resolve against its xkb data.
const KEYMAP: &str = "xkb_keymap {
    xkb_keycodes { include \"evdev+aliases(qwerty)\" };
    xkb_types { include \"complete\" };
    xkb_compat { include \"complete\" };
    xkb_symbols { include \"pc+us+inet(evdev)\" };
};\n";

const XKB_V1: u32 = 1;
const SHIFT: u32 = 1;
const CTRL: u32 = 1 << 2;
// evdev key codes
const KEY_INSERT: u32 = 110;
const KEY_V: u32 = 47;
const PRESSED: u32 = 1;
const RELEASED: u32 = 0;

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(State: ignore WlSeat);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardV1);

fn keymap_fd() -> Result<OwnedFd> {
    // SAFETY: memfd_create with a valid C string; the returned fd is owned by us.
    let fd = unsafe { libc::memfd_create(c"clippo-keymap".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        bail!("could not create keymap file: {}", std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut file = std::fs::File::from(fd);
    file.write_all(KEYMAP.as_bytes())?;
    file.write_all(&[0])?;
    Ok(file.into())
}

pub fn paste(keys: PasteKeys) -> Result<()> {
    let conn = Connection::connect_to_env().context("could not connect to Wayland")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let manager: ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .context("the compositor has no virtual keyboard support")?;
    let seat: WlSeat = globals.bind(&qh, 1..=9, ()).context("no seat")?;
    let kb = manager.create_virtual_keyboard(&seat, &qh, ());

    let fd = keymap_fd()?;
    kb.keymap(XKB_V1, fd.as_fd(), KEYMAP.len() as u32 + 1);

    let (mods, key) = match keys {
        PasteKeys::ShiftInsert => (SHIFT, KEY_INSERT),
        PasteKeys::CtrlV => (CTRL, KEY_V),
        PasteKeys::CtrlShiftV => (CTRL | SHIFT, KEY_V),
    };
    let start = Instant::now();
    let time = || start.elapsed().as_millis() as u32;
    kb.modifiers(mods, 0, 0, 0);
    kb.key(time(), key, PRESSED);
    kb.key(time(), key, RELEASED);
    kb.modifiers(0, 0, 0, 0);
    queue.roundtrip(&mut State)?;
    kb.destroy();
    conn.flush()?;
    // A protocol error (e.g. access denied) arrives asynchronously; give it a moment to show.
    std::thread::sleep(Duration::from_millis(20));
    queue.roundtrip(&mut State)?;
    Ok(())
}
