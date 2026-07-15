use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    fn as_str(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(TodoStatus::Pending),
            "in_progress" | "in-progress" | "inprogress" => Ok(TodoStatus::InProgress),
            "completed" | "done" => Ok(TodoStatus::Completed),
            "cancelled" | "canceled" | "skipped" => Ok(TodoStatus::Cancelled),
            other => Err(format!(
                "Unsupported todo status '{}'. Expected one of: pending, in_progress, completed, cancelled.",
                other
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoRecord {
    pub id: String,
    pub session_id: String,
    pub list_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoListSummary {
    pub list_id: String,
    pub session_id: String,
    pub title: String,
    pub description: String,
    pub total: i64,
    pub pending: i64,
    pub in_progress: i64,
    pub completed: i64,
    pub cancelled: i64,
    pub created_at: String,
    pub updated_at: String,
    pub todos: Vec<TodoRecord>,
}

const DEFAULT_LIST_TITLE: &str = "Working plan";
const DEFAULT_LIST_DESCRIPTION: &str =
    "Auto-generated todo list for tracking complex multi-step work in this session.";

fn ensure_schema(db: &Connection) -> Result<(), String> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS todo_lists (
            list_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'active',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    db.execute(
        "CREATE TABLE IF NOT EXISTS todos (
            id TEXT PRIMARY KEY,
            list_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            position INTEGER NOT NULL DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            completed_at DATETIME,
            FOREIGN KEY(list_id) REFERENCES todo_lists(list_id) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_todos_list_position
         ON todos (list_id, position)",
        [],
    )
    .map_err(|e| e.to_string())?;

    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_todos_session_status
         ON todos (session_id, status)",
        [],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoRecord> {
    Ok(TodoRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        list_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        status: row.get(5)?,
        position: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        completed_at: row.get(9)?,
    })
}

fn find_active_list_id(db: &Connection, session_id: &str) -> Result<Option<String>, String> {
    let mut stmt = db
        .prepare(
            "SELECT list_id FROM todo_lists
             WHERE session_id = ?1 AND status = 'active'
             ORDER BY updated_at DESC LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![session_id]).map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(Some(row.get(0).map_err(|e| e.to_string())?))
    } else {
        Ok(None)
    }
}

fn next_position(db: &Connection, list_id: &str) -> Result<i64, String> {
    let mut stmt = db
        .prepare("SELECT COALESCE(MAX(position), -1) + 1 FROM todos WHERE list_id = ?1")
        .map_err(|e| e.to_string())?;
    let pos: i64 = stmt
        .query_row(params![list_id], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    Ok(pos)
}

fn get_or_create_active_list(db: &Connection, session_id: &str) -> Result<String, String> {
    if let Some(existing) = find_active_list_id(db, session_id)? {
        return Ok(existing);
    }
    let list_id = Uuid::new_v4().to_string();
    db.execute(
        "INSERT INTO todo_lists (list_id, session_id, title, description, status)
         VALUES (?1, ?2, ?3, ?4, 'active')",
        params![
            list_id,
            session_id,
            DEFAULT_LIST_TITLE,
            DEFAULT_LIST_DESCRIPTION
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(list_id)
}

fn load_todos(db: &Connection, list_id: &str) -> Result<Vec<TodoRecord>, String> {
    let mut stmt = db
        .prepare(
            "SELECT id, session_id, list_id, title, description, status, position,
                    created_at, updated_at, completed_at
             FROM todos
             WHERE list_id = ?1
             ORDER BY position ASC, created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![list_id], row_to_record)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn summarize(db: &Connection, list_id: &str) -> Result<TodoListSummary, String> {
    let mut stmt = db
        .prepare(
            "SELECT list_id, session_id, title, description, created_at, updated_at
             FROM todo_lists WHERE list_id = ?1 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![list_id]).map_err(|e| e.to_string())?;
    let row = rows
        .next()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Todo list '{}' not found.", list_id))?;
    let todos = load_todos(db, list_id)?;
    let total = todos.len() as i64;
    let pending = todos.iter().filter(|t| t.status == "pending").count() as i64;
    let in_progress = todos.iter().filter(|t| t.status == "in_progress").count() as i64;
    let completed = todos.iter().filter(|t| t.status == "completed").count() as i64;
    let cancelled = todos.iter().filter(|t| t.status == "cancelled").count() as i64;
    Ok(TodoListSummary {
        list_id: row.get(0).map_err(|e| e.to_string())?,
        session_id: row.get(1).map_err(|e| e.to_string())?,
        title: row.get(2).map_err(|e| e.to_string())?,
        description: row.get(3).map_err(|e| e.to_string())?,
        total,
        pending,
        in_progress,
        completed,
        cancelled,
        created_at: row.get(4).map_err(|e| e.to_string())?,
        updated_at: row.get(5).map_err(|e| e.to_string())?,
        todos,
    })
}

pub fn get_list(app: &AppHandle, list_id: &str) -> Result<TodoListSummary, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    summarize(&db, list_id)
}

pub fn get_active_list(
    app: &AppHandle,
    session_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    match find_active_list_id(&db, session_id)? {
        Some(list_id) => Ok(Some(summarize(&db, &list_id)?)),
        None => Ok(None),
    }
}

pub fn add_todo(
    app: &AppHandle,
    session_id: &str,
    title: &str,
    description: &str,
) -> Result<TodoListSummary, String> {
    let trimmed_title = title.trim();
    if trimmed_title.is_empty() {
        return Err("Todo title cannot be empty.".to_string());
    }
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    let list_id = get_or_create_active_list(&db, session_id)?;
    let id = Uuid::new_v4().to_string();
    let pos = next_position(&db, &list_id)?;
    db.execute(
        "INSERT INTO todos (id, list_id, session_id, title, description, status, position)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
        params![
            id,
            list_id,
            session_id,
            trimmed_title,
            description.trim(),
            pos
        ],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "UPDATE todo_lists SET updated_at = CURRENT_TIMESTAMP WHERE list_id = ?1",
        params![list_id],
    )
    .map_err(|e| e.to_string())?;
    summarize(&db, &list_id)
}

pub fn update_todo_status(
    app: &AppHandle,
    todo_id: &str,
    status: &str,
) -> Result<TodoRecord, String> {
    let new_status = TodoStatus::parse(status)?;
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    let updated = db
        .execute(
            "UPDATE todos SET
                status = ?1,
                updated_at = CURRENT_TIMESTAMP,
                completed_at = CASE WHEN ?1 = 'completed' THEN CURRENT_TIMESTAMP
                                    WHEN ?1 IN ('pending','in_progress','cancelled') THEN NULL
                                    ELSE completed_at END
             WHERE id = ?2",
            params![new_status.as_str(), todo_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err(format!("Todo '{}' not found.", todo_id));
    }
    let mut stmt = db
        .prepare(
            "SELECT id, session_id, list_id, title, description, status, position,
                    created_at, updated_at, completed_at
             FROM todos WHERE id = ?1 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let record = stmt
        .query_row(params![todo_id], row_to_record)
        .map_err(|e| format!("Todo '{}' not found: {}", todo_id, e))?;
    db.execute(
        "UPDATE todo_lists SET updated_at = CURRENT_TIMESTAMP
         WHERE list_id = (SELECT list_id FROM todos WHERE id = ?1)",
        params![todo_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(record)
}

pub fn update_todo_text(
    app: &AppHandle,
    todo_id: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<TodoRecord, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    let mut sets: Vec<&str> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    if let Some(t) = title {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            return Err("Todo title cannot be empty.".to_string());
        }
        sets.push("title = ?");
        values.push(trimmed.to_string());
    }
    if let Some(d) = description {
        sets.push("description = ?");
        values.push(d.trim().to_string());
    }
    if sets.is_empty() {
        return Err("No updates provided.".to_string());
    }
    let mut sql = String::from("UPDATE todos SET ");
    sql.push_str(&sets.join(", "));
    sql.push_str(", updated_at = CURRENT_TIMESTAMP WHERE id = ?");
    let mut params_vec: Vec<&dyn rusqlite::ToSql> =
        values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
    params_vec.push(&todo_id);
    let updated = db
        .execute(&sql, params_vec.as_slice())
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err(format!("Todo '{}' not found.", todo_id));
    }
    let mut stmt = db
        .prepare(
            "SELECT id, session_id, list_id, title, description, status, position,
                    created_at, updated_at, completed_at
             FROM todos WHERE id = ?1 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let record = stmt
        .query_row(params![todo_id], row_to_record)
        .map_err(|e| format!("Todo '{}' not found: {}", todo_id, e))?;
    Ok(record)
}

pub fn delete_todo(app: &AppHandle, todo_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    let deleted = db
        .execute("DELETE FROM todos WHERE id = ?1", params![todo_id])
        .map_err(|e| e.to_string())?;
    if deleted == 0 {
        return Err(format!("Todo '{}' not found.", todo_id));
    }
    Ok(())
}

pub fn clear_completed(app: &AppHandle, session_id: &str) -> Result<TodoListSummary, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    let list_id = match find_active_list_id(&db, session_id)? {
        Some(id) => id,
        None => return Err("No active todo list for this session.".to_string()),
    };
    db.execute(
        "DELETE FROM todos WHERE list_id = ?1 AND status = 'completed'",
        params![list_id],
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "UPDATE todo_lists SET updated_at = CURRENT_TIMESTAMP WHERE list_id = ?1",
        params![list_id],
    )
    .map_err(|e| e.to_string())?;
    summarize(&db, &list_id)
}

pub fn archive_list(app: &AppHandle, list_id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    let updated = db
        .execute(
            "UPDATE todo_lists SET status = 'archived', updated_at = CURRENT_TIMESTAMP
             WHERE list_id = ?1",
            params![list_id],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err(format!("Todo list '{}' not found.", list_id));
    }
    Ok(())
}

pub fn new_list(
    app: &AppHandle,
    session_id: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<TodoListSummary, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().map_err(|e| e.to_string())?;
    ensure_schema(&db)?;
    // Archive existing active list so a new active list becomes the current one.
    db.execute(
        "UPDATE todo_lists SET status = 'archived', updated_at = CURRENT_TIMESTAMP
         WHERE session_id = ?1 AND status = 'active'",
        params![session_id],
    )
    .map_err(|e| e.to_string())?;
    let list_id = Uuid::new_v4().to_string();
    let title = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_LIST_TITLE);
    let description = description.map(str::trim).unwrap_or("");
    db.execute(
        "INSERT INTO todo_lists (list_id, session_id, title, description, status)
         VALUES (?1, ?2, ?3, ?4, 'active')",
        params![list_id, session_id, title, description],
    )
    .map_err(|e| e.to_string())?;
    summarize(&db, &list_id)
}

pub fn render_active_list_for_context(session_id: &str, db: &Connection) -> Option<String> {
    ensure_schema(db).ok()?;
    let list_id = find_active_list_id(db, session_id).ok().flatten()?;
    let summary = summarize(db, &list_id).ok()?;
    if summary.todos.is_empty() {
        return None;
    }
    let mut body = String::new();
    body.push_str(&format!(
        "Active session todo list: \"{}\" ({}/{})\n",
        summary.title,
        summary.completed + summary.in_progress + summary.cancelled,
        summary.total
    ));
    body.push_str(&format!(
        "  pending={}  in_progress={}  completed={}  cancelled={}\n",
        summary.pending, summary.in_progress, summary.completed, summary.cancelled
    ));
    body.push_str("Items (id | status | title):\n");
    for todo in &summary.todos {
        let short_id = if todo.id.len() >= 8 {
            &todo.id[..8]
        } else {
            &todo.id
        };
        body.push_str(&format!(
            "  - [{}] {} | {}\n",
            short_id, todo.status, todo.title
        ));
    }
    body.push_str(
        "\nUse the `todo_*` tools (todo_add, todo_update_status, todo_list, todo_clear_completed, todo_archive) to manage this list. Re-run `todo_list` whenever you need a fresh view of progress before planning the next step."
    );
    Some(body)
}

#[tauri::command]
pub fn get_session_todo(
    session_id: String,
    app: AppHandle,
) -> Result<Option<TodoListSummary>, String> {
    get_active_list(&app, &session_id)
}

#[tauri::command]
pub fn get_todo_list(list_id: String, app: AppHandle) -> Result<TodoListSummary, String> {
    get_list(&app, &list_id)
}

#[tauri::command]
pub fn create_session_todo(
    session_id: String,
    title: String,
    description: Option<String>,
    app: AppHandle,
) -> Result<TodoListSummary, String> {
    add_todo(
        &app,
        &session_id,
        &title,
        description.unwrap_or_default().as_str(),
    )
}

#[tauri::command]
pub fn update_todo_status_cmd(
    todo_id: String,
    status: String,
    app: AppHandle,
) -> Result<TodoRecord, String> {
    update_todo_status(&app, &todo_id, &status)
}

#[tauri::command]
pub fn update_todo_text_cmd(
    todo_id: String,
    title: Option<String>,
    description: Option<String>,
    app: AppHandle,
) -> Result<TodoRecord, String> {
    update_todo_text(&app, &todo_id, title.as_deref(), description.as_deref())
}

#[tauri::command]
pub fn delete_todo_cmd(todo_id: String, app: AppHandle) -> Result<(), String> {
    delete_todo(&app, &todo_id)
}

#[tauri::command]
pub fn clear_completed_todos(
    session_id: String,
    app: AppHandle,
) -> Result<TodoListSummary, String> {
    clear_completed(&app, &session_id)
}

#[tauri::command]
pub fn archive_todo_list(list_id: String, app: AppHandle) -> Result<(), String> {
    archive_list(&app, &list_id)
}

#[tauri::command]
pub fn create_todo_list(
    session_id: String,
    title: Option<String>,
    description: Option<String>,
    app: AppHandle,
) -> Result<TodoListSummary, String> {
    new_list(&app, &session_id, title.as_deref(), description.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_status_parse() {
        assert_eq!(TodoStatus::parse("pending").unwrap(), TodoStatus::Pending);
        assert_eq!(
            TodoStatus::parse("in_progress").unwrap(),
            TodoStatus::InProgress
        );
        assert_eq!(TodoStatus::parse("DONE").unwrap(), TodoStatus::Completed);
        assert_eq!(TodoStatus::parse("skipped").unwrap(), TodoStatus::Cancelled);
        assert!(TodoStatus::parse("nope").is_err());
    }
}
