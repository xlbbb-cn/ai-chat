use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::AppState;

// ─── Domain types ────────────────────────────────────────────────────────────

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

// ─── Per-session locking ─────────────────────────────────────────────────────
//
// Every read-modify-write cycle on a session's todo files is wrapped in a
// per-session mutex to prevent clobbering concurrent updates. The registry
// is a process-wide OnceLock; lock objects are reference-counted so unused
// sessions are reclaimed when the last Arc is dropped.

static SESSION_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn session_lock(session_id: &str) -> Arc<Mutex<()>> {
    let map = SESSION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().expect("session lock registry poisoned");
    guard
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn lock_session<T>(session_id: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let lock = session_lock(session_id);
    let _guard = lock.lock().expect("session todo lock poisoned");
    f()
}

// ─── Path helpers ────────────────────────────────────────────────────────────

fn todo_root(workspace: &Path) -> PathBuf {
    workspace.join("todos")
}

fn session_dir(workspace: &Path, session_id: &str) -> PathBuf {
    todo_root(workspace).join(session_id)
}

fn session_lists_dir(workspace: &Path, session_id: &str) -> PathBuf {
    session_dir(workspace, session_id).join("lists")
}

fn list_path(workspace: &Path, session_id: &str, list_id: &str) -> PathBuf {
    session_lists_dir(workspace, session_id).join(format!("{list_id}.json"))
}

fn active_marker(workspace: &Path, session_id: &str) -> PathBuf {
    session_dir(workspace, session_id).join(".active")
}

fn ensure_session_dirs(workspace: &Path, session_id: &str) -> Result<(), String> {
    fs::create_dir_all(session_lists_dir(workspace, session_id))
        .map_err(|e| format!("Failed to create todo directories: {e}"))
}

// ─── Atomic file I/O ─────────────────────────────────────────────────────────

fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Path '{}' has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create parent dir '{}': {e}", parent.display()))?;

    let file_name = path
        .file_name()
        .ok_or_else(|| format!("Invalid path '{}'", path.display()))?;
    let tmp = parent.join(format!("{}.tmp", file_name.to_string_lossy()));
    fs::write(&tmp, contents).map_err(|e| format!("Failed to write '{}': {e}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Failed to commit '{}': {e}", path.display()));
    }
    Ok(())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn find_active_list_id(workspace: &Path, session_id: &str) -> Result<Option<String>, String> {
    let marker = active_marker(workspace, session_id);
    match fs::read_to_string(&marker) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let path = list_path(workspace, session_id, trimmed);
            if path.exists() {
                Ok(Some(trimmed.to_string()))
            } else {
                // Stale marker pointing to a non-existent list — clean up.
                let _ = fs::remove_file(&marker);
                Ok(None)
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("Failed to read '{}': {err}", marker.display())),
    }
}

fn set_active_list_id(workspace: &Path, session_id: &str, list_id: &str) -> Result<(), String> {
    ensure_session_dirs(workspace, session_id)?;
    write_atomic(&active_marker(workspace, session_id), list_id)
}

fn clear_active_list_id(workspace: &Path, session_id: &str) -> Result<(), String> {
    let marker = active_marker(workspace, session_id);
    match fs::remove_file(&marker) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("Failed to remove '{}': {err}", marker.display())),
    }
}

fn load_list(workspace: &Path, session_id: &str, list_id: &str) -> Result<TodoListSummary, String> {
    let path = list_path(workspace, session_id, list_id);
    let text =
        fs::read_to_string(&path).map_err(|e| format!("Todo list '{list_id}' not found: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse todo list '{list_id}': {e}"))
}

fn save_list(workspace: &Path, list: &TodoListSummary) -> Result<(), String> {
    ensure_session_dirs(workspace, &list.session_id)?;
    let path = list_path(workspace, &list.session_id, &list.list_id);
    let json = serde_json::to_string_pretty(list)
        .map_err(|e| format!("Failed to serialize todo list: {e}"))?;
    write_atomic(&path, &json)
}

