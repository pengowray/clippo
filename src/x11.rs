//! The X11 clipboard: watching it (XFixes), reading it, owning it, and pressing paste keys
//! (XTEST). Used when the session is X11 (see `session.rs`); Wayland goes through
//! `wl-clipboard` and `vkbd.rs` instead.
//!
//! X11 has no clipboard daemon: whoever copied keeps the data and hands it out on request.
//! So a copy runs `clippo serve-clipboard`, a small process that owns the selection, answers
//! requests until another app copies, and then exits. It owns PRIMARY as well as CLIPBOARD,
//! because Shift+Insert (the default paste keys) pastes PRIMARY in xterm and VTE terminals.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask,
    KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Keycode, PropMode, Property, SELECTION_NOTIFY_EVENT,
    SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use crate::config::PasteKeys;
use crate::store::Format;

/// How long an app gets to answer a request for clipboard data, and between INCR chunks.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// How long `copy` waits for `serve-clipboard` to report that it owns the clipboard.
const SERVE_START_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a paste waits for the shortcut's keys to be released.
const KEY_RELEASE_WAIT: Duration = Duration::from_secs(1);
/// Data bigger than this is sent in chunks of this size (the INCR protocol).
const INCR_CHUNK: usize = 256 * 1024;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        MULTIPLE,
        TIMESTAMP,
        SAVE_TARGETS,
        INCR,
        TEXT,
        UTF8_STRING,
        TEXT_PLAIN: b"text/plain",
        TEXT_PLAIN_UTF8: b"text/plain;charset=utf-8",
        CLIPPO_DATA,
    }
}

/// A connection with a hidden window to receive clipboard data on.
struct Client {
    conn: RustConnection,
    root: Window,
    win: Window,
    atoms: Atoms,
}

