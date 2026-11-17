//! Commands for `figures` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::figures::SavedFigure;
use crate::AppState;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) fn save_figure(
    state: State<'_, AppState>,
    id: String,
    png_base64: String,
) -> Result<SavedFigure, String> {
    crate::figures::save_figure(state, id, png_base64)
}

#[tauri::command]
pub(crate) fn save_figure_html(
    state: State<'_, AppState>,
    id: String,
    html: String,
) -> Result<SavedFigure, String> {
    crate::figures::save_figure_html(state, id, html)
}

#[tauri::command]
pub(crate) fn app_open_figures_dir(app: AppHandle) -> Result<(), String> {
    crate::figures::app_open_figures_dir(app)
}
