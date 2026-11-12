use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(target_os = "macos")]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_opener::OpenerExt;

pub mod agents;
mod backup;
mod db;
mod evolution_maintenance;
mod figures;
mod instance;
mod llm_complete;
mod logger;
// `src/utils/` is a plain folder holding standalone crates (workspace members
// `pdf2md-rs` and `knowledge-graph`), not a module: nothing here declares it.
pub mod mcp;
pub mod model_metadata;
mod pdf_mentions;
mod process_guard;
mod skills;
mod tools;
mod turn_guard;
mod update;
mod workspace;
mod workspace_files;

use logger::{AppLogger, LoggerOutput};
use tools::{timer, todos};

const OPEN_APP_DATA_DIR_MENU_ID: &str = "open-app-data-dir";
const OPEN_WORKSPACE_DIR_MENU_ID: &str = "open-workspace-dir";
const SAVE_BACKUP_MENU_ID: &str = "save-backup";
const RESTORE_BACKUP_MENU_ID: &str = "restore-backup";
const MARKDOWN_EDIT_MENU_ID: &str = "markdown-edit";
const OPEN_ISSUE_MENU_ID: &str = "open-issue";
const ABOUT_MENU_ID: &str = "about";
/// Where "Report an issue..." sends the user. Fixed on purpose — see
/// `open_issue_page`.
const ISSUE_URL: &str = "https://github.com/xlbbb-cn/ai-chat/issues/new";
const MARKDOWN_EDIT_OPEN_EVENT: &str = "markdown-edit-open";
const MARKDOWN_EDIT_ERROR_EVENT: &str = "markdown-edit-error";
const WORKSPACE_CHANGED_EVENT: &str = "workspace-changed";

/// Tool ids that no longer exist in the tool set.
///
/// They can still linger in a persisted `selected_tools` (the knowledge graph
/// tool was removed, its client now lives in the standalone `knowledge-graph`
/// crate at `src/utils/knowledge_graph`). Dropping them on load / save / read
/// keeps the prompt, the dispatcher and the Tools panel free of phantom tools.
pub const RETIRED_TOOL_IDS: [&str; 1] = ["knowledge_graph"];

/// Tools that self-evolution mode always keeps enabled.
///
/// The assistant must be able to edit its own skills (`file_actions`) and to
/// remember what it learned (`memory`). While the mode is on they cannot be
/// switched off — not from the Tools panel and not by hand-editing `config.json`.
pub const SELF_EVOLUTION_REQUIRED_TOOLS: [&str; 2] = ["file_actions", "memory"];

/// How many workspaces `AppConfig::workspace_history` keeps.
///
/// The Settings dropdown is a shortcut, not an archive: ten entries cover the
/// handful of projects one usually moves between without becoming a list
/// nobody scans.
pub const WORKSPACE_HISTORY_LIMIT: usize = 10;

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

