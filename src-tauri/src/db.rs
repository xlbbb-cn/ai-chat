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
    /// JSON-serialized `Attachment[]` for user messages — stores display
    /// metadata (name, kind, mime, data URL) for the files the user
    /// attached, so the chat history can re-render thumbnails/pills
    /// without re-reading the binary content from `content`.
    pub attachments: Option<String>,
}

#[tauri::command]
pub fn save_history(
    session_id: String,
    role: String,
    content: String,
    tool_calls: Option<String>,
    reasoning_content: Option<String>,
    attachments: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<i64, String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO history (session_id, role, content, tool_calls, reasoning_content, attachments) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![session_id, role, content, tool_calls, reasoning_content, attachments],
    )
    .map_err(|e| e.to_string())?;
    let id = db.last_insert_rowid();
    Ok(id)
}

/// One row of the history sidebar: everything the session list needs, without
/// the (potentially huge) message bodies. Message content is fetched per
/// session via [`load_session_messages`] when the user opens it.
#[derive(Serialize)]
pub struct HistorySessionSummary {
    pub session_id: String,
    pub message_count: i64,
    /// Timestamp of the session's first message.
    pub created_at: String,
    /// Text of the session's first user message, clamped — used as the
    /// default session title when no custom title is set.
    pub first_user_content: String,
}

/// Longest title snippet stored per session. Only ever used to render a
/// truncated one-line label, so the full message body is never needed.
const SESSION_TITLE_SNIPPET_CHARS: usize = 512;

/// Escape `%`, `_` and the escape character itself so a keyword is matched
/// literally (a user typing `%` must not turn the search into a wildcard).
pub(crate) fn escape_like(keyword: &str) -> String {
    let mut escaped = String::with_capacity(keyword.len());
    for ch in keyword.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// List every session with its message count and title snippet.
///
/// The keyword filter selects whole sessions (never individual rows), so a
/// match inside one message still reports the session's true message count
/// and never cuts a session in half. Passing `None` returns every session —
/// this query has no row cap, which is what keeps older conversations
/// reachable from the sidebar.
pub(crate) fn query_history_sessions(
    conn: &Connection,
    keyword: Option<&str>,
) -> rusqlite::Result<Vec<HistorySessionSummary>> {
    let pattern = keyword
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| format!("%{}%", escape_like(k)));

    // The title snippet resolves to plain text even for multimodal rows:
    // their `content` is a JSON array, so the first `text` part is extracted
    // before clamping. `->>` / `json_each` need SQLite's JSON1 (built into
    // the bundled SQLite).
    let mut stmt = conn.prepare(
        "SELECT h.session_id, \
                COUNT(*), \
                COALESCE(MIN(h.timestamp), ''), \
                COALESCE(( \
                    SELECT substr( \
                        CASE WHEN json_valid(u.content) AND json_type(u.content) = 'array' \
                             THEN COALESCE(( \
                                 SELECT je.value ->> 'text' FROM json_each(u.content) je \
                                 WHERE je.value ->> 'type' = 'text' LIMIT 1 \
                             ), '') \
                             ELSE u.content END, 1, ?2) \
                    FROM history u \
                    WHERE u.session_id = h.session_id AND u.role = 'user' \
                    ORDER BY u.id ASC LIMIT 1 \
                ), '') \
         FROM history h \
         WHERE ?1 IS NULL \
            OR h.session_id LIKE ?1 ESCAPE '\\' \
            OR EXISTS ( \
                   SELECT 1 FROM history m \
                   WHERE m.session_id = h.session_id AND m.content LIKE ?1 ESCAPE '\\' \
               ) \
         GROUP BY h.session_id \
         ORDER BY MAX(h.id) DESC",
    )?;

    let sessions = stmt
        .query_map(
            rusqlite::params![pattern, SESSION_TITLE_SNIPPET_CHARS as i64],
            |row| {
                Ok(HistorySessionSummary {
                    session_id: row.get(0)?,
                    message_count: row.get(1)?,
                    created_at: row.get(2)?,
                    first_user_content: row.get(3)?,
                })
            },
        )?
        .filter_map(Result::ok)
        .collect();

    Ok(sessions)
}

/// Load every message of one session, oldest first. Crucially this has no
/// row cap: opening a conversation must show it in full.
pub(crate) fn query_session_messages(
    conn: &Connection,
    session_id: &str,
) -> rusqlite::Result<Vec<HistoryRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, COALESCE(timestamp, ''), tool_calls, reasoning_content, attachments \
         FROM history WHERE session_id = ?1 ORDER BY id ASC",
    )?;

    let messages = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(HistoryRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                timestamp: row.get(4)?,
                tool_calls: row.get(5)?,
                reasoning_content: row.get(6)?,
                attachments: row.get(7)?,
            })
        })?
        .filter_map(Result::ok)
        .collect();

    Ok(messages)
}

