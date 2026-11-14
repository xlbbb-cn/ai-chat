use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Open the workspace database and bring its schema up to date.
///
/// The database follows the active workspace (`<workspace>/.data/chat.db`), so
/// this runs both at startup and whenever the workspace is switched. Every
/// statement is `IF NOT EXISTS` / lazy `ALTER TABLE`, so an existing database
/// upgrades in place and a fresh one is created empty.
pub fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    // A second window can be pointed at this workspace by hand; wait for its
    // write lock instead of failing immediately with `SQLITE_BUSY`.
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute("CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp DATETIME DEFAULT CURRENT_TIMESTAMP)", [])?;
    let _ = db.execute("ALTER TABLE history ADD COLUMN tool_calls TEXT", []);
    let _ = db.execute("ALTER TABLE history ADD COLUMN reasoning_content TEXT", []);
    // Attachments column: JSON-serialized `Attachment[]` for user messages.
    // The column is added lazily via ALTER TABLE so older databases (without
    // the column) upgrade in place; new rows always include it.
    let _ = db.execute("ALTER TABLE history ADD COLUMN attachments TEXT", []);
    // Mid-turn injections: JSON-serialized list of the messages the user pushed
    // into this assistant turn while it was already running. They belong to the
    // assistant row (one turn = one row) but must be replayed as user input.
    let _ = db.execute("ALTER TABLE history ADD COLUMN injections TEXT", []);
    // Partial assistant turns: `error` keeps the UI-only failure text (never
    // sent as model content) and `status` distinguishes a completed turn from
    // an errored/stopped one whose visible output can still be continued.
    let _ = db.execute("ALTER TABLE history ADD COLUMN error TEXT", []);
    let _ = db.execute("ALTER TABLE history ADD COLUMN status TEXT", []);
    // The sidebar lists sessions and loads one session at a time, both
    // filtered by `session_id`.
    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_history_session_id ON history (session_id, id)",
        [],
    )?;
    let _ = db.execute("ALTER TABLE api_requests ADD COLUMN reasoning_content TEXT", []);
    // Prompt-cache metrics (Phase 0): `cached_tokens` stays NULL when the
    // provider does not report caching, so a reported zero-hit request differs
    // from "no cache feedback" in every metric surface.
    let _ = db.execute(
        "ALTER TABLE api_requests ADD COLUMN cached_tokens INTEGER",
        [],
    );
    // Streamed phase timings (see `StreamTiming` in `llm_complete.rs`).
    // `reasoning_ms` stays NULL when the model answered without streaming
    // reasoning, so "thought for 0 ms" never stands in for "does not think".
    let _ = db.execute(
        "ALTER TABLE api_requests ADD COLUMN reasoning_ms INTEGER",
        [],
    );
    let _ = db.execute(
        "ALTER TABLE api_requests ADD COLUMN first_token_ms INTEGER",
        [],
    );
    // Session meta: per-session title / favorite / archived flags.
    // Kept in a separate table so history rows stay append-only.
    db.execute(
        "CREATE TABLE IF NOT EXISTS session_meta (\
            session_id TEXT PRIMARY KEY, \
            title TEXT, \
            favorite INTEGER NOT NULL DEFAULT 0, \
            archived INTEGER NOT NULL DEFAULT 0\
        )",
        [],
    )?;
    // Composer `+agent` roles embedded for the whole session: a JSON array of
    // `EmbeddedRole` snapshots taken when the session's first message is sent.
    // Lazy ALTER so existing databases upgrade in place.
    let _ = db.execute(
        "ALTER TABLE session_meta ADD COLUMN embedded_roles TEXT NOT NULL DEFAULT ''",
        [],
    );
    db.execute(
        "CREATE TABLE IF NOT EXISTS session_summaries (\
            session_id TEXT PRIMARY KEY, \
            summary TEXT NOT NULL, \
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        )",
        [],
    )?;
    // Coverage bookkeeping for cross-turn summary replacement: a compression
    // records how many UI history messages the summary stands in for
    // (`covered_messages`) and the flattened prefix of the last covered
    // message (`boundary_text`), which the next turn verifies before dropping
    // that prefix. Lazy ALTER so existing databases upgrade in place; NULL on
    // legacy rows means the old additive-only behavior.
    let _ = db.execute(
        "ALTER TABLE session_summaries ADD COLUMN covered_messages INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = db.execute(
        "ALTER TABLE session_summaries ADD COLUMN boundary_text TEXT NOT NULL DEFAULT ''",
        [],
    );
    db.execute(
        "CREATE TABLE IF NOT EXISTS api_requests (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            session_id TEXT, \
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP, \
            model TEXT, \
            request_body TEXT, \
            response_content TEXT, \
            tool_calls TEXT, \
            finish_reason TEXT, \
            prompt_tokens INTEGER DEFAULT 0, \
            completion_tokens INTEGER DEFAULT 0, \
            reasoning_ms INTEGER, \
            first_token_ms INTEGER, \
            duration_ms INTEGER DEFAULT 0, \
            error TEXT\
        )",
        [],
    )?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS interaction_log (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            session_id TEXT NOT NULL, \
            interaction_type TEXT NOT NULL, \
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP, \
            actor TEXT, \
            action_name TEXT, \
            input_data TEXT, \
            output_data TEXT, \
            error_message TEXT, \
            duration_ms INTEGER DEFAULT 0, \
            metadata TEXT\
        )",
        [],
    )?;
    // Both log tables are filtered by `session_id` (Monitor list, session
    // report) and by `timestamp` (the retention prune that runs on every
    // launch). Without these indexes every one of those queries is a full
    // table scan: on a 200k-row `interaction_log` the prune alone measured
    // 89 ms against 0.3 ms with the index.
    for statement in [
        "CREATE INDEX IF NOT EXISTS idx_api_requests_session ON api_requests (session_id, id)",
        "CREATE INDEX IF NOT EXISTS idx_api_requests_timestamp ON api_requests (timestamp)",
        "CREATE INDEX IF NOT EXISTS idx_interaction_log_session ON interaction_log (session_id, id)",
        "CREATE INDEX IF NOT EXISTS idx_interaction_log_timestamp ON interaction_log (timestamp)",
    ] {
        db.execute(statement, [])?;
    }
    db.execute(
        "CREATE TABLE IF NOT EXISTS agent_missions (\
            mission_id TEXT PRIMARY KEY, \
            session_id TEXT, \
            parent_task_id TEXT, \
            agent_id TEXT NOT NULL, \
            root_task_description TEXT NOT NULL, \
            root_task_context TEXT NOT NULL DEFAULT '', \
            status TEXT NOT NULL DEFAULT 'running', \
            mission_accomplished INTEGER NOT NULL DEFAULT 0, \
            episodic_summary TEXT NOT NULL DEFAULT '', \
            final_report TEXT NOT NULL DEFAULT '', \
            help_notes TEXT NOT NULL DEFAULT '', \
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP, \
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        )",
        [],
    )?;
    // Main-agent help log (one line per `ask_main_agent` exchange). Added
    // lazily so databases created before the feature upgrade in place.
    let _ = db.execute(
        "ALTER TABLE agent_missions ADD COLUMN help_notes TEXT NOT NULL DEFAULT ''",
        [],
    );
    // Mission message bus of the orchestration batch (NULL for direct
    // `call_subagent` runs and old rows). Added lazily like `help_notes`.
    let _ = db.execute("ALTER TABLE agent_missions ADD COLUMN bus_id TEXT", []);
    // Durable record of planner/agent bus traffic, shown in the Mission panel.
    // One row per publish (topic fan-out is summarised by `recipient_count`).
    db.execute(
        "CREATE TABLE IF NOT EXISTS agent_bus_messages (\
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            bus_id TEXT NOT NULL, \
            session_id TEXT NOT NULL, \
            from_id TEXT NOT NULL, \
            from_name TEXT NOT NULL, \
            target_kind TEXT NOT NULL, \
            target_id TEXT NOT NULL, \
            to_name TEXT NOT NULL DEFAULT '', \
            topic TEXT NOT NULL DEFAULT '', \
            body TEXT NOT NULL, \
            recipient_count INTEGER NOT NULL DEFAULT 0, \
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        )",
        [],
    )?;
    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_bus_messages_bus \
         ON agent_bus_messages (bus_id, id)",
        [],
    )?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS agent_tasks (\
            task_id TEXT PRIMARY KEY, \
            mission_id TEXT NOT NULL, \
            name TEXT NOT NULL, \
            description TEXT NOT NULL, \
            status TEXT NOT NULL DEFAULT 'pending', \
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP, \
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, \
            completed_at DATETIME\
        )",
        [],
    )?;
    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_agent_tasks_mission_status \
         ON agent_tasks (mission_id, status)",
        [],
    )?;
    Ok(db)
}

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
    /// JSON-serialized list of the user's mid-turn injections for this
    /// assistant turn (see `TurnInboxItem` in `lib.rs`).
    pub injections: Option<String>,
    /// UI-only failure text for an interrupted assistant turn.
    pub error: Option<String>,
    /// `done` / `error` / `stopped`; `None` means a normal completed turn.
    pub status: Option<String>,
}

