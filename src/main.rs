mod clipboard;
mod config;
mod ingest;
mod macros;
mod markdown;
mod menu;
mod ocr;
mod paste;
mod plain;
mod service;
mod skip;
mod store;
mod thumbs;
mod vkbd;
mod watch;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};

use clipboard::CopyMode;
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
        /// Also remove Markdown syntax if the text looks like Markdown, even if `plain.strip_markdown` is off
        #[arg(long, conflicts_with = "no_markdown")]
        markdown: bool,
        /// Keep Markdown syntax as it is
        #[arg(long)]
        no_markdown: bool,
    },
    /// Put an entry on the clipboard without pasting, with all its stored formats
    Copy {
        id: i64,
        /// Offer only plain text (for images, the recognised text)
        #[arg(long, conflicts_with = "no_markdown")]
        plain: bool,
        /// Offer only plain text with Markdown syntax removed
        #[arg(long)]
        no_markdown: bool,
    },
    /// Write an entry's content to stdout
    Get {
        id: i64,
        /// For images, output the recognised text instead of image data
        #[arg(long)]
        plain: bool,
        /// Output the text with Markdown syntax removed
        #[arg(long)]
        strip_markdown: bool,
    },
    /// List entries, newest first, as "id<TAB>label"
    List {
        /// Only entries used in the last 24 hours
        #[arg(long)]
        recent: bool,
    },
    /// Delete one entry (undo with `undelete` within a minute)
    Delete { id: i64 },
    /// Bring back an entry deleted in the last minute
    Undelete { id: i64 },
    /// Delete all entries
    Clear,
    /// Paste the current time or date (see `[[macros.items]]` in the config)
    Macro {
        /// Macro number, starting at 1
        #[arg(required_unless_present = "list")]
        n: Option<usize>,
        /// Show the macros and what each would paste now
        #[arg(long)]
        list: bool,
    },
    /// Show whether the clippo service is running, its OCR engine and entry counts
    Status,
    /// Make the running clippo service re-read the config file
    Reload,
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
        Cmd::Plain {
            paste,
            no_paste,
            markdown,
            no_markdown,
        } => {
            let strip = match (markdown, no_markdown) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            plain::run(&cfg, &paths, paste || (cfg.paste.auto_paste && !no_paste), strip)
        }
        Cmd::Copy {
            id,
            plain,
            no_markdown,
        } => {
            let mode = match (plain, no_markdown) {
                (true, _) => CopyMode::Plain,
                (_, true) => CopyMode::NoMarkdown,
                _ => CopyMode::Full,
            };
            clipboard::copy_entry(&paths, &Store::open(&paths.db)?, id, mode)
        }
        Cmd::Get {
            id,
            plain,
            strip_markdown,
        } => get(&cfg, &paths, id, plain, strip_markdown),
        Cmd::List { recent } => {
            let store = Store::open(&paths.db)?;
            let entries = if recent {
                store.list_since(Store::recent_cutoff())?
            } else {
                store.list()?
            };
            let mut out = std::io::stdout().lock();
            for entry in entries {
                writeln!(out, "{}\t{}", entry.id, menu::label(&entry))?;
            }
            Ok(())
        }
        Cmd::Delete { id } => {
            if !Store::open(&paths.db)?.delete(id)? {
                return Err(anyhow!("no entry with id {id}"));
            }
            // The thumbnail stays until the entry is purged, in case of undelete.
            println!("Deleted entry {id}");
            Ok(())
        }
        Cmd::Undelete { id } => {
            if !Store::open(&paths.db)?.undelete(id)? {
                return Err(anyhow!("no deleted entry with id {id}"));
            }
            println!("Restored entry {id}");
            Ok(())
        }
        Cmd::Clear => {
            let n = Store::open(&paths.db)?.clear()?;
            thumbs::remove_all(&paths.thumbs_dir);
            println!("Deleted {n} {}", if n == 1 { "entry" } else { "entries" });
            Ok(())
        }
        Cmd::Macro { n, list } => {
            if list {
                let mut out = std::io::stdout().lock();
                for line in macros::list(&cfg) {
                    writeln!(out, "{line}")?;
                }
                return Ok(());
            }
            macros::run(&cfg, &paths, n.expect("required unless --list"))
        }
        Cmd::Status => {
            match service::status(&paths) {
                Ok(s) => {
                    println!("Service: running");
                    let engine = match s.ocr.as_deref() {
                        Some(engine) => engine,
                        None if cfg.ocr.engine == config::OcrEngineKind::Off => "off",
                        None => "none (run `clippo setup-ocr` or install tesseract)",
                    };
                    println!("OCR engine: {engine}");
                    println!("Entries: {}", s.items);
                    println!("Images waiting for OCR: {}", s.pending_ocr);
                }
                Err(e) if e.is::<service::Unavailable>() => {
                    println!("Service: not running (start it with `systemctl --user start clippo`)");
                    let (items, pending) = Store::open(&paths.db)?.counts()?;
                    println!("Entries: {items}");
                    println!("Images waiting for OCR: {pending}");
                }
                Err(e) => return Err(e),
            }
            Ok(())
        }
        Cmd::Reload => {
            service::reload(&paths)?;
            println!("Config reloaded");
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

fn get(cfg: &Config, paths: &Paths, id: i64, plain: bool, strip_markdown: bool) -> Result<()> {
    let store = Store::open(&paths.db)?;
    let entry = store
        .summary(id)?
        .ok_or_else(|| anyhow!("no entry with id {id}"))?;
    let mut out = std::io::stdout().lock();
    if entry.is_image() {
        if !plain && !strip_markdown {
            let content = store
                .content(id)?
                .ok_or_else(|| anyhow!("no entry with id {id}"))?;
            return Ok(out.write_all(&content)?);
        }
        let text = match (entry.ocr_status, entry.ocr_text) {
            (OcrStatus::Done, Some(text)) => text,
            _ => {
                let backend = ocr::backend(&cfg.ocr, paths)?.ok_or_else(ocr::no_engine_error)?;
                ocr::ocr_entry(&store, backend.as_ref(), id)?
            }
        };
        // Recognised text is never Markdown; --strip-markdown just means "text, please".
        out.write_all(text.as_bytes())?;
    } else {
        let content = store
            .content(id)?
            .ok_or_else(|| anyhow!("no entry with id {id}"))?;
        if strip_markdown {
            let text = markdown::strip(&String::from_utf8_lossy(&content));
            out.write_all(text.as_bytes())?;
        } else {
            out.write_all(&content)?;
        }
    }
    Ok(())
}
