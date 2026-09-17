//! Talking to the running `clippo watch` over its socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

// TODO(backend): use `paste::socket_path()` once main makes it public; same location.
fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("clippo.sock")
}

pub struct Status {
    pub running: bool,
    /// OCR is on but no engine is installed (design 7.3).
    pub ocr_engine_missing: bool,
}

/// One request line, one reply line.
pub fn request(line: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(format!("{line}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply.trim().to_string())
}

pub fn status() -> Status {
    // TODO(backend): parse the `status` reply (`ocr=<engine|none> watching=1`) once main
    // adds it. Until then a reachable socket means running, and the engine check is local.
    match request("status") {
        Ok(reply) => Status {
            running: true,
            ocr_engine_missing: reply.split_whitespace().any(|kv| kv == "ocr=none"),
        },
        Err(_) => Status {
            running: false,
            ocr_engine_missing: false,
        },
    }
}

/// Ask the service to re-read its config. `false` if it is not reachable.
pub fn reload() -> bool {
    request("reload").is_ok_and(|r| r == "ok")
}

/// Ask a resident window to show or hide. `false` if there is no service to ask.
pub fn menu_toggle() -> bool {
    request("menu toggle").is_ok_and(|r| r == "ok")
}