#[tauri::command]
pub fn save_history(
    session_id: String,
    role: String,
    content: String,
    tool_calls: Option<String>,
    reasoning_content: Option<String>,
    attachments: Option<String>,
    injections: Option<String>,
    error: Option<String>,
    status: Option<String>,
    state: tauri::State<'_, crate::AppState>,
) -> Result<i64, String> {
    let db = state.db.lock().unwrap();
    db.execute(
        "INSERT INTO history (session_id, role, content, tool_calls, reasoning_content, attachments, injections, error, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![session_id, role, content, tool_calls, reasoning_content, attachments, injections, error, status],
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
        "SELECT id, session_id, role, content, COALESCE(timestamp, ''), tool_calls, reasoning_content, attachments, injections, error, status \
         FROM history WHERE session_id = ?1 ORDER BY id ASC",
    )?;

    let messages = stmt
        .query_map(rusqlite::params![session_id], history_record_from_row)?
        .filter_map(Result::ok)
        .collect();

    Ok(messages)
}

/// Load the most recent `limit` messages of a session, oldest first. Used by
/// the sub-agent main-agent helper, which only needs a bounded transcript and
/// must not read a multi-thousand-message history just to drop it again.
pub(crate) fn query_recent_session_messages(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<HistoryRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, COALESCE(timestamp, ''), tool_calls, reasoning_content, attachments, injections, error, status \
         FROM history WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;

    let mut messages: Vec<HistoryRecord> = stmt
        .query_map(
            rusqlite::params![session_id, limit as i64],
            history_record_from_row,
        )?
        .filter_map(Result::ok)
        .collect();
    messages.reverse();
    Ok(messages)
}