/// Open the OS file manager at the active workspace directory.
///
/// The live `AppState.workspace_dir` is authoritative (it moves when the user
/// switches workspaces), so the command and the native menu share this helper.
fn open_workspace_dir(app: &AppHandle) -> Result<(), String> {
    let dir = app
        .state::<AppState>()
        .workspace_dir
        .lock()
        .unwrap()
        .clone();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open the GitHub "new issue" page in the system browser.
///
/// The destination is a constant rather than a command argument: the menu only
/// ever needs this one page, and a command that opens arbitrary URLs would let
/// anything running in the webview drive the browser.
fn open_issue_page(app: &AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(ISSUE_URL, None::<&str>)
        .map_err(|e| e.to_string())
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

/// Restore a backup archive and log failures without interrupting the caller.
fn import_profile_archive(app: &AppHandle, profile_path: &Path) {
    if let Err(err) = backup::import_profile(app, profile_path) {
        app.state::<AppState>()
            .logger
            .lock()
            .unwrap()
            .log("ERROR", &format!("Restore backup failed: {err}"));
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
fn app_open_workspace_dir(app: AppHandle) -> Result<(), String> {
    open_workspace_dir(&app)
}

/// Open `<workspace>/.data/tmp` — the workspace scratch root.
///
/// It holds the sub-agent deliverable exchange (permanent), the PDF conversion
/// cache (expires on its own) and the composer pastes (24 h), so this is the
/// manual cleanup entry point for the batches.
#[tauri::command]
fn app_open_workspace_tmp_dir(app: AppHandle) -> Result<(), String> {
    let workspace = app
        .state::<AppState>()
        .workspace_dir
        .lock()
        .unwrap()
        .clone();
    let dir = workspace::tmp_root(&workspace);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open the GitHub new-issue page (titlebar `⋯` menu → Report an issue…).
#[tauri::command]
fn app_open_issue_page(app: AppHandle) -> Result<(), String> {
    open_issue_page(&app)
}

/// Open `<app_data>/agents` — the folder holding every `llm` agent definition.
#[tauri::command]
fn app_open_agents_dir(app: AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("agents");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn app_open_markdown(app: AppHandle, path: String) {
    read_markdown_into_editor(&app, Path::new(&path));
}

#[tauri::command]
fn app_export_profile(app: AppHandle, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Backup path is empty".into());
    }
    backup::spawn_profile_export(app, PathBuf::from(path));
    Ok(())
}

#[tauri::command]
async fn app_import_profile(app: AppHandle, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Backup path is empty".into());
    }
    let profile_path = PathBuf::from(path);
    // Import rewrites the workspace and the SQLite snapshot, so keep it off
    // the IPC thread.
    tauri::async_runtime::spawn_blocking(move || backup::import_profile(&app, &profile_path))
        .await
        .map_err(|err| format!("Backup import task failed: {err}"))?
}

fn resolve_workspace_path(
    app: &AppHandle,
    instance: &instance::Instance,
    workspace_dir: Option<&str>,
) -> Result<PathBuf, String> {
    Ok(match workspace_dir.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let app_workspace = app
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())?
                .join("workspace");
            instance.default_workspace(app_workspace)
        }
    })
}

/// Create the hidden `<workspace>/.data` layout (best effort, logged on error).
fn ensure_workspace_data_layout(state: &AppState, workspace_dir: &PathBuf) {
    if let Err(err) = workspace::ensure_data_layout(workspace_dir) {
        state.logger.lock().unwrap().log(
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
    // Self-evolution mode owns `file_actions` and `memory`: they are forced
    // on before anything is written, so no caller (UI, backup restore) can switch them off while the
    // mode is active.
    let mut config = config;
    config.strip_retired_tools();
    config.enforce_self_evolution_tools();
    config.normalize_turn_limits();

    // The workspace history lives in the persisted config, so the resolved
    // path must be recorded *before* the file is written. Resolving it here
    // (the switch logic below reuses the value) means every route that
    // applies a config — Settings save, titlebar switch, backup restore — feeds
    // the same list.
    let new_workspace_path =
        resolve_workspace_path(app, &state.instance, config.workspace_dir.as_deref())?;
    config.remember_workspace(&new_workspace_path.to_string_lossy());

    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    // `write_config` serialises writes to the *shared* config, so a shadow
    // window merging its workspace history on exit cannot interleave with this
    // read-modify-write (a shadow's own copy needs no lock).
    state
        .instance
        .write_config(&state.config_path, &json)
        .map_err(|e| e.to_string())?;

    let logger_output = config.logger_output.clone();
    // A changed failure limit must also release tools that were disabled under
    // the old value: raising the limit is how a user un-breaks a session
    // without restarting the app.
    let previous_failure_limit = {
        let mut current = state.config.lock().unwrap();
        let previous = current.tool_failure_limit;
        *current = config;
        previous
    };
    let failure_limit_changed =
        previous_failure_limit != state.config.lock().unwrap().tool_failure_limit;
    if failure_limit_changed {
        state.tool_guard.reset_all();
        state
            .logger
            .lock()
            .unwrap()
            .log("INFO", "Tool failure guard reset (limit changed)");
    }

    fs::create_dir_all(&new_workspace_path).ok();
    // Note: `<workspace>/.data/skills` is NOT created eagerly. Skills are
    // discovered by scanning; a missing folder simply yields no workspace
    // skills until the Skills panel (or self-evolution) creates one.
    let previous_workspace_path = std::mem::replace(
        &mut *state.workspace_dir.lock().unwrap(),
        new_workspace_path.clone(),
    );

    // A silent maintenance pass belongs to the workspace (and to the mode) it
    // started in. A switch or turning self-evolution off cancels it, so it can
    // never keep writing into a workspace the user just left.
    if previous_workspace_path != new_workspace_path
        || !state.config.lock().unwrap().self_evolution_mode
    {
        evolution_maintenance::cancel(state);
    }

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

        // Timers resume a chat session that lives in this workspace's
        // database, so the registry moves with the workspace: the current set
        // is flushed back to the old file (it re-arms when that workspace is
        // opened again) and the new file's set is armed here.
        timer::rebase_workspace(app, workspace::timers_path(&new_workspace_path));

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
        let title = state.instance.window_title(&new_workspace_path);
        let _ = win.set_title(&title);
    }

    let mut logger = state.logger.lock().unwrap();
    logger.set_output(logger_output);
    logger.log("INFO", "Configuration updated");

    Ok(())
}

// ─── App State ───────────────────────────────────────────────────────────────

/// One extra HTTP header sent with every upstream model request.
///
/// Header names are case-insensitive; the value is forwarded verbatim (after
/// validation in `llm_complete::apply_request_headers`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestHeader {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

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
    /// messages as DeepSeek-family thinking models require. DeepSeek-family
    /// models always get it (see `ds_format_for`); the switch only forces it
    /// for other providers.
    #[serde(default)]
    pub ds_format: bool,
    /// Per-request correlation: the chat session id is forwarded under this
    /// name on every upstream request. Blank/absent → `x-conversation-id`.
    #[serde(default)]
    pub conversation_id_header: Option<String>,
    /// Extra HTTP headers attached to every upstream model request (chat,
    /// sub-agents, model listing) — e.g. gateway routing or auth headers.
    #[serde(default)]
    pub custom_headers: Vec<RequestHeader>,
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
    /// Allow the model to run `run_cmd` / `run_shell` / `file_actions search`
    /// as background tasks (`run_in_background: true`). The turn stays open
    /// until a started task delivers its result or is cancelled.
    #[serde(default = "update::default_true")]
    pub background_tools: bool,
    /// Allow `llm` sub-agents to call `ask_main_agent` and have the main agent
    /// (main model, tools, skills, MCP and conversation history) collect
    /// material for them. Disabled means the tool is neither advertised nor
    /// executable.
    #[serde(default = "update::default_true")]
    pub sub_agent_main_help: bool,
    /// Draw with `svg` / `carve` figures instead of Mermaid diagrams
    /// (Settings → Advanced, on by default). Off means the system prompt asks
    /// for a fenced `mermaid` block, which the transcript renders itself; the
    /// rasterise-and-save path (and 3D scenes) stay unused.
    #[serde(default = "update::default_true")]
    pub enhanced_figures: bool,
    /// Upper bound on model requests in one chat turn (`0` = unlimited; values
    /// above `turn_guard::MAX_MAIN_AGENT_ROUNDS` are clamped). A ReAct turn is
    /// driven by the model, so without a cap a tool loop could run forever;
    /// when the budget is spent the turn sends one last request *without*
    /// tools, so the assistant has to summarise what it already has.
    #[serde(default = "turn_guard::default_main_agent_max_rounds")]
    pub main_agent_max_rounds: u32,
    /// Upper bound on tool calls executed in one chat turn (`0` = unlimited).
    /// Calls beyond it are answered with a notice instead of running, which
    /// keeps every `tool_call_id` answered.
    #[serde(default = "turn_guard::default_main_agent_max_tool_calls")]
    pub main_agent_max_tool_calls: u32,
    /// Disable a tool for the rest of the session after this many consecutive
    /// failures (`0` = never disable). A successful call resets the count.
    #[serde(default = "turn_guard::default_tool_failure_limit")]
    pub tool_failure_limit: u32,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    /// Workspaces that were actually activated, newest first (capped at
    /// `WORKSPACE_HISTORY_LIMIT`). Recorded by `apply_config`, so every route
    /// that switches the workspace (Settings save, titlebar switch, backup
    /// restore) feeds the same list; the titlebar indicator offers it as a menu.
    #[serde(default)]
    pub workspace_history: Vec<String>,
    #[serde(default)]
    pub logger_output: LoggerOutput,
    #[serde(default)]
    pub auto_accept_confirm_kinds: Vec<String>,
    #[serde(default)]
    pub theme: Option<String>,
    /// Colour scheme layered on top of the light / dark theme
    /// (`default`, `sakura`, `cream`). Persisted like `theme`; the backend never
    /// interprets the value — the frontend turns it into `data-palette`.
    #[serde(default)]
    pub palette: Option<String>,
    /// Frosted-glass level (`off`, `subtle`, `standard`, `strong`). Persisted
    /// like `palette`; the backend never interprets the value — the frontend
    /// turns it into `data-glass`, which selects the `--glass-*` token block
    /// in App.css. Missing means `standard` (the design's own blur).
    #[serde(default)]
    pub glass_effect: Option<String>,
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
    /// Keep a single running instance: launching the app again focuses the
    /// window that is already open instead of starting a second process.
    ///
    /// The plugin that enforces this is registered before the Tauri builder
    /// exists (it must run first), so this value is read straight from
    /// `config.json` at process start and a change applies on the next launch.
    #[serde(default = "update::default_true")]
    pub single_instance: bool,
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
            conversation_id_header: None,
            custom_headers: vec![],
            model_context_lengths: HashMap::new(),
            model_metadata: HashMap::new(),
            model_reasoning_effort: HashMap::new(),
            system_message: String::new(),
            selected_tools: vec![],
            self_evolution_mode: false,
            background_tools: true,
            sub_agent_main_help: true,
            enhanced_figures: true,
            main_agent_max_rounds: turn_guard::DEFAULT_MAIN_AGENT_MAX_ROUNDS,
            main_agent_max_tool_calls: turn_guard::DEFAULT_MAIN_AGENT_MAX_TOOL_CALLS,
            tool_failure_limit: turn_guard::DEFAULT_TOOL_FAILURE_LIMIT,
            workspace_dir: None,
            workspace_history: vec![],
            logger_output: LoggerOutput::default(),
            auto_accept_confirm_kinds: vec![],
            theme: None,
            palette: None,
            glass_effect: None,
            language: None,
            check_updates_on_startup: true,
            include_prerelease_updates: false,
            log_retention_days: db::default_log_retention_days(),
            single_instance: true,
        }
    }
}

impl AppConfig {
    /// Drop tool ids that were removed from the tool set.
    ///
    /// Called on every config write, on startup and whenever the config is
    /// handed to the UI, so a hand-edited or stale `config.json` cannot keep a
    /// retired tool selected.
    fn strip_retired_tools(&mut self) {
        self.selected_tools
            .retain(|tool| !RETIRED_TOOL_IDS.contains(&tool.as_str()));
    }

