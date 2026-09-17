//! Desktop notifications for what happens after the window has closed (design 7.4).

use cosmic::app::Task;

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

/// Send a notification over `org.freedesktop.Notifications`.
pub async fn send(title: String, body: String) -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "clippo",
            0u32,
            "edit-paste-symbolic",
            title,
            body,
            Vec::<String>::new(),
            std::collections::HashMap::<String, zbus::zvariant::Value>::new(),
            -1i32,
        ),
    )
    .await?;
    Ok(())
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
