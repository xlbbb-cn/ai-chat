//! Commands for `experiences` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use serde_json::Value;
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn list_experiences(app: AppHandle) -> Result<Value, String> {
    crate::tools::experience::list_experiences(app)
}

#[tauri::command]
pub(crate) fn delete_experience(app: AppHandle, slug: String) -> Result<(), String> {
    crate::tools::experience::delete_experience(app, slug)
}
