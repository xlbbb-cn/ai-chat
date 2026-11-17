//! Mission tools: the sub-agent task ledger.
//!
//! Split out of the dispatcher so the ledger calls are visible on their own;
//! the dispatcher only forwards.

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

/// The four mission-ledger tools. Returns `None` for any other tool name, so
/// dispatchers can chain it in front of the regular tool executor.
pub(crate) fn handle(
    app: &AppHandle,
    name: &str,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    args: &Value,
) -> Option<String> {
    let out = match name {
        "add_task" => add_task(app, mission_id, session_id, args),
        "update_task_status" => update_task_status(app, mission_id, session_id, args),
        "get_active_tasks" => get_active_tasks(app, mission_id),
        "mark_mission_accomplished" => {
            mark_mission_accomplished(app, mission_id, session_id, args)
        }
        _ => return None,
    };
    Some(out)
}

fn add_task(
    app: &AppHandle,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    args: &Value,
) -> String {
    let Some(mission_id) = mission_id else {
        return "Error: add_task is only available inside an autonomous sub-agent mission."
            .to_string();
    };
    let description = args["description"].as_str().unwrap_or("");
    let name = args["name"].as_str().unwrap_or(description);
    let state = app.state::<crate::AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(err) => return format!("Error: failed to lock mission database: {err}"),
    };
    match crate::agents::add_mission_task(&db, mission_id, name, description) {
        Ok(task) => {
            let payload = json!({
                "mission_id": mission_id,
                "session_id": session_id,
                "task": task,
            });
            let _ = app.emit(crate::events::AGENT_TASK_STATE, payload.clone());
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        }
        Err(err) => format!("Error: {err}"),
    }
}

fn update_task_status(
    app: &AppHandle,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    args: &Value,
) -> String {
    let Some(mission_id) = mission_id else {
        return "Error: update_task_status is only available inside an autonomous sub-agent mission.".to_string();
    };
    let task_id = args["task_id"].as_str().unwrap_or("");
    let status = args["status"].as_str().unwrap_or("");
    let state = app.state::<crate::AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(err) => return format!("Error: failed to lock mission database: {err}"),
    };
    match crate::agents::update_mission_task_status(&db, mission_id, task_id, status) {
        Ok(task) => {
            let payload = json!({
                "mission_id": mission_id,
                "session_id": session_id,
                "task": task,
            });
            let _ = app.emit(crate::events::AGENT_TASK_STATE, payload.clone());
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        }
        Err(err) => format!("Error: {err}"),
    }
}

fn get_active_tasks(app: &AppHandle, mission_id: Option<&str>) -> String {
    let Some(mission_id) = mission_id else {
        return "Error: get_active_tasks is only available inside an autonomous sub-agent mission."
            .to_string();
    };
    let state = app.state::<crate::AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(err) => return format!("Error: failed to lock mission database: {err}"),
    };
    match crate::agents::get_active_mission_tasks(&db, mission_id) {
        Ok(tasks) => serde_json::to_string_pretty(&json!({
            "mission_id": mission_id,
            "active_tasks": tasks,
        }))
        .unwrap_or_else(|_| "[]".to_string()),
        Err(err) => format!("Error: {err}"),
    }
}

fn mark_mission_accomplished(
    app: &AppHandle,
    mission_id: Option<&str>,
    session_id: Option<&str>,
    args: &Value,
) -> String {
    let Some(mission_id) = mission_id else {
        return "Error: mark_mission_accomplished is only available inside an autonomous sub-agent mission.".to_string();
    };
    let final_report = args["final_report"].as_str();
    let state = app.state::<crate::AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(err) => return format!("Error: failed to lock mission database: {err}"),
    };
    match crate::agents::mark_mission_accomplished(&db, mission_id, final_report) {
        Ok(()) => {
            let payload = json!({
                "mission_id": mission_id,
                "session_id": session_id,
                "status": "completed",
                "final_report": final_report.unwrap_or(""),
            });
            let _ = app.emit(crate::events::AGENT_TASK_STATE, payload.clone());
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        }
        Err(err) => format!("Error: {err}"),
    }
}
