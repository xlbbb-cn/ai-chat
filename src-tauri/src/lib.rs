use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_opener::OpenerExt;

pub mod agents;
mod backup;
mod db;
mod llm_complete;
mod logger;
pub mod mcp;
pub mod model_metadata;
mod process_guard;
mod skills;
mod tools;
mod update;
mod workspace;

use logger::{AppLogger, LoggerOutput};
pub use tools::neo4j_db;
use tools::{timer, todos};

const OPEN_APP_DATA_DIR_MENU_ID: &str = "open-app-data-dir";
const SAVE_PROFILE_MENU_ID: &str = "save-profile";
const RESTORE_PROFILE_MENU_ID: &str = "restore-profile";
const MARKDOWN_EDIT_MENU_ID: &str = "markdown-edit";
const ABOUT_MENU_ID: &str = "about";
const MARKDOWN_EDIT_OPEN_EVENT: &str = "markdown-edit-open";
const MARKDOWN_EDIT_ERROR_EVENT: &str = "markdown-edit-error";
const WORKSPACE_CHANGED_EVENT: &str = "workspace-changed";

#[derive(Debug, Clone, Serialize)]
struct MarkdownEditPayload {
    path: String,
    content: String,
}

// ─── App menu actions ────────────────────────────────────────────────────────
//
// Shared by the native menu (macOS global menu bar, and the File/Tools/About
// entries on platforms that still show one) and the custom titlebar's `⋯`
// menu. The titlebar picks its paths with the dialog plugin on the frontend,
// then calls the commands below — Rust never blocks on a dialog there.

/// Open the OS file manager at the app data directory.
fn open_app_data_dir(app: &AppHandle) {
    if let Ok(data_dir) = app.path().app_data_dir() {
        let _ = app
            .opener()
            .open_path(data_dir.to_string_lossy().into_owned(), None::<&str>);
    }
}

/// Read a Markdown file and hand it to the built-in editor via events.
fn read_markdown_into_editor(app: &AppHandle, markdown_path: &Path) {
    match fs::read_to_string(markdown_path) {
        Ok(content) => {
            let payload = MarkdownEditPayload {
                path: markdown_path.to_string_lossy().to_string(),
                content,
            };
            let _ = app.emit(MARKDOWN_EDIT_OPEN_EVENT, payload);
        }
        Err(err) => {
            let msg = format!("Failed to open markdown file: {err}");
            app.state::<AppState>()
                .logger
                .lock()
                .unwrap()
                .log("ERROR", &msg);
            let _ = app.emit(MARKDOWN_EDIT_ERROR_EVENT, msg);
        }
    }
}

/// Restore a profile archive and log failures without interrupting the caller.
fn import_profile_archive(app: &AppHandle, profile_path: &Path) {
    if let Err(err) = backup::import_profile(app, profile_path) {
        app.state::<AppState>()
            .logger
            .lock()
            .unwrap()
            .log("ERROR", &format!("Restore profile failed: {err}"));
    }
}

/// Native About dialog, shared by the menu bar and the titlebar menu.
fn show_about(app: &AppHandle) {
    app.dialog()
        .message(format!(
            "About AI Chat\n\nAI Chat\nOpenAI-compatible desktop assistant\nVersion {}",
            env!("CARGO_PKG_VERSION")
        ))
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}

// ─── Custom titlebar commands ────────────────────────────────────────────────

#[tauri::command]
fn app_open_data_dir(app: AppHandle) {
    open_app_data_dir(&app);
}

#[tauri::command]
fn app_open_markdown(app: AppHandle, path: String) {
    read_markdown_into_editor(&app, Path::new(&path));
}

#[tauri::command]
fn app_export_profile(app: AppHandle, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Profile path is empty".into());
    }
    backup::spawn_profile_export(app, PathBuf::from(path));
    Ok(())
}

#[tauri::command]
async fn app_import_profile(app: AppHandle, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Profile path is empty".into());
    }
    let profile_path = PathBuf::from(path);
    // Import rewrites the workspace and the SQLite snapshot, so keep it off
    // the IPC thread.
    tauri::async_runtime::spawn_blocking(move || backup::import_profile(&app, &profile_path))
        .await
        .map_err(|err| format!("Profile import task failed: {err}"))?
}

fn resolve_workspace_path(app: &AppHandle, workspace_dir: Option<&str>) -> Result<PathBuf, String> {
    Ok(match workspace_dir.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("workspace"),
    })
}

/// Create the hidden `<workspace>/.data` layout (best effort, logged on error).
fn ensure_workspace_data_layout(state: &AppState, workspace_dir: &PathBuf) {
    if let Err(err) = workspace::ensure_data_layout(workspace_dir) {
        state
            .logger
            .lock()
            .unwrap()
            .log(
                "WARN",
                &format!(
                    "Failed to create workspace data directory '{}': {err}",
                    workspace::data_root(workspace_dir).display()
                ),
            );
    }
}

