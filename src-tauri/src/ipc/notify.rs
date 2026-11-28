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
pub(crate) fn hide_notify_card(app: AppHandle) {
    crate::notify::hide_notify_card(app)
}

#[tauri::command]
pub(crate) fn focus_main_from_notify(app: AppHandle) {
    crate::notify::focus_main_from_notify(app)
}

#[tauri::command]
pub(crate) fn refresh_notify_backdrop(app: AppHandle) -> Option<String> {
    crate::notify::refresh_notify_backdrop(app)
}

/// Start the pet's window drag (the window itself is decided in Rust, so the
/// command takes no arguments from the document).
#[tauri::command]
pub(crate) fn pet_begin_drag(window: WebviewWindow) {
    crate::notify::pet_begin_drag(window)
}

/// Push the pet's mood (`idle` / `thinking` / `done` / `error` / `ask` /
/// `dreaming`) from the main window to the companion window.
#[tauri::command]
pub(crate) fn set_companion_state(app: AppHandle, state: String) {
    crate::notify::set_companion_state(&app, state)
}