impl Client {
    fn connect() -> Result<Self> {
        let (conn, screen) = x11rb::connect(None).context("could not connect to the X display")?;
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id()?;
        conn.create_window(
            0,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let atoms = Atoms::new(&conn)?.reply()?;
        Ok(Self {
            conn,
            root,
            win,
            atoms,
        })
    }

    /// The next event, or `None` once `deadline` passes.
    fn next_event(&self, deadline: Instant) -> Result<Option<Event>> {
        self.conn.flush()?;
        loop {
            if let Some(e) = self.conn.poll_for_event()? {
                return Ok(Some(e));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let mut fd = libc::pollfd {
                fd: self.conn.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for the duration of the call.
            unsafe { libc::poll(&mut fd, 1, left.as_millis().min(i32::MAX as u128) as i32) };
        }
    }

    /// The X server's current time. ICCCM and EWMH ask for a real timestamp rather than
    /// CurrentTime: make a property change and read the time from its notify.
    fn server_time(&self) -> Result<u32> {
        self.conn.change_property8(
            PropMode::APPEND,
            self.win,
            self.atoms.CLIPPO_DATA,
            AtomEnum::STRING,
            &[],
        )?;
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            match self.next_event(deadline)? {
                None => bail!("the X server did not answer"),
                Some(Event::PropertyNotify(e)) if e.window == self.win => return Ok(e.time),
                Some(_) => {}
            }
        }
    }

    fn atom(&self, name: &str) -> Result<Atom> {
        Ok(self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    }

    fn atom_names(&self, atoms: &[Atom]) -> Result<Vec<String>> {
        let cookies = atoms
            .iter()
            .map(|a| self.conn.get_atom_name(*a))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(cookies
            .into_iter()
            .filter_map(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .collect())
    }

    fn has_owner(&self) -> Result<bool> {
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner;
        Ok(owner != NONE)
    }

    /// Ask the clipboard owner for `target`. `None` if it refuses or doesn't answer in time.
    fn convert(&self, target: Atom) -> Result<Option<(Atom, Vec<u8>)>> {
        let prop = self.atoms.CLIPPO_DATA;
        self.conn.delete_property(self.win, prop)?;
        self.conn
            .convert_selection(self.win, self.atoms.CLIPBOARD, target, prop, CURRENT_TIME)?;
        let deadline = Instant::now() + READ_TIMEOUT;
        let property = loop {
            match self.next_event(deadline)? {
                None => return Ok(None),
                Some(Event::SelectionNotify(e)) if e.requestor == self.win => break e.property,
                Some(_) => {}
            }
        };
        if property == NONE {
            return Ok(None);
        }
        let reply = self
            .conn
            .get_property(true, self.win, prop, AtomEnum::ANY, 0, u32::MAX / 4)?
            .reply()?;
        if reply.type_ != self.atoms.INCR {
            return Ok(Some((reply.type_, reply.value)));
        }
        // INCR: deleting the property (done by the get above) asks for each next chunk, and a
        // zero-length chunk ends the transfer.
        let mut data = Vec::new();
        let mut kind = NONE;
        loop {
            let deadline = Instant::now() + READ_TIMEOUT;
            loop {
                match self.next_event(deadline)? {
                    None => return Ok(None),
                    Some(Event::PropertyNotify(e))
                        if e.window == self.win
                            && e.atom == prop
                            && e.state == Property::NEW_VALUE =>
                    {
                        break;
                    }
                    Some(_) => {}
                }
            }
            let chunk = self
                .conn
                .get_property(true, self.win, prop, AtomEnum::ANY, 0, u32::MAX / 4)?
                .reply()?;
            if chunk.value.is_empty() {
                return Ok(Some((kind, data)));
            }
            kind = chunk.type_;
            data.extend_from_slice(&chunk.value);
        }
    }

    fn targets(&self) -> Result<Vec<Atom>> {
        let Some((_, raw)) = self.convert(self.atoms.TARGETS)? else {
            return Ok(Vec::new());
        };
        Ok(raw
            .chunks_exact(4)
            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }
}

/// Types offered by the current clipboard owner, without X11's bookkeeping targets.
pub fn list_types() -> Vec<String> {
    let run = || -> Result<Vec<String>> {
        let c = Client::connect()?;
        if !c.has_owner()? {
            return Ok(Vec::new());
        }
        let a = &c.atoms;
        let meta = [a.TARGETS, a.MULTIPLE, a.TIMESTAMP, a.SAVE_TARGETS];
        let targets: Vec<Atom> = c.targets()?.into_iter().filter(|t| !meta.contains(t)).collect();
        c.atom_names(&targets)
    };
    run().unwrap_or_default()
}

/// The clipboard's data as `mime`, if the owner offers it.
pub fn read(mime: &str) -> Option<Vec<u8>> {
    let run = || -> Result<Option<Vec<u8>>> {
        let c = Client::connect()?;
        if !c.has_owner()? {
            return Ok(None);
        }
        Ok(c.convert(c.atom(mime)?)?.map(|(_, data)| data))
    };
    run().ok().flatten()
}

/// The clipboard's text as UTF-8, from whichever text type the owner offers.
pub fn read_text() -> Option<Vec<u8>> {
    let run = || -> Result<Option<Vec<u8>>> {
        let c = Client::connect()?;
        if !c.has_owner()? {
            return Ok(None);
        }
        let a = &c.atoms;
        let offered = c.targets()?;
        let utf8 = [a.UTF8_STRING, a.TEXT_PLAIN_UTF8, a.TEXT_PLAIN];
        if let Some(t) = utf8.iter().find(|t| offered.contains(t)) {
            return Ok(c.convert(*t)?.map(|(_, d)| d));
        }
        if offered.contains(&AtomEnum::STRING.into()) {
            // STRING is Latin-1.
            return Ok(c
                .convert(AtomEnum::STRING.into())?
                .map(|(_, d)| d.iter().map(|b| *b as char).collect::<String>().into_bytes()));
        }
        Ok(None)
    };
    run().ok().flatten()
}

/// Call `on_change` whenever an app takes the clipboard, and once at the start if it has an
/// owner. Changes that arrive while `on_change` runs are folded into one more call.
/// Returns only on error.
pub fn watch(mut on_change: impl FnMut()) -> Result<std::convert::Infallible> {
    let c = Client::connect()?;
    c.conn
        .xfixes_query_version(5, 0)?
        .reply()
        .context("the X server has no XFixes extension")?;
    c.conn.xfixes_select_selection_input(
        c.root,
        c.atoms.CLIPBOARD,
        xfixes::SelectionEventMask::SET_SELECTION_OWNER,
    )?;
    c.conn.flush()?;
    let changed = |e: &Event| matches!(e, Event::XfixesSelectionNotify(n) if n.owner != NONE);
    if c.has_owner()? {
        on_change();
    }
    loop {
        let e = c.conn.wait_for_event()?;
        if !changed(&e) {
            continue;
        }
        loop {
            on_change();
            let mut again = false;
            while let Some(e) = c.conn.poll_for_event()? {
                again |= changed(&e);
            }
            if !again {
                break;
            }
        }
    }
}

/// Put `formats` on the clipboard (the first is the primary). Returns once a
/// `clippo serve-clipboard` process owns it.
pub fn copy(formats: &[Format]) -> Result<()> {
    if formats.is_empty() {
        bail!("nothing to copy");
    }
    let exe = std::env::current_exe().context("could not find clippo executable")?;
    let mut child = Command::new(exe)
        .arg("serve-clipboard")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start clippo serve-clipboard")?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let mut buf = Vec::new();
    encode_formats(formats, &mut buf);
    stdin.write_all(&buf)?;
    drop(stdin);
    let reply = rx.recv_timeout(SERVE_START_TIMEOUT);
    // The server runs until another app copies; reap it then (it may outlive this process).
    std::thread::spawn(move || child.wait());
    match reply {
        Ok(line) if line.trim() == "ok" => Ok(()),
        Ok(line) => match line.trim().strip_prefix("error ") {
            Some(msg) => bail!("could not take the clipboard: {msg}"),
            None => bail!("could not take the clipboard"),
        },
        Err(_) => bail!("could not take the clipboard: no answer from clippo serve-clipboard"),
    }
}

/// `clippo serve-clipboard`: read formats from stdin, own the clipboard, print `ok` (or
/// `error <message>`), then serve until another app copies.
pub fn serve_from_stdin() -> Result<()> {
    // Stay alive when the command that started us exits or its terminal closes.
    // SAFETY: setsid has no memory effects.
    unsafe { libc::setsid() };
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw)?;
    let started = decode_formats(&raw).and_then(Server::start);
    let mut out = std::io::stdout();
    let server = match started {
        Ok(s) => {
            writeln!(out, "ok")?;
            s
        }
        Err(e) => {
            let msg: String = format!("{e:#}").split_whitespace().collect::<Vec<_>>().join(" ");
            writeln!(out, "error {msg}")?;
            return Err(e);
        }
    };
    out.flush()?;
    // Close stdout so the caller's read ends, even if it captures our output.
    if let Ok(null) = std::fs::File::open("/dev/null") {
        // SAFETY: dup2 onto stdout with a valid fd.
        unsafe { libc::dup2(null.as_raw_fd(), 1) };
    }
    server.run()
}

/// Framing for `serve-clipboard`'s stdin: per format, `mime\nlength\n` then the bytes.
fn encode_formats(formats: &[Format], out: &mut Vec<u8>) {
    for f in formats {
        out.extend_from_slice(format!("{}\n{}\n", f.mime, f.content.len()).as_bytes());
        out.extend_from_slice(&f.content);
    }
}

fn decode_formats(mut raw: &[u8]) -> Result<Vec<Format>> {
    let line = |raw: &mut &[u8]| -> Result<String> {
        let end = raw
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| anyhow!("bad clipboard data"))?;
        let s = String::from_utf8(raw[..end].to_vec())?;
        *raw = &raw[end + 1..];
        Ok(s)
    };
    let mut formats = Vec::new();
    while !raw.is_empty() {
        let mime = line(&mut raw)?;
        let len: usize = line(&mut raw)?.parse()?;
        if raw.len() < len {
            bail!("bad clipboard data");
        }
        formats.push(Format {
            mime,
            content: raw[..len].to_vec(),
        });
        raw = &raw[len..];
    }
    if formats.is_empty() {
        bail!("nothing to copy");
    }
    Ok(formats)
}

/// A chunked (INCR) transfer in progress to one requestor.
struct Transfer {
    target: Atom,
    format: usize,
    offset: usize,
}

struct Server {
    client: Client,
    formats: Vec<Format>,
    /// What each offered target serves: an index into `formats`, plus whether it is the
    /// Latin-1 `STRING` form of the text.
    targets: Vec<(Atom, usize, bool)>,
    time: u32,
    /// Keyed by (requestor window, property).
    transfers: HashMap<(Window, Atom), Transfer>,
}

impl Server {
    fn start(formats: Vec<Format>) -> Result<Self> {
        let client = Client::connect()?;
        let a = &client.atoms;
        let mut targets = Vec::new();
        for (i, f) in formats.iter().enumerate() {
            if f.mime.starts_with("text/plain") {
                for t in [a.UTF8_STRING, a.TEXT_PLAIN_UTF8, a.TEXT_PLAIN, a.TEXT] {
                    targets.push((t, i, false));
                }
                targets.push((AtomEnum::STRING.into(), i, true));
            }
            let atom = client.atom(&f.mime)?;
            if !targets.iter().any(|(t, _, _)| *t == atom) {
                targets.push((atom, i, false));
            }
        }
        let time = client.server_time()?;
        let primary: Atom = AtomEnum::PRIMARY.into();
        for sel in [client.atoms.CLIPBOARD, primary] {
            client.conn.set_selection_owner(client.win, sel, time)?;
        }
        let owner = client
            .conn
            .get_selection_owner(client.atoms.CLIPBOARD)?
            .reply()?
            .owner;
        if owner != client.win {
            bail!("another app kept the clipboard");
        }
        Ok(Self {
            client,
            formats,
            targets,
            time,
            transfers: HashMap::new(),
        })
    }

    fn run(mut self) -> Result<()> {
        loop {
            match self.client.conn.wait_for_event()? {
                Event::SelectionClear(e) if e.selection == self.client.atoms.CLIPBOARD => {
                    return Ok(());
                }
                Event::SelectionRequest(req) => {
                    let property = self.answer(&req)?;
                    let notify = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: req.time,
                        requestor: req.requestor,
                        selection: req.selection,
                        target: req.target,
                        property,
                    };
                    self.client
                        .conn
                        .send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
                    self.client.conn.flush()?;
                }
                Event::PropertyNotify(e) if e.state == Property::DELETE => {
                    self.next_chunk(e.window, e.atom)?;
                }
                // Errors (such as a requestor window that has gone) need no action.
                _ => {}
            }
        }
    }

    /// Put the requested data on the requestor's property; returns the property to report,
    /// or NONE to refuse.
    fn answer(&mut self, req: &SelectionRequestEvent) -> Result<Atom> {
        let conn = &self.client.conn;
        let a = &self.client.atoms;
        // Clients from before ICCCM leave the property empty and mean the target's name.
        let property = if req.property == NONE {
            req.target
        } else {
            req.property
        };
        if req.target == a.TARGETS {
            let mut list = vec![a.TARGETS, a.TIMESTAMP];
            list.extend(self.targets.iter().map(|(t, _, _)| *t));
            conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                property,
                AtomEnum::ATOM,
                &list,
            )?;
            return Ok(property);
        }
        if req.target == a.TIMESTAMP {
            conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                property,
                AtomEnum::INTEGER,
                &[self.time],
            )?;
            return Ok(property);
        }
        let Some(&(target, index, latin1)) =
            self.targets.iter().find(|(t, _, _)| *t == req.target)
        else {
            return Ok(NONE);
        };
        let latin1_data;
        let data = if latin1 {
            latin1_data = to_latin1(&self.formats[index].content);
            &latin1_data
        } else {
            &self.formats[index].content
        };
        if data.len() <= INCR_CHUNK {
            conn.change_property8(PropMode::REPLACE, req.requestor, property, target, data)?;
            return Ok(property);
        }
        if latin1 {
            // Too big to bother converting chunk by chunk; UTF8_STRING is offered too.
            return Ok(NONE);
        }
        // Chunked: announce the size, then send a chunk each time the requestor deletes the
        // property.
        conn.change_window_attributes(
            req.requestor,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            a.INCR,
            &[data.len() as u32],
        )?;
        self.transfers.insert(
            (req.requestor, property),
            Transfer {
                target,
                format: index,
                offset: 0,
            },
        );
        Ok(property)
    }

