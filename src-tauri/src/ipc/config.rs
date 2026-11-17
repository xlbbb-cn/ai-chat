//! Commands for `config` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::model_metadata::ModelMetadata;
use crate::AppConfig;
use crate::AppState;
use crate::RemoteModel;
use crate::RequestHeader;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) fn get_config(state: State<'_, AppState>) -> AppConfig {
    crate::get_config(state)
}

#[tauri::command]
pub(crate) fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    crate::save_config(app, state, config)
}

#[tauri::command]
pub(crate) fn set_model_context_length(
    app: AppHandle,
    state: State<'_, AppState>,
    model: String,
    context_length: Option<u32>,
) -> Result<(), String> {
    crate::set_model_context_length(app, state, model, context_length)
}

#[tauri::command]
pub(crate) async fn fetch_models(
    state: State<'_, AppState>,
    api_base_url: Option<String>,
    api_key: Option<String>,
    custom_headers: Option<Vec<RequestHeader>>,
) -> Result<Vec<RemoteModel>, String> {
    crate::fetch_models(state, api_base_url, api_key, custom_headers).await
}

#[tauri::command]
pub(crate) async fn fetch_model_metadata(
    app: tauri::AppHandle,
    force: Option<bool>,
) -> Result<Vec<ModelMetadata>, String> {
    crate::model_metadata::fetch_model_metadata(app, force).await
}
