//! Commands for `history` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::db::CompactResult;
use crate::db::HistoryRecord;
use crate::db::HistorySessionSummary;
use crate::db::InteractionLogDetail;
use crate::db::InteractionLogRecord;
use crate::db::SessionMeta;
use crate::db::SessionStats;

#[tauri::command]
pub(crate) fn save_history(
    session_id: String,
    role: String,
    content: String,
    tool_calls: Option<String>,
    reasoning_content: Option<String>,
    attachments: Option<String>,
    injections: Option<String>,
    error: Option<String>,
    status: Option<String>,
    usage_meta: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<i64, String> {
    crate::db::save_history(
        session_id,
        role,
        content,
        tool_calls,
        reasoning_content,
        attachments,
        injections,
        error,
        status,
        usage_meta,
        state,
    )
}

#[tauri::command]
pub(crate) fn list_history_sessions(
    keyword: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistorySessionSummary>, String> {
    crate::db::list_history_sessions(keyword, state)
}

#[tauri::command]
pub(crate) fn load_session_messages(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistoryRecord>, String> {
    crate::db::load_session_messages(session_id, state)
}

#[tauri::command]
pub(crate) fn delete_history(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    crate::db::delete_history(session_id, state)
}

#[tauri::command]
pub(crate) fn delete_message(
    message_id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    crate::db::delete_message(message_id, state)
}

#[tauri::command]
pub(crate) fn delete_messages_after(
    session_id: String,
    after_message_id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<usize, String> {
    crate::db::delete_messages_after(session_id, after_message_id, state)
}

#[tauri::command]
pub(crate) fn fork_session(
    source_session_id: String,
    new_session_id: String,
    up_to_message_id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<i64, String> {
    crate::db::fork_session(source_session_id, new_session_id, up_to_message_id, state)
}

#[tauri::command]
pub(crate) fn list_session_meta(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<SessionMeta>, String> {
    crate::db::list_session_meta(state)
}

#[tauri::command]
pub(crate) fn update_session_meta(
    session_id: String,
    title: Option<String>,
    favorite: Option<bool>,
    archived: Option<bool>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    crate::db::update_session_meta(session_id, title, favorite, archived, state)
}

#[tauri::command]
pub(crate) fn clear_logs(state: tauri::State<'_, crate::AppState>) -> Result<usize, String> {
    crate::db::clear_logs(state)
}

#[tauri::command]
pub(crate) async fn compact_database(
    state: tauri::State<'_, crate::AppState>,
) -> Result<CompactResult, String> {
    crate::db::compact_database(state).await
}

#[tauri::command]
pub(crate) fn list_interactions(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<InteractionLogRecord>, String> {
    crate::db::list_interactions(session_id, state)
}

#[tauri::command]
pub(crate) fn get_interaction(
    id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<InteractionLogDetail, String> {
    crate::db::get_interaction(id, state)
}

#[tauri::command]
pub(crate) fn get_session_stats(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<SessionStats, String> {
    crate::db::get_session_stats(session_id, state)
}
