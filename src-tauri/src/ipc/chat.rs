//! Commands for `chat` — thin forwarders to the domain modules.
//!
//! Keep the argument names identical to `src/api.ts`: they are the wire names.

use crate::llm_complete::ChatMessage;
use crate::tools;
use crate::tools::todos;
use crate::AppState;
use crate::InjectUserMessageResult;
use tauri::AppHandle;
use tauri::State;

#[tauri::command]
pub(crate) async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_ids: Vec<String>,
    session_id: String,
    model_override: Option<String>,
    use_agents: Option<bool>,
    auto_plan: Option<bool>,
    embedded_agent_ids: Option<Vec<String>>,
    turn_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::llm_complete::chat_completion(
        app,
        messages,
        skill_ids,
        session_id,
        model_override,
        use_agents,
        auto_plan,
        embedded_agent_ids,
        turn_id,
        state,
    )
    .await
}

#[tauri::command]
pub(crate) fn stop_chat_completion(app: AppHandle, state: State<'_, AppState>) {
    crate::stop_chat_completion(app, state)
}

#[tauri::command]
pub(crate) fn inject_user_message(
    session_id: String,
    id: String,
    text: String,
    state: State<'_, AppState>,
) -> InjectUserMessageResult {
    crate::inject_user_message(session_id, id, text, state)
}

#[tauri::command]
pub(crate) fn confirm_command(
    state: State<'_, AppState>,
    request_id: String,
    confirmed: bool,
    username: Option<String>,
    password: Option<String>,
) {
    crate::confirm_command(state, request_id, confirmed, username, password)
}

#[tauri::command]
pub(crate) fn todo_ask_reply(
    state: State<'_, AppState>,
    request_id: String,
    answers: Option<Vec<todos::TodoAskAnswer>>,
) {
    crate::todo_ask_reply(state, request_id, answers)
}

#[tauri::command]
pub(crate) fn get_pending_todo_ask(state: State<'_, AppState>) -> Option<serde_json::Value> {
    crate::get_pending_todo_ask(state)
}

#[tauri::command]
pub(crate) fn get_active_todo_list(
    app: AppHandle,
    session_id: String,
) -> Result<Option<todos::TodoListSummary>, String> {
    crate::get_active_todo_list(app, session_id)
}

#[tauri::command]
pub(crate) fn list_background_tasks(
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Vec<tools::background::BackgroundTaskSnapshot> {
    crate::list_background_tasks(session_id, state)
}

#[tauri::command]
pub(crate) fn cancel_background_task(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<tools::background::BackgroundTaskSnapshot, String> {
    crate::cancel_background_task(task_id, state)
}
