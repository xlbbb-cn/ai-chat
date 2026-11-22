//! Commands for `app` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::instance;
use crate::AppState;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) fn app_open_data_dir(app: AppHandle) {
    crate::app_open_data_dir(app)
}

#[tauri::command]
pub(crate) fn app_open_workspace_dir(app: AppHandle) -> Result<(), String> {
    crate::app_open_workspace_dir(app)
}

#[tauri::command]
pub(crate) fn app_open_workspace_tmp_dir(app: AppHandle) -> Result<(), String> {
    crate::app_open_workspace_tmp_dir(app)
}

#[tauri::command]
pub(crate) fn app_open_ws_data_dir(app: AppHandle) -> Result<(), String> {
    crate::app_open_ws_data_dir(app)
}

#[tauri::command]
pub(crate) fn app_open_session_deliver_dir(
    app: AppHandle,
    session_id: String,
) -> Result<(), String> {
    crate::app_open_session_deliver_dir(app, session_id)
}

#[tauri::command]
pub(crate) fn app_open_issue_page(app: AppHandle) -> Result<(), String> {
    crate::app_open_issue_page(app)
}

#[tauri::command]
pub(crate) fn app_open_agents_dir(app: AppHandle) -> Result<(), String> {
    crate::app_open_agents_dir(app)
}

#[tauri::command]
pub(crate) fn app_open_markdown(app: AppHandle, path: String) {
    crate::app_open_markdown(app, path)
}

#[tauri::command]
pub(crate) fn app_export_backup(app: AppHandle, path: String) -> Result<(), String> {
    crate::app_export_backup(app, path)
}

#[tauri::command]
pub(crate) async fn app_import_backup(app: AppHandle, path: String) -> Result<(), String> {
    crate::app_import_backup(app, path).await
}

#[tauri::command]
pub(crate) fn set_chat_background(
    app: AppHandle,
    state: State<'_, AppState>,
    source_path: String,
) -> Result<crate::background::ChatBackground, String> {
    crate::background::set(&app, &state, &source_path)
}

#[tauri::command]
pub(crate) fn use_default_chat_background(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<crate::background::ChatBackground, String> {
    crate::background::use_default(&app, &state)
}

#[tauri::command]
pub(crate) fn clear_chat_background(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::background::clear(&app, &state)
}

#[tauri::command]
pub(crate) fn read_chat_background(app: AppHandle, state: State<'_, AppState>) -> Option<String> {
    crate::background::read(&app, &state)
}

#[tauri::command]
pub(crate) async fn app_export_pdf(html: String, out_path: String) -> Result<String, String> {
    crate::app_export_pdf(html, out_path).await
}

#[tauri::command]
pub(crate) fn save_markdown_file(path: String, content: String) -> Result<(), String> {
    crate::save_markdown_file(path, content)
}

#[tauri::command]
pub(crate) fn set_window_vibrancy(
    window: tauri::WebviewWindow,
    level: String,
    dark: bool,
) -> Option<String> {
    crate::set_window_vibrancy(window, level, dark)
}

#[tauri::command]
pub(crate) fn get_instance_info(state: State<'_, AppState>) -> instance::InstanceInfo {
    crate::get_instance_info(state)
}
