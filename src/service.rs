//! The Unix socket between `clippo watch` and the other commands.
//!
//! One request line per connection, one reply line back. Replies start with `ok` (optionally
//! followed by fields) or `error <message>`.
//!
//! | Request | Reply | Effect |
//! |---|---|---|
//! | `ocr` | `ok` | Wake the OCR worker (a new image was stored) |
//! | `paste <keys> <delay_ms> <release>` | `ok` | Press the paste keys through the service's uinput keyboard |
//! | `copy <id> [plain\|nomd]` | `ok` | Put an entry on the clipboard, served with every stored format |
//! | `status` | `ok ocr=<engine\|none> watching=1 items=<n> pending=<n>` | Engine in use and counts |
//! | `reload` | `ok` | Re-read the config file |

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, RwLock, mpsc};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::clipboard::{self, CopyMode};
use crate::config::{Config, PasteConfig, Paths};
use crate::ocr;
use crate::paste::{self, Keyboard, NEW_DEVICE_SETTLE};
use crate::store::Store;

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The service could not be reached (not running, or its socket is gone). Callers that have
/// a fallback check for this; any other error came from the service itself.
#[derive(Debug)]
pub struct Unavailable(std::io::Error);

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "clippo service not running ({})", self.0)
    }
}

impl std::error::Error for Unavailable {}

pub fn socket_path(paths: &Paths) -> PathBuf {
    // Kept beside (not inside) the runtime dir so older clients keep finding it.
    paths.runtime_dir.with_file_name("clippo.sock")
}