    fn next_chunk(&mut self, window: Window, property: Atom) -> Result<()> {
        let Some(t) = self.transfers.get_mut(&(window, property)) else {
            return Ok(());
        };
        let data = &self.formats[t.format].content;
        let end = (t.offset + INCR_CHUNK).min(data.len());
        let chunk = &data[t.offset..end];
        self.client
            .conn
            .change_property8(PropMode::REPLACE, window, property, t.target, chunk)?;
        if chunk.is_empty() {
            self.transfers.remove(&(window, property));
        } else {
            t.offset = end;
        }
        self.client.conn.flush()?;
        Ok(())
    }
}

fn to_latin1(utf8: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(utf8)
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

// Keysyms
const XK_SHIFT_L: u32 = 0xffe1;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_INSERT: u32 = 0xff63;
const XK_V: u32 = 0x0076;
/// Super, Alt, Meta and Hyper, left and right: keys a shortcut may still be holding.
const HELD_MODIFIERS: [u32; 8] = [0xffeb, 0xffec, 0xffe9, 0xffea, 0xffe7, 0xffe8, 0xffed, 0xffee];

/// Press the paste keys through XTEST, once keys other than modifiers are released (or after
/// `KEY_RELEASE_WAIT`). With `release_modifiers`, Super, Alt, Meta and Hyper
/// keys that are still down are released first, so the paste isn't read as another shortcut.
pub fn paste(keys: PasteKeys, release_modifiers: bool) -> Result<()> {
    let (conn, screen) = x11rb::connect(None).context("could not connect to the X display")?;
    let root = conn.setup().roots[screen].root;
    conn.extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)?
        .ok_or_else(|| anyhow!("the X server has no XTEST extension"))?;
    let min = conn.setup().min_keycode;
    let max = conn.setup().max_keycode;
    let map = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = map.keysyms_per_keycode as usize;
    let keycodes = |sym: u32| -> Vec<Keycode> {
        map.keysyms
            .chunks(per.max(1))
            .enumerate()
            .filter(|(_, syms)| syms.contains(&sym))
            .map(|(i, _)| min + i as u8)
            .collect()
    };
    let keycode = |sym: u32| -> Result<Keycode> {
        keycodes(sym)
            .first()
            .copied()
            .ok_or_else(|| anyhow!("no key for keysym {sym:#x} in the keyboard map"))
    };
    let fake = |kind: u8, code: Keycode| conn.xtest_fake_input(kind, code, CURRENT_TIME, root, 0, 0, 0);

    // While a shortcut's key (the V of Super+V) is held, the window manager's key grab
    // receives every key, so the paste would go to it instead of the focused app. Wait for
    // the non-modifier keys to be released.
    let modifier_keys = conn.get_modifier_mapping()?.reply()?.keycodes;
    let deadline = Instant::now() + KEY_RELEASE_WAIT;
    let mut down = conn.query_keymap()?.reply()?.keys;
    let is_down = |down: &[u8; 32], k: Keycode| down[k as usize / 8] & (1 << (k % 8)) != 0;
    while (min..=max).any(|k| is_down(&down, k) && !modifier_keys.contains(&k))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
        down = conn.query_keymap()?.reply()?.keys;
    }
    if release_modifiers {
        let is_down = |k: Keycode| is_down(&down, k);
        for sym in HELD_MODIFIERS {
            for k in keycodes(sym).into_iter().filter(|k| is_down(*k)) {
                fake(KEY_RELEASE_EVENT, k)?;
            }
        }
    }
    let seq: Vec<Keycode> = match keys {
        PasteKeys::ShiftInsert => vec![keycode(XK_SHIFT_L)?, keycode(XK_INSERT)?],
        PasteKeys::CtrlV => vec![keycode(XK_CONTROL_L)?, keycode(XK_V)?],
        PasteKeys::CtrlShiftV => vec![
            keycode(XK_CONTROL_L)?,
            keycode(XK_SHIFT_L)?,
            keycode(XK_V)?,
        ],
    };
    for k in &seq {
        fake(KEY_PRESS_EVENT, *k)?;
    }
    for k in seq.iter().rev() {
        fake(KEY_RELEASE_EVENT, *k)?;
    }
    // A round trip makes sure the server has processed the keys before we disconnect.
    conn.get_input_focus()?.reply()?;
    Ok(())
}

