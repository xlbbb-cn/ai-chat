//! Commands for `update` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::update::UpdateInfo;
use crate::AppState;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) async fn check_update(
    app: AppHandle,
    state: State<'_, AppState>,
    include_prerelease: Option<bool>,
) -> Result<UpdateInfo, String> {
    crate::update::check_update(app, state, include_prerelease).await
}

#[tauri::command]
pub(crate) async fn download_update(
    app: AppHandle,
    state: State<'_, AppState>,
    asset_name: String,
    download_url: String,
) -> Result<String, String> {
    crate::update::download_update(app, state, asset_name, download_url).await
}

#[tauri::command]
pub(crate) fn open_update_file(app: AppHandle, path: String) -> Result<(), String> {
    crate::update::open_update_file(app, path)
}

#[tauri::command]
pub(crate) fn reveal_update_file(app: AppHandle, path: String) -> Result<(), String> {
    crate::update::reveal_update_file(app, path)
}

#[tauri::command]
pub(crate) fn open_release_page(app: AppHandle, url: String) -> Result<(), String> {
    crate::update::open_release_page(app, url)
}

#[tauri::command]
pub(crate) fn get_app_version(app: AppHandle) -> String {
    crate::update::get_app_version(app)
}