    /// Force the self-evolution tools into the persisted selection.
    ///
    /// Called on every config write and on startup, so the invariant holds
    /// even when the config was edited outside the app.
    fn enforce_self_evolution_tools(&mut self) {
        if !self.self_evolution_mode {
            return;
        }
        for required in SELF_EVOLUTION_REQUIRED_TOOLS {
            if !self.selected_tools.iter().any(|tool| tool == required) {
                self.selected_tools.push(required.to_string());
            }
        }
    }

    /// Clamp the turn guards to their hard ceilings.
    ///
    /// Called on every config write (and on startup), so neither a hand-edited
    /// `config.json` nor an imported backup can set a number the loops would
    /// still have to iterate towards.
    fn normalize_turn_limits(&mut self) {
        self.main_agent_max_rounds = self
            .main_agent_max_rounds
            .min(turn_guard::MAX_MAIN_AGENT_ROUNDS);
        self.main_agent_max_tool_calls = self
            .main_agent_max_tool_calls
            .min(turn_guard::MAX_MAIN_AGENT_TOOL_CALLS);
        self.tool_failure_limit = self.tool_failure_limit.min(turn_guard::MAX_TOOL_FAILURE_LIMIT);
    }

    /// Record `path` as the most recent workspace.
    ///
    /// Called on every config write and once at startup (for the workspace we
    /// just opened), so the list holds the workspaces that were really active
    /// rather than the ones somebody typed in a form.
    /// Windows paths compare case-insensitively, so entries are deduplicated
    /// on the lowercased form while keeping the caller's spelling.
    ///
    /// Returns whether the list changed, so a caller that persists afterwards
    /// can skip a pointless write.
    ///
    /// `pub(crate)` because the multi-instance exit path (`instance.rs`) merges
    /// a shadow window's history back into this list.
    pub(crate) fn remember_workspace(&mut self, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        let key = path.to_lowercase();
        let already_newest = self
            .workspace_history
            .first()
            .map(|entry| entry.trim().to_lowercase() == key)
            .unwrap_or(false);
        if already_newest {
            return false;
        }
        self.workspace_history
            .retain(|entry| entry.trim().to_lowercase() != key);
        self.workspace_history.insert(0, path.to_string());
        self.workspace_history.truncate(WORKSPACE_HISTORY_LIMIT);
        true
    }

    /// The tool ids actually offered to the model.
    ///
    /// Self-evolution mode forces `file_actions` and `memory` on here, so a
    /// hand-edited config or a stale in-memory selection can never take them
    /// away from the assistant.
    pub fn effective_tools(&self) -> Vec<String> {
        let mut tools: Vec<String> = Vec::with_capacity(self.selected_tools.len() + 2);
        for tool in &self.selected_tools {
            if RETIRED_TOOL_IDS.contains(&tool.as_str()) {
                continue;
            }
            if !tools.iter().any(|existing| existing == tool) {
                tools.push(tool.clone());
            }
        }
        if self.self_evolution_mode {
            for required in SELF_EVOLUTION_REQUIRED_TOOLS {
                if !tools.iter().any(|existing| existing == required) {
                    tools.push(required.to_string());
                }
            }
        }
        tools
    }