/// Point the SQLite connection at `workspace_dir`'s database.
///
/// The database lives inside the workspace (`<workspace>/.data/chat.db`), so a
/// workspace switch must close the old connection and open (creating if
/// needed) the new one — otherwise history rows would keep landing in the
/// previous workspace's file.
fn reopen_workspace_db(state: &AppState, workspace_dir: &PathBuf) -> Result<(), String> {
    ensure_workspace_data_layout(state, workspace_dir);
    let db_path = workspace::db_path(workspace_dir);
    let db = db::open_db(&db_path)
        .map_err(|e| format!("failed to open database '{}': {e}", db_path.display()))?;
    let retention_days = state.config.lock().unwrap().log_retention_days as i64;
    match db::prune_old_logs(&db, retention_days) {
        Ok(0) => {}
        Ok(removed) => state.logger.lock().unwrap().log(
            "INFO",
            &format!("Log retention: removed {removed} rows older than {retention_days} days"),
        ),
        Err(err) => state
            .logger
            .lock()
            .unwrap()
            .log("WARN", &format!("Log retention failed: {err}")),
    }
    *state.db.lock().unwrap() = db;
    Ok(())
}

pub(crate) fn apply_config(
    app: &AppHandle,
    state: &AppState,
    config: AppConfig,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&state.config_path, json).map_err(|e| e.to_string())?;

    let logger_output = config.logger_output.clone();
    *state.config.lock().unwrap() = config;

    let new_workspace_path = resolve_workspace_path(
        app,
        state.config.lock().unwrap().workspace_dir.as_deref(),
    )?;
    fs::create_dir_all(&new_workspace_path).ok();
    // Note: `<workspace>/.data/skills` is NOT created eagerly. Skills are
    // discovered by scanning; a missing folder simply yields no workspace
    // skills until the Skills panel (or self-evolution) creates one.
    let previous_workspace_path = std::mem::replace(
        &mut *state.workspace_dir.lock().unwrap(),
        new_workspace_path.clone(),
    );

    // The database, memories, todo lists and skills are all workspace-scoped
    // (`<workspace>/.data`), so a workspace switch must re-point the SQLite
    // connection before the UI reloads anything from it.
    if previous_workspace_path != new_workspace_path {
        // Stdio MCP children are rooted in the workspace they were started in
        // (`mcp::build_stdio_cmd`), so a switch must retire them. The next MCP
        // tool call re-spawns them through `ensure_stdio_process`, now rooted
        // in the new workspace.
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(state) = app_handle.try_state::<AppState>() {
                mcp::stop_all_stdio_processes(&state).await;
            }
        });

        reopen_workspace_db(state, &new_workspace_path)?;

        // The active skill list is workspace-scoped too: skills that came from
        // the old workspace may not exist in the new one. Notify the UI so it
        // can reload `skills.status` and re-validate the selection.
        let _ = app.emit(
            WORKSPACE_CHANGED_EVENT,
            new_workspace_path.to_string_lossy().to_string(),
        );
    } else {
        ensure_workspace_data_layout(state, &new_workspace_path);
    }

    if let Some(win) = app.get_webview_window("main") {
        let title = format!("AI Chat — {}", new_workspace_path.display());
        let _ = win.set_title(&title);
    }

    let mut logger = state.logger.lock().unwrap();
    logger.set_output(logger_output);
    logger.log("INFO", "Configuration updated");

    Ok(())
}