fn history_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRecord> {
    Ok(HistoryRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        timestamp: row.get(4)?,
        tool_calls: row.get(5)?,
        reasoning_content: row.get(6)?,
        attachments: row.get(7)?,
        injections: row.get(8)?,
        error: row.get(9)?,
        status: row.get(10)?,
    })
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
    {
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
    }

    // Deliverables belong to the session that produced them. The directory is
    // workspace state, not a cache, so it goes with the chat row; a failure to
    // remove it is logged and never blocks deleting the conversation.
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    if let Ok(dir) = crate::workspace::session_deliver_root(&workspace_dir, &session_id) {
        if let Err(err) = std::fs::remove_dir_all(&dir) {
            if err.kind() != std::io::ErrorKind::NotFound {
                state.logger.lock().unwrap().log(
                    "WARN",
                    &format!(
                        "Could not delete session deliverables '{}': {err}",
                        dir.display()
                    ),
                );
            }
        }
    }
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

/// Delete every message a session stored after `after_message_id`.
///
/// Retrying a failed turn removes what the failed attempt produced, and the
/// transcript alone is not enough: the partial assistant row is persisted too,
/// so a reload would bring it back. The cutoff is scoped to one session, and
/// rows before it (the user message being retried) stay untouched.
pub(crate) fn delete_history_after(
    db: &Connection,
    session_id: &str,
    after_message_id: i64,
) -> Result<usize, String> {
    db.execute(
        "DELETE FROM history WHERE session_id = ?1 AND id > ?2",
        rusqlite::params![session_id, after_message_id],
    )
    .map_err(|e| e.to_string())
}

/// Backend of `deleteMessagesAfter` — see [`delete_history_after`].
#[tauri::command]
pub fn delete_messages_after(
    session_id: String,
    after_message_id: i64,
    state: tauri::State<'_, crate::AppState>,
) -> Result<usize, String> {
    let db = state.db.lock().unwrap();
    delete_history_after(&db, &session_id, after_message_id)
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
            "SELECT role, content, tool_calls, reasoning_content, attachments, injections, error, status FROM history \
             WHERE session_id = ?1 AND id <= ?2 ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;

    let messages: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
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
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    let count = messages.len() as i64;
    for (role, content, tool_calls, reasoning_content, attachments, injections, error, status) in messages {
        db.execute(
            "INSERT INTO history (session_id, role, content, tool_calls, reasoning_content, attachments, injections, error, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![new_session_id, role, content, tool_calls, reasoning_content, attachments, injections, error, status],
        )
        .map_err(|e| e.to_string())?;
    }

    // A fork is a copy of the conversation, so it inherits the roles embedded
    // at its first send and keeps answering in the same voice.
    copy_embedded_roles(&db, &source_session_id, &new_session_id)?;

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

/// One sub-agent role embedded into a chat session with `+agent`.
///
/// This is a **snapshot** taken when the session's first message is sent, not a
/// live reference: editing or enabling/disabling the agent definition later
/// must never change a conversation that already started, and a deleted agent
/// must not break an existing session. `prompt` is the agent's `system_prompt`
/// at snapshot time.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EmbeddedRole {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
}

/// Read the embedded-role snapshot of a session.
///
/// A missing meta row, an empty column (the lazy `ALTER TABLE` default) and
/// malformed JSON all read as "no roles": a broken snapshot must never fail a
/// chat turn.
pub fn get_embedded_roles(db: &Connection, session_id: &str) -> Vec<EmbeddedRole> {
    let raw: Option<String> = db
        .query_row(
            "SELECT embedded_roles FROM session_meta WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .ok();
    raw.filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Vec<EmbeddedRole>>(&s).ok())
        .unwrap_or_default()
}