/// Send one request and return the reply's fields after `ok`.
pub fn request(paths: &Paths, line: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket_path(paths)).map_err(Unavailable)?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.write_all(format!("{line}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let reply = reply.trim();
    match reply.split_once(' ') {
        _ if reply == "ok" => Ok(String::new()),
        Some(("ok", rest)) => Ok(rest.to_string()),
        Some(("error", msg)) => Err(anyhow!("{msg}")),
        _ if reply.is_empty() => Err(anyhow!("no reply from the clippo service")),
        _ => Err(anyhow!("unexpected reply from the clippo service: {reply}")),
    }
}

/// Tell the running `clippo watch` there is a new image to OCR. Does nothing if it isn't running.
pub fn notify_ocr(paths: &Paths) {
    if let Ok(mut stream) = UnixStream::connect(socket_path(paths)) {
        let _ = stream.write_all(b"ocr\n");
    }
}

/// Press the paste keys through the service's uinput keyboard.
pub fn paste(paths: &Paths, cfg: &PasteConfig) -> Result<()> {
    request(paths, &paste::encode(cfg)).map(drop)
}

/// Ask the service to put an entry on the clipboard with all its formats.
pub fn copy_entry(paths: &Paths, id: i64, mode: CopyMode) -> Result<()> {
    let line = match mode.as_word() {
        Some(word) => format!("copy {id} {word}"),
        None => format!("copy {id}"),
    };
    request(paths, &line).map(drop)
}

pub fn reload(paths: &Paths) -> Result<()> {
    request(paths, "reload").map(drop)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Engine the service would use for new images, if any.
    pub ocr: Option<String>,
    pub items: usize,
    pub pending_ocr: usize,
}

impl Status {
    fn encode(&self) -> String {
        format!(
            "ocr={} watching=1 items={} pending={}",
            self.ocr.as_deref().unwrap_or("none"),
            self.items,
            self.pending_ocr
        )
    }

    fn parse(fields: &str) -> Result<Self> {
        let mut status = Self {
            ocr: None,
            items: 0,
            pending_ocr: 0,
        };
        for field in fields.split_whitespace() {
            match field.split_once('=') {
                Some(("ocr", "none")) => status.ocr = None,
                Some(("ocr", v)) => status.ocr = Some(v.to_string()),
                Some(("items", v)) => status.items = v.parse()?,
                Some(("pending", v)) => status.pending_ocr = v.parse()?,
                _ => {}
            }
        }
        Ok(status)
    }
}

pub fn status(paths: &Paths) -> Result<Status> {
    Status::parse(&request(paths, "status")?)
}

/// State shared between the socket loop and the OCR worker.
pub struct Shared {
    pub cfg: RwLock<Config>,
    pub paths: Paths,
}

impl Shared {
    pub fn config(&self) -> Config {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Serve requests from other clippo commands. Runs until the process exits.
pub fn serve(shared: Arc<Shared>, wake_ocr: mpsc::Sender<()>) -> Result<()> {
    let path = socket_path(&shared.paths);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("could not listen on {}", path.display()))?;
    let mut server = Server {
        shared,
        wake_ocr,
        kb: None,
        store: None,
    };
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut line = String::new();
        let reply = match BufReader::new(&stream).read_line(&mut line) {
            Ok(_) => match server.handle(line.trim()) {
                Ok(fields) if fields.is_empty() => "ok".to_string(),
                Ok(fields) => format!("ok {fields}"),
                Err(e) => format!("error {e:#}"),
            },
            Err(e) => format!("error {e}"),
        };
        // One reply line: a TOML parse error, for one, spans several.
        let reply: String = reply.split_whitespace().collect::<Vec<_>>().join(" ");
        let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
    }
    Err(anyhow!("clippo socket closed"))
}

struct Server {
    shared: Arc<Shared>,
    wake_ocr: mpsc::Sender<()>,
    /// Only created when a uinput paste is first requested.
    kb: Option<Keyboard>,
    /// Opened on the first request that needs it.
    store: Option<Store>,
}

impl Server {
    fn store(&mut self) -> Result<&Store> {
        if self.store.is_none() {
            self.store = Some(Store::open(&self.shared.paths.db)?);
        }
        Ok(self.store.as_ref().expect("opened above"))
    }

    fn handle(&mut self, line: &str) -> Result<String> {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("ocr") => {
                let _ = self.wake_ocr.send(());
                Ok(String::new())
            }
            Some("paste") => {
                let cfg = paste::decode(line).ok_or_else(|| anyhow!("bad paste request"))?;
                self.paste(&cfg)?;
                Ok(String::new())
            }
            Some("copy") => {
                let id: i64 = words
                    .next()
                    .and_then(|w| w.parse().ok())
                    .ok_or_else(|| anyhow!("bad copy request"))?;
                let mode = CopyMode::parse(words.next()).ok_or_else(|| anyhow!("bad copy request"))?;
                let formats = clipboard::entry_formats(self.store()?, id, mode)?;
                clipboard::copy_formats(&formats)?;
                Ok(String::new())
            }
            Some("status") => {
                let cfg = self.shared.config();
                let ocr = ocr::available(&cfg.ocr, &self.shared.paths).map(str::to_string);
                let (items, pending_ocr) = self.store()?.counts()?;
                Ok(Status {
                    ocr,
                    items,
                    pending_ocr,
                }
                .encode())
            }
            Some("reload") => {
                let cfg = Config::load(&self.shared.paths)?;
                *self.shared.cfg.write().unwrap_or_else(|e| e.into_inner()) = cfg;
                let _ = self.wake_ocr.send(());
                Ok(String::new())
            }
            _ => bail!("bad request"),
        }
    }

    fn paste(&mut self, cfg: &PasteConfig) -> Result<()> {
        match &mut self.kb {
            Some(kb) => kb.paste(cfg),
            None => {
                let mut new = Keyboard::new()?;
                // A new keyboard needs time before the compositor reads it.
                sleep(NEW_DEVICE_SETTLE);
                let r = new.paste(cfg);
                self.kb = Some(new);
                r
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips() {
        let s = Status {
            ocr: Some("ocrs".into()),
            items: 12,
            pending_ocr: 3,
        };
        assert_eq!(s.encode(), "ocr=ocrs watching=1 items=12 pending=3");
        assert_eq!(Status::parse(&s.encode()).unwrap(), s);
        let none = Status {
            ocr: None,
            ..s
        };
        assert_eq!(Status::parse(&none.encode()).unwrap(), none);
    }
}