// ─── App State ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSettings {
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub reasoning_effort: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub max_complete_tokens: Option<u32>,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            temperature: None,
            top_p: None,
            reasoning_effort: String::new(),
            max_tokens: None,
            max_complete_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub model_catalog: Vec<String>,
    #[serde(default)]
    pub model_settings: ModelSettings,
    /// DS-Format (DeepSeek format): pass `reasoning_content` back on assistant
    /// messages as DeepSeek-family thinking models require. Off by default —
    /// other providers receive plain messages.
    #[serde(default)]
    pub ds_format: bool,
    /// Per-model context window (tokens). Auto-filled from `/models` when the
    /// provider reports one; otherwise set manually in Settings as a fallback.
    #[serde(default)]
    pub model_context_lengths: HashMap<String, u32>,
    /// Per-model capabilities (tools / reasoning / vision / files …) learned
    /// from the public llm-metadata catalogue when the model list was
    /// refreshed. Keyed by the remote model id; `model_name` inside the value
    /// is the catalogue entry it was matched to. Informational only — never
    /// sent upstream.
    #[serde(default)]
    pub model_metadata: HashMap<String, model_metadata::ModelMetadata>,
    /// Per-model reasoning effort ("thinking depth") picked in the chat
    /// toolbar. Keyed by the model id the request is sent with; the value is
    /// forwarded as `reasoning_effort` and must be one of `minimal`, `low`,
    /// `medium`, `high`. Absent/empty = let the provider decide; falls back to
    /// `model_settings.reasoning_effort`.
    #[serde(default)]
    pub model_reasoning_effort: HashMap<String, String>,
    #[serde(default)]
    pub system_message: String,
    #[serde(default)]
    pub selected_tools: Vec<String>,
    #[serde(default)]
    pub self_evolution_mode: bool,
    pub kg_engine: Option<String>,
    pub neo4j_uri: Option<String>,
    pub neo4j_user: Option<String>,
    pub neo4j_password: Option<String>,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    #[serde(default)]
    pub logger_output: LoggerOutput,
    #[serde(default)]
    pub auto_accept_confirm_kinds: Vec<String>,
    #[serde(default)]
    pub theme: Option<String>,
    /// Colour scheme layered on top of the light / dark theme
    /// (`default`, `fig`, `cream`). Persisted like `theme`; the backend never
    /// interprets the value — the frontend turns it into `data-palette`.
    #[serde(default)]
    pub palette: Option<String>,
    /// UI language (`en`, `zh-CN`, `zh-TW`). Persisted so the frontend restores
    /// it on startup; the backend never interprets the value.
    #[serde(default)]
    pub language: Option<String>,
    /// Check the GitHub Releases API for a newer version when the app starts.
    #[serde(default = "update::default_true")]
    pub check_updates_on_startup: bool,
    /// Log retention window in days for the request / interaction log tables
    /// (`api_requests`, `interaction_log`). Older rows are dropped at startup
    /// and when the monitor compacts the database. `0` keeps logs forever.
    /// Chat history is never pruned.
    #[serde(default = "db::default_log_retention_days")]
    pub log_retention_days: u32,
    /// Also consider releases flagged as pre-release in the update check.
    #[serde(default)]
    pub include_prerelease_updates: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            model_catalog: vec!["gpt-4o-mini".to_string()],
            model_settings: ModelSettings::default(),
            ds_format: false,
            model_context_lengths: HashMap::new(),
            model_metadata: HashMap::new(),
            model_reasoning_effort: HashMap::new(),
            system_message: String::new(),
            selected_tools: vec![],
            self_evolution_mode: false,
            kg_engine: None,
            neo4j_uri: Some("bolt://localhost:7687".to_string()),
            neo4j_user: Some("neo4j".to_string()),
            neo4j_password: Some(String::new()),
            workspace_dir: None,
            logger_output: LoggerOutput::default(),
            auto_accept_confirm_kinds: vec![],
            theme: None,
            palette: None,
            language: None,
            check_updates_on_startup: true,
            include_prerelease_updates: false,
            log_retention_days: db::default_log_retention_days(),
        }
    }
}

/// One message the user pushed into a turn that was already running.
///
/// A turn is a *sequence* of requests (one per tool round), so an injection
/// can only join at a request boundary — the inbox is drained there, never in
/// the middle of a stream. `id` is generated by the UI, which uses it to tell
/// "delivered to a request" (`chat-injected`) from "still queued".
#[derive(Clone, Debug, Serialize)]
pub struct TurnInboxItem {
    pub id: String,
    pub text: String,
    /// Unix milliseconds, rendered as the "added at" clock label.
    pub created_at_ms: i64,
    /// Turn that accepted this item. Items are scoped by turn id so a turn
    /// that is winding down can never drain or clear a newer turn's inbox.
    pub turn_id: u64,
}