/// Replace the embedded-role snapshot of a session, creating its meta row when
/// needed. Title/favorite/archived are left untouched.
pub fn set_embedded_roles(
    db: &Connection,
    session_id: &str,
    roles: &[EmbeddedRole],
) -> Result<(), String> {
    let json = serde_json::to_string(roles).map_err(|e| e.to_string())?;
    db.execute(
        "INSERT OR IGNORE INTO session_meta (session_id) VALUES (?1)",
        rusqlite::params![session_id],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "UPDATE session_meta SET embedded_roles = ?2 WHERE session_id = ?1",
        rusqlite::params![session_id, json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Copy the role snapshot when a session is forked, so a branch keeps the roles
/// that shaped its transcript.
pub fn copy_embedded_roles(db: &Connection, from: &str, to: &str) -> Result<(), String> {
    let roles = get_embedded_roles(db, from);
    if roles.is_empty() {
        return Ok(());
    }
    set_embedded_roles(db, to, &roles)
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

/// A persisted session summary plus the coverage bookkeeping written by the
/// context compressor (`llm_complete.rs`): `covered_messages` counts the UI
/// history messages the summary replaces and `boundary_text` is the flattened
/// prefix of the last covered message, re-verified before that prefix is
/// dropped from a later turn's history.
#[derive(Debug, Clone)]
pub struct SessionSummaryRow {
    pub body: String,
    pub covered_messages: i64,
    pub boundary_text: String,
}

pub fn get_session_summary(
    db: &Connection,
    session_id: &str,
) -> Result<Option<SessionSummaryRow>, String> {
    let mut stmt = db
        .prepare(
            "SELECT summary, covered_messages, boundary_text \
             FROM session_summaries WHERE session_id = ?1 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;

    let result = stmt.query_row(rusqlite::params![session_id], |row| {
        // Rows written before the coverage columns existed report NULL —
        // the legacy "additive summary" shape (never trim history).
        let covered: Option<i64> = row.get(1)?;
        let boundary: Option<String> = row.get(2)?;
        Ok(SessionSummaryRow {
            body: row.get(0)?,
            covered_messages: covered.unwrap_or(0),
            boundary_text: boundary.unwrap_or_default(),
        })
    });
    match result {
        Ok(summary) => Ok(Some(summary)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn save_session_summary(
    db: &Connection,
    session_id: &str,
    body: &str,
    covered_messages: i64,
    boundary_text: &str,
) -> Result<(), String> {
    db.execute(
        "INSERT INTO session_summaries (session_id, summary, covered_messages, boundary_text, updated_at) \
         VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP) \
         ON CONFLICT(session_id) DO UPDATE SET \
             summary = excluded.summary, \
             covered_messages = excluded.covered_messages, \
             boundary_text = excluded.boundary_text, \
             updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![session_id, body, covered_messages, boundary_text],
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
    let db_path = state.db_path();
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
    /// Metadata JSON (`"{}"` when absent). `llm_api` rows carry the
    /// prompt-cache metrics (`prompt_tokens` / `cached_tokens` / `hit_ratio`)
    /// here, so the Monitor list can render a per-row hit chip and an
    /// aggregate without re-fetching details.
    pub metadata: String,
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

/// Preview width of a list row. The Monitor shows one clamped line per side;
/// `get_interaction` carries the full payload for the detail pane.
const PREVIEW_CHARS: i64 = 150;
/// Error text shown under a row (the full text stays in the detail view).
const ERROR_PREVIEW_CHARS: i64 = 500;
/// Rows per session in the Monitor list.
const INTERACTION_PAGE: i64 = 500;

/// The Monitor's list query.
///
/// The previews are cut **in SQL** (`substr` counts characters for TEXT, so CJK
/// payloads are never split): reading the whole `input_data` / `output_data`
/// first and truncating in Rust pulled up to tens of megabytes per session and
/// held the shared connection for the whole copy.
pub fn query_interactions(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<InteractionLogRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(session_id,''), COALESCE(interaction_type,''), \
             COALESCE(timestamp,''), COALESCE(actor,''), COALESCE(action_name,''), \
             substr(COALESCE(error_message,''),1,?2), COALESCE(duration_ms,0), \
             substr(COALESCE(input_data,''),1,?3), substr(COALESCE(output_data,''),1,?3), \
             substr(COALESCE(metadata,'{}'),1,4000) \
             FROM interaction_log \
             WHERE session_id = ?1 \
             ORDER BY id DESC LIMIT ?4",
        )
        .map_err(|e| e.to_string())?;

    let records = stmt
        .query_map(
            rusqlite::params![
                session_id,
                ERROR_PREVIEW_CHARS,
                PREVIEW_CHARS,
                INTERACTION_PAGE
            ],
            |row| {
                Ok(InteractionLogRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    interaction_type: row.get(2)?,
                    timestamp: row.get(3)?,
                    actor: row.get(4)?,
                    action_name: row.get(5)?,
                    error_message: row.get(6)?,
                    duration_ms: row.get(7)?,
                    input_preview: row.get(8)?,
                    output_preview: row.get(9)?,
                    metadata: row.get(10)?,
                })
            },
        )
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(records)
}

#[tauri::command]
pub fn list_interactions(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<InteractionLogRecord>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    query_interactions(&db, &session_id)
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

// ─── Session report (Monitor → bottom strip) ─────────────────────────────────

/// Interaction types written by the tool dispatch path in `llm_complete`.
/// `mcp_response` is the success row of an MCP call and `mcp_call` its failure
/// row — the naming is historical, so both the membership list and the failure
/// predicate have to spell them out.
const TOOL_INTERACTION_TYPES: &str = "('tool_output','tool_error','mcp_response','mcp_call')";
const TOOL_FAILURE_TYPES: &str = "('tool_error','mcp_call')";

/// How many tools the report's ranking list keeps.
const TOOL_RANKING_LIMIT: usize = 8;

/// One tool's activity inside a session, for the report's ranking list.
#[derive(Debug, Default, Serialize)]
pub struct ToolUsageStat {
    /// Tool name as logged in `action_name`.
    pub name: String,
    pub calls: i64,
    /// Calls that ended in an error row.
    pub failures: i64,
    /// Summed wall clock of those calls.
    pub duration_ms: i64,
}

/// Aggregated analysis of one chat session.
///
/// Token counters read `api_requests` instead of the `interaction_log`
/// metadata: the metadata only carries usage when the provider reported prompt
/// caching, while every successful request stores its usage in `api_requests`.
#[derive(Debug, Default, Serialize)]
pub struct SessionStats {
    pub requests: i64,
    /// Requests that ended with an error row.
    pub failed_requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// Prompt tokens served from the provider cache — the report's "hit" amount.
    pub hit_tokens: i64,
    /// Prompt tokens that had to be processed (`prompt_tokens - hit_tokens`).
    pub miss_tokens: i64,
    /// Requests that reported cache metrics at all. Endpoints without a cache
    /// dialect report nothing, which is not the same as a 0 % hit.
    pub cache_reported_requests: i64,
    /// Requests whose prompt was at least partly served from the cache.
    pub cache_hit_requests: i64,
    /// Prompt tokens of the newest successful request — the context in use.
    pub context_tokens: i64,
    /// Context window of the model behind that request (`0` = unknown).
    pub context_window: i64,
    pub tool_calls: i64,
    pub tool_failures: i64,
    pub llm_errors: i64,
    /// Interaction rows recorded for the session, all types.
    pub interactions: i64,
    pub total_request_ms: i64,
    pub total_tool_ms: i64,
    /// Rounds that reported a thinking phase (the model streamed reasoning
    /// before its answer). Non-thinking models report none — the average must
    /// not be diluted by rounds that never had a thinking phase.
    pub reasoning_requests: i64,
    /// Summed thinking phase over those rounds.
    pub reasoning_ms_total: i64,
    /// Rounds that reported a first-token latency, and its total.
    pub first_token_requests: i64,
    pub first_token_ms_total: i64,
    /// Busiest tools, at most [`TOOL_RANKING_LIMIT`] rows.
    pub tools: Vec<ToolUsageStat>,
}

/// Aggregate the session's request and interaction logs. `context_window` is
/// resolved by the caller — it depends on the configured model, not on the DB.
pub fn session_stats(
    db: &rusqlite::Connection,
    session_id: &str,
    context_window: i64,
) -> Result<SessionStats, String> {
    let mut stats = SessionStats {
        context_window,
        ..Default::default()
    };

    db.query_row(
        "SELECT COUNT(*), \
         COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0), \
         COALESCE(SUM(COALESCE(cached_tokens,0)),0), \
         COALESCE(SUM(CASE WHEN cached_tokens IS NOT NULL THEN 1 ELSE 0 END),0), \
         COALESCE(SUM(CASE WHEN cached_tokens > 0 THEN 1 ELSE 0 END),0), \
         COALESCE(SUM(CASE WHEN error IS NOT NULL AND error <> '' THEN 1 ELSE 0 END),0), \
         COALESCE(SUM(duration_ms),0), \
         COALESCE(SUM(CASE WHEN reasoning_ms IS NOT NULL THEN 1 ELSE 0 END),0), \
         COALESCE(SUM(COALESCE(reasoning_ms,0)),0), \
         COALESCE(SUM(CASE WHEN first_token_ms IS NOT NULL THEN 1 ELSE 0 END),0), \
         COALESCE(SUM(COALESCE(first_token_ms,0)),0) \
         FROM api_requests WHERE session_id = ?1",
        rusqlite::params![session_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
            ))
        },
    )
    .map(
        |(
            requests,
            prompt,
            completion,
            hit,
            reported,
            hit_requests,
            failed,
            total_ms,
            reasoning_requests,
            reasoning_ms,
            first_token_requests,
            first_token_ms,
        )| {
            stats.requests = requests;
            stats.prompt_tokens = prompt;
            stats.completion_tokens = completion;
            stats.hit_tokens = hit;
            stats.miss_tokens = (prompt - hit).max(0);
            stats.cache_reported_requests = reported;
            stats.cache_hit_requests = hit_requests;
            stats.failed_requests = failed;
            stats.total_request_ms = total_ms;
            stats.reasoning_requests = reasoning_requests;
            stats.reasoning_ms_total = reasoning_ms;
            stats.first_token_requests = first_token_requests;
            stats.first_token_ms_total = first_token_ms;
        },
    )
    .map_err(|e| e.to_string())?;

    // Failed requests store no usage, so the context in use is the prompt of the
    // newest row that actually reported one.
    stats.context_tokens = db
        .query_row(
            "SELECT COALESCE(prompt_tokens,0) FROM api_requests \
             WHERE session_id = ?1 AND prompt_tokens > 0 ORDER BY id DESC LIMIT 1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    db.query_row(
        &format!(
            "SELECT COUNT(*), \
             COALESCE(SUM(CASE WHEN interaction_type = 'llm_error' THEN 1 ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN interaction_type IN {TOOL_INTERACTION_TYPES} THEN 1 ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN interaction_type IN {TOOL_FAILURE_TYPES} THEN 1 ELSE 0 END),0), \
             COALESCE(SUM(CASE WHEN interaction_type IN {TOOL_INTERACTION_TYPES} \
                 THEN duration_ms ELSE 0 END),0) \
             FROM interaction_log WHERE session_id = ?1"
        ),
        rusqlite::params![session_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    )
    .map(|(interactions, llm_errors, tool_calls, tool_failures, tool_ms)| {
        stats.interactions = interactions;
        stats.llm_errors = llm_errors;
        stats.tool_calls = tool_calls;
        stats.tool_failures = tool_failures;
        stats.total_tool_ms = tool_ms;
    })
    .map_err(|e| e.to_string())?;

    let mut stmt = db
        .prepare(&format!(
            "SELECT COALESCE(action_name,''), COUNT(*), \
             COALESCE(SUM(CASE WHEN interaction_type IN {TOOL_FAILURE_TYPES} THEN 1 ELSE 0 END),0), \
             COALESCE(SUM(duration_ms),0) \
             FROM interaction_log WHERE session_id = ?1 \
             AND interaction_type IN {TOOL_INTERACTION_TYPES} \
             GROUP BY COALESCE(action_name,'') ORDER BY 2 DESC, 1 ASC LIMIT {TOOL_RANKING_LIMIT}"
        ))
        .map_err(|e| e.to_string())?;
    stats.tools = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok(ToolUsageStat {
                name: row.get(0)?,
                calls: row.get(1)?,
                failures: row.get(2)?,
                duration_ms: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|row| row.ok())
        .collect();

    Ok(stats)
}

/// Session analysis for the Monitor panel's bottom strip.
#[tauri::command]
pub fn get_session_stats(
    session_id: String,
    state: tauri::State<'_, crate::AppState>,
) -> Result<SessionStats, String> {
    let (mut stats, model) = {
        let db = state.db.lock().unwrap();
        let model: String = db
            .query_row(
                "SELECT COALESCE(model,'') FROM api_requests WHERE session_id = ?1 \
                 AND prompt_tokens > 0 ORDER BY id DESC LIMIT 1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .unwrap_or_default();
        (session_stats(&db, &session_id, 0)?, model)
    };
    // The config guard is taken *after* the database guard is dropped: chat
    // completion holds `config` while it locks `db`, so nesting them the other
    // way round could deadlock two threads.
    stats.context_window = {
        let config = state.config.lock().unwrap();
        let model = if model.is_empty() {
            config.model.clone()
        } else {
            model
        };
        config
            .model_context_lengths
            .get(&model)
            .copied()
            .or_else(|| {
                config
                    .model_metadata
                    .get(&model)
                    .and_then(|meta| meta.context_length)
            })
            .unwrap_or(0) as i64
    };
    Ok(stats)
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
                attachments TEXT, \
                injections TEXT, \
                error TEXT, \
                status TEXT\
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE session_meta (\
                session_id TEXT PRIMARY KEY, \
                title TEXT, \
                favorite INTEGER NOT NULL DEFAULT 0, \
                archived INTEGER NOT NULL DEFAULT 0, \
                embedded_roles TEXT NOT NULL DEFAULT ''\
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

    /// Retrying a failed turn deletes what the attempt produced after the user
    /// message, and nothing else: the retried message itself survives, earlier
    /// turns stay, and another session's history is untouched.
    #[test]
    fn delete_history_after_keeps_the_cutoff_and_other_sessions() {
        let conn = test_conn();
        insert(&conn, "retry", "user", "first question");
        insert(&conn, "retry", "assistant", "first answer");
        insert(&conn, "retry", "user", "retried question");
        insert(&conn, "retry", "assistant", "failed attempt 1");
        insert(&conn, "retry", "assistant", "failed attempt 2");
        insert(&conn, "other", "user", "other question");
        insert(&conn, "other", "assistant", "other answer");

        let cutoff: i64 = conn
            .query_row(
                "SELECT id FROM history WHERE session_id = 'retry' AND content = 'retried question'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let removed = delete_history_after(&conn, "retry", cutoff).unwrap();
        assert_eq!(removed, 2, "both failed assistant rows are removed");

        let kept = query_session_messages(&conn, "retry").unwrap();
        assert_eq!(kept.len(), 3, "earlier turns and the retried message stay");
        assert_eq!(kept[2].content, "retried question");
        assert_eq!(
            query_session_messages(&conn, "other").unwrap().len(),
            2,
            "another session is never touched"
        );
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

    /// Mid-turn injections ride on the assistant row (`history.injections`), so
    /// the column must survive the full save → load round trip; the UI parses
    /// it straight back into the transcript.
    #[test]
    fn session_messages_round_trip_mid_turn_injections() {
        let conn = test_conn();
        let stored = r#"[{"id":"inj-1","text":"build finished","created_at_ms":1700000000000}]"#;
        conn.execute(
            "INSERT INTO history (session_id, role, content, injections) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["s", "assistant", "done", stored],
        )
        .unwrap();

        let messages = query_session_messages(&conn, "s").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].injections.as_deref(), Some(stored));
    }

    /// Partial turns persist their visible output together with the UI-only
    /// error/status so a reload can keep the reply and offer continuation,
    /// while the error text stays out of the model context.
    #[test]
    fn partial_assistant_rows_round_trip_error_and_status() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO history (session_id, role, content, reasoning_content, error, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                "s",
                "assistant",
                "partial answer",
                "partial thought",
                "rate limited",
                "error"
            ],
        )
        .unwrap();

        let messages = query_session_messages(&conn, "s").unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "partial answer");
        assert_eq!(messages[0].reasoning_content.as_deref(), Some("partial thought"));
        assert_eq!(messages[0].error.as_deref(), Some("rate limited"));
        assert_eq!(messages[0].status.as_deref(), Some("error"));
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

    /// The Monitor's session report must read token usage from `api_requests`
    /// (always written) and tool traffic from `interaction_log`, without
    /// counting a session's rows from a different session.
    #[test]
    fn session_stats_aggregates_tokens_cache_and_tool_rows() {
        let conn = test_conn();
        conn.execute(
            "CREATE TABLE api_requests (\
                id INTEGER PRIMARY KEY, session_id TEXT, model TEXT, \
                prompt_tokens INTEGER DEFAULT 0, completion_tokens INTEGER DEFAULT 0, \
                cached_tokens INTEGER, reasoning_ms INTEGER, first_token_ms INTEGER, \
                duration_ms INTEGER DEFAULT 0, error TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE interaction_log (\
                id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, interaction_type TEXT NOT NULL, \
                action_name TEXT, duration_ms INTEGER DEFAULT 0)",
            [],
        )
        .unwrap();

        let request = |session: &str,
                       prompt: i64,
                       completion: i64,
                       cached: Option<i64>,
                       reasoning: Option<i64>,
                       first_token: Option<i64>,
                       ms: i64| {
            conn.execute(
                "INSERT INTO api_requests \
                 (session_id, model, prompt_tokens, completion_tokens, cached_tokens, \
                  reasoning_ms, first_token_ms, duration_ms) \
                 VALUES (?1, 'gpt-test', ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![session, prompt, completion, cached, reasoning, first_token, ms],
            )
            .unwrap();
        };
        request("s", 1000, 100, Some(800), Some(2400), Some(2100), 500);
        request("s", 2000, 200, Some(1000), Some(3600), Some(2800), 700);
        // A round without a thinking phase (non-reasoning model / tool round)
        // must not be counted as a 0 ms think.
        request("s", 500, 50, Some(0), None, Some(400), 200);
        request("other", 9999, 999, Some(9999), Some(1), Some(1), 1);
        // A failed request reports no usage, so it must not reset the context.
        conn.execute(
            "INSERT INTO api_requests (session_id, model, error) VALUES ('s', 'gpt-test', 'boom')",
            [],
        )
        .unwrap();

        let interaction = |session: &str, kind: &str, tool: &str, ms: i64| {
            conn.execute(
                "INSERT INTO interaction_log (session_id, interaction_type, action_name, duration_ms) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![session, kind, tool, ms],
            )
            .unwrap();
        };
        interaction("s", "llm_api", "chat_completion", 500);
        interaction("s", "tool_output", "read_file", 30);
        interaction("s", "tool_output", "read_file", 20);
        interaction("s", "tool_error", "read_file", 10);
        interaction("s", "mcp_call", "mcp_search", 40);
        interaction("s", "llm_error", "chat_completion", 90);
        interaction("other", "tool_output", "read_file", 10);

        let stats = session_stats(&conn, "s", 128_000).unwrap();

        assert_eq!(stats.requests, 4, "only this session's requests");
        assert_eq!(stats.failed_requests, 1);
        assert_eq!(stats.prompt_tokens, 3500);
        assert_eq!(stats.completion_tokens, 350);
        assert_eq!(stats.hit_tokens, 1800);
        assert_eq!(stats.miss_tokens, 1700);
        assert_eq!(stats.cache_reported_requests, 3);
        assert_eq!(stats.cache_hit_requests, 2);
        assert_eq!(stats.context_tokens, 500, "newest request that reported usage");
        assert_eq!(stats.context_window, 128_000);
        assert_eq!(stats.reasoning_requests, 2, "a round without thinking is not a 0 ms think");
        assert_eq!(stats.reasoning_ms_total, 6000);
        assert_eq!(stats.first_token_requests, 3);
        assert_eq!(stats.first_token_ms_total, 5300);
        assert_eq!(stats.interactions, 6);
        assert_eq!(stats.llm_errors, 1);
        assert_eq!(stats.tool_calls, 4);
        assert_eq!(stats.tool_failures, 2);
        assert_eq!(stats.total_tool_ms, 100);
        assert_eq!(stats.tools.len(), 2);
        assert_eq!(stats.tools[0].name, "read_file");
        assert_eq!(stats.tools[0].calls, 3);
        assert_eq!(stats.tools[0].failures, 1);
        assert_eq!(stats.tools[0].duration_ms, 60);
        assert_eq!(stats.tools[1].name, "mcp_search");
        assert_eq!(stats.tools[1].failures, 1);
    }

    fn role(id: &str, prompt: &str) -> EmbeddedRole {
        EmbeddedRole {
            id: id.to_string(),
            name: format!("role-{id}"),
            description: String::new(),
            prompt: prompt.to_string(),
        }
    }

    #[test]
    fn embedded_roles_round_trip_and_ignore_blank_state() {
        let conn = test_conn();

        // No meta row at all reads as "no roles", never an error.
        assert!(get_embedded_roles(&conn, "s").is_empty());

        let roles = vec![role("a", "You are A."), role("b", "You are B.")];
        set_embedded_roles(&conn, "s", &roles).unwrap();
        assert_eq!(get_embedded_roles(&conn, "s"), roles);

        // Title/favorite updates must not clobber the snapshot.
        conn.execute(
            "UPDATE session_meta SET title = 'kept', favorite = 1 WHERE session_id = 's'",
            [],
        )
        .unwrap();
        assert_eq!(get_embedded_roles(&conn, "s"), roles);

        // An empty snapshot clears the roles but keeps the row.
        set_embedded_roles(&conn, "s", &[]).unwrap();
        assert!(get_embedded_roles(&conn, "s").is_empty());

        // Malformed JSON degrades to "no roles" instead of failing a turn.
        conn.execute(
            "UPDATE session_meta SET embedded_roles = 'not json' WHERE session_id = 's'",
            [],
        )
        .unwrap();
        assert!(get_embedded_roles(&conn, "s").is_empty());
    }

    #[test]
    fn embedded_roles_copy_to_a_forked_session() {
        let conn = test_conn();
        let roles = vec![role("a", "You are A.")];
        set_embedded_roles(&conn, "source", &roles).unwrap();

        copy_embedded_roles(&conn, "source", "fork").unwrap();
        assert_eq!(get_embedded_roles(&conn, "fork"), roles);

        // Copying a session without roles leaves the target untouched.
        copy_embedded_roles(&conn, "plain", "other").unwrap();
        assert!(get_embedded_roles(&conn, "other").is_empty());
    }

    /// The Monitor must transfer a bounded preview per row, not the payload:
    /// the previews are cut in SQL, so a huge (CJK) body can neither bloat the
    /// query result nor be split mid-character.
    #[test]
    fn interaction_list_carries_only_a_bounded_preview() {
        let conn = test_conn();
        conn.execute(
            "CREATE TABLE interaction_log (\
                id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, interaction_type TEXT NOT NULL, \
                timestamp DATETIME, actor TEXT, action_name TEXT, input_data TEXT, \
                output_data TEXT, error_message TEXT, duration_ms INTEGER DEFAULT 0, metadata TEXT)",
            [],
        )
        .unwrap();

        let body = "工".repeat(10_000);
        conn.execute(
            "INSERT INTO interaction_log \
             (session_id, interaction_type, timestamp, actor, action_name, input_data, \
              output_data, error_message, duration_ms, metadata) \
             VALUES ('s', 'tool_output', datetime('now'), 'tool', 'read_file', ?1, ?1, ?1, 12, '{}')",
            rusqlite::params![body],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interaction_log \
             (session_id, interaction_type, actor, action_name, input_data, output_data) \
             VALUES ('other', 'llm_api', 'assistant', 'chat_completion', 'x', 'y')",
            [],
        )
        .unwrap();

        let rows = query_interactions(&conn, "s").unwrap();
        assert_eq!(rows.len(), 1, "only this session's rows");
        let row = &rows[0];
        assert_eq!(row.input_preview.chars().count(), 150);
        assert_eq!(row.output_preview.chars().count(), 150);
        assert_eq!(row.error_message.chars().count(), 500);
        assert!(row.input_preview.chars().all(|c| c == '工'), "no split character");
        assert_eq!(row.action_name, "read_file");
        assert_eq!(row.duration_ms, 12);
    }
}