fn find_list_by_id(workspace: &Path, list_id: &str) -> Result<Option<TodoListSummary>, String> {
    let root = todo_root(workspace);
    if !root.exists() {
        return Ok(None);
    }
    for session_entry in fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let session_id = session_entry.file_name().to_string_lossy().to_string();
        if session_id.starts_with('.') {
            continue;
        }
        let candidate = list_path(workspace, &session_id, list_id);
        if candidate.exists() {
            return Ok(Some(load_list(workspace, &session_id, list_id)?));
        }
    }
    Ok(None)
}

fn find_list_containing_todo(
    workspace: &Path,
    todo_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let root = todo_root(workspace);
    if !root.exists() {
        return Ok(None);
    }
    for session_entry in fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let session_id = session_entry.file_name().to_string_lossy().to_string();
        if session_id.starts_with('.') {
            continue;
        }
        let lists_dir = session_lists_dir(workspace, &session_id);
        if !lists_dir.exists() {
            continue;
        }
        for list_file in fs::read_dir(&lists_dir)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let path = list_file.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let list: TodoListSummary = match serde_json::from_str(&text) {
                Ok(l) => l,
                Err(_) => continue,
            };
            if list.todos.iter().any(|t| t.id == todo_id) {
                return Ok(Some(list));
            }
        }
    }
    Ok(None)
}

fn recompute_counts(todos: &[TodoRecord]) -> (i64, i64, i64, i64) {
    let mut pending = 0i64;
    let mut in_progress = 0i64;
    let mut completed = 0i64;
    let mut cancelled = 0i64;
    for t in todos {
        match t.status.as_str() {
            "pending" => pending += 1,
            "in_progress" => in_progress += 1,
            "completed" => completed += 1,
            "cancelled" => cancelled += 1,
            _ => {}
        }
    }
    (pending, in_progress, completed, cancelled)
}

fn apply_counts(mut list: TodoListSummary) -> TodoListSummary {
    let total = list.todos.len() as i64;
    let (pending, in_progress, completed, cancelled) = recompute_counts(&list.todos);
    list.total = total;
    list.pending = pending;
    list.in_progress = in_progress;
    list.completed = completed;
    list.cancelled = cancelled;
    list
}

fn get_or_create_active_list(
    workspace: &Path,
    session_id: &str,
) -> Result<TodoListSummary, String> {
    if let Some(list_id) = find_active_list_id(workspace, session_id)? {
        return load_list(workspace, session_id, &list_id);
    }
    ensure_session_dirs(workspace, session_id)?;
    let now = now_iso();
    let list = TodoListSummary {
        list_id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        title: DEFAULT_LIST_TITLE.to_string(),
        description: DEFAULT_LIST_DESCRIPTION.to_string(),
        total: 0,
        pending: 0,
        in_progress: 0,
        completed: 0,
        cancelled: 0,
        created_at: now.clone(),
        updated_at: now,
        todos: Vec::new(),
    };
    save_list(workspace, &list)?;
    set_active_list_id(workspace, session_id, &list.list_id)?;
    Ok(list)
}