#[derive(Serialize)]
pub struct InjectUserMessageResult {
    /// `false` — no turn is running for this session; the UI sends the text as
    /// a normal next-turn message instead of leaving it in the inbox.
    pub accepted: bool,
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    pub workspace_dir: Mutex<PathBuf>,
    pub mcp_servers_path: PathBuf,
    pub agents_config_path: PathBuf,
    pub profiles_path: PathBuf,
    pub db: Mutex<Connection>,
    pub logger: Mutex<AppLogger>,
    /// Per-turn stop flag, published by `stop_chat_completion` (Stop button).
    /// `chat_completion` swaps in a fresh `Arc<AtomicBool>` for every new turn,
    /// which (a) makes a turn's cancellation race-free — a superseded turn keeps
    /// polling its own flag, which stays `true` forever, so it can never be
    /// resurrected by the next turn resetting the slot — and (b) means Stop
    /// pressed while idle cannot leak into the next turn (the next turn
    /// installs its own flag anyway).
    pub chat_cancelled: Mutex<Arc<AtomicBool>>,
    /// Monotonic id for the injection window of every chat turn.
    pub turn_seq: AtomicU64,
    /// `(turn_id, session_id)` of the turn currently executing, or `None`.
    /// `inject_user_message` refuses anything else: an item accepted for a
    /// turn that never reaches another request would otherwise sit in the
    /// inbox and be drained by the *next* turn, long after the user typed it.
    pub active_turn: Mutex<Option<(u64, String)>>,
    /// Mid-turn injections waiting for the next request boundary of that turn.
    pub turn_inbox: Mutex<Vec<TurnInboxItem>>,
    /// One-shot channel sender used to relay the user's confirm/deny response
    /// back to a waiting `execute_tool` call. Stores the request_id alongside
    /// the sender so a stale confirmation can never approve the wrong command.
    pub confirm_sender: Mutex<Option<(String, tokio::sync::oneshot::Sender<ToolConfirmation>)>>,
    /// Per-server diagnostic log buffer (in-memory ring buffer, last
    /// `MAX_MCP_LOG_ENTRIES` entries per server). Populated by mcp.rs at
    /// every save / test / tool call so the UI can show what happened.
    pub mcp_logs: std::sync::Arc<
        Mutex<std::collections::HashMap<String, std::collections::VecDeque<mcp::McpLogEntry>>>,
    >,
    /// Server IDs whose current test has been cancelled by the user.
    pub mcp_cancelled_tests: std::sync::Arc<Mutex<std::collections::HashSet<String>>>,
    /// Live stdio MCP child processes, keyed by server id. Held as a Tokio
    /// mutex so async tasks can lock across await points.
    pub mcp_children: std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<String, mcp::StdioProcessHandle>>,
    >,
    /// Long-lived background children started by `run_shell run_type="serve"`.
    /// Killed on demand by the tool and automatically at app exit.
    pub serve_processes: tools::serve::ServeRegistry,
    /// Session-scoped memories: session_id -> key -> JSON entry.
    pub session_memories: Mutex<
        std::collections::HashMap<String, std::collections::HashMap<String, serde_json::Value>>,
    >,
    /// External-agent ids (`cli` / `cloud`) the user already approved in this
    /// app run. Each such agent asks once, then runs without prompting until
    /// restart — unless its `require_approval` switch is off.
    pub external_agent_confirmed: Mutex<std::collections::HashSet<String>>,
    /// Serializes external-agent approval requests: `confirm_sender` holds one
    /// pending request at a time, so parallel agents must queue here instead of
    /// overwriting each other's dialog.
    pub external_agent_confirm_lock: tokio::sync::Mutex<()>,
}

impl AppState {
    /// Hidden workspace state root (`<workspace>/.data`).
    pub fn data_dir(&self) -> PathBuf {
        workspace::data_root(&self.workspace_dir.lock().unwrap())
    }

    /// Active workspace SQLite database (`<workspace>/.data/chat.db`).
    pub fn db_path(&self) -> PathBuf {
        workspace::db_path(&self.workspace_dir.lock().unwrap())
    }

    /// Active workspace skill status file (`<workspace>/.data/skills.status`).
    pub fn skills_status_path(&self) -> PathBuf {
        workspace::skills_status_path(&self.workspace_dir.lock().unwrap())
    }

    /// Start the injection window of a new turn and return its id.
    ///
    /// Anything still sitting in the inbox belongs to a turn that ended
    /// without delivering it (the UI re-sends those as a normal next-turn
    /// message), so it is dropped here rather than drained by this turn.
    pub fn open_turn_window(&self, session_id: &str) -> u64 {
        let turn_id = self
            .turn_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut active) = self.active_turn.lock() {
            // Clear leftovers *before* publishing the new window, so an item a
            // previous turn never delivered cannot be drained by this one.
            // Lock order everywhere is `active_turn` → `turn_inbox`.
            if let Ok(mut inbox) = self.turn_inbox.lock() {
                inbox.clear();
            }
            *active = Some((turn_id, session_id.to_string()));
        }
        turn_id
    }

    /// Close an injection window. Guarded by the turn id: a turn that is
    /// winding down must never close the window of a newer turn the user has
    /// already started, nor drop that turn's queued items.
    pub fn close_turn_window(&self, turn_id: u64) {
        if let Ok(mut active) = self.active_turn.lock() {
            if matches!(active.as_ref(), Some((id, _)) if *id == turn_id) {
                *active = None;
            }
        }
        if let Ok(mut inbox) = self.turn_inbox.lock() {
            inbox.retain(|item| item.turn_id != turn_id);
        }
    }

    /// Take the injections queued for `turn_id` — called at a request
    /// boundary, right before the next request is built.
    pub fn drain_turn_injections(&self, turn_id: u64) -> Vec<TurnInboxItem> {
        let Ok(mut inbox) = self.turn_inbox.lock() else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        let mut rest = Vec::with_capacity(inbox.len());
        for item in inbox.drain(..) {
            if item.turn_id == turn_id {
                taken.push(item);
            } else {
                rest.push(item);
            }
        }
        *inbox = rest;
        taken
    }
}