/// Ask the window manager to raise and focus `window`. A request from an app with no recent
/// input would be refused by focus-stealing prevention, so this asks as a pager (source 2)
/// with the current server time.
pub fn activate(window: Window) -> Result<()> {
    let c = Client::connect()?;
    let time = c.server_time()?;
    let active = c.atom("_NET_ACTIVE_WINDOW")?;
    let event = ClientMessageEvent::new(32, window, active, [2, time, 0, 0, 0]);
    c.conn.send_event(
        false,
        c.root,
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
        event,
    )?;
    c.conn.flush()?;
    Ok(())
}

/// The monitor under the mouse pointer, as (x, y, width, height) in pixels.
pub fn pointer_monitor() -> Option<(i16, i16, u16, u16)> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    let pointer = conn.query_pointer(root).ok()?.reply().ok()?;
    let (px, py) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
    let monitors = conn.randr_get_monitors(root, true).ok()?.reply().ok()?.monitors;
    monitors
        .iter()
        .find(|m| {
            let (x, y) = (i32::from(m.x), i32::from(m.y));
            (x..x + i32::from(m.width)).contains(&px) && (y..y + i32::from(m.height)).contains(&py)
        })
        .or_else(|| monitors.iter().find(|m| m.primary))
        .map(|m| (m.x, m.y, m.width, m.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_framing_round_trips() {
        let formats = vec![
            Format {
                mime: "text/plain;charset=utf-8".into(),
                content: b"two\nlines\n".to_vec(),
            },
            Format {
                mime: "text/html".into(),
                content: Vec::new(),
            },
            Format {
                mime: "image/png".into(),
                content: vec![0, 10, 255, 10],
            },
        ];
        let mut buf = Vec::new();
        encode_formats(&formats, &mut buf);
        let back = decode_formats(&buf).unwrap();
        assert_eq!(back.len(), 3);
        for (a, b) in formats.iter().zip(&back) {
            assert_eq!(a.mime, b.mime);
            assert_eq!(a.content, b.content);
        }
        assert!(decode_formats(b"").is_err());
        assert!(decode_formats(b"text/plain\n10\nshort").is_err());
    }

    #[test]
    fn latin1_replaces_wide_chars() {
        assert_eq!(to_latin1("café τ".as_bytes()), b"caf\xe9 ?");
    }
}
