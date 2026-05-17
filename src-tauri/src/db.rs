use serde::Serialize;

#[derive(Serialize)]
pub struct HistoryRecord {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

#[tauri::command]
pub fn save_history(
    session_id: String,
    role: String,
    content: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO history (session_id, role, content) VALUES (?1, ?2, ?3)",
        rusqlite::params![session_id, role, content],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn load_history(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistoryRecord>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, session_id, role, content, COALESCE(timestamp, '') \
             FROM history ORDER BY id ASC LIMIT 500",
        )
        .map_err(|e| e.to_string())?;
    let history = stmt
        .query_map([], |row| {
            Ok(HistoryRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(history)
}

#[tauri::command]
pub fn delete_history(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "DELETE FROM history WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── API Request Monitor ──────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ApiRequestRecord {
    pub id: i64,
    pub session_id: String,
    pub timestamp: String,
    pub model: String,
    pub finish_reason: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub duration_ms: i64,
    pub error: String,
    // Truncated preview of response content (first 200 chars)
    pub response_preview: String,
}

#[derive(Serialize)]
pub struct ApiRequestDetail {
    pub id: i64,
    pub session_id: String,
    pub timestamp: String,
    pub model: String,
    pub request_body: String,
    pub response_content: String,
    pub tool_calls: String,
    pub finish_reason: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub duration_ms: i64,
    pub error: String,
}

#[tauri::command]
pub fn list_api_requests(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<ApiRequestRecord>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, COALESCE(session_id,''), COALESCE(timestamp,''), COALESCE(model,''), \
             COALESCE(finish_reason,''), COALESCE(prompt_tokens,0), COALESCE(completion_tokens,0), \
             COALESCE(duration_ms,0), COALESCE(error,''), COALESCE(response_content,'') \
             FROM api_requests ORDER BY id DESC LIMIT 200",
        )
        .map_err(|e| e.to_string())?;

    let records = stmt
        .query_map([], |row| {
            let full_response: String = row.get(9)?;
            let preview: String = full_response.chars().take(200).collect();
            Ok(ApiRequestRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                timestamp: row.get(2)?,
                model: row.get(3)?,
                finish_reason: row.get(4)?,
                prompt_tokens: row.get(5)?,
                completion_tokens: row.get(6)?,
                duration_ms: row.get(7)?,
                error: row.get(8)?,
                response_preview: preview,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(records)
}

#[tauri::command]
pub fn get_api_request(
    id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<ApiRequestDetail, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, COALESCE(session_id,''), COALESCE(timestamp,''), COALESCE(model,''), \
             COALESCE(request_body,''), COALESCE(response_content,''), COALESCE(tool_calls,''), \
             COALESCE(finish_reason,''), COALESCE(prompt_tokens,0), COALESCE(completion_tokens,0), \
             COALESCE(duration_ms,0), COALESCE(error,'') \
             FROM api_requests WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_row(rusqlite::params![id], |row| {
        Ok(ApiRequestDetail {
            id: row.get(0)?,
            session_id: row.get(1)?,
            timestamp: row.get(2)?,
            model: row.get(3)?,
            request_body: row.get(4)?,
            response_content: row.get(5)?,
            tool_calls: row.get(6)?,
            finish_reason: row.get(7)?,
            prompt_tokens: row.get(8)?,
            completion_tokens: row.get(9)?,
            duration_ms: row.get(10)?,
            error: row.get(11)?,
        })
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_api_request(
    id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM api_requests WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn clear_api_requests(
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM api_requests", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}
