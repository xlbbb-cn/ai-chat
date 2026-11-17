//! Commands for `skills` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::skills::Skill;
use crate::skills::SkillsStatus;
use crate::AppState;
use tauri::State;

#[tauri::command]
pub(crate) fn get_skills_status(state: State<'_, AppState>) -> SkillsStatus {
    crate::skills::get_skills_status(state)
}

#[tauri::command]
pub(crate) fn save_skills_status(
    state: State<'_, AppState>,
    status: SkillsStatus,
) -> Result<(), String> {
    crate::skills::save_skills_status(state, status)
}

#[tauri::command]
pub(crate) fn list_skills(state: State<'_, AppState>) -> Vec<Skill> {
    crate::skills::list_skills(state)
}

#[tauri::command]
pub(crate) fn save_skill(state: State<'_, AppState>, skill: Skill) -> Result<(), String> {
    crate::skills::save_skill(state, skill)
}

#[tauri::command]
pub(crate) fn delete_skill(state: State<'_, AppState>, name: String) -> Result<(), String> {
    crate::skills::delete_skill(state, name)
}

#[tauri::command]
pub(crate) fn filter_existing_skills(
    state: State<'_, AppState>,
    names: Vec<String>,
) -> Vec<String> {
    crate::skills::filter_existing_skills(state, names)
}
