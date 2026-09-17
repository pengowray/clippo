use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Run `wl-paste` with `args`; `None` if it fails (e.g. empty clipboard or type not offered).
pub fn paste(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Types offered by the current clipboard owner.
pub fn list_types() -> Vec<String> {
    let raw = paste(&["--list-types"]).unwrap_or_default();
    String::from_utf8_lossy(&raw)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Put `data` on the clipboard. With no `mime`, wl-copy offers the usual set of text types.
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
