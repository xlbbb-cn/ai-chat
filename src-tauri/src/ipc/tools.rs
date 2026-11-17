//! Commands for `tools` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::tools::memory::ArchivedSkill;
use crate::tools::memory::EvolutionMemoryView;
use crate::tools::serve::ServeProcessInfo;
use crate::tools::timer::SleepLock;
use crate::tools::timer::SleepLockState;
use crate::tools::timer::TimerEntry;
use crate::tools::timer::TimerRegistry;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) fn list_evolution_memory(state: State<'_, crate::AppState>) -> Vec<EvolutionMemoryView> {
    crate::tools::memory::list_evolution_memory(state)
}

#[tauri::command]
pub(crate) fn read_evolution_entry(
    state: State<'_, crate::AppState>,
    key: String,
) -> Result<String, String> {
    crate::tools::memory::read_evolution_entry(state, key)
}

#[tauri::command]
pub(crate) fn list_archived_skills(state: State<'_, crate::AppState>) -> Vec<ArchivedSkill> {
    crate::tools::memory::list_archived_skills(state)
}

#[tauri::command]
pub(crate) fn list_serve_processes(state: State<'_, crate::AppState>) -> Vec<ServeProcessInfo> {
    crate::tools::serve::list_serve_processes(state)
}

#[tauri::command]
pub(crate) fn stop_serve_process(state: State<'_, crate::AppState>, serve_id: String) -> String {
    crate::tools::serve::stop_serve_process(state, serve_id)
}

#[tauri::command]
pub(crate) fn list_timers(state: State<'_, TimerRegistry>) -> Vec<TimerEntry> {
    crate::tools::timer::list_timers(state)
}

#[tauri::command]
pub(crate) fn get_sleep_lock_state(state: State<'_, SleepLock>) -> SleepLockState {
    crate::tools::timer::get_sleep_lock_state(state)
}

#[tauri::command]
pub(crate) fn cancel_timer(app: AppHandle, id: String) -> Result<TimerEntry, String> {
    crate::tools::timer::cancel_timer(app, id)
}
