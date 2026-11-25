//! GNOME on Wayland: Mutter has no data-control protocol, so clippo goes through its GNOME
//! Shell extension (`gnome-extension/`), a D-Bus service inside the shell. The extension
//! signals clipboard changes, reads and sets the clipboard, presses paste keys, and places
//! clippo's window.
//!
//! The extension offers only the first format of a copy: GJS cannot implement a selection
//! source with several types, so stored HTML and RTF are not offered on GNOME.

use std::future::Future;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use futures_util::{FutureExt, StreamExt};

use super::{Backend, Watcher};
use crate::config::PasteKeys;
use crate::store::Format;

const NAME: &str = "io.github.pengowray.clippo.Shell";
const PATH: &str = "/io/github/pengowray/clippo/Shell";
const IFACE: &str = "io.github.pengowray.clippo.Shell1";

/// Run a D-Bus exchange to completion on a runtime of its own (callers are synchronous).
fn block_on<T>(f: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(f)
}

async fn proxy() -> Result<zbus::Proxy<'static>> {
    let conn = zbus::Connection::session().await?;
    Ok(zbus::Proxy::new(&conn, NAME, PATH, IFACE).await?)
}

/// Explain the usual failure: the extension is not installed or not enabled.
fn not_running(e: zbus::Error) -> anyhow::Error {
    let text = e.to_string();
    if text.contains("ServiceUnknown") || text.contains("NameHasNoOwner") {
        anyhow!(
            "the clippo GNOME Shell extension is not running: install and enable clippo@pengowray.github.io"
        )
    } else {
        anyhow::Error::from(e)
    }
}

async fn call<B, R>(method: &str, body: &B) -> Result<R>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    proxy().await?.call(method, body).await.map_err(not_running)
}

pub fn list_types() -> Vec<String> {
    block_on(call("ListTypes", &())).unwrap_or_default()
}

pub fn read(mime: &str) -> Option<Vec<u8>> {
    let data: Vec<u8> = block_on(call("Read", &(mime,))).ok()?;
    (!data.is_empty()).then_some(data)
}

pub fn read_text() -> Option<Vec<u8>> {
    let types = list_types();
    ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"]
        .into_iter()
        .find(|t| types.iter().any(|offered| offered == t))
        .and_then(read)
}

pub fn copy_formats(formats: &[Format]) -> Result<()> {
    let pairs: Vec<(&str, &[u8])> = formats
        .iter()
        .map(|f| (f.mime.as_str(), f.content.as_slice()))
        .collect();
    block_on(call::<_, ()>("SetContent", &(pairs,))).context("could not take the clipboard")
}

pub fn paste(keys: PasteKeys) -> Result<()> {
    let word = match keys {
        PasteKeys::ShiftInsert => "shift-insert",
        PasteKeys::CtrlV => "ctrl-v",
        PasteKeys::CtrlShiftV => "ctrl-shift-v",
    };
    block_on(call::<_, ()>("PressKeys", &(word,)))
}

/// Store the clipboard now and after each `OwnerChanged` signal. Until the extension is
/// running (at login the service can start before GNOME Shell loads extensions), wait for it.
pub fn start_watch(exe: &Path) -> Result<Watcher> {
    let exe = exe.to_path_buf();
    Ok(Box::new(move || {
        let watched = block_on(async {
            wait_for_extension().await?;
            let p = proxy().await?;
            let mut changes = p.receive_signal("OwnerChanged").await?;
            // Changes that arrive while one is being stored are folded into one more ingest,
            // as on X11.
            let ingest = |exe: std::path::PathBuf| {
                tokio::task::spawn_blocking(move || super::run_ingest(&exe, Backend::Gnome))
            };
            ingest(exe.clone()).await?;
            while changes.next().await.is_some() {
                while let Some(Some(_)) = changes.next().now_or_never() {}
                ingest(exe.clone()).await?;
            }
            Err::<(), _>(anyhow!("the clippo GNOME Shell extension stopped"))
        });
        match watched {
            Ok(()) => "clipboard watching stopped".into(),
            Err(e) => format!("clipboard watching stopped: {e:#}"),
        }
    }))
}

/// Is the extension running (its D-Bus name has an owner)?
pub fn extension_running() -> bool {
    block_on(async {
        let conn = zbus::Connection::session().await?;
        let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
        Ok(dbus.name_has_owner(zbus::names::BusName::try_from(NAME)?).await?)
    })
    .unwrap_or(false)
}

/// Return once the extension's D-Bus name has an owner, saying in the log that clippo waits.
async fn wait_for_extension() -> Result<()> {
    let conn = zbus::Connection::session().await?;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let name = zbus::names::BusName::try_from(NAME)?;
    let mut waiting = false;
    while !dbus.name_has_owner(name.clone()).await? {
        if !waiting {
            eprintln!(
                "clippo: waiting for the clippo GNOME Shell extension (clippo@pengowray.github.io) to start"
            );
            waiting = true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    if waiting {
        eprintln!("clippo: the clippo GNOME Shell extension is running");
    }
    Ok(())
}