fn workspace_path(app: &AppHandle) -> Result<PathBuf, String> {
    let state = app.state::<AppState>();
    state
        .workspace_dir
        .lock()
        .map(|p| p.clone())
        .map_err(|e| e.to_string())
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub fn get_list(app: &AppHandle, list_id: &str) -> Result<TodoListSummary, String> {
    let workspace = workspace_path(app)?;
    find_list_by_id(&workspace, list_id)?.ok_or_else(|| format!("Todo list '{list_id}' not found."))
}

pub fn get_active_list(
    app: &AppHandle,
    session_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let workspace = workspace_path(app)?;
    let Some(list_id) = find_active_list_id(&workspace, session_id)? else {
        return Ok(None);
    };
    Ok(Some(load_list(&workspace, session_id, &list_id)?))
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
    let workspace = workspace_path(app)?;
    lock_session(session_id, || {
        let mut list = get_or_create_active_list(&workspace, session_id)?;
        let pos = list
            .todos
            .iter()
            .map(|t| t.position)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        let now = now_iso();
        let record = TodoRecord {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            list_id: list.list_id.clone(),
            title: trimmed_title.to_string(),
            description: description.trim().to_string(),
            status: TodoStatus::Pending.as_str().to_string(),
            position: pos,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
        };
        list.todos.push(record);
        list.updated_at = now_iso();
        let list = apply_counts(list);
        save_list(&workspace, &list)?;
        Ok(list)
    })
}

pub fn update_todo_status(
    app: &AppHandle,
    todo_id: &str,
    status: &str,
) -> Result<TodoRecord, String> {
    let new_status = TodoStatus::parse(status)?;
    let workspace = workspace_path(app)?;
    let list = find_list_containing_todo(&workspace, todo_id)?
        .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
    lock_session(&list.session_id, || {
        // Re-read inside the lock so we operate on the latest snapshot.
        let mut list = find_list_containing_todo(&workspace, todo_id)?
            .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
        let idx = list
            .todos
            .iter()
            .position(|t| t.id == todo_id)
            .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
        let now = now_iso();
        {
            let todo = &mut list.todos[idx];
            todo.status = new_status.as_str().to_string();
            todo.updated_at = now.clone();
            todo.completed_at = if new_status == TodoStatus::Completed {
                Some(now.clone())
            } else {
                None
            };
        }
        list.updated_at = now;
        let updated_record = list.todos[idx].clone();
        let list = apply_counts(list);
        save_list(&workspace, &list)?;
        Ok(updated_record)
    })
}

pub fn update_todo_text(
    app: &AppHandle,
    todo_id: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<TodoRecord, String> {
    if title.is_none() && description.is_none() {
        return Err("No updates provided.".to_string());
    }
    let workspace = workspace_path(app)?;
    let list = find_list_containing_todo(&workspace, todo_id)?
        .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
    lock_session(&list.session_id, || {
        let mut list = find_list_containing_todo(&workspace, todo_id)?
            .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
        let idx = list
            .todos
            .iter()
            .position(|t| t.id == todo_id)
            .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
        if let Some(t) = title {
            let trimmed = t.trim();
            if trimmed.is_empty() {
                return Err("Todo title cannot be empty.".to_string());
            }
            list.todos[idx].title = trimmed.to_string();
        }
        if let Some(d) = description {
            list.todos[idx].description = d.trim().to_string();
        }
        let now = now_iso();
        list.todos[idx].updated_at = now.clone();
        list.updated_at = now;
        let updated_record = list.todos[idx].clone();
        let list = apply_counts(list);
        save_list(&workspace, &list)?;
        Ok(updated_record)
    })
}

pub fn delete_todo(app: &AppHandle, todo_id: &str) -> Result<(), String> {
    let workspace = workspace_path(app)?;
    let list = find_list_containing_todo(&workspace, todo_id)?
        .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
    lock_session(&list.session_id, || {
        let mut list = find_list_containing_todo(&workspace, todo_id)?
            .ok_or_else(|| format!("Todo '{todo_id}' not found."))?;
        let initial_len = list.todos.len();
        list.todos.retain(|t| t.id != todo_id);
        if list.todos.len() == initial_len {
            return Err(format!("Todo '{todo_id}' not found."));
        }
        list.updated_at = now_iso();
        let list = apply_counts(list);
        save_list(&workspace, &list)?;
        Ok(())
    })
}

pub fn clear_completed(app: &AppHandle, session_id: &str) -> Result<TodoListSummary, String> {
    let workspace = workspace_path(app)?;
    lock_session(session_id, || {
        let list_id = find_active_list_id(&workspace, session_id)?
            .ok_or_else(|| "No active todo list for this session.".to_string())?;
        let mut list = load_list(&workspace, session_id, &list_id)?;
        list.todos.retain(|t| t.status != "completed");
        list.updated_at = now_iso();
        let list = apply_counts(list);
        save_list(&workspace, &list)?;
        Ok(list)
    })
}

pub fn archive_list(app: &AppHandle, list_id: &str) -> Result<(), String> {
    let workspace = workspace_path(app)?;
    let list = find_list_by_id(&workspace, list_id)?
        .ok_or_else(|| format!("Todo list '{list_id}' not found."))?;
    lock_session(&list.session_id, || {
        let active = find_active_list_id(&workspace, &list.session_id)?;
        if active.as_deref() == Some(list_id) {
            clear_active_list_id(&workspace, &list.session_id)?;
        }
        Ok(())
    })
}

pub fn new_list(
    app: &AppHandle,
    session_id: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<TodoListSummary, String> {
    let workspace = workspace_path(app)?;
    lock_session(session_id, || {
        ensure_session_dirs(&workspace, session_id)?;
        let now = now_iso();
        let list = TodoListSummary {
            list_id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            title: title
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_LIST_TITLE)
                .to_string(),
            description: description.map(str::trim).unwrap_or("").to_string(),
            total: 0,
            pending: 0,
            in_progress: 0,
            completed: 0,
            cancelled: 0,
            created_at: now.clone(),
            updated_at: now,
            todos: Vec::new(),
        };
        save_list(&workspace, &list)?;
        set_active_list_id(&workspace, session_id, &list.list_id)?;
        Ok(list)
    })
}

pub fn render_active_list_for_context(workspace: &Path, session_id: &str) -> Option<String> {
    let list_id = find_active_list_id(workspace, session_id).ok().flatten()?;
    let summary = load_list(workspace, session_id, &list_id).ok()?;
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

// ─── Tauri commands ──────────────────────────────────────────────────────────

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
    use std::fs;
    use uuid::Uuid;

    fn make_temp_workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-chat-todos-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_record(list_id: &str, session_id: &str, status: &str) -> TodoRecord {
        TodoRecord {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            list_id: list_id.to_string(),
            title: "Test todo".to_string(),
            description: "".to_string(),
            status: status.to_string(),
            position: 0,
            created_at: "2024-01-01T00:00:00+00:00".to_string(),
            updated_at: "2024-01-01T00:00:00+00:00".to_string(),
            completed_at: None,
        }
    }

    fn sample_list(list_id: &str, session_id: &str, todos: Vec<TodoRecord>) -> TodoListSummary {
        let total = todos.len() as i64;
        let (pending, in_progress, completed, cancelled) = recompute_counts(&todos);
        TodoListSummary {
            list_id: list_id.to_string(),
            session_id: session_id.to_string(),
            title: "Working plan".to_string(),
            description: "".to_string(),
            total,
            pending,
            in_progress,
            completed,
            cancelled,
            created_at: "2024-01-01T00:00:00+00:00".to_string(),
            updated_at: "2024-01-01T00:00:00+00:00".to_string(),
            todos,
        }
    }

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

    #[test]
    fn path_helpers_locate_session_files() {
        let workspace = make_temp_workspace("paths");
        let session = "sess-abc";
        let list_id = "list-xyz";

        assert_eq!(todo_root(&workspace), workspace.join("todos"));
        assert_eq!(
            session_dir(&workspace, session),
            workspace.join("todos").join(session)
        );
        assert_eq!(
            session_lists_dir(&workspace, session),
            workspace.join("todos").join(session).join("lists")
        );
        assert_eq!(
            list_path(&workspace, session, list_id),
            workspace
                .join("todos")
                .join(session)
                .join("lists")
                .join(format!("{list_id}.json"))
        );
        assert_eq!(
            active_marker(&workspace, session),
            workspace.join("todos").join(session).join(".active")
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let workspace = make_temp_workspace("atomic");
        let path = workspace.join("hello.txt");
        fs::write(&path, "first").unwrap();
        write_atomic(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        // No leftover .tmp file
        assert!(!workspace.join("hello.txt.tmp").exists());
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn save_and_load_round_trip() {
        let workspace = make_temp_workspace("roundtrip");
        let list_id = "list-1";
        let session = "sess-1";
        let original = sample_list(
            list_id,
            session,
            vec![
                sample_record(list_id, session, "pending"),
                sample_record(list_id, session, "in_progress"),
                sample_record(list_id, session, "completed"),
                sample_record(list_id, session, "cancelled"),
            ],
        );
        save_list(&workspace, &original).unwrap();
        let loaded = load_list(&workspace, session, list_id).unwrap();
        assert_eq!(loaded.todos.len(), 4);
        assert_eq!(loaded.total, 4);
        assert_eq!(loaded.pending, 1);
        assert_eq!(loaded.in_progress, 1);
        assert_eq!(loaded.completed, 1);
        assert_eq!(loaded.cancelled, 1);
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn find_active_list_id_returns_none_for_empty_workspace() {
        let workspace = make_temp_workspace("empty");
        assert!(find_active_list_id(&workspace, "missing-session")
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn set_active_list_id_persists_to_marker() {
        let workspace = make_temp_workspace("marker");
        let session = "sess-marker";
        let list_id = "list-1";
        // Create the actual list file so the marker isn't considered stale.
        let list = sample_list(list_id, session, vec![]);
        save_list(&workspace, &list).unwrap();
        set_active_list_id(&workspace, session, list_id).unwrap();
        assert_eq!(
            find_active_list_id(&workspace, session).unwrap(),
            Some(list_id.to_string())
        );
        clear_active_list_id(&workspace, session).unwrap();
        assert!(find_active_list_id(&workspace, session).unwrap().is_none());
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn find_list_containing_todo_searches_all_sessions() {
        let workspace = make_temp_workspace("find-todo");
        let list_a = sample_list(
            "list-A",
            "sess-A",
            vec![sample_record("list-A", "sess-A", "pending")],
        );
        let list_b = sample_list(
            "list-B",
            "sess-B",
            vec![sample_record("list-B", "sess-B", "completed")],
        );
        save_list(&workspace, &list_a).unwrap();
        save_list(&workspace, &list_b).unwrap();

        let target_id = list_b.todos[0].id.clone();
        let found = find_list_containing_todo(&workspace, &target_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.list_id, "list-B");
        assert_eq!(found.session_id, "sess-B");

        // Unknown todo id returns None
        assert!(find_list_containing_todo(&workspace, "does-not-exist")
            .unwrap()
            .is_none());

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn apply_counts_recomputes_from_todos() {
        let list = sample_list(
            "L",
            "S",
            vec![
                sample_record("L", "S", "pending"),
                sample_record("L", "S", "pending"),
                sample_record("L", "S", "completed"),
            ],
        );
        let applied = apply_counts(list);
        assert_eq!(applied.total, 3);
        assert_eq!(applied.pending, 2);
        assert_eq!(applied.completed, 1);
        assert_eq!(applied.in_progress, 0);
        assert_eq!(applied.cancelled, 0);
    }

    #[test]
    fn session_lock_serializes_concurrent_writers() {
        use std::sync::Arc;
        use std::thread;
        let session = "concurrent-session";
        let workspace = make_temp_workspace("concurrent");
        let list = sample_list("L", session, vec![sample_record("L", session, "pending")]);
        save_list(&workspace, &list).unwrap();
        set_active_list_id(&workspace, session, "L").unwrap();

        let workspace = Arc::new(workspace);
        let mut handles = Vec::new();
        for _ in 0..4 {
            let workspace = Arc::clone(&workspace);
            handles.push(thread::spawn(move || {
                lock_session(session, || {
                    let mut current = load_list(&workspace, session, "L").unwrap();
                    current.todos.push(sample_record("L", session, "pending"));
                    current.updated_at = now_iso();
                    let updated = apply_counts(current);
                    save_list(&workspace, &updated)
                })
                .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let final_list = load_list(&workspace, session, "L").unwrap();
        // 1 initial + 4 appended = 5, proving all writes were observed.
        assert_eq!(final_list.todos.len(), 5);
        let _ = fs::remove_dir_all(workspace.as_ref());
    }
}
