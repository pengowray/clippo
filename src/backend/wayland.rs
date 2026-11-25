//! Wayland: the clipboard through `wl-clipboard` and `wl-clipboard-rs`, which need a
//! compositor with a data-control protocol, and paste keys through the virtual keyboard
//! protocol (`vkbd.rs`).
//!
//! Single-type copies go through `wl-copy`, which forks a process that serves the clipboard
//! until someone else copies. A copy that offers several types (the primary plus stored HTML
//! or RTF) uses `wl-clipboard-rs` instead: `wl-copy` serves one type per process and running
//! one per type does not work, as each takes ownership from the last. `wl-clipboard-rs` 0.9
//! binds `ext_data_control_v1` when the compositor has it and falls back to
//! `zwlr_data_control_v1`, so it works on COSMIC either way. Its source lives in a thread of
//! the calling process, so the resident `clippo watch` does the serving (see `service::serve`)
//! and the clipboard is lost if the service restarts, unlike `wl-copy`'s fork.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

use super::Watcher;
use crate::store::Format;

/// How long to wait for the compositor to report our multi-type source as the owner.
const OWNERSHIP_WAIT: Duration = Duration::from_millis(500);

/// Run `wl-paste` with `args`; `None` if it fails (e.g. empty clipboard or type not offered).
fn wl_paste(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

pub fn read(mime: &str) -> Option<Vec<u8>> {
    wl_paste(&["--no-newline", "--type", mime])
}

pub fn read_text() -> Option<Vec<u8>> {
    wl_paste(&["--no-newline", "--type", "text"])
}

pub fn list_types() -> Vec<String> {
    let raw = wl_paste(&["--list-types"]).unwrap_or_default();
    String::from_utf8_lossy(&raw)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Put `data` on the clipboard through `wl-copy`. With no `mime`, wl-copy offers the usual set
/// of text types.
pub fn copy(mime: Option<&str>, data: &[u8]) -> Result<()> {
    let mut cmd = Command::new("wl-copy");
    if let Some(mime) = mime {
        cmd.args(["--type", mime]);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .spawn()
        .context("could not run wl-copy")?;
    child.stdin.take().expect("piped stdin").write_all(data)?;
    // wl-copy forks to serve the clipboard, so this returns once it has taken ownership.
    if !child.wait()?.success() {
        bail!("wl-copy failed");
    }
    Ok(())
}

/// Offer every format at once, from a thread of this process that serves until another
/// client takes the clipboard. The first format is the primary. Returns once the compositor
/// reports the new offer, so a paste keystroke sent afterwards gets this content.
pub fn copy_formats(formats: &[Format]) -> Result<()> {
    let primary = formats.first().ok_or_else(|| anyhow!("nothing to copy"))?;
    let sources = formats
        .iter()
        .map(|f| MimeSource {
            source: Source::Bytes(f.content.clone().into_boxed_slice()),
            mime_type: MimeType::Specific(f.mime.clone()),
        })
        .collect();
    Options::new()
        .copy_multi(sources)
        .map_err(|e| anyhow!("could not take the clipboard: {e}"))?;
    // copy_multi returns before its thread flushes set_selection. Waiting until the primary
    // type is offered catches the change for images and for text replacing an image; when
    // text replaces text the previous owner offers the same type, so this passes at once and
    // the remaining race is the thread's first flush (microseconds; the menu also waits 50 ms
    // before pasting).
    let start = Instant::now();
    loop {
        if list_types().contains(&primary.mime) {
            return Ok(());
        }
        if start.elapsed() > OWNERSHIP_WAIT {
            bail!("the compositor did not accept the clipboard offer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `wl-paste --watch clippo ingest`, which runs ingest for each copy. The process is started
/// here, so a missing `wl-paste` is reported before the service starts.
pub fn start_watch(exe: &Path) -> Result<Watcher> {
    let mut cmd = Command::new("wl-paste");
    cmd.arg("--watch").arg(exe).arg("ingest");
    cmd.env(super::OVERRIDE_VAR, super::Backend::Wayland.word());
    // SAFETY: prctl is async-signal-safe. It makes wl-paste exit when we do,
    // however we die.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("could not run wl-paste")?;
    Ok(Box::new(move || match child.wait() {
        Ok(s) => format!("clipboard watching stopped: wl-paste exited ({s})"),
        Err(e) => format!("clipboard watching stopped: {e}"),
    }))
}
