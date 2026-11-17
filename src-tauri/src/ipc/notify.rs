//! Commands for `notify` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use tauri::AppHandle;
use tauri::WebviewWindow;

#[tauri::command]
pub(crate) async fn show_notify_card(
    app: AppHandle,
    title: String,
    body: String,
    kind: String,
) -> Result<(), String> {
    crate::notify::show_notify_card(app, title, body, kind).await
}

#[tauri::command]
pub(crate) fn hide_notify_card(window: WebviewWindow) {
    crate::notify::hide_notify_card(window)
}

#[tauri::command]
pub(crate) fn focus_main_from_notify(app: AppHandle, window: WebviewWindow) {
    crate::notify::focus_main_from_notify(app, window)
}

#[tauri::command]
pub(crate) fn refresh_notify_backdrop(app: AppHandle) -> Option<String> {
    crate::notify::refresh_notify_backdrop(app)
}