    /// Effective DS-Format state for a request against `model`.
    ///
    /// The manual switch covers every provider, but DeepSeek-family models
    /// always get the normalization: their thinking mode rejects assistant
    /// messages that do not carry `reasoning_content`, so asking the user to
    /// flip a switch per model is a needless failure mode. The catalogue
    /// vendor is checked as well, for gateways that expose a DeepSeek model
    /// under a renamed id.
    pub fn ds_format_for(&self, model: &str) -> bool {
        self.ds_format
            || llm_complete::is_deepseek_model(model)
            || self
                .model_metadata
                .get(model)
                .map(|meta| meta.vendor.to_ascii_lowercase().contains("deepseek"))
                .unwrap_or(false)
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
    /// `false` → no turn is running for this session; the UI sends the text as
    /// a normal next-turn message instead of leaving it in the inbox.
    pub accepted: bool,
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    /// Which window this is — primary (owns the shared config) or a shadow
    /// running on a private copy (see `instance.rs`).
    pub instance: instance::Instance,
    pub workspace_dir: Mutex<PathBuf>,
    pub mcp_servers_path: PathBuf,
    pub agents_config_path: PathBuf,
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
    /// Cancellation slot of the silent evolution-maintenance turn (the
    /// empty-state "nap face"). `Some(flag)` while a pass is running; a new
    /// pass is refused until it clears. Separate from `chat_cancelled` on
    /// purpose: the maintenance turn must never share a cancel flag or the
    /// `chat-*` event stream with a real chat turn.
    pub evolution_maintenance: Mutex<Option<Arc<AtomicBool>>>,
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
    /// Serializes tool confirmation dialogs: `confirm_sender` is one slot, so
    /// the main agent and background tasks must queue instead of overwriting
    /// each other's pending request.
    pub tool_confirm_lock: tokio::sync::Mutex<()>,
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
    /// Monotonic revision of the MCP registry (`mcp_servers.json`).
    ///
    /// Bumped by every entry change (`save_mcp_server`, `delete_mcp_server`,
    /// `mcp::upsert_server` / `remove_server`). `chat_completion` captures it
    /// when it assembles the tool payload and rebuilds that payload at the next
    /// request boundary when it changed — that is what makes a server created
    /// and enabled by the `mcp_self` tool usable inside the *same* turn.
    pub mcp_revision: AtomicU64,
    /// Long-lived background children started by `run_shell run_type="serve"`.
    /// Killed on demand by the tool and automatically at app exit.
    pub serve_processes: tools::serve::ServeRegistry,
    /// Turn-scoped background tool tasks (`run_in_background: true`). Their
    /// finished results are injected into the conversation at the next request
    /// boundary; the whole session is torn down when the turn ends.
    pub background_tasks: tools::background::BackgroundRegistry,
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
    /// Tools disabled for the rest of a chat session after repeated failures
    /// (Settings → Advanced). Shared by the main agent, its helper and every
    /// sub-agent of the session; in-memory only, cleared when the limit is
    /// reconfigured or the app exits.
    pub tool_guard: turn_guard::ToolFailureGuard,
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

    /// Current MCP registry revision (see `AppState::mcp_revision`).
    pub fn mcp_revision(&self) -> u64 {
        self.mcp_revision.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Mark the MCP registry as changed, so an in-flight turn rebuilds its MCP
    /// tool segment at the next request boundary.
    pub fn bump_mcp_revision(&self) -> u64 {
        self.mcp_revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
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
    let mut config = state.config.lock().unwrap().clone();
    // Retired tool ids are dropped on the way out too, so the UI never renders
    // a switch for a tool that no longer exists.
    config.strip_retired_tools();
    // The UI must see the enforced selection, so the two self-evolution tools
    // always show as enabled while the mode is on.
    config.enforce_self_evolution_tools();
    config
}

#[tauri::command]
fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    apply_config(&app, &state, config)
}

/// Store (or clear) the context window for one model id.
///
/// The Settings form only edits the main model's window, but an `llm`
/// sub-agent may run a different model. The budget is keyed by model id and
/// shared by every agent that uses it, so the Agents panel updates just this
/// entry instead of round-tripping the whole `AppConfig` form.
#[tauri::command]
fn set_model_context_length(
    app: AppHandle,
    state: State<'_, AppState>,
    model: String,
    context_length: Option<u32>,
) -> Result<(), String> {
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("A model id is required to store a context window.".to_string());
    }

    let mut config = state.config.lock().unwrap().clone();
    match context_length.filter(|value| *value > 0) {
        Some(value) => {
            config.model_context_lengths.insert(model, value);
        }
        None => {
            config.model_context_lengths.remove(&model);
        }
    }
    apply_config(&app, &state, config)
}

#[tauri::command]
async fn fetch_models(state: State<'_, AppState>) -> Result<Vec<RemoteModel>, String> {
    let config = state.config.lock().unwrap().clone();
    let url = format!("{}/models", config.api_base_url.trim_end_matches('/'));

    let client = llm_complete::build_http_client()?;
    let headers = llm_complete::custom_request_headers(&config);
    let mut request = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/json");
    if !llm_complete::has_authorization_header(&headers) {
        request = request.bearer_auth(&config.api_key);
    }
    let res = llm_complete::apply_request_headers(request, &headers)
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
fn stop_chat_completion(app: AppHandle, state: State<'_, AppState>) {
    // Cancel the *current* turn's flag. If no turn is running this simply
    // marks the slot's default flag — a new turn installs a fresh flag anyway,
    // so the stop never leaks into the next turn.
    if let Ok(flag) = state.chat_cancelled.lock() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // Background tasks must die immediately, not at the next loop iteration:
    // a foreground tool may still be blocking the turn. The active turn carries
    // the session id, so only this session's tasks are touched.
    let session_id = state
        .active_turn
        .lock()
        .ok()
        .and_then(|active| active.as_ref().map(|(_, session_id)| session_id.clone()));
    if let Some(session_id) = session_id {
        let cancelled = state.background_tasks.cancel_session(&session_id);
        // Broadcast before the turn unwinds. Stop is the one path where the
        // turn-end teardown may drop the entry before the runner can report it,
        // so the UI would otherwise keep the row (and its cancel button) alive
        // for a task that is already dying.
        for snapshot in &cancelled {
            let _ = app.emit(
                tools::background::BACKGROUND_TASK_STATE_EVENT,
                snapshot.clone(),
            );
        }
        let _ = app.emit(
            tools::background::BACKGROUND_WAITING_EVENT,
            serde_json::json!({ "session_id": session_id, "pending": 0 }),
        );
        if !cancelled.is_empty() {
            let count = cancelled.len();
            let _ = state.logger.lock().map(|logger| {
                logger.log(
                    "INFO",
                    &format!(
                        "Stop pressed: cancelled {count} background task(s) of session {session_id}"
                    ),
                );
            });
        }
    }
}

/// Snapshot of the background tasks of one chat session (UI management).
#[tauri::command]
fn list_background_tasks(
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Vec<tools::background::BackgroundTaskSnapshot> {
    match session_id {
        Some(session_id) => state.background_tasks.list(&session_id),
        None => Vec::new(),
    }
}

/// Cancel one background task (the UI's cancel button). Returns the updated
/// snapshot so the caller can update its entry without waiting for an event.
#[tauri::command]
fn cancel_background_task(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<tools::background::BackgroundTaskSnapshot, String> {
    state
        .background_tasks
        .cancel(&task_id)
        .ok_or_else(|| format!("Unknown background task '{task_id}'"))
}

/// The session's active todo list, or `null` when it has none. The todo panel
/// is mounted per assistant message, so a list created in an earlier turn (or
/// before a reload) has to be fetched instead of waiting for the next
/// `todo-state` event.
#[tauri::command]
fn get_active_todo_list(
    app: AppHandle,
    session_id: String,
) -> Result<Option<todos::TodoListSummary>, String> {
    todos::get_active_list(&app, &session_id)
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

/// Windows paths differ only in case and separator style, so the workspace
/// switcher compares them the way the history dedup does.
fn same_workspace_path(a: &str, b: &str) -> bool {
    let normalize = |value: &str| {
        value
            .trim()
            .trim_end_matches(['/', '\\'])
            .replace('\\', "/")
            .to_lowercase()
    };
    normalize(a) == normalize(b)
}

/// Switch the workspace in place — the titlebar's workspace indicator.
///
/// The Settings panel *stages* `workspace_dir` and only applies it on "Save
/// Changes"; the titlebar is a shortcut, so it writes that one field onto the
/// current config instead of round-tripping the whole form. A form copy that
/// predates another panel's edit must never overwrite it, which is why this is
/// a dedicated command (same reason as `set_model_context_length`).
#[tauri::command]
fn set_workspace_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_dir: Option<String>,
) -> Result<String, String> {
    // A running turn owns the workspace it started in: its tool calls resolve
    // paths against it, and its rows are written through the database handle
    // the switch closes. The UI disables the rows while streaming; this guard
    // is what makes a hand-crafted invoke fail instead of corrupting a turn.
    if state
        .active_turn
        .lock()
        .map(|active| active.is_some())
        .unwrap_or(false)
    {
        return Err(
            "A reply is still streaming — switch the workspace when the turn ends.".to_string(),
        );
    }

    let mut config = state.config.lock().unwrap().clone();
    config.workspace_dir = workspace_dir
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty());
    apply_config(&app, &state, config)?;
    Ok(state
        .workspace_dir
        .lock()
        .unwrap()
        .to_string_lossy()
        .to_string())
}

/// One row of the titlebar workspace switcher.
#[derive(Debug, Clone, Serialize)]
struct WorkspaceHistoryEntry {
    path: String,
    /// Whether the folder is still on disk — a recorded workspace may have
    /// been moved or deleted since it was last used.
    exists: bool,
    /// Whether this is the workspace the window shows now. Compared against
    /// the *resolved* path, so the default (app-data) workspace highlights
    /// correctly even though `config.workspace_dir` is `None` there.
    current: bool,
}

/// The workspaces the titlebar switcher offers — one call, everything the
/// popup needs.
///
/// The list is derived (recorded history + resolved current path), so the UI
/// never rebuilds it from `AppConfig`: that only knows `workspace_dir: None`
/// for the default workspace, not the path it resolves to.
#[tauri::command]
fn workspace_history_entries(state: State<'_, AppState>) -> Vec<WorkspaceHistoryEntry> {
    let current = state.workspace_dir.lock().unwrap().to_string_lossy().to_string();
    let config = state.config.lock().unwrap();
    config
        .workspace_history
        .iter()
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| WorkspaceHistoryEntry {
            path: entry.clone(),
            exists: PathBuf::from(entry).is_dir(),
            current: same_workspace_path(entry, &current),
        })
        .collect()
}

/// Which window this is: the primary, or a shadow running on a temporary copy
/// of the configuration (see `instance.rs`). The Settings panel uses it to
/// explain why this window's changes will not be remembered.
#[tauri::command]
fn get_instance_info(state: State<'_, AppState>) -> instance::InstanceInfo {
    state.instance.info()
}

#[tauri::command]
fn save_markdown_file(path: String, content: String) -> Result<(), String> {
    fs::write(PathBuf::from(path), content).map_err(|e| e.to_string())
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// `dirs::data_dir()/<identifier>` — the path `app.path().app_data_dir()`
/// resolves to (Tauri's `PathResolver::app_data_dir` uses `dirs::data_dir()`,
/// *not* the local variant; on Windows that is `%APPDATA%`, i.e. Roaming).
///
/// Needed *before* Tauri exists: the single-instance plugin and the primary
/// lock are both decided at process start, long before `setup` runs. If this
/// ever disagrees with Tauri, `setup` re-claims under the real directory — a
/// mismatch here would otherwise read (and seed) a default config in a
/// directory the app does not use.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn app_data_dir_before_tauri(identifier: &str) -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join(identifier))
}

/// Just the one field the plugin wiring needs, so a config that is missing,
/// half-written, or from an older version falls back to the default instead of
/// failing to parse.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Deserialize)]
struct SingleInstanceProbe {
    #[serde(default = "update::default_true")]
    single_instance: bool,
}

/// The persisted `single_instance` switch, readable *before* Tauri exists.
///
/// The plugin decides at registration time whether a second process is
/// terminated, so the flag cannot come from `AppState` (managed in `setup`,
/// after the builder). `dirs::data_dir()/identifier` is the same path
/// `app.path().app_data_dir()` resolves to.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)] // release-only: debug builds never register the plugin
fn persisted_single_instance(identifier: &str) -> bool {
    let Some(data_dir) = app_data_dir_before_tauri(identifier) else {
        return true;
    };
    fs::read_to_string(data_dir.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<SingleInstanceProbe>(&raw).ok())
        .map(|probe| probe.single_instance)
        .unwrap_or(true)
}

/// Bring the window of the already-running instance back to the front.
///
/// Only setters are sent, with no state probe (`unminimize` runs
/// unconditionally): on Windows the plugin invokes this callback on the main
/// thread from a `WM_COPYDATA` handler, where Tauri dispatches window messages
/// inline. Windows lets `set_focus` succeed because the second instance hands
/// over the foreground right (`AllowSetForegroundWindow`) before the plugin
/// exits it.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[allow(dead_code)] // release-only: debug builds never register the plugin
fn focus_main_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.show();
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

// ─── Window backdrop (frosted glass) ─────────────────────────────────────────

/// Paint the window-level backdrop for one frosted-glass level and report what
/// ended up in effect.
///
/// A webview can blur only its own pixels, so the desktop seen *through* the
/// window can only be blurred by the compositor: Mica on Windows 11, Acrylic
/// on Windows 10 (where the Mica call fails). The returned kind is what the
/// frontend writes to `<html data-vibrancy>`; `None` means no backdrop is in
/// effect and the app keeps its opaque canvas — which is also the only safe
/// answer on platforms without a supported backdrop, because a transparent
/// window with nothing behind it would show the raw, unblurred desktop.
///
/// The backdrop itself stays nearly colourless: the density of each level
/// lives in `--c-window` (`src/App.css`), so Mica and Acrylic land on the same
/// contrast and the level can be changed without touching the native side.
#[cfg(target_os = "windows")]
fn apply_window_backdrop(
    window: &tauri::WebviewWindow,
    level: &str,
    dark: bool,
) -> Option<&'static str> {
    use window_vibrancy::{apply_acrylic, apply_mica, clear_acrylic, clear_mica};

    if level == "off" {
        // Clear both, so switching the switch off never leaves the previous
        // backdrop behind on a host that only supports one of them.
        let _ = clear_mica(window);
        let _ = clear_acrylic(window);
        return None;
    }

    // Acrylic before Mica, deliberately. Mica only samples the wallpaper, so
    // against a plain background the frame reads as a flat pane — the glass
    // this app sells is "what is behind the window is blurred", and that is
    // DWMSBT_TRANSIENTWINDOW (Acrylic), the same backdrop the Windows 11
    // flyouts use. `apply_mica` still runs first for its side effect: it is
    // the only call that sets DWMWA_USE_IMMERSIVE_DARK_MODE, which makes the
    // Acrylic tint follow the *app* theme instead of the system one, and the
    // Acrylic call then replaces the Mica backdrop it left behind.
    let mica = apply_mica(window, Some(dark)).is_ok();
    // The colour only reaches the legacy Windows 10 v1809+ path; the Windows
    // 11 system backdrop is tinted by the OS, so this stays a light wash.
    let tint: (u8, u8, u8, u8) = if dark {
        (18, 18, 18, 120)
    } else {
        (250, 250, 250, 120)
    };
    if apply_acrylic(window, Some(tint)).is_ok() {
        return Some("acrylic");
    }
    if mica {
        return Some("mica");
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn apply_window_backdrop(
    _window: &tauri::WebviewWindow,
    _level: &str,
    _dark: bool,
) -> Option<&'static str> {
    // No window backdrop on macOS / Linux: the config promises a glass level,
    // not a platform feature, and a transparent window without a backdrop
    // would show the desktop unblurred behind the chrome.
    None
}

/// Re-apply the window backdrop for a frosted-glass level (`off`, `subtle`,
/// `standard`, `strong`) and the theme it should match.
///
/// Returns the kind now in effect (`mica` / `acrylic`) or `None`; the frontend
/// writes that to `<html data-vibrancy>`, which is the only switch that turns
/// the translucent window tokens on. Called on startup (see `setup`) and from
/// Settings → Appearance, including the live preview.
#[tauri::command]
fn set_window_vibrancy(
    window: tauri::WebviewWindow,
    level: String,
    dark: bool,
) -> Option<String> {
    apply_window_backdrop(&window, &level, dark).map(str::to_string)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();

    // Multi-instance bookkeeping (see `instance.rs`): the first process takes
    // the primary role and keeps the shared config; every later one has to run
    // on a private copy. The role is settled here because the lock is the only
    // thing that tells a second process not to write the shared file — but the
    // shadow *directory* is only created in `setup`, so a launch the
    // single-instance plugin aborts leaves nothing behind.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let claimed_instance = app_data_dir_before_tauri(context.config().identifier.as_str())
        .map(instance::Instance::claim);
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let claimed_instance: Option<instance::Instance> = None;

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    // Must stay the first plugin: a second instance has to be detected before
    // any other setup runs. The switch is read from disk here because the
    // plugin is installed before `setup` (and therefore before `AppState`).
    //
    // Release builds only: a debug build shares the bundle identifier with the
    // installed app, so registering the plugin there would make `tauri dev`
    // exit (and focus the packaged app) whenever it is already running.
    #[cfg(all(
        not(debug_assertions),
        not(any(target_os = "android", target_os = "ios"))
    ))]
    if persisted_single_instance(context.config().identifier.as_str()) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .on_menu_event(|app, event| {
            if event.id() == OPEN_APP_DATA_DIR_MENU_ID {
                open_app_data_dir(app);
            } else if event.id() == OPEN_WORKSPACE_DIR_MENU_ID {
                let _ = open_workspace_dir(app);
            } else if event.id() == SAVE_BACKUP_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Backup", &["zip"])
                    .set_file_name("ai-chat.backup.zip")
                    .blocking_save_file()
                {
                    if let Some(profile_path) = path.into_path().ok() {
                        backup::spawn_profile_export(app.clone(), profile_path);
                    }
                }
            } else if event.id() == RESTORE_BACKUP_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Backup", &["zip"])
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
            } else if event.id() == OPEN_ISSUE_MENU_ID {
                let _ = open_issue_page(app);
            } else if event.id() == ABOUT_MENU_ID {
                show_about(app);
            }
        })
        .setup(move |app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            fs::create_dir_all(&data_dir).ok();

            // Only macOS gets a native menu bar: there it lands in the
            // system-wide bar, which is how a Mac app exposes these commands.
            // On GTK/Win32 `set_menu` instead draws a bar *inside* the window,
            // stacked above our decorations-less custom titlebar — so those
            // platforms rely on the titlebar's `⋯` menu, which carries the
            // very same entries (see `TitleBar.tsx`).
            #[cfg(target_os = "macos")]
            {
                let open_app_data_dir_item = MenuItem::with_id(
                    app,
                    OPEN_APP_DATA_DIR_MENU_ID,
                    "Open App Data Directory",
                    true,
                    None::<&str>,
                )
                .expect("failed to create menu item");
                let open_workspace_dir_item = MenuItem::with_id(
                    app,
                    OPEN_WORKSPACE_DIR_MENU_ID,
                    "Open Workspace Directory",
                    true,
                    None::<&str>,
                )
                .expect("failed to create workspace menu item");
                let save_backup_item = MenuItem::with_id(
                    app,
                    SAVE_BACKUP_MENU_ID,
                    "Save Backup...",
                    true,
                    None::<&str>,
                )
                .expect("failed to create menu item");
                let restore_backup_item = MenuItem::with_id(
                    app,
                    RESTORE_BACKUP_MENU_ID,
                    "Restore Backup...",
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
                    &[
                        &save_backup_item,
                        &restore_backup_item,
                        &open_app_data_dir_item,
                        &open_workspace_dir_item,
                    ],
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
                        &PredefinedMenuItem::undo(app, None::<&str>)
                            .expect("failed to create undo menu item"),
                        &PredefinedMenuItem::redo(app, None::<&str>)
                            .expect("failed to create redo menu item"),
                        &PredefinedMenuItem::separator(app).expect("failed to create separator"),
                        &PredefinedMenuItem::cut(app, None::<&str>)
                            .expect("failed to create cut menu item"),
                        &PredefinedMenuItem::copy(app, None::<&str>)
                            .expect("failed to create copy menu item"),
                        &PredefinedMenuItem::paste(app, None::<&str>)
                            .expect("failed to create paste menu item"),
                        &PredefinedMenuItem::select_all(app, None::<&str>)
                            .expect("failed to create select all menu item"),
                    ],
                )
                .expect("failed to create edit menu");

                let about_item =
                    MenuItem::with_id(app, ABOUT_MENU_ID, "About AI Chat", true, None::<&str>)
                        .expect("failed to create about menu item");
                let issue_item = MenuItem::with_id(
                    app,
                    OPEN_ISSUE_MENU_ID,
                    "Report an Issue...",
                    true,
                    None::<&str>,
                )
                .expect("failed to create issue menu item");
                let about_menu =
                    Submenu::with_items(app, "About", true, &[&issue_item, &about_item])
                        .expect("failed to create about menu");
                let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &tools_menu, &about_menu])
                    .expect("failed to create app menu");
                app.set_menu(menu).expect("failed to set app menu");
            }

            // Multi-instance: drop copies left behind by killed windows, then
            // settle this process's role. The primary keeps using the shared
            // `config.json`; every later window gets a private snapshot so it
            // can neither read nor move another window's workspace.
            //
            // The claim was taken before Tauri existed, from the same path
            // `app_data_dir()` resolves to. Should the two ever diverge, release
            // the wrong claim and take the real one: continuing with it would
            // read and seed a config in a directory the app does not use.
            let mut instance = match claimed_instance {
                Some(instance) if instance.data_dir() == data_dir => instance,
                Some(instance) => {
                    instance.shutdown();
                    instance::Instance::claim(data_dir.clone())
                }
                None => instance::Instance::claim(data_dir.clone()),
            };
            let swept = instance::collect_garbage(&data_dir);
            // Reported to `app.log` below — the logger does not exist yet.
            let mut prepare_error: Option<String> = None;
            let config_path = instance.prepare().unwrap_or_else(|err| {
                // Opening the app beats perfect isolation: fall back to the
                // shared config instead of refusing to start.
                prepare_error = Some(err);
                instance.main_config_path()
            });

            let mcp_servers_path = data_dir.join("mcp_servers.json");
            let agents_config_path = data_dir.join("sub_agents.json");
            let config = if config_path.exists() {
                fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default()
            } else {
                AppConfig::default()
            };
            // A config edited outside the app may be missing the tools that
            // self-evolution mode requires — re-add them before the first turn.
            let mut config = config;
            config.strip_retired_tools();
            config.enforce_self_evolution_tools();
            config.normalize_turn_limits();

            // Resolve the active workspace first: the database, memory, todo
            // lists and skills all live in its hidden `.data` directory, so the
            // workspace must be known before the database is opened.
            let workspace_dir = match config.workspace_dir.as_deref().filter(|s| !s.is_empty()) {
                Some(dir) => PathBuf::from(dir),
                None => instance.default_workspace(data_dir.join("workspace")),
            };
            fs::create_dir_all(&workspace_dir).ok();
            workspace::ensure_data_layout(&workspace_dir).ok();
            // Seed the history with the workspace we are about to open: on the
            // first launch after the upgrade this is the only entry, and it
            // becomes pickable again as soon as the user switches away once.
            if config.remember_workspace(&workspace_dir.to_string_lossy()) {
                if let Ok(json) = serde_json::to_string_pretty(&config) {
                    instance.write_config(&config_path, &json).ok();
                }
            }
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

            if swept > 0 {
                app_logger.log(
                    "INFO",
                    &format!("Multi-instance: removed {swept} leftover shadow directory(ies)"),
                );
            }
            if let Some(err) = prepare_error {
                app_logger.log(
                    "WARN",
                    &format!("Multi-instance: {err}; using the shared config"),
                );
            }

            // Timers used to live in the app data directory, shared by every
            // workspace; they now sit next to the sessions they resume. Move
            // the old file into the active workspace once.
            let timers_path = workspace::timers_path(&workspace_dir);
            if let Err(err) =
                timer::migrate_legacy_file(&data_dir.join("timers.json"), &timers_path)
            {
                app_logger.log("WARN", &format!("Timer migration failed: {err}"));
            }

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
                instance,
                workspace_dir: Mutex::new(workspace_dir.clone()),
                db: Mutex::new(db),
                logger: Mutex::new(app_logger),
                mcp_servers_path: mcp_servers_path.clone(),
                agents_config_path,
                chat_cancelled: Mutex::new(Arc::new(AtomicBool::new(false))),
                evolution_maintenance: Mutex::new(None),
                turn_seq: AtomicU64::new(1),
                active_turn: Mutex::new(None),
                turn_inbox: Mutex::new(Vec::new()),
                confirm_sender: Mutex::new(None),
                tool_confirm_lock: tokio::sync::Mutex::new(()),
                mcp_logs: std::sync::Arc::new(Mutex::new(Default::default())),
                mcp_cancelled_tests: std::sync::Arc::new(Mutex::new(Default::default())),
                mcp_children: std::sync::Arc::new(tokio::sync::Mutex::new(Default::default())),
                mcp_revision: AtomicU64::new(1),
                serve_processes: Default::default(),
                background_tasks: Default::default(),
                session_memories: Mutex::new(Default::default()),
                external_agent_confirmed: Mutex::new(Default::default()),
                external_agent_confirm_lock: tokio::sync::Mutex::new(()),
                tool_guard: Default::default(),
            });

            // Delayed "continue the task" timers, mirrored to
            // `<workspace>/.data/timers.json` so a pending wait survives an app
            // restart *and* stays with the workspace whose session it resumes.
            let sleep_lock = timer::SleepLock::new(app.handle().clone());
            app.manage(sleep_lock.clone());
            app.manage(timer::TimerRegistry::load(timers_path, sleep_lock));
            timer::restore_on_startup(app.handle());

            // Window title: the active workspace, tagged when this is a shadow
            // window so two of them are distinguishable in the task bar.
            if let Some(win) = app.get_webview_window("main") {
                let title = app.state::<AppState>().instance.window_title(&workspace_dir);
                let _ = win.set_title(&title);
            }

            // Window backdrop: on Windows the glass level is a window-level
            // effect, so it is applied once here and re-applied whenever
            // Settings changes it. `data-vibrancy` is written only when a
            // backdrop really is in effect — the opaque canvas then stays the
            // fallback instead of a transparent window over a raw desktop.
            if let Some(win) = app.get_webview_window("main") {
                let (level, dark) = {
                    let state = app.state::<AppState>();
                    let config = state.config.lock().unwrap();
                    (
                        config
                            .glass_effect
                            .clone()
                            .unwrap_or_else(|| "standard".to_string()),
                        config.theme.as_deref() == Some("dark"),
                    )
                };
                if let Some(kind) = apply_window_backdrop(&win, &level, dark) {
                    let _ = win.eval(&format!(
                        "document.documentElement.dataset.vibrancy = '{kind}'"
                    ));
                }
            }

            // Warmup: auto-start all enabled stdio MCP servers
            {
                // Servers the assistant built live in the workspace roster
                // (`.data/mcp.conf`); fold any leftover ones out of the
                // app-data file before the merged roster is read.
                let managed = app.try_state::<AppState>();
                if let Some(state) = managed.as_deref() {
                    match mcp::migrate_self_built_servers(state) {
                        Ok(0) => {}
                        Ok(moved) => state.logger.lock().unwrap().log(
                            "INFO",
                            &format!(
                                "Moved {moved} self-built MCP server(s) into <workspace>/.data/mcp.conf"
                            ),
                        ),
                        Err(err) => state.logger.lock().unwrap().log(
                            "WARN",
                            &format!("Could not move self-built MCP servers into the workspace: {err}"),
                        ),
                    }
                }
                let servers = match managed.as_deref() {
                    Some(state) => mcp::load_effective_servers(state),
                    None => mcp::load_servers(&mcp_servers_path),
                };
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
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                // The only window is closing, so the app is exiting. Tear down
                // every child tree (MCP servers, serve processes, CLI agents)
                // instead of only the MCP pool.
                process_guard::kill_all();
                // A shadow window merges its workspace history and deletes its
                // temporary config; the primary releases its lock. Idempotent,
                // because the exit hook below runs again on quits that never
                // close a window.
                if let Some(state) = window.app_handle().try_state::<AppState>() {
                    if let Some(warning) = state.instance.shutdown() {
                        state
                            .logger
                            .lock()
                            .unwrap()
                            .log("WARN", &format!("Multi-instance: {warning}"));
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            llm_complete::chat_completion,
            stop_chat_completion,
            evolution_maintenance::start_evolution_maintenance,
            evolution_maintenance::stop_evolution_maintenance,
            evolution_maintenance::get_evolution_maintenance_state,
            inject_user_message,
            list_background_tasks,
            cancel_background_task,
            get_active_todo_list,
            confirm_command,
            get_config,
            save_config,
            set_model_context_length,
            fetch_models,
            model_metadata::fetch_model_metadata,
            get_workspace_dir,
            set_workspace_dir,
            workspace_history_entries,
            get_instance_info,
            set_window_vibrancy,
            workspace_files::list_workspace_files,
            workspace_files::read_workspace_file_range,
            workspace_files::read_workspace_file_bytes,
            workspace_files::save_pasted_file,
            workspace_files::import_workspace_paths,
            pdf_mentions::convert_pdf_mention,
            save_markdown_file,
            figures::save_figure,
            figures::save_figure_html,
            figures::app_open_figures_dir,
            app_open_data_dir,
            app_open_workspace_dir,
            app_open_workspace_tmp_dir,
            app_open_issue_page,
            app_open_markdown,
            app_export_profile,
            app_import_profile,
            skills::list_skills,
            skills::save_skill,
            skills::delete_skill,
            skills::filter_existing_skills,
            skills::get_skills_status,
            skills::save_skills_status,
            tools::memory::list_evolution_memory,
            tools::memory::read_evolution_entry,
            tools::memory::list_archived_skills,
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
            db::get_session_stats,
            mcp::list_mcp_servers,
            mcp::save_mcp_server,
            mcp::delete_mcp_server,
            mcp::test_mcp_server,
            mcp::get_mcp_logs,
            mcp::clear_mcp_logs,
            mcp::mcp_start_stdio,
            mcp::mcp_stop_stdio,
            agents::list_sub_agents,
            agents::list_agent_mentions,
            agents::refresh_agent_index,
            agents::save_sub_agent,
            agents::delete_sub_agent,
            agents::get_agent_orchestration,
            agents::save_agent_orchestration,
            agents::list_agent_missions,
            agents::mbus::list_mission_bus_messages,
            agents::probe_agent_cli,
            agents::probe_agent_cloud,
            app_open_agents_dir,
            timer::list_timers,
            timer::cancel_timer,
            timer::get_sleep_lock_state,
            tools::serve::list_serve_processes,
            tools::serve::stop_serve_process,
            update::check_update,
            update::download_update,
            update::open_update_file,
            update::reveal_update_file,
            update::open_release_page,
            update::get_app_version,
        ])
        .build(context)
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Normal quit path. `kill_all` is idempotent, so running it after
            // the window-close hook is harmless; it also covers tray/menu
            // quits that never emit `CloseRequested`.
            if let tauri::RunEvent::Exit = event {
                process_guard::kill_all();
                if let Some(state) = app_handle.try_state::<AppState>() {
                    if let Some(warning) = state.instance.shutdown() {
                        state
                            .logger
                            .lock()
                            .unwrap()
                            .log("WARN", &format!("Multi-instance: {warning}"));
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-instance switch is read from `config.json` before Tauri
    /// starts, so the probe must tolerate a config that predates the field.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn single_instance_probe_defaults_on_and_reads_the_switch() {
        let legacy: SingleInstanceProbe = serde_json::from_str(r#"{"model":"gpt-4o-mini"}"#)
            .expect("a partial config still parses");
        assert!(legacy.single_instance, "a missing field means enabled");

        let off: SingleInstanceProbe =
            serde_json::from_str(r#"{"single_instance":false}"#).expect("the switch parses");
        assert!(!off.single_instance);
    }

    /// Self-evolution mode owns `file_actions` and `memory`: the assistant
    /// must be able to edit its own skills and to remember what it learned.
    #[test]
    fn self_evolution_forces_the_file_and_memory_tools() {
        let mut config = AppConfig::default();
        config.selected_tools = vec!["run_cmd".to_string()];
        config.self_evolution_mode = true;

        config.enforce_self_evolution_tools();
        assert!(config.selected_tools.contains(&"file_actions".to_string()));
        assert!(config.selected_tools.contains(&"memory".to_string()));
        assert!(config.selected_tools.contains(&"run_cmd".to_string()));

        // Enforcing twice must not duplicate the entries.
        config.enforce_self_evolution_tools();
        assert_eq!(
            config
                .selected_tools
                .iter()
                .filter(|tool| *tool == "file_actions")
                .count(),
            1
        );
    }

    /// A hand-edited config that drops the tools cannot take them away from
    /// the model while the mode is on.
    #[test]
    fn retired_tools_are_dropped_from_the_selection() {
        let mut config = AppConfig::default();
        config.selected_tools = vec![
            "run_cmd".to_string(),
            "knowledge_graph".to_string(),
            "memory".to_string(),
        ];

        config.strip_retired_tools();
        assert_eq!(
            config.selected_tools,
            vec!["run_cmd".to_string(), "memory".to_string()]
        );

        // `effective_tools` is the last line of defence for a stale in-memory
        // selection that never went through `strip_retired_tools`.
        config.selected_tools.push("knowledge_graph".to_string());
        let tools = config.effective_tools();
        assert!(!tools.contains(&"knowledge_graph".to_string()));
        assert!(tools.contains(&"run_cmd".to_string()));
    }

    /// A `config.json` written by an older version still parses: the removed
    /// knowledge graph settings are ignored instead of failing deserialization.
    #[test]
    fn legacy_graph_settings_are_ignored() {
        let config: AppConfig = serde_json::from_str(
            r#"{
                "api_base_url": "https://api.openai.com/v1",
                "api_key": "sk-test",
                "model": "gpt-4o-mini",
                "selected_tools": ["run_cmd", "knowledge_graph"],
                "kg_engine": "neo4j",
                "neo4j_uri": "bolt://db:7687",
                "neo4j_user": "neo4j",
                "neo4j_password": "secret"
            }"#,
        )
        .expect("a legacy config still parses");

        assert_eq!(
            config.selected_tools,
            vec!["run_cmd".to_string(), "knowledge_graph".to_string()]
        );
    }

    #[test]
    fn effective_tools_keeps_the_required_tools_even_if_the_config_lacks_them() {
        let mut config = AppConfig::default();
        config.selected_tools = vec!["run_shell".to_string()];
        config.self_evolution_mode = true;

        let tools = config.effective_tools();
        assert!(tools.contains(&"file_actions".to_string()));
        assert!(tools.contains(&"memory".to_string()));
        assert!(tools.contains(&"run_shell".to_string()));

        // With the mode off the selection is untouched.
        config.self_evolution_mode = false;
        let tools = config.effective_tools();
        assert!(!tools.contains(&"file_actions".to_string()));
        assert!(!tools.contains(&"memory".to_string()));
    }

    /// DeepSeek-family models get DS-Format without the manual switch; the
    /// catalogue vendor is the fallback for renamed gateway ids.
    #[test]
    fn ds_format_is_automatic_for_deepseek_models() {
        let mut config = AppConfig::default();

        assert!(!config.ds_format_for("gpt-4o-mini"));
        assert!(config.ds_format_for("deepseek-chat"));
        assert!(config.ds_format_for("deepseek-ai/DeepSeek-V3.1"));

        config.ds_format = true;
        assert!(config.ds_format_for("gpt-4o-mini"));

        config.ds_format = false;
        config.model_metadata.insert(
            "renamed-v3".to_string(),
            serde_json::from_value(serde_json::json!({
                "model_name": "DeepSeek-V3",
                "vendor": "DeepSeek",
            }))
            .unwrap(),
        );
        assert!(config.ds_format_for("renamed-v3"));
    }

    /// Duplicates in the persisted selection are collapsed.
    #[test]
    fn effective_tools_deduplicates_the_selection() {
        let mut config = AppConfig::default();
        config.selected_tools = vec![
            "memory".to_string(),
            "run_cmd".to_string(),
            "memory".to_string(),
        ];

        let tools = config.effective_tools();
        assert_eq!(tools.iter().filter(|tool| *tool == "memory").count(), 1);
        assert_eq!(tools.len(), 2);
    }

    /// An older `config.json` gets the turn-guard defaults, and absurd values
    /// are clamped instead of trusted.
    #[test]
    fn turn_guards_default_and_clamp() {
        let legacy: AppConfig = serde_json::from_str(
            r#"{
                "api_base_url": "https://api.openai.com/v1",
                "api_key": "sk-test",
                "model": "gpt-4o-mini"
            }"#,
        )
        .expect("a config without the new fields still parses");
        assert_eq!(
            legacy.main_agent_max_rounds,
            turn_guard::DEFAULT_MAIN_AGENT_MAX_ROUNDS
        );
        assert_eq!(
            legacy.main_agent_max_tool_calls,
            turn_guard::DEFAULT_MAIN_AGENT_MAX_TOOL_CALLS
        );
        assert_eq!(
            legacy.tool_failure_limit,
            turn_guard::DEFAULT_TOOL_FAILURE_LIMIT
        );

        let mut config = AppConfig::default();
        config.main_agent_max_rounds = u32::MAX;
        config.main_agent_max_tool_calls = 99_999;
        config.tool_failure_limit = 999;
        config.normalize_turn_limits();
        assert_eq!(
            config.main_agent_max_rounds,
            turn_guard::MAX_MAIN_AGENT_ROUNDS
        );
        assert_eq!(
            config.main_agent_max_tool_calls,
            turn_guard::MAX_MAIN_AGENT_TOOL_CALLS
        );
        assert_eq!(config.tool_failure_limit, turn_guard::MAX_TOOL_FAILURE_LIMIT);

        // `0` means "no limit" and must survive normalization.
        config.main_agent_max_rounds = 0;
        config.main_agent_max_tool_calls = 0;
        config.tool_failure_limit = 0;
        config.normalize_turn_limits();
        assert_eq!(config.main_agent_max_rounds, 0);
        assert_eq!(config.main_agent_max_tool_calls, 0);
        assert_eq!(config.tool_failure_limit, 0);
    }

    /// The workspace dropdown lists the workspaces that were really active:
    /// newest first, no duplicates (Windows paths differ only in case), capped.
    #[test]
    fn workspace_history_is_deduped_and_capped() {
        let mut config = AppConfig::default();

        config.remember_workspace("D:\\projects\\alpha");
        config.remember_workspace("D:\\projects\\beta");
        assert_eq!(
            config.workspace_history,
            vec!["D:\\projects\\beta", "D:\\projects\\alpha"]
        );

        // Re-activating an entry moves it to the front instead of duplicating
        // it, and the comparison ignores case.
        config.remember_workspace("d:\\PROJECTS\\alpha");
        assert_eq!(
            config.workspace_history,
            vec!["d:\\PROJECTS\\alpha", "D:\\projects\\beta"]
        );

        // Blank values (the "default workspace" form) are not history.
        config.remember_workspace("   ");
        assert_eq!(config.workspace_history.len(), 2);

        for index in 0..WORKSPACE_HISTORY_LIMIT + 5 {
            config.remember_workspace(&format!("C:\\tmp\\ws{index}"));
        }
        assert_eq!(config.workspace_history.len(), WORKSPACE_HISTORY_LIMIT);
        assert_eq!(
            config.workspace_history[0],
            format!("C:\\tmp\\ws{}", WORKSPACE_HISTORY_LIMIT + 4)
        );
    }

    /// The titlebar switcher marks the active row by comparing the recorded
    /// entry against the *resolved* workspace path.
    #[test]
    fn workspace_paths_compare_across_case_and_separators() {
        assert!(same_workspace_path("D:\\projects\\alpha", "d:/projects/alpha"));
        // A trailing separator is not a different workspace.
        assert!(same_workspace_path("/home/leonard/acme/", "/home/leonard/acme"));
        assert!(same_workspace_path("  /home/leonard/acme  ", "/home/leonard/acme"));
        assert!(!same_workspace_path("/home/leonard/acme", "/home/leonard/acme2"));
        assert!(!same_workspace_path("C:\\one", "C:\\two"));
    }
}
