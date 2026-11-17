//! Commands for `evolution` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::AppState;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) fn start_evolution_maintenance(
    app: AppHandle,
    state: State<'_, AppState>,
    model_override: Option<String>,
) -> Result<(), String> {
    crate::evolution_maintenance::start_evolution_maintenance(app, state, model_override)
}

#[tauri::command]
pub(crate) fn stop_evolution_maintenance(state: State<'_, AppState>) {
    crate::evolution_maintenance::stop_evolution_maintenance(state)
}

#[tauri::command]
pub(crate) fn get_evolution_maintenance_state(state: State<'_, AppState>) -> bool {
    crate::evolution_maintenance::get_evolution_maintenance_state(state)
}
