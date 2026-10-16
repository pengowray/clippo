//! The `Save image` submenu's dialog (design 6): the XDG portal's save dialog, shown after
//! the window has closed, because the window is an overlay above every other window.

use std::path::PathBuf;
use std::time::Duration;

use ashpd::desktop::ResponseError;
use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

use crate::save::{self, SaveFormat};
use crate::store::Store;
use crate::ui::strings;

/// Ask where to save entry `id` and write it there. `Ok(None)` when the dialog is cancelled.
pub async fn run(
    db: PathBuf,
    id: i64,
    mime: String,
    format: SaveFormat,
    close_wait: Duration,
) -> Result<Option<PathBuf>, String> {
    // Let the window unmap before the dialog maps.
    tokio::time::sleep(close_wait).await;
    let content = tokio::task::spawn_blocking(move || {
        Store::open(&db)
            .and_then(|s| s.content(id))
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())??
    .ok_or_else(|| strings::ENTRY_GONE.to_string())?;

    let filter = filter_for(&mime, format);
    let name = save::suggested_name(&mime, format, &chrono::Local::now());
    let request = SelectedFiles::save_file()
        .title(strings::SAVE_DIALOG_TITLE)
        .modal(true)
        .current_name(name.as_str())
        .filter(filter.clone())
        .current_filter(filter)
        .current_folder::<PathBuf>(save::default_dir())
        .map_err(|e| e.to_string())?;
    let selected = match request.send().await.and_then(|r| r.response()) {
        Ok(s) => s,
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let path = selected
        .uris()
        .first()
        .and_then(|u| u.to_file_path().ok())
        .ok_or_else(|| strings::NOT_A_LOCAL_FILE.to_string())?;

    let format = save::format_for_path(&mime, &path, format);
    let path = save::with_extension(path, &mime, format);
    tokio::task::spawn_blocking(move || {
        save::write(&path, &mime, &content, format)
            .map(|()| Some(path))
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn filter_for(mime: &str, format: SaveFormat) -> FileFilter {
    let (label, mime, ext) = match format {
        SaveFormat::Png => ("PNG".to_string(), "image/png", "png"),
        SaveFormat::Jpeg => ("JPG".to_string(), "image/jpeg", "jpg"),
        SaveFormat::Original => (
            save::format_label(mime),
            mime,
            save::extension_for_mime(mime),
        ),
    };
    FileFilter::new(&strings::save_filter(&label))
        .mimetype(mime)
        .glob(&format!("*.{ext}"))
}
