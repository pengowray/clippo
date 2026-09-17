mod clipboard;
mod config;
mod ingest;
mod menu;
mod ocr;
mod paste;
mod plain;
mod store;
mod thumbs;
mod watch;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};

use config::{Config, Paths};
use store::{OcrStatus, Store};

#[derive(Parser)]
#[command(
    name = "clippo",
    version,
    about = "Clipboard history for Wayland, with OCR for images"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Watch the clipboard and record history (runs until stopped)
    Watch,
    /// Store clipboard data from stdin (called by `wl-paste --watch`)
    Ingest,
    /// Open the history picker; selecting an entry copies it, and pastes it if `paste.paste_on_select` is on
    Menu,
    /// Replace the clipboard with plain text (recognised text for images), then paste it
    Plain {
        /// Paste after replacing the clipboard, even if `paste.auto_paste` is off
        #[arg(long, conflicts_with = "no_paste")]
        paste: bool,
        /// Only replace the clipboard; don't paste
        #[arg(long)]
        no_paste: bool,
    },
    /// Write an entry's content to stdout
    Get {
        id: i64,
        /// For images, output the recognised text instead of image data
        #[arg(long)]
        plain: bool,
    },
    /// List entries, newest first, as "id<TAB>label"
    List,
    /// Delete one entry
    Delete { id: i64 },
    /// Delete all entries
    Clear,
    /// Recognise text in an image file ("-" for stdin)
    Ocr { file: PathBuf },
    /// Download the ocrs OCR models
    SetupOcr,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("clippo: {e:#}");
            log(&format!("error: {e:#}"));
            ExitCode::FAILURE
        }
    }
}

/// Append a line to `$XDG_STATE_HOME/clippo/clippo.log`. Shortcuts run clippo with nowhere to show stderr.
pub fn log(msg: &str) {
    use std::io::Write;
    let Some(dirs) = directories::ProjectDirs::from("", "", "clippo") else {
        return;
    };
    let Some(dir) = dirs.state_dir() else { return };
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("clippo.log"))
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let args: Vec<String> = std::env::args().skip(1).collect();
        let _ = writeln!(f, "{secs} [{}] {msg}", args.join(" "));
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::resolve()?;
    let cfg = Config::load(&paths)?;
    match cli.command {
        Cmd::Watch => watch::run(&cfg, &paths),
        Cmd::Ingest => ingest::run(&cfg, &paths),
        Cmd::Menu => menu::run(&cfg, &Store::open(&paths.db)?, &paths),
        Cmd::Plain { paste, no_paste } => {
            plain::run(&cfg, &paths, paste || (cfg.paste.auto_paste && !no_paste))
        }
        Cmd::Get { id, plain } => get(&cfg, &paths, id, plain),
        Cmd::List => {
            let store = Store::open(&paths.db)?;
            let mut out = std::io::stdout().lock();
            for entry in store.list()? {
                writeln!(out, "{}\t{}", entry.id, menu::label(&entry))?;
            }
            Ok(())
        }
        Cmd::Delete { id } => {
            if !Store::open(&paths.db)?.delete(id)? {
                return Err(anyhow!("no entry with id {id}"));
            }
            thumbs::remove(&paths.thumbs_dir, &[id]);
            println!("Deleted entry {id}");
            Ok(())
        }
        Cmd::Clear => {
            let n = Store::open(&paths.db)?.clear()?;
            thumbs::remove_all(&paths.thumbs_dir);
            println!("Deleted {n} {}", if n == 1 { "entry" } else { "entries" });
            Ok(())
        }
        Cmd::Ocr { file } => {
            let data = if file.as_os_str() == "-" {
                let mut buf = Vec::new();
                std::io::stdin().read_to_end(&mut buf)?;
                buf
            } else {
                std::fs::read(&file)
                    .with_context(|| format!("could not read {}", file.display()))?
            };
            let backend = ocr::backend(&cfg.ocr, &paths)?.ok_or_else(ocr::no_engine_error)?;
            println!("{}", backend.recognize(&data)?);
            Ok(())
        }
        Cmd::SetupOcr => ocr::setup(&paths),
    }
}

fn get(cfg: &Config, paths: &Paths, id: i64, plain: bool) -> Result<()> {
    let store = Store::open(&paths.db)?;
    let entry = store
        .summary(id)?
        .ok_or_else(|| anyhow!("no entry with id {id}"))?;
    let mut out = std::io::stdout().lock();
    if plain && entry.is_image() {
        let text = match (entry.ocr_status, entry.ocr_text) {
            (OcrStatus::Done, Some(text)) => text,
            _ => {
                let backend = ocr::backend(&cfg.ocr, paths)?.ok_or_else(ocr::no_engine_error)?;
                ocr::ocr_entry(&store, backend.as_ref(), id)?
            }
        };
        out.write_all(text.as_bytes())?;
    } else {
        let content = store
            .content(id)?
            .ok_or_else(|| anyhow!("no entry with id {id}"))?;
        out.write_all(&content)?;
    }
    Ok(())
}
