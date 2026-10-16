//! Desktop notifications for what happens after the window has closed (design 7.4).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cosmic::app::Task;
use cosmic::iced::futures::{StreamExt, stream};

use crate::clipboard;
use crate::save;

use crate::config::PasteKeys;
use crate::ui::app::{JobError, Message};
use crate::ui::strings;

fn key_name(keys: PasteKeys) -> &'static str {
    match keys {
        PasteKeys::ShiftInsert => "Shift+Insert",
        PasteKeys::CtrlV => "Ctrl+V",
        PasteKeys::CtrlShiftV => "Ctrl+Shift+V",
    }
}

/// How long a notification's button stays usable (from the notification history too).
const ACTION_WAIT: Duration = Duration::from_secs(30 * 60);

fn notify_args(
    icon: &str,
    title: String,
    body: String,
    actions: Vec<String>,
) -> (
    &'static str,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, zbus::zvariant::Value<'static>>,
    i32,
) {
    ("clippo", 0, icon.into(), title, body, actions, HashMap::new(), -1)
}

async fn proxy() -> zbus::Result<zbus::Proxy<'static>> {
    let conn = zbus::Connection::session().await?;
    zbus::Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
}

/// Send a notification over `org.freedesktop.Notifications`.
pub async fn send(title: String, body: String) -> zbus::Result<()> {
    let p = proxy().await?;
    let _: u32 = p
        .call("Notify", &notify_args("edit-paste-symbolic", title, body, Vec::new()))
        .await?;
    Ok(())
}

/// Send a notification with one button, and wait until the button is pressed (true) or the
/// notification is dismissed or `ACTION_WAIT` passes (false).
async fn send_with_button(
    icon: &str,
    title: String,
    body: String,
    action: &str,
    label: &str,
) -> zbus::Result<bool> {
    let p = proxy().await?;
    // Subscribe first, so a quick press cannot arrive before we listen.
    let invoked = p.receive_signal("ActionInvoked").await?;
    let closed = p.receive_signal("NotificationClosed").await?;
    let actions = vec![action.to_string(), label.to_string()];
    let id: u32 = p.call("Notify", &notify_args(icon, title, body, actions)).await?;
    let events = stream::select(
        invoked.filter_map(|m| async move {
            let (n, key): (u32, String) = m.body().deserialize().ok()?;
            Some((n, Some(key)))
        }),
        closed.filter_map(|m| async move {
            let (n, _reason): (u32, u32) = m.body().deserialize().ok()?;
            Some((n, None))
        }),
    );
    let mut events = std::pin::pin!(events);
    let wait = async {
        while let Some((n, key)) = events.next().await {
            if n != id {
                continue;
            }
            return key.as_deref() == Some(action);
        }
        false
    };
    Ok(tokio::time::timeout(ACTION_WAIT, wait).await.unwrap_or(false))
}

/// After a save from the dialog: name the file, and offer to copy its path.
pub fn image_saved(path: PathBuf) -> Task<Message> {
    cosmic::iced::Task::future(async move {
        let body = save::display_path(&path);
        match send_with_button(
            "document-save-symbolic",
            strings::NOTIFY_IMAGE_SAVED_TITLE.into(),
            body,
            "copy-path",
            strings::COPY_PATH,
        )
        .await
        {
            Ok(true) => {
                let text = path.to_string_lossy().into_owned();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    clipboard::copy(None, text.as_bytes())
                })
                .await
                .unwrap_or_else(|e| Err(e.into()))
                {
                    crate::log(&format!("copy path failed: {e:#}"));
                }
            }
            Ok(false) => {}
            Err(e) => crate::log(&format!("notification failed: {e}")),
        }
    })
    .discard()
}

/// A save from the dialog failed after the window closed.
pub fn save_failed(err: String) -> Task<Message> {
    cosmic::iced::Task::future(async move {
        if let Err(e) = send(strings::NOTIFY_SAVE_FAILED_TITLE.into(), err).await {
            crate::log(&format!("notification failed: {e}"));
        }
    })
    .discard()
}

/// Notify about a failed copy or paste; success is silent.
pub fn after_close(result: Result<(), JobError>, keys: PasteKeys) -> Task<Message> {
    let (title, body) = match result {
        Ok(()) => return Task::none(),
        Err(JobError::Copy(e)) => (strings::NOTIFY_COPY_FAILED_TITLE.to_string(), e),
        Err(JobError::Paste(e)) => (
            strings::NOTIFY_PASTE_FAILED_TITLE.to_string(),
            strings::notify_paste_failed_body(&e, key_name(keys)),
        ),
    };
    cosmic::iced::Task::future(async move {
        if let Err(e) = send(title, body).await {
            crate::log(&format!("notification failed: {e}"));
        }
    })
    .discard()
}
