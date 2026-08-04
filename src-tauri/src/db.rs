use rusqlite::Connection;
use serde::Serialize;

#[derive(Serialize)]
pub struct HistoryRecord {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub tool_calls: Option<String>,
    pub reasoning_content: Option<String>,
}

#[tauri::command]
pub fn save_history(
    session_id: String,
    role: String,
    content: String,
    tool_calls: Option<String>,
    reasoning_content: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<i64, String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO history (session_id, role, content, tool_calls, reasoning_content) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![session_id, role, content, tool_calls, reasoning_content],
    )
    .map_err(|e| e.to_string())?;
    let id = db.last_insert_rowid();
    Ok(id)
}

#[tauri::command]
pub fn load_history(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistoryRecord>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, session_id, role, content, COALESCE(timestamp, ''), tool_calls, reasoning_content \
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
                tool_calls: row.get(5)?,
                reasoning_content: row.get(6)?,
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
    delete_session_summary(&db, &session_id)?;
    Ok(())
}

/// Delete a single message by its database id.
#[tauri::command]
pub fn delete_message(
    message_id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "DELETE FROM history WHERE id = ?1",
        rusqlite::params![message_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Fork a session: copy all messages up to and including `up_to_message_id`
/// from `source_session_id` into a new session `new_session_id`.
/// Returns the number of messages copied.
#[tauri::command]
pub fn fork_session(
    source_session_id: String,
    new_session_id: String,
    up_to_message_id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<i64, String> {
    let db = state.db.lock().unwrap();

    // Find the position (id) of the cutoff message in the source session
    let cutoff_id: i64 = db
        .query_row(
            "SELECT id FROM history WHERE id = ?1 AND session_id = ?2",
            rusqlite::params![up_to_message_id, source_session_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    // Copy all messages from the source session up to and including the cutoff
    let mut stmt = db
        .prepare(
            "SELECT role, content, tool_calls, reasoning_content FROM history \
             WHERE session_id = ?1 AND id <= ?2 ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;

    let messages: Vec<(String, String, Option<String>, Option<String>)> = stmt
        .query_map(rusqlite::params![source_session_id, cutoff_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let count = messages.len() as i64;
    for (role, content, tool_calls, reasoning_content) in messages {
        db.execute(
            "INSERT INTO history (session_id, role, content, tool_calls, reasoning_content) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![new_session_id, role, content, tool_calls, reasoning_content],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(count)
}

pub fn get_session_summary(db: &Connection, session_id: &str) -> Result<Option<String>, String> {
    let mut stmt = db
        .prepare("SELECT summary FROM session_summaries WHERE session_id = ?1 LIMIT 1")
        .map_err(|e| e.to_string())?;

    let result = stmt.query_row(rusqlite::params![session_id], |row| row.get::<_, String>(0));
    match result {
        Ok(summary) => Ok(Some(summary)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn save_session_summary(
    db: &Connection,
    session_id: &str,
    summary: &str,
) -> Result<(), String> {
    db.execute(
        "INSERT INTO session_summaries (session_id, summary, updated_at) \
         VALUES (?1, ?2, CURRENT_TIMESTAMP) \
         ON CONFLICT(session_id) DO UPDATE SET summary = excluded.summary, updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![session_id, summary],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_session_summary(db: &Connection, session_id: &str) -> Result<(), String> {
    db.execute(
        "DELETE FROM session_summaries WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Session History Search (for memory tool) ────────────────────────────────

#[derive(Serialize)]
pub struct HistoryMatch {
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// Search past chat history messages across all sessions (excluding the current one)
/// using LIKE-based substring matching. Results are ordered most-recent-first.
pub fn search_history_messages(
    db: &Connection,
    current_session_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<HistoryMatch>, String> {
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let mut stmt = db
        .prepare(
            "SELECT session_id, role, content, COALESCE(timestamp, '') \
             FROM history \
             WHERE session_id != ?1 AND content LIKE ?2 ESCAPE '\\' \
             ORDER BY id DESC \
             LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;

    let results = stmt
        .query_map(
            rusqlite::params![current_session_id, pattern, limit as i64],
            |row| {
                Ok(HistoryMatch {
                    session_id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                })
            },
        )
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(results)
}

/// Search session summaries across all sessions (excluding the current one)
/// for summaries matching the query.
pub fn search_session_summaries(
    db: &Connection,
    current_session_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<HistoryMatch>, String> {
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let mut stmt = db
        .prepare(
            "SELECT session_id, summary, COALESCE(updated_at, '') \
             FROM session_summaries \
             WHERE session_id != ?1 AND summary LIKE ?2 ESCAPE '\\' \
             ORDER BY updated_at DESC \
             LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;

    let results = stmt
        .query_map(
            rusqlite::params![current_session_id, pattern, limit as i64],
            |row| {
                Ok(HistoryMatch {
                    session_id: row.get(0)?,
                    role: "summary".to_string(),
                    content: row.get(1)?,
                    timestamp: row.get(2)?,
                })
            },
        )
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(results)
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
    pub reasoning_content: String,
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
             COALESCE(request_body,''), COALESCE(response_content,''), COALESCE(reasoning_content,''), COALESCE(tool_calls,''), \
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
            reasoning_content: row.get(6)?,
            tool_calls: row.get(7)?,
            finish_reason: row.get(8)?,
            prompt_tokens: row.get(9)?,
            completion_tokens: row.get(10)?,
            duration_ms: row.get(11)?,
            error: row.get(12)?,
        })
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_api_request(id: i64, state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "DELETE FROM api_requests WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn clear_api_requests(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute("DELETE FROM api_requests", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Interaction Log Monitor ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct InteractionLogRecord {
    pub id: i64,
    pub session_id: String,
    pub interaction_type: String,
    pub timestamp: String,
    pub actor: String,
    pub action_name: String,
    pub error_message: String,
    pub duration_ms: i64,
    // Truncated preview (first 150 chars)
    pub input_preview: String,
    pub output_preview: String,
}

#[derive(Serialize)]
pub struct InteractionLogDetail {
    pub id: i64,
    pub session_id: String,
    pub interaction_type: String,
    pub timestamp: String,
    pub actor: String,
    pub action_name: String,
    pub input_data: String,
    pub output_data: String,
    pub error_message: String,
    pub duration_ms: i64,
    pub metadata: String,
}

pub fn save_interaction_log(
    db: &rusqlite::Connection,
    session_id: &str,
    interaction_type: &str,
    actor: &str,
    action_name: &str,
    input_data: &str,
    output_data: &str,
    error_message: Option<&str>,
    duration_ms: i64,
    metadata: Option<&str>,
) -> Result<(), String> {
    db.execute(
        "INSERT INTO interaction_log (session_id, interaction_type, actor, action_name, \
         input_data, output_data, error_message, duration_ms, metadata) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            session_id,
            interaction_type,
            actor,
            action_name,
            input_data,
            output_data,
            error_message.unwrap_or(""),
            duration_ms,
            metadata.unwrap_or("{}")
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_interactions(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<InteractionLogRecord>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, COALESCE(session_id,''), COALESCE(interaction_type,''), \
             COALESCE(timestamp,''), COALESCE(actor,''), COALESCE(action_name,''), \
             COALESCE(error_message,''), COALESCE(duration_ms,0), \
             COALESCE(input_data,''), COALESCE(output_data,'') \
             FROM interaction_log \
             WHERE session_id = ?1 \
             ORDER BY id DESC LIMIT 500",
        )
        .map_err(|e| e.to_string())?;

    let records = stmt
        .query_map(rusqlite::params![session_id], |row| {
            let input: String = row.get(8)?;
            let output: String = row.get(9)?;
            let input_preview: String = input.chars().take(150).collect();
            let output_preview: String = output.chars().take(150).collect();
            Ok(InteractionLogRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                interaction_type: row.get(2)?,
                timestamp: row.get(3)?,
                actor: row.get(4)?,
                action_name: row.get(5)?,
                error_message: row.get(6)?,
                duration_ms: row.get(7)?,
                input_preview,
                output_preview,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(records)
}

#[tauri::command]
pub fn get_interaction(
    id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<InteractionLogDetail, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare(
            "SELECT id, COALESCE(session_id,''), COALESCE(interaction_type,''), \
             COALESCE(timestamp,''), COALESCE(actor,''), COALESCE(action_name,''), \
             COALESCE(input_data,''), COALESCE(output_data,''), \
             COALESCE(error_message,''), COALESCE(duration_ms,0), COALESCE(metadata,'{}') \
             FROM interaction_log WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_row(rusqlite::params![id], |row| {
        Ok(InteractionLogDetail {
            id: row.get(0)?,
            session_id: row.get(1)?,
            interaction_type: row.get(2)?,
            timestamp: row.get(3)?,
            actor: row.get(4)?,
            action_name: row.get(5)?,
            input_data: row.get(6)?,
            output_data: row.get(7)?,
            error_message: row.get(8)?,
            duration_ms: row.get(9)?,
            metadata: row.get(10)?,
        })
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_interactions(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "DELETE FROM interaction_log WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