#[derive(Clone, Debug)]
pub struct ToolConfirmation {
    pub confirmed: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

// ─── Config commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    apply_config(&app, &state, config)
}

#[tauri::command]
async fn fetch_models(state: State<'_, AppState>) -> Result<Vec<RemoteModel>, String> {
    let config = state.config.lock().unwrap().clone();
    let url = format!("{}/models", config.api_base_url.trim_end_matches('/'));

    let client = llm_complete::build_http_client()?;
    let res = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/json")
        .bearer_auth(config.api_key)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let body = res
            .text()
            .await
            .unwrap_or_else(|_| "failed to fetch models".to_string());
        return Err(llm_complete::extract_upstream_error_message(&body));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if let Some(object_type) = body.get("object").and_then(|v| v.as_str()) {
        if object_type != "list" {
            return Err(format!(
                "Invalid models response: expected object='list', got '{object_type}'"
            ));
        }
    }
    let models = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Invalid models response: missing data array".to_string())?;

    let mut models_out: Vec<RemoteModel> = models
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(|id| id.as_str())?.to_string();
            Some(RemoteModel {
                id,
                context_length: extract_context_length(m),
            })
        })
        .collect();

    models_out.sort_by(|a, b| a.id.cmp(&b.id));
    models_out.dedup_by(|a, b| a.id == b.id);
    Ok(models_out)
}

/// One entry of the remote `/models` listing.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteModel {
    pub id: String,
    /// Context window in tokens, when the provider reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
}

/// Best-effort extraction of a context window from a `/models` entry.
///
/// There is no universal OpenAI-compatible field for this, but several
/// providers expose it under well-known names:
/// - OpenRouter: `context_length` or nested `top_provider.context_length`
/// - Groq: `context_window`
/// - vLLM / SGLang / LM Studio: `max_model_len`
fn extract_context_length(model: &serde_json::Value) -> Option<u64> {
    const KEYS: [&str; 4] = [
        "context_length",
        "context_window",
        "max_model_len",
        "max_context_length",
    ];
    let parse = |v: &serde_json::Value| -> Option<u64> {
        let n = v.as_u64().or_else(|| v.as_str()?.parse().ok())?;
        (n > 0).then_some(n)
    };
    for key in KEYS {
        if let Some(v) = model.get(key).and_then(parse) {
            return Some(v);
        }
    }
    model
        .get("top_provider")
        .and_then(|tp| tp.get("context_length"))
        .and_then(parse)
}