/// Lightweight session list for the history sidebar. `keyword` filters by
/// session id, message content or the first user message.
#[tauri::command]
pub fn list_history_sessions(
    keyword: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistorySessionSummary>, String> {
    let db = state.db.lock().unwrap();
    query_history_sessions(&db, keyword.as_deref()).map_err(|e| e.to_string())
}

/// Load all messages of one session (no truncation).
#[tauri::command]
pub fn load_session_messages(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<HistoryRecord>, String> {
    let db = state.db.lock().unwrap();
    query_session_messages(&db, &session_id).map_err(|e| e.to_string())
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
    // Also drop the session's title/favorite/archived meta row.
    db.execute(
        "DELETE FROM session_meta WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;
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
            "SELECT role, content, tool_calls, reasoning_content, attachments FROM history \
             WHERE session_id = ?1 AND id <= ?2 ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;

    let messages: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = stmt
        .query_map(rusqlite::params![source_session_id, cutoff_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let count = messages.len() as i64;
    for (role, content, tool_calls, reasoning_content, attachments) in messages {
        db.execute(
            "INSERT INTO history (session_id, role, content, tool_calls, reasoning_content, attachments) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![new_session_id, role, content, tool_calls, reasoning_content, attachments],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(count)
}

// ─── Session Meta (title / favorite / archived) ─────────────────────────────

#[derive(Serialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub title: Option<String>,
    pub favorite: bool,
    pub archived: bool,
}

/// List all session meta rows. Sessions without a meta row are simply absent
/// from the result (the frontend treats missing meta as defaults).
#[tauri::command]
pub fn list_session_meta(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<SessionMeta>, String> {
    let db = state.db.lock().unwrap();
    let mut stmt = db
        .prepare("SELECT session_id, title, favorite, archived FROM session_meta")
        .map_err(|e| e.to_string())?;
    let metas = stmt
        .query_map([], |row| {
            Ok(SessionMeta {
                session_id: row.get(0)?,
                title: row.get(1)?,
                favorite: row.get::<_, i64>(2)? != 0,
                archived: row.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(metas)
}

/// Upsert session meta fields. `None` leaves the field unchanged, so the
/// frontend can update title / favorite / archived independently.
#[tauri::command]
pub fn update_session_meta(
    session_id: String,
    title: Option<String>,
    favorite: Option<bool>,
    archived: Option<bool>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();

    // Ensure the row exists first (INSERT OR IGNORE keeps existing values).
    db.execute(
        "INSERT OR IGNORE INTO session_meta (session_id) VALUES (?1)",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;

    if let Some(t) = title {
        db.execute(
            "UPDATE session_meta SET title = ?2 WHERE session_id = ?1",
            rusqlite::params![session_id, t],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(f) = favorite {
        db.execute(
            "UPDATE session_meta SET favorite = ?2 WHERE session_id = ?1",
            rusqlite::params![session_id, f as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(a) = archived {
        db.execute(
            "UPDATE session_meta SET archived = ?2 WHERE session_id = ?1",
            rusqlite::params![session_id, a as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Delete the meta row for a session (called when the session is deleted).
#[tauri::command]
pub fn delete_session_meta(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "DELETE FROM session_meta WHERE session_id = ?1",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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

// ─── Log retention / database compaction ─────────────────────────────────────

/// Default log retention window. The effective value comes from
/// `AppConfig::log_retention_days`; this is only the fallback for configs
/// written before the setting existed. Each request stores its full prompt
/// body, so unbounded growth here is what inflates `chat.db` (a heavy agent
/// day can add ~80 MB).
pub const DEFAULT_LOG_RETENTION_DAYS: i64 = 90;

/// Serde default for `AppConfig::log_retention_days`.
pub fn default_log_retention_days() -> u32 {
    DEFAULT_LOG_RETENTION_DAYS as u32
}

/// Delete log rows older than `days`. Returns the number of removed rows.
pub fn prune_old_logs(db: &rusqlite::Connection, days: i64) -> Result<usize, String> {
    if days <= 0 {
        return Ok(0);
    }

    let cutoff = format!("-{days} days");
    let mut removed = 0usize;
    for table in ["api_requests", "interaction_log"] {
        removed += db
            .execute(
                &format!(
                    "DELETE FROM {table} WHERE timestamp IS NOT NULL AND timestamp < datetime('now', ?1)"
                ),
                rusqlite::params![cutoff],
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

/// Logical database size (page count × page size). After `VACUUM` this equals
/// the on-disk file size.
fn db_size_bytes(db: &rusqlite::Connection) -> i64 {
    db.query_row(
        "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

#[derive(Serialize)]
pub struct CompactResult {
    /// Log rows dropped by the retention policy during this run.
    pub rows_pruned: usize,
    /// Retention window that was applied (`0` = pruning disabled).
    pub retention_days: i64,
    pub bytes_before: i64,
    pub bytes_after: i64,
}

/// Apply the retention policy, then `VACUUM` so the freed space actually
/// returns to the filesystem (deleting rows alone leaves the file size
/// unchanged). Runs on its own connection in a blocking thread because a
/// 1 GB+ `VACUUM` takes seconds and must not freeze the UI; the app's own
/// connection is left untouched.
pub fn compact_logs_at(path: &std::path::Path, days: i64) -> Result<CompactResult, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    // Wait for the app's connection instead of failing with SQLITE_BUSY.
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(|e| e.to_string())?;

    let rows_pruned = prune_old_logs(&conn, days)?;
    let bytes_before = db_size_bytes(&conn);
    conn.execute("VACUUM", []).map_err(|e| e.to_string())?;
    let bytes_after = db_size_bytes(&conn);

    Ok(CompactResult {
        rows_pruned,
        retention_days: days,
        bytes_before,
        bytes_after,
    })
}

/// Delete every log row. The space is only reclaimed by a later `VACUUM`
/// ([`compact_database`]).
#[tauri::command]
pub fn clear_logs(state: tauri::State<'_, crate::AppState>) -> Result<usize, String> {
    let db = state.db.lock().unwrap();
    let mut removed = 0usize;
    for table in ["api_requests", "interaction_log"] {
        removed += db
            .execute(&format!("DELETE FROM {table}"), [])
            .map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

/// Prune expired logs and compact the database file. The retention window
/// comes from the user's settings (`log_retention_days`).
#[tauri::command]
pub async fn compact_database(
    state: tauri::State<'_, crate::AppState>,
) -> Result<CompactResult, String> {
    let db_path = state.db_path.clone();
    let days = {
        let config = state.config.lock().unwrap();
        config.log_retention_days as i64
    };
    tauri::async_runtime::spawn_blocking(move || compact_logs_at(&db_path, days))
        .await
        .map_err(|e| e.to_string())?
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE history (\
                id INTEGER PRIMARY KEY, \
                session_id TEXT, \
                role TEXT, \
                content TEXT, \
                timestamp DATETIME DEFAULT CURRENT_TIMESTAMP, \
                tool_calls TEXT, \
                reasoning_content TEXT, \
                attachments TEXT\
            )",
            [],
        )
        .unwrap();
        conn
    }

    fn insert(conn: &Connection, session_id: &str, role: &str, content: &str) {
        conn.execute(
            "INSERT INTO history (session_id, role, content) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_id, role, content],
        )
        .unwrap();
    }

    /// Regression: the sidebar must list sessions that start past any fixed
    /// row offset (the old `ORDER BY id ASC LIMIT 500` dropped them).
    #[test]
    fn session_list_is_not_capped_by_row_offset() {
        let conn = test_conn();
        for session in 0..60 {
            for message in 0..12 {
                insert(
                    &conn,
                    &format!("session-{session:02}"),
                    "user",
                    &format!("m{message}"),
                );
            }
        }

        let sessions = query_history_sessions(&conn, None).unwrap();
        assert_eq!(sessions.len(), 60, "every session must be listed");
        assert!(sessions.iter().all(|s| s.message_count == 12));
        assert!(sessions.iter().any(|s| s.session_id == "session-00"));
        assert!(sessions.iter().any(|s| s.session_id == "session-59"));
    }

    /// Sessions are ordered by most recent activity, so a resumed
    /// conversation moves back to the top.
    #[test]
    fn session_list_orders_by_last_activity() {
        let conn = test_conn();
        insert(&conn, "old", "user", "old first");
        insert(&conn, "new", "user", "new first");
        insert(&conn, "old", "assistant", "old resumed");

        let sessions = query_history_sessions(&conn, None).unwrap();
        assert_eq!(
            sessions
                .iter()
                .map(|s| s.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["old", "new"]
        );
    }

    /// A keyword hit in one message must not shrink the session's count nor
    /// hide the other messages of that session.
    #[test]
    fn keyword_filter_returns_whole_session() {
        let conn = test_conn();
        insert(&conn, "hit", "user", "first message");
        insert(&conn, "hit", "assistant", "buried needle here");
        insert(&conn, "hit", "user", "last message");
        insert(&conn, "miss", "user", "unrelated");

        let sessions = query_history_sessions(&conn, Some("needle")).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "hit");
        assert_eq!(
            sessions[0].message_count, 3,
            "count covers the whole session"
        );

        let messages = query_session_messages(&conn, "hit").unwrap();
        assert_eq!(messages.len(), 3, "opening the session shows every message");
    }

    /// `%` and `_` typed by the user are literal, not wildcards.
    #[test]
    fn keyword_wildcards_are_literal() {
        let conn = test_conn();
        insert(&conn, "literal", "user", "100% done");
        insert(&conn, "other", "user", "nothing to see");

        let sessions = query_history_sessions(&conn, Some("%")).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "literal");
    }

    /// The list payload carries a title snippet only — never the full row.
    #[test]
    fn session_list_extracts_title_text_from_multimodal_content() {
        let conn = test_conn();
        let multimodal = r#"[{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}},{"type":"text","text":"describe this chart"}]"#;
        insert(&conn, "multi", "user", multimodal);
        insert(&conn, "multi", "assistant", "a chart");
        insert(&conn, "plain", "user", "plain title");

        let sessions = query_history_sessions(&conn, None).unwrap();
        let multi = sessions.iter().find(|s| s.session_id == "multi").unwrap();
        assert_eq!(multi.first_user_content, "describe this chart");
        assert!(
            !multi.first_user_content.contains("base64"),
            "base64 payloads must not leak into the sidebar payload"
        );

        let plain = sessions.iter().find(|s| s.session_id == "plain").unwrap();
        assert_eq!(plain.first_user_content, "plain title");
    }

    /// The title snippet is clamped so a huge first message cannot bloat the
    /// session list.
    #[test]
    fn session_list_clamps_title_snippet() {
        let conn = test_conn();
        insert(&conn, "long", "user", &"x".repeat(5_000));

        let sessions = query_history_sessions(&conn, None).unwrap();
        assert_eq!(
            sessions[0].first_user_content.chars().count(),
            SESSION_TITLE_SNIPPET_CHARS
        );
    }

    /// Regression: opening a session must load all of its messages, even when
    /// the session alone exceeds the old global 500-row window.
    #[test]
    fn session_messages_load_completely_beyond_500_rows() {
        let conn = test_conn();
        for i in 0..550 {
            insert(&conn, "big", "user", &format!("message {i}"));
        }
        for i in 0..40 {
            insert(&conn, "small", "user", &format!("other {i}"));
        }

        let messages = query_session_messages(&conn, "big").unwrap();
        assert_eq!(messages.len(), 550);
        assert_eq!(messages.first().unwrap().content, "message 0");
        assert_eq!(messages.last().unwrap().content, "message 549");
    }

    #[test]
    fn escape_like_escapes_wildcards_and_backslash() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c\\d"), "c\\\\d");
    }

    #[test]
    fn prune_old_logs_drops_expired_rows_and_keeps_history() {
        let conn = test_conn();
        conn.execute(
            "CREATE TABLE api_requests (id INTEGER PRIMARY KEY, timestamp DATETIME)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE interaction_log (id INTEGER PRIMARY KEY, timestamp DATETIME)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_requests (timestamp) VALUES (datetime('now', '-100 days'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_requests (timestamp) VALUES (datetime('now', '-10 days'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interaction_log (timestamp) VALUES (datetime('now', '-200 days'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interaction_log (timestamp) VALUES (datetime('now'))",
            [],
        )
        .unwrap();
        insert(&conn, "keep", "user", "conversation must survive");

        let removed = prune_old_logs(&conn, 90).unwrap();
        assert_eq!(removed, 2, "one expired row per log table");

        let count = |table: &str| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(count("api_requests"), 1);
        assert_eq!(count("interaction_log"), 1);
        assert_eq!(
            query_session_messages(&conn, "keep").unwrap().len(),
            1,
            "history must never be pruned by the log retention"
        );
    }

    #[test]
    fn prune_old_logs_is_a_no_op_for_nonpositive_window() {
        let conn = test_conn();
        conn.execute(
            "CREATE TABLE api_requests (id INTEGER PRIMARY KEY, timestamp DATETIME)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_requests (timestamp) VALUES (datetime('now', '-999 days'))",
            [],
        )
        .unwrap();

        assert_eq!(prune_old_logs(&conn, 0).unwrap(), 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM api_requests", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
