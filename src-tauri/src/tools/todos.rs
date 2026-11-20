use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use super::emit_tool_progress;
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
// per-session mutex to prevent clobbering concurrent updates — parallel
// sub-agents all share the parent chat session, so this is the hot path.
// The registry is a process-wide OnceLock holding one lock per session id
// for the lifetime of the process. Poisoned locks are recovered instead of
// panicking, so a single failed tool call cannot disable todo access for
// every agent in the session.

static SESSION_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn session_lock(session_id: &str) -> Arc<Mutex<()>> {
    let map = SESSION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn lock_session<T>(session_id: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let lock = session_lock(session_id);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

// ─── Path helpers ────────────────────────────────────────────────────────────

fn todo_root(workspace: &Path) -> PathBuf {
    crate::workspace::todos_root(workspace)
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

fn find_list_containing_todo_in_session(
    workspace: &Path,
    session_id: &str,
    todo_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let lists_dir = session_lists_dir(workspace, session_id);
    if !lists_dir.exists() {
        return Ok(None);
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

pub fn get_active_list(
    app: &AppHandle,
    session_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let workspace = workspace_path(app)?;
    active_list_snapshot(&workspace, session_id)
}

/// Active list snapshot for callers that already hold the workspace path — the
/// turn builder hands the same snapshot to the prompt block and to the UI
/// event, so the panel and the model can never disagree about the plan.
pub fn active_list_snapshot(
    workspace: &Path,
    session_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let Some(list_id) = find_active_list_id(workspace, session_id)? else {
        return Ok(None);
    };
    Ok(Some(load_list(workspace, session_id, &list_id)?))
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

// Must be called while holding the session lock. Reads the latest snapshot
// from disk (single scoped lookup) before mutating, so concurrent writers in
// the same session cannot lose updates.
fn update_todo_status_locked(
    workspace: &Path,
    session_id: &str,
    todo_id: &str,
    new_status: TodoStatus,
) -> Result<TodoRecord, String> {
    let mut list = find_list_containing_todo_in_session(workspace, session_id, todo_id)?
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
    save_list(workspace, &list)?;
    Ok(updated_record)
}

/// Update a todo in the caller's session only (single scoped lookup).
pub fn update_todo_status(
    app: &AppHandle,
    session_id: &str,
    todo_id: &str,
    status: &str,
) -> Result<TodoRecord, String> {
    let new_status = TodoStatus::parse(status)?;
    let workspace = workspace_path(app)?;
    lock_session(session_id, || {
        update_todo_status_locked(&workspace, session_id, todo_id, new_status)
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

/// Put the plan back into a resumable state after the user pressed Stop.
///
/// Nothing is running any more, so an `in_progress` item is a lie: the next
/// turn — and the goal loop, which resumes an unfinished list — would believe
/// work was still underway and resume against a state nobody is working on.
/// Every interrupted item goes back to `pending`.
///
/// `None` means there was no active list or nothing to settle, so the caller
/// only publishes a change that actually happened.
pub fn settle_interrupted(
    app: &AppHandle,
    session_id: &str,
) -> Result<Option<TodoListSummary>, String> {
    let workspace = workspace_path(app)?;
    lock_session(session_id, || {
        let Some(list_id) = find_active_list_id(&workspace, session_id)? else {
            return Ok(None);
        };
        let mut list = load_list(&workspace, session_id, &list_id)?;
        let now = now_iso();
        let mut settled = false;
        for todo in list.todos.iter_mut() {
            if todo.status == TodoStatus::InProgress.as_str() {
                todo.status = TodoStatus::Pending.as_str().to_string();
                todo.updated_at = now.clone();
                todo.completed_at = None;
                settled = true;
            }
        }
        if !settled {
            return Ok(None);
        }
        list.updated_at = now;
        let list = apply_counts(list);
        save_list(&workspace, &list)?;
        Ok(Some(list))
    })
}

// ─── Q&A (the `ask` action) ──────────────────────────────────────────────────
//
// The model hands over a list of questions; the tool opens a form in the UI
// (tab + cards, one card per question) and BLOCKS the tool loop until the user
// submits answers, dismisses the form, stops the turn or the timeout elapses.
// The blocking mechanics mirror `request_tool_confirmation`: a one-shot
// channel slot in `AppState` addressed by request id, serialized by a tokio
// mutex so only one form is ever open, and a 100 ms poll of the turn's cancel
// flag so Stop ends the wait promptly.

/// Bounds for one `ask` call. A form the user must read should never be huge,
/// and the result is echoed back into the prompt, so both sides stay capped.
pub(super) const ASK_MAX_QUESTIONS: usize = 8;
const ASK_MAX_OPTIONS: usize = 12;
const ASK_TITLE_MAX_CHARS: usize = 500;
const ASK_DESCRIPTION_MAX_CHARS: usize = 2000;
const ASK_OPTION_MAX_CHARS: usize = 300;
const ASK_ID_MAX_CHARS: usize = 64;
/// The user may be away from the desk; 15 minutes is generous and Stop always
/// ends the wait earlier.
const ASK_TIMEOUT_SECS: u64 = 900;

/// One question as the model passed it in (raw shape, serde-tolerant).
#[derive(Debug, Deserialize)]
pub(super) struct TodoAskQuestionInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub allow_custom: Option<bool>,
}

/// One question as it reaches the UI and as the answer is matched back.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(super) struct TodoAskQuestion {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Selectable answers; `None` = free-text question.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// A manual input is offered beside (or instead of) the options.
    pub allow_custom: bool,
}

/// One answer as the frontend submits it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoAskAnswer {
    pub id: String,
    pub value: String,
}

/// The reply of one pending form: `Some(answers)` on submit, `None` on
/// dismissal (the panel's cancel button).
#[derive(Debug)]
pub struct TodoAskReply {
    pub answers: Option<Vec<TodoAskAnswer>>,
}

/// One question's answer as it is returned to the model.
#[derive(Debug, Serialize)]
struct TodoAskAnswerOut {
    id: String,
    question: String,
    answer: String,
    /// `option` when it was one of the listed choices, `custom` for typed text.
    source: &'static str,
}

/// Terminal outcomes of a pending form. Only malformed *arguments* are tool
/// errors; the wait itself always ends in a structured JSON result so the
/// tool-failure guard (`turn_guard::tool_output_is_error`) never counts a
/// user who was away or pressed Stop as "the todos tool failing".
#[derive(Debug)]
enum AskOutcome {
    Answered(Vec<TodoAskAnswerOut>),
    Dismissed,
    Timeout,
    Cancelled,
}

/// Char-safe truncation — the model may hand over multi-byte text, and byte
/// slicing would panic on non-boundary offsets.
fn take_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn trim_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Normalise and validate the `questions` argument of the `ask` action.
///
/// Trims strings, drops empty options, caps counts and lengths on char
/// boundaries, and dedupes ids (a missing or colliding id becomes `q1`,
/// `q2`, …). Returns a human-readable error when the shape is unusable.
pub(super) fn normalize_ask_questions(
    raw: Option<&Value>,
) -> Result<Vec<TodoAskQuestion>, String> {
    let Some(raw) = raw else {
        return Err("'questions' is required for the 'ask' action — pass an array of { title, options? } objects.".to_string());
    };
    let Ok(inputs) =
        serde_json::from_value::<Vec<TodoAskQuestionInput>>(raw.clone())
    else {
        return Err(
            "'questions' must be an array of objects like { title, description?, options?, allow_custom? }."
                .to_string(),
        );
    };
    if inputs.is_empty() {
        return Err("'questions' must contain at least one question.".to_string());
    }
    if inputs.len() > ASK_MAX_QUESTIONS {
        return Err(format!(
            "'questions' may hold at most {ASK_MAX_QUESTIONS} questions per call (got {}); split the rest across further calls.",
            inputs.len()
        ));
    }

    let mut used_ids = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.into_iter().enumerate() {
        let title = take_chars(input.title.trim(), ASK_TITLE_MAX_CHARS);
        if title.is_empty() {
            return Err(format!(
                "Question {} has no title — every question needs one.",
                index + 1
            ));
        }
        let description = trim_opt(input.description.as_deref())
            .map(|d| take_chars(&d, ASK_DESCRIPTION_MAX_CHARS));

        let mut options: Option<Vec<String>> = input.options.map(|list| {
            list.into_iter()
                .filter_map(|o| trim_opt(Some(o.as_str())))
                .map(|o| take_chars(&o, ASK_OPTION_MAX_CHARS))
                .take(ASK_MAX_OPTIONS)
                .collect::<Vec<_>>()
        });
        // An option list that lost every entry to trimming is no list at all.
        if options.as_ref().is_some_and(Vec::is_empty) {
            options = None;
        }
        // Without options the manual input is the only way to answer, so it is
        // always on; with options the model opts in explicitly.
        let allow_custom = options.is_none() || input.allow_custom.unwrap_or(false);

        // Id: the model's choice when unique, else a stable `q<n>` that cannot
        // collide with anything the model supplied.
        let id = match trim_opt(input.id.as_deref()) {
            Some(id) if id.chars().count() <= ASK_ID_MAX_CHARS && used_ids.insert(id.clone()) => {
                id
            }
            _ => {
                let mut n = index + 1;
                loop {
                    let candidate = format!("q{n}");
                    if used_ids.insert(candidate.clone()) {
                        break candidate;
                    }
                    n += 1;
                }
            }
        };

        out.push(TodoAskQuestion {
            id,
            title,
            description,
            options,
            allow_custom,
        });
    }
    Ok(out)
}

/// Validate one submitted answer against its question. Unknown ids and empty
/// values are rejected; an option question only accepts a listed choice unless
/// it allows custom input.
fn validate_answer(question: &TodoAskQuestion, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("Answer for '{}' is empty.", question.id));
    }
    match &question.options {
        Some(options) if options.iter().any(|o| o == value) => Ok(value.to_string()),
        Some(_) if question.allow_custom => Ok(value.to_string()),
        Some(_) => Err(format!(
            "Answer for '{}' is not one of its options and the question does not allow custom input.",
            question.id
        )),
        None => Ok(value.to_string()),
    }
}

/// Show the question form and wait for the user.
///
/// Registered in `AppState::todo_ask` (payload + one-shot sender), announced
/// with `todo-ask`, and resolved by the `todo_ask_reply` command. The queue
/// lock is held across the whole wait so two parallel callers (a mission's
/// sub-agents) present their forms one after another instead of stacking two
/// panels.
async fn ask_user(
    app: &AppHandle,
    session_id: &str,
    call_id: Option<&str>,
    cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
    topic: Option<String>,
    questions: Vec<TodoAskQuestion>,
) -> AskOutcome {
    use std::sync::atomic::Ordering;

    let state = app.state::<AppState>();
    // One form at a time — same rule as the confirmation dialogs.
    let _queue_guard = state.confirm.todo_ask_lock.lock().await;

    let request_id = format!("todo-ask-{}", Uuid::new_v4());
    let payload = json!({
        "request_id": request_id,
        "session_id": session_id,
        "call_id": call_id,
        "topic": topic,
        "questions": questions,
    });

    // Register the reply slot *before* emitting, so an immediate submit cannot
    // find an empty slot and be dropped.
    let (tx, rx) = tokio::sync::oneshot::channel::<TodoAskReply>();
    {
        let mut slot = state.confirm.todo_ask.lock().unwrap();
        *slot = Some(crate::TodoAskPending {
            request_id: request_id.clone(),
            payload: payload.clone(),
            tx: Some(tx),
        });
    }

    let _ = app.emit(crate::events::TODO_ASK, payload);

    // Wait for the reply, the turn's cancel flag, or the timeout — whichever
    // lands first. Cancellation is polled every 100 ms like the confirmation
    // dialogs, so Stop ends the wait within one poll cycle.
    let await_reply = async {
        match cancel {
            Some(flag) => {
                tokio::select! {
                    reply = rx => reply.ok(),
                    _ = async {
                        loop {
                            if flag.load(Ordering::SeqCst) {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    } => None,
                }
            }
            None => rx.await.ok(),
        }
    };

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(ASK_TIMEOUT_SECS), await_reply)
        .await
        .ok()
        .flatten();

    // Drop our own slot entry (timeout / cancellation). A late submit then hits
    // an unknown request id and is ignored by `todo_ask_reply`.
    {
        let mut slot = state.confirm.todo_ask.lock().unwrap();
        if matches!(slot.as_ref(), Some(pending) if pending.request_id == request_id) {
            *slot = None;
        }
    }
    // Tell every window the form is gone (timeout, Stop, or a reply that came
    // from another window): a stale panel would keep asking for answers nobody
    // reads any more. The answering window already cleared its own panel.
    let _ = app.emit(crate::events::TODO_ASK_CLOSED, json!({ "request_id": request_id }));

    match outcome {
        Some(reply) => match reply.answers {
            Some(answers) => match collect_answers(&questions, answers) {
                Ok(list) => AskOutcome::Answered(list),
                // An invalid submission is treated like a dismissal plus a
                // note — the panel enforces completeness, so this is defensive
                // only and must not poison the tool-failure guard.
                Err(err) => {
                    state.logger.lock().unwrap().log(
                        "WARN",
                        &format!("todo ask '{request_id}': invalid submission — {err}"),
                    );
                    AskOutcome::Dismissed
                }
            },
            None => AskOutcome::Dismissed,
        },
        None => {
            let cancelled = cancel
                .map(|flag| flag.load(Ordering::SeqCst))
                .unwrap_or(false);
            if cancelled {
                AskOutcome::Cancelled
            } else {
                AskOutcome::Timeout
            }
        }
    }
}

/// Match submitted answers back onto the questions (by id) and validate each
/// value against its question.
fn collect_answers(
    questions: &[TodoAskQuestion],
    answers: Vec<TodoAskAnswer>,
) -> Result<Vec<TodoAskAnswerOut>, String> {
    let by_id: HashMap<&str, &str> = answers
        .iter()
        .map(|a| (a.id.as_str(), a.value.as_str()))
        .collect();
    let mut out = Vec::with_capacity(questions.len());
    for question in questions {
        let raw = by_id
            .get(question.id.as_str())
            .copied()
            .ok_or_else(|| format!("Answer for '{}' is missing.", question.id))?;
        let value = validate_answer(question, raw)?;
        let source = match &question.options {
            Some(options) if options.iter().any(|o| o == &value) => "option",
            Some(_) => "custom",
            None => "custom",
        };
        out.push(TodoAskAnswerOut {
            id: question.id.clone(),
            question: question.title.clone(),
            answer: value,
            source,
        });
    }
    Ok(out)
}

/// Format a terminal outcome as the tool result the model reads.
fn format_ask_result(outcome: AskOutcome) -> String {
    match outcome {
        AskOutcome::Answered(answers) => serde_json::to_string_pretty(&json!({
            "status": "answered",
            "answers": answers,
        }))
        .unwrap_or_else(|_| "{\"status\":\"answered\",\"answers\":[]}".to_string()),
        AskOutcome::Dismissed => serde_json::to_string_pretty(&json!({
            "status": "dismissed",
            "note": "The user dismissed the question form without answering. Do not wait or re-ask the same questions; continue with your best judgement and state the assumption you made.",
        }))
        .unwrap_or_else(|_| "{\"status\":\"dismissed\"}".to_string()),
        AskOutcome::Timeout => serde_json::to_string_pretty(&json!({
            "status": "timeout",
            "note": "No answer arrived within the wait window — the user is probably away. Continue with your best judgement; you may ask again later.",
        }))
        .unwrap_or_else(|_| "{\"status\":\"timeout\"}".to_string()),
        AskOutcome::Cancelled => serde_json::to_string_pretty(&json!({
            "status": "cancelled",
            "note": "The turn was stopped while the question was pending.",
        }))
        .unwrap_or_else(|_| "{\"status\":\"cancelled\"}".to_string()),
    }
}

// Creates a fresh list and installs it as the session's active list. Must
// be called while holding the session lock; the marker swap is the only
// activation step, so a previous list simply stops being active.
fn create_and_activate_list(
    workspace: &Path,
    session_id: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<TodoListSummary, String> {
    ensure_session_dirs(workspace, session_id)?;
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
    save_list(workspace, &list)?;
    set_active_list_id(workspace, session_id, &list.list_id)?;
    Ok(list)
}

fn rotate_active_list_locked(
    workspace: &Path,
    session_id: &str,
    title: Option<&str>,
) -> Result<TodoListSummary, String> {
    create_and_activate_list(workspace, session_id, title, None)
}

/// Archive (deactivate) the current list and start a fresh active one in a
/// single critical section. Used by the `todos` tool's `archive` action so a
/// concurrent `add` from another agent can never land in a list that is
/// created and then immediately orphaned by the marker swap.
pub fn rotate_active_list(
    app: &AppHandle,
    session_id: &str,
    title: Option<&str>,
) -> Result<TodoListSummary, String> {
    let workspace = workspace_path(app)?;
    lock_session(session_id, || {
        rotate_active_list_locked(&workspace, session_id, title)
    })
}

/// Render the `INTERNAL CONTEXT - ACTIVE TODO LIST` block for a list that was
/// already loaded (the turn builder takes one snapshot and feeds both this
/// block and the UI event from it). `None` when there is nothing worth
/// injecting: an empty list, or one where every item is done — the UI still
/// keeps showing such a list, the model just has nothing left to plan.
pub fn render_list_block(summary: &TodoListSummary) -> Option<String> {
    if summary.todos.is_empty() {
        return None;
    }
    // Skip injecting the todo list when everything is done — nothing left to track.
    if summary.completed == summary.total {
        return None;
    }
    Some(render_list_block_unchecked(summary))
}

fn render_list_block_unchecked(summary: &TodoListSummary) -> String {
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
        // Char-boundary safe: ids are UUIDs today, but a manually edited list
        // file must not be able to panic this render path.
        let short_id: String = todo.id.chars().take(8).collect();
        body.push_str(&format!(
            "  - [{}] {} | {}\n",
            short_id, todo.status, todo.title
        ));
    }
    body.push_str(
        "\nUse the `todos` tool (actions: list, add, update_status, clear_completed, archive, ask) to manage this list. Re-run action `list` whenever you need a fresh view of progress before planning the next step."
    );
    body
}

// ─── Tool handler (LLM interface) ────────────────────────────────────────────

/// Handle the `todos` tool call — a single action-based entry point modelled
/// on `file_actions`. The legacy per-action tool names (`todo_add`, …) are
/// also routed here so conversations continued from an older session keep
/// working; they map onto their equivalent action.
///
/// Every mutating action emits `todo-state` so the UI mirrors the list live.
/// The `ask` action additionally opens the interactive question form and
/// awaits the user's answers.
pub(super) async fn handle_todos_tool(
    app: &AppHandle,
    name: &str,
    args: &Value,
    session_id: Option<&str>,
    // Current autonomous mission id, so progress lines reach the task row
    // instead of the main chat's `tool-call` transcript.
    mission_id: Option<&str>,
    // `tool_call.id` of the call being executed (see `emit_tool_progress`).
    call_id: Option<&str>,
    // Cancel flag of the turn this call belongs to: Stop pressed while the
    // form is open must end the wait instead of blocking the unwind.
    cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    let action = args["action"].as_str().unwrap_or(match name {
        "todo_add" => "add",
        "todo_update_status" => "update_status",
        "todo_list" => "list",
        "todo_clear_completed" => "clear_completed",
        "todo_archive" => "archive",
        _ => "",
    });

    match action {
        "list" => {
            let Some(session_id) = session_id else {
                return "Error: todos requires an active chat session.".to_string();
            };
            let _ = emit_tool_progress(app, mission_id, session_id, call_id, "*Listing active todos*\n".to_string());
            match get_active_list(app, session_id) {
                Ok(Some(summary)) => {
                    serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "null".to_string())
                }
                Ok(None) => {
                    "No active todo list for this session. Call `todos` with action `add` to start one."
                        .to_string()
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "add" => {
            let Some(session_id) = session_id else {
                return "Error: todos requires an active chat session.".to_string();
            };
            let title = args["title"].as_str().unwrap_or("").to_string();
            if title.trim().is_empty() {
                return "Error: 'title' is required for the 'add' action.".to_string();
            }
            let description = args["description"].as_str().unwrap_or("").to_string();
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!("*Adding todo: {}*\n", title.trim()),
            );
            match add_todo(app, session_id, &title, &description) {
                Ok(summary) => {
                    let payload = json!({
                        "type": "todo_state",
                        "list_id": summary.list_id,
                        "session_id": summary.session_id,
                        "summary": &summary,
                    });
                    let _ = app.emit(crate::events::TODO_STATE, payload.clone());
                    serde_json::to_string_pretty(&summary).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "update_status" => {
            let Some(session_id) = session_id else {
                return "Error: todos requires an active chat session.".to_string();
            };
            let todo_id = args["todo_id"].as_str().unwrap_or("");
            let status = args["status"].as_str().unwrap_or("");
            if todo_id.is_empty() {
                return "Error: 'todo_id' is required for the 'update_status' action.".to_string();
            }
            // Truncate on char boundaries only: the model may pass multi-byte
            // utf-8 ids, and byte slicing would panic on non-boundary offsets.
            let id_preview: String = todo_id.chars().take(8).collect();
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!("*Updating todo status: {} → {}*\n", id_preview, status),
            );
            match update_todo_status(app, session_id, todo_id, status) {
                Ok(record) => {
                    let _ = app.emit(
                        crate::events::TODO_STATE,
                        json!({
                            "type": "todo_updated",
                            "todo": &record,
                        }),
                    );
                    serde_json::to_string_pretty(&record).unwrap_or_else(|_| record.id.clone())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "clear_completed" => {
            let Some(session_id) = session_id else {
                return "Error: todos requires an active chat session.".to_string();
            };
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                "*Clearing completed todos*\n".to_string(),
            );
            match clear_completed(app, session_id) {
                Ok(summary) => {
                    let _ = app.emit(
                        crate::events::TODO_STATE,
                        json!({
                            "type": "todo_cleared",
                            "list_id": summary.list_id,
                            // Ship the authoritative snapshot so the UI never has
                            // to prune locally: parallel agents may write between
                            // the clear and the event being handled.
                            "summary": &summary,
                        }),
                    );
                    serde_json::to_string_pretty(&summary)
                        .unwrap_or_else(|_| summary.list_id.clone())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "archive" => {
            let Some(session_id) = session_id else {
                return "Error: todos requires an active chat session.".to_string();
            };
            let new_title = args["new_title"].as_str();
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!(
                    "*Archiving todo list, starting new: {}*\n",
                    new_title.unwrap_or("Working plan")
                ),
            );
            // Rotate in a single critical section: a concurrent `add` from
            // another agent can no longer create a list that is immediately
            // orphaned by the marker swap.
            match rotate_active_list(app, session_id, new_title) {
                Ok(summary) => {
                    let _ = app.emit(
                        crate::events::TODO_STATE,
                        json!({
                            "type": "todo_list_changed",
                            "list_id": summary.list_id,
                            "summary": &summary,
                        }),
                    );
                    serde_json::to_string_pretty(&summary)
                        .unwrap_or_else(|_| summary.list_id.clone())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "ask" => {
            let Some(session_id) = session_id else {
                return "Error: todos requires an active chat session.".to_string();
            };
            let questions = match normalize_ask_questions(args.get("questions")) {
                Ok(questions) => questions,
                Err(err) => return format!("Error: {err}"),
            };
            let topic = trim_opt(args["topic"].as_str())
                .map(|t| take_chars(&t, 200));
            let _ = emit_tool_progress(
                app,
                mission_id,
                session_id,
                call_id,
                format!(
                    "*Asking the user {} question(s)*\n",
                    questions.len()
                ),
            );
            let outcome =
                ask_user(app, session_id, call_id, cancel, topic, questions).await;
            format_ask_result(outcome)
        }
        _ => format!(
            "Unknown todos action '{}'. Supported: list, add, update_status, clear_completed, archive, ask.",
            action
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn make_temp_workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rabbit-chat-studio-todos-{label}-{}", Uuid::new_v4()));
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

        assert_eq!(todo_root(&workspace), workspace.join(".data").join("todos"));
        assert_eq!(
            session_dir(&workspace, session),
            workspace.join(".data").join("todos").join(session)
        );
        assert_eq!(
            session_lists_dir(&workspace, session),
            workspace.join(".data").join("todos").join(session).join("lists")
        );
        assert_eq!(
            list_path(&workspace, session, list_id),
            workspace
                .join(".data")
                .join("todos")
                .join(session)
                .join("lists")
                .join(format!("{list_id}.json"))
        );
        assert_eq!(
            active_marker(&workspace, session),
            workspace
                .join(".data")
                .join("todos")
                .join(session)
                .join(".active")
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

    #[test]
    fn find_list_containing_todo_in_session_is_scoped() {
        let workspace = make_temp_workspace("scoped-find");
        let list_a = sample_list(
            "list-A",
            "sess-A",
            vec![sample_record("list-A", "sess-A", "pending")],
        );
        let list_b = sample_list(
            "list-B",
            "sess-B",
            vec![sample_record("list-B", "sess-B", "pending")],
        );
        save_list(&workspace, &list_a).unwrap();
        save_list(&workspace, &list_b).unwrap();

        let todo_a = list_a.todos[0].id.clone();
        // A todo owned by session A must not be found from session B.
        assert!(
            find_list_containing_todo_in_session(&workspace, "sess-B", &todo_a)
                .unwrap()
                .is_none()
        );
        let found = find_list_containing_todo_in_session(&workspace, "sess-A", &todo_a)
            .unwrap()
            .unwrap();
        assert_eq!(found.list_id, "list-A");
        // Unknown sessions are simply empty.
        assert!(
            find_list_containing_todo_in_session(&workspace, "missing", &todo_a)
                .unwrap()
                .is_none()
        );
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn update_todo_status_locked_updates_only_its_own_session() {
        let workspace = make_temp_workspace("scoped-update");
        let session = "sess-update";
        let list = sample_list(
            "L",
            session,
            vec![
                sample_record("L", session, "pending"),
                sample_record("L", session, "pending"),
            ],
        );
        save_list(&workspace, &list).unwrap();
        let target = list.todos[0].id.clone();

        let updated =
            update_todo_status_locked(&workspace, session, &target, TodoStatus::Completed).unwrap();
        assert_eq!(updated.status, "completed");
        assert!(updated.completed_at.is_some());

        let reloaded = load_list(&workspace, session, "L").unwrap();
        assert_eq!(reloaded.completed, 1);
        assert_eq!(reloaded.pending, 1);
        assert_eq!(reloaded.total, 2);

        // Unknown ids are rejected instead of silently succeeding.
        assert!(update_todo_status_locked(
            &workspace,
            session,
            "does-not-exist",
            TodoStatus::Completed
        )
        .is_err());
        // A todo from another session cannot be updated through this session.
        assert!(update_todo_status_locked(
            &workspace,
            "other-session",
            &target,
            TodoStatus::Completed
        )
        .is_err());

        // Status can be reverted; completion timestamp is cleared again.
        let reverted =
            update_todo_status_locked(&workspace, session, &target, TodoStatus::Pending).unwrap();
        assert!(reverted.completed_at.is_none());
        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn rotate_active_list_switches_marker_and_keeps_old_list() {
        let workspace = make_temp_workspace("rotate");
        let session = "sess-rotate";
        let old = sample_list(
            "old-list",
            session,
            vec![sample_record("old-list", session, "pending")],
        );
        save_list(&workspace, &old).unwrap();
        set_active_list_id(&workspace, session, "old-list").unwrap();

        let fresh = rotate_active_list_locked(&workspace, session, Some("Round 2")).unwrap();
        assert_eq!(fresh.title, "Round 2");
        assert!(fresh.todos.is_empty());
        assert_ne!(fresh.list_id, "old-list");
        assert_eq!(
            find_active_list_id(&workspace, session).unwrap(),
            Some(fresh.list_id.clone())
        );

        // The previous list stays on disk (archived) with its items intact.
        let archived = load_list(&workspace, session, "old-list").unwrap();
        assert_eq!(archived.todos.len(), 1);

        // Rotating a session with no active list simply creates one.
        let another = rotate_active_list_locked(&workspace, "fresh-session", None).unwrap();
        assert_eq!(another.title, DEFAULT_LIST_TITLE);
        assert_eq!(
            find_active_list_id(&workspace, "fresh-session").unwrap(),
            Some(another.list_id.clone())
        );
        let _ = fs::remove_dir_all(&workspace);
    }

    // ─── Q&A (`ask` action) tests ────────────────────────────────────────────

    #[test]
    fn normalize_ask_questions_rejects_missing_and_empty() {
        assert!(normalize_ask_questions(None).is_err());
        assert!(normalize_ask_questions(Some(&serde_json::json!([]))).is_err());
        assert!(normalize_ask_questions(Some(&serde_json::json!("nope"))).is_err());
        assert!(normalize_ask_questions(Some(&serde_json::json!([{}]))).is_err());
        // A blank title is the same as no title.
        assert!(normalize_ask_questions(Some(&serde_json::json!([{"title": "   "}]))).is_err());
    }

    #[test]
    fn normalize_ask_questions_caps_counts_and_lengths() {
        let many: Vec<Value> = (0..10).map(|i| serde_json::json!({ "title": format!("Q{i}") })).collect();
        let err = normalize_ask_questions(Some(&Value::Array(many))).unwrap_err();
        assert!(err.contains("at most 8"));

        let long_title = "字".repeat(600);
        let questions =
            normalize_ask_questions(Some(&serde_json::json!([{ "title": long_title }]))).unwrap();
        // Char-boundary safe truncation, not a panic on a multi-byte slice.
        assert_eq!(questions[0].title.chars().count(), 500);

        let questions = normalize_ask_questions(Some(&serde_json::json!([{
            "title": "Pick",
            "options": ["a", " ", "", "b"],
        }])))
        .unwrap();
        // Empty options are dropped; the rest stays.
        assert_eq!(
            questions[0].options,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn normalize_ask_questions_generates_unique_ids() {
        let questions = normalize_ask_questions(Some(&serde_json::json!([
            { "title": "One", "id": "q2" },
            { "title": "Two" },
            { "title": "Three", "id": "q2" },
            { "title": "Four" },
        ])))
        .unwrap();
        // Supplied ids are kept; missing and colliding ones fall back to
        // generated ids that skip everything already taken.
        let ids: Vec<&str> = questions.iter().map(|q| q.id.as_str()).collect();
        let unique = ids.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(unique, 4);
        assert_eq!(ids[0], "q2");
    }

    #[test]
    fn normalize_ask_questions_defaults_allow_custom() {
        let questions = normalize_ask_questions(Some(&serde_json::json!([
            { "title": "Text only" },
            { "title": "Choice", "options": ["A", "B"] },
            { "title": "Both", "options": ["A", "B"], "allow_custom": true },
        ])))
        .unwrap();
        // No options → manual input is the only way to answer → always on.
        assert!(questions[0].options.is_none() && questions[0].allow_custom);
        // Options without the flag → choices only.
        assert!(questions[1].options.is_some() && !questions[1].allow_custom);
        // Options plus the flag → both.
        assert!(questions[2].options.is_some() && questions[2].allow_custom);
    }

    #[test]
    fn validate_answer_enforces_options() {
        let choice = TodoAskQuestion {
            id: "q1".to_string(),
            title: "Pick one".to_string(),
            description: None,
            options: Some(vec!["A".to_string(), "B".to_string()]),
            allow_custom: false,
        };
        assert!(validate_answer(&choice, "A").is_ok());
        assert!(validate_answer(&choice, " not-an-option ").is_err());
        assert!(validate_answer(&choice, "   ").is_err());

        let mut free = choice.clone();
        free.options = None;
        assert!(validate_answer(&free, " anything ").is_ok());
    }

    #[test]
    fn collect_answers_matches_by_id_and_reports_missing() {
        let questions = normalize_ask_questions(Some(&serde_json::json!([
            { "title": "Pick", "id": "pick", "options": ["React", "Vue"] },
            { "title": "Notes", "id": "notes" },
        ])))
        .unwrap();
        let answers = vec![
            TodoAskAnswer { id: "pick".into(), value: "React".into() },
            TodoAskAnswer { id: "notes".into(), value: " keep it small ".into() },
        ];
        let out = collect_answers(&questions, answers).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].answer, "React");
        assert_eq!(out[0].source, "option");
        assert_eq!(out[1].answer, "keep it small");
        assert_eq!(out[1].source, "custom");

        // A missing answer is an error the tool reports.
        let answers = vec![TodoAskAnswer { id: "notes".into(), value: "x".into() }];
        assert!(collect_answers(&questions, answers).is_err());
    }

    #[test]
    fn format_ask_result_statuses() {
        let answered = format_ask_result(AskOutcome::Answered(vec![]));
        assert!(answered.contains("\"status\": \"answered\""));
        assert!(format_ask_result(AskOutcome::Dismissed).contains("\"dismissed\""));
        assert!(format_ask_result(AskOutcome::Timeout).contains("\"timeout\""));
        assert!(format_ask_result(AskOutcome::Cancelled).contains("\"cancelled\""));
        // None of the wait outcomes may read like a tool error, or the failure
        // guard would disable the todos tool while the user is away.
        for text in [
            format_ask_result(AskOutcome::Dismissed),
            format_ask_result(AskOutcome::Timeout),
            format_ask_result(AskOutcome::Cancelled),
        ] {
            assert!(!crate::turn_guard::tool_output_is_error(&text));
        }
    }
}