#[tauri::command]
fn stop_chat_completion(state: State<'_, AppState>) {
    // Cancel the *current* turn's flag. If no turn is running this simply
    // marks the slot's default flag — a new turn installs a fresh flag anyway,
    // so the stop never leaks into the next turn.
    if let Ok(flag) = state.chat_cancelled.lock() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Push a message into the turn that is currently running, so it reaches the
/// model at the next request boundary (i.e. before the next tool round or the
/// final answer) instead of waiting for the turn to end.
///
/// Returns `accepted: false` when no turn is running for this session — the
/// UI then sends the text as a normal next-turn message, which is the only
/// other safe option: a turn that already finished will never make another
/// request, so an item left here would be drained by an unrelated later turn.
#[tauri::command]
fn inject_user_message(
    session_id: String,
    id: String,
    text: String,
    state: State<'_, AppState>,
) -> InjectUserMessageResult {
    let text = text.trim().to_string();
    if text.is_empty() {
        return InjectUserMessageResult { accepted: false };
    }

    // Hold the window lock while queueing: a turn that is closing right now
    // must accept the item completely or reject it, never strand it in an
    // inbox no turn will read. Lock order `active_turn` → `turn_inbox` matches
    // `open_turn_window` / `close_turn_window`.
    let Ok(active) = state.active_turn.lock() else {
        return InjectUserMessageResult { accepted: false };
    };
    let Some(turn_id) = active
        .as_ref()
        .filter(|(_, session)| session == &session_id)
        .map(|(turn_id, _)| *turn_id)
    else {
        return InjectUserMessageResult { accepted: false };
    };

    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    if let Ok(mut inbox) = state.turn_inbox.lock() {
        inbox.push(TurnInboxItem {
            id,
            text,
            created_at_ms,
            turn_id,
        });
    }
    InjectUserMessageResult { accepted: true }
}

/// Called by the frontend to confirm or deny a pending dangerous-command execution.
/// `request_id` must match the currently pending request; stale confirmations
/// from an earlier dialog are ignored so they cannot approve the wrong command.
#[tauri::command]
fn confirm_command(
    state: State<'_, AppState>,
    request_id: String,
    confirmed: bool,
    username: Option<String>,
    password: Option<String>,
) {
    let mut guard = state.confirm_sender.lock().unwrap();
    if let Some((pending_id, tx)) = guard.take() {
        if pending_id == request_id {
            let _ = tx.send(ToolConfirmation {
                confirmed,
                username,
                password,
            });
        }
    }
}

#[tauri::command]
fn get_workspace_dir(state: State<'_, AppState>) -> String {
    state
        .workspace_dir
        .lock()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

#[tauri::command]
fn save_markdown_file(path: String, content: String) -> Result<(), String> {
    fs::write(PathBuf::from(path), content).map_err(|e| e.to_string())
}

// ─── Named Profiles ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub selected_tools: Vec<String>,
    pub agents: Vec<agents::SubAgent>,
    pub orchestration: agents::AgentOrchestration,
    /// Legacy standalone CLI agents (pre agent-type profiles). Migrated into
    /// `agents` with `agent_type = "cli"` when the profile is applied.
    #[serde(default)]
    pub cli_agents: Vec<agents::LegacyCliAgent>,
    pub mcp_servers: Vec<mcp::McpServer>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProfilesFile {
    profiles: Vec<Profile>,
}

fn load_profiles(path: &PathBuf) -> Vec<Profile> {
    if !path.exists() {
        return vec![];
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<ProfilesFile>(&s).ok())
        .map(|f| f.profiles)
        .unwrap_or_default()
}

fn save_profiles(path: &PathBuf, profiles: &[Profile]) -> Result<(), String> {
    let file = ProfilesFile {
        profiles: profiles.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_profiles(state: State<'_, AppState>) -> Vec<Profile> {
    load_profiles(&state.profiles_path)
}

#[tauri::command]
fn save_profile_config(state: State<'_, AppState>, profile: Profile) -> Result<(), String> {
    let mut profiles = load_profiles(&state.profiles_path);

    // Update timestamps
    let now = chrono::Utc::now().to_rfc3339();
    let mut updated_profile = profile;

    if let Some(existing) = profiles.iter_mut().find(|p| p.name == updated_profile.name) {
        updated_profile.created_at = existing.created_at.clone();
        updated_profile.updated_at = now;
        *existing = updated_profile;
    } else {
        updated_profile.created_at = now.clone();
        updated_profile.updated_at = now;
        profiles.push(updated_profile);
    }

    save_profiles(&state.profiles_path, &profiles)
}

#[tauri::command]
fn delete_profile_config(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut profiles = load_profiles(&state.profiles_path);
    profiles.retain(|p| p.name != name);
    save_profiles(&state.profiles_path, &profiles)
}

#[tauri::command]
fn apply_profile_config(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<(), String> {
    let profiles = load_profiles(&state.profiles_path);
    let profile = profiles
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("Profile '{}' not found", name))?;

    // Update config with the profile's selected tools. Skill enablement is not
    // part of a profile: it is per-workspace state (`<workspace>/.data/skills.status`).
    let mut config = state.config.lock().unwrap().clone();
    config.selected_tools = profile.selected_tools;

    apply_config(&app, &state, config)?;

    // Save agents config (legacy standalone CLI agents are folded into the
    // typed agent list by the normalizing save).
    let agents_config = agents::AgentsConfig {
        agents: profile.agents,
        orchestration: profile.orchestration,
        cli_agents: profile.cli_agents,
    };
    agents::save_agents_config_file(&state.agents_config_path, &agents_config)?;

    // Save MCP servers
    let mcp_file = mcp::McpServersFile {
        servers: profile.mcp_servers,
    };
    let mcp_json = serde_json::to_string_pretty(&mcp_file).map_err(|e| e.to_string())?;
    fs::write(&state.mcp_servers_path, mcp_json).map_err(|e| e.to_string())?;

    let _ = app.emit(backup::PROFILE_RESTORED_EVENT, ());
    Ok(())
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .on_menu_event(|app, event| {
            if event.id() == OPEN_APP_DATA_DIR_MENU_ID {
                open_app_data_dir(app);
            } else if event.id() == SAVE_PROFILE_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Profile", &["zip"])
                    .set_file_name("ai-chat.profile.zip")
                    .blocking_save_file()
                {
                    if let Some(profile_path) = path.into_path().ok() {
                        backup::spawn_profile_export(app.clone(), profile_path);
                    }
                }
            } else if event.id() == RESTORE_PROFILE_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Profile", &["zip"])
                    .blocking_pick_file()
                {
                    if let Some(profile_path) = path.into_path().ok() {
                        import_profile_archive(app, &profile_path);
                    }
                }
            } else if event.id() == MARKDOWN_EDIT_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
                    .blocking_pick_file()
                {
                    if let Some(markdown_path) = path.into_path().ok() {
                        read_markdown_into_editor(app, &markdown_path);
                    }
                }
            } else if event.id() == ABOUT_MENU_ID {
                show_about(app);
            }
        })
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            fs::create_dir_all(&data_dir).ok();

            let open_app_data_dir_item = MenuItem::with_id(
                app,
                OPEN_APP_DATA_DIR_MENU_ID,
                "Open App Data Directory",
                true,
                None::<&str>,
            )
            .expect("failed to create menu item");
            let save_profile_item = MenuItem::with_id(
                app,
                SAVE_PROFILE_MENU_ID,
                "Save Profile...",
                true,
                None::<&str>,
            )
            .expect("failed to create menu item");
            let restore_profile_item = MenuItem::with_id(
                app,
                RESTORE_PROFILE_MENU_ID,
                "Restore Profile...",
                true,
                None::<&str>,
            )
            .expect("failed to create menu item");
            let markdown_edit_item = MenuItem::with_id(
                app,
                MARKDOWN_EDIT_MENU_ID,
                "Markdown Edit...",
                true,
                None::<&str>,
            )
            .expect("failed to create tools menu item");
            let file_menu = Submenu::with_items(
                app,
                "File",
                true,
                &[&save_profile_item, &restore_profile_item, &open_app_data_dir_item],
            )
                .expect("failed to create app menu");
            let tools_menu = Submenu::with_items(app, "Tools", true, &[&markdown_edit_item])
                .expect("failed to create tools menu");

            // Standard Edit menu for copy/paste/select-all keyboard shortcuts
            let edit_menu = Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None::<&str>).expect("failed to create undo menu item"),
                    &PredefinedMenuItem::redo(app, None::<&str>).expect("failed to create redo menu item"),
                    &PredefinedMenuItem::separator(app).expect("failed to create separator"),
                    &PredefinedMenuItem::cut(app, None::<&str>).expect("failed to create cut menu item"),
                    &PredefinedMenuItem::copy(app, None::<&str>).expect("failed to create copy menu item"),
                    &PredefinedMenuItem::paste(app, None::<&str>).expect("failed to create paste menu item"),
                    &PredefinedMenuItem::select_all(app, None::<&str>).expect("failed to create select all menu item"),
                ],
            )
            .expect("failed to create edit menu");

            let about_item = MenuItem::with_id(
                app,
                ABOUT_MENU_ID,
                "About AI Chat",
                true,
                None::<&str>,
            )
            .expect("failed to create about menu item");
            let about_menu = Submenu::with_items(app, "About", true, &[&about_item])
                .expect("failed to create about menu");
            let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &tools_menu, &about_menu])
                .expect("failed to create app menu");
            app.set_menu(menu).expect("failed to set app menu");

            let mcp_servers_path = data_dir.join("mcp_servers.json");
            let agents_config_path = data_dir.join("sub_agents.json");
            let profiles_path = data_dir.join("profiles.json");
            let config_path = data_dir.join("config.json");
            let config = if config_path.exists() {
                fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default()
            } else {
                AppConfig::default()
            };

            // Resolve the active workspace first: the database, memory, todo
            // lists and skills all live in its hidden `.data` directory, so the
            // workspace must be known before the database is opened.
            let workspace_dir = match config.workspace_dir.as_deref().filter(|s| !s.is_empty()) {
                Some(dir) => PathBuf::from(dir),
                None => data_dir.join("workspace"),
            };
            fs::create_dir_all(&workspace_dir).ok();
            workspace::ensure_data_layout(&workspace_dir).ok();
            // `<workspace>/.data/skills` is created on demand (e.g. by the Skills
            // panel or self-evolution); skill listing tolerates its absence.

            let db_path = workspace::db_path(&workspace_dir);
            let db = db::open_db(&db_path).expect("failed to open workspace database");

            let app_logger = AppLogger::new(
                cfg!(debug_assertions),
                config.logger_output.clone(),
                data_dir.join("app.log"),
            );
            app_logger.log("INFO", "Logger initialized");

            // Log retention: request/interaction rows are debugging data, so
            // drop anything past the retention window (Settings → Runtime &
            // Debug) on startup. Conversations in `history` are never touched.
            match db::prune_old_logs(&db, config.log_retention_days as i64) {
                Ok(0) => {}
                Ok(removed) => app_logger.log(
                    "INFO",
                    &format!(
                        "Log retention: removed {removed} rows older than {} days",
                        config.log_retention_days
                    ),
                ),
                Err(err) => app_logger.log("WARN", &format!("Log retention failed: {err}")),
            }

            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                workspace_dir: Mutex::new(workspace_dir.clone()),
                db: Mutex::new(db),
                logger: Mutex::new(app_logger),
                mcp_servers_path: mcp_servers_path.clone(),
                agents_config_path,
                profiles_path,
                chat_cancelled: Mutex::new(Arc::new(AtomicBool::new(false))),
                turn_seq: AtomicU64::new(1),
                active_turn: Mutex::new(None),
                turn_inbox: Mutex::new(Vec::new()),
                confirm_sender: Mutex::new(None),
                mcp_logs: std::sync::Arc::new(Mutex::new(Default::default())),
                mcp_cancelled_tests: std::sync::Arc::new(Mutex::new(Default::default())),
                mcp_children: std::sync::Arc::new(tokio::sync::Mutex::new(Default::default())),
                serve_processes: Default::default(),
                session_memories: Mutex::new(Default::default()),
                external_agent_confirmed: Mutex::new(Default::default()),
                external_agent_confirm_lock: tokio::sync::Mutex::new(()),
            });

            // Delayed "continue the task" timers, mirrored to timers.json so a
            // pending wait survives an app restart.
            let sleep_lock = timer::SleepLock::new(app.handle().clone());
            app.manage(sleep_lock.clone());
            app.manage(timer::TimerRegistry::load(
                data_dir.join("timers.json"),
                sleep_lock,
            ));
            timer::restore_on_startup(app.handle());

            // Set window title to show current workspace directory
            if let Some(win) = app.get_webview_window("main") {
                let title = format!("AI Chat — {}", workspace_dir.display());
                let _ = win.set_title(&title);
            }

            // Warmup: auto-start all enabled stdio MCP servers
            {
                let servers = mcp::load_servers(&mcp_servers_path);
                let enabled_stdio: Vec<_> = servers
                    .iter()
                    .filter(|s| s.enabled && matches!(s.transport, mcp::McpTransport::Stdio))
                    .cloned()
                    .collect();
                if !enabled_stdio.is_empty() {
                    let app_handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            for server in enabled_stdio {
                                let _ = mcp::start_stdio_process(&state, &server).await;
                            }
                        }
                    });
                }
            }

            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                // The only window is closing, so the app is exiting. Tear down
                // every child tree (MCP servers, serve processes, CLI agents)
                // instead of only the MCP pool.
                process_guard::kill_all();
            }
        })
        .invoke_handler(tauri::generate_handler![
            llm_complete::chat_completion,
            stop_chat_completion,
            inject_user_message,
            confirm_command,
            get_config,
            save_config,
            fetch_models,
            model_metadata::fetch_model_metadata,
            get_workspace_dir,
            save_markdown_file,
            app_open_data_dir,
            app_open_markdown,
            app_export_profile,
            app_import_profile,
            skills::list_skills,
            skills::save_skill,
            skills::delete_skill,
            skills::filter_existing_skills,
            skills::get_skills_status,
            skills::save_skills_status,
            db::save_history,
            db::list_history_sessions,
            db::load_session_messages,
            db::delete_history,
            db::delete_message,
            db::fork_session,
            db::list_session_meta,
            db::update_session_meta,
            db::delete_session_meta,
            db::list_api_requests,
            db::get_api_request,
            db::delete_api_request,
            db::clear_api_requests,
            db::clear_logs,
            db::compact_database,
            db::list_interactions,
            db::get_interaction,
            db::clear_interactions,
            mcp::list_mcp_servers,
            mcp::save_mcp_server,
            mcp::delete_mcp_server,
            mcp::test_mcp_server,
            mcp::get_mcp_logs,
            mcp::clear_mcp_logs,
            mcp::mcp_start_stdio,
            mcp::mcp_stop_stdio,
            agents::list_sub_agents,
            agents::save_sub_agent,
            agents::delete_sub_agent,
            agents::get_agent_orchestration,
            agents::save_agent_orchestration,
            agents::list_agent_missions,
            agents::probe_agent_cli,
            agents::probe_agent_cloud,
            timer::list_timers,
            timer::cancel_timer,
            timer::get_sleep_lock_state,
            tools::serve::list_serve_processes,
            tools::serve::stop_serve_process,
            list_profiles,
            save_profile_config,
            delete_profile_config,
            apply_profile_config,
            update::check_update,
            update::download_update,
            update::open_update_file,
            update::reveal_update_file,
            update::open_release_page,
            update::get_app_version,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Normal quit path. `kill_all` is idempotent, so running it after
            // the window-close hook is harmless; it also covers tray/menu
            // quits that never emit `CloseRequested`.
            if let tauri::RunEvent::Exit = event {
                process_guard::kill_all();
            }
        });
}
