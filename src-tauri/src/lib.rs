use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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
pub mod neo4j_db;
mod skills;
mod todos;
mod tools;
mod update;

use logger::{AppLogger, LoggerOutput};

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

pub(crate) fn apply_config(
    app: &AppHandle,
    state: &AppState,
    config: AppConfig,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&state.config_path, json).map_err(|e| e.to_string())?;

    let new_workspace_path = resolve_workspace_path(app, config.workspace_dir.as_deref())?;
    fs::create_dir_all(&new_workspace_path).ok();
    // Note: workspace/skills is NOT created eagerly. Skills are discovered by
    // scanning; when the folder is absent only the app-managed skills root is used.
    let previous_workspace_path = std::mem::replace(
        &mut *state.workspace_dir.lock().unwrap(),
        new_workspace_path.clone(),
    );

    // The active skill list is workspace-scoped: skills that came from the old
    // workspace may not exist in the new one. Notify the UI so it can re-validate
    // `selected_skills` against the skills that actually resolve now.
    if previous_workspace_path != new_workspace_path {
        let _ = app.emit(
            WORKSPACE_CHANGED_EVENT,
            new_workspace_path.to_string_lossy().to_string(),
        );
    }

    if let Some(win) = app.get_webview_window("main") {
        let title = format!("AI Chat — {}", new_workspace_path.display());
        let _ = win.set_title(&title);
    }

    let logger_output = config.logger_output.clone();
    *state.config.lock().unwrap() = config;

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
    /// Per-model context window (tokens). Auto-filled from `/models` when the
    /// provider reports one; otherwise set manually in Settings as a fallback.
    #[serde(default)]
    pub model_context_lengths: HashMap<String, u32>,
    #[serde(default)]
    pub system_message: String,
    #[serde(default)]
    pub selected_tools: Vec<String>,
    #[serde(default)]
    pub selected_skills: Vec<String>,
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
    /// Check the GitHub Releases API for a newer version when the app starts.
    #[serde(default = "update::default_true")]
    pub check_updates_on_startup: bool,
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
            model_context_lengths: HashMap::new(),
            system_message: String::new(),
            selected_tools: vec![],
            selected_skills: vec![],
            self_evolution_mode: false,
            kg_engine: None,
            neo4j_uri: Some("bolt://localhost:7687".to_string()),
            neo4j_user: Some("neo4j".to_string()),
            neo4j_password: Some(String::new()),
            workspace_dir: None,
            logger_output: LoggerOutput::default(),
            auto_accept_confirm_kinds: vec![],
            theme: None,
            check_updates_on_startup: true,
            include_prerelease_updates: false,
        }
    }
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    pub workspace_dir: Mutex<PathBuf>,
    pub db_path: PathBuf,
    // App-managed skills root under app data. Workspace-local skills live under workspace_dir/skills.
    pub skills_dir: PathBuf,
    pub mcp_servers_path: PathBuf,
    pub agents_config_path: PathBuf,
    pub profiles_path: PathBuf,
    pub db: Mutex<Connection>,
    pub logger: Mutex<AppLogger>,
    pub chat_cancelled: AtomicBool,
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
    /// Session-scoped memories: session_id -> key -> JSON entry.
    pub session_memories: Mutex<
        std::collections::HashMap<String, std::collections::HashMap<String, serde_json::Value>>,
    >,
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
    state
        .chat_cancelled
        .store(true, std::sync::atomic::Ordering::SeqCst);
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
    pub selected_skills: Vec<String>,
    pub selected_tools: Vec<String>,
    pub agents: Vec<agents::SubAgent>,
    pub orchestration: agents::AgentOrchestration,
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

    // Update config with profile's selected_skills and selected_tools
    let mut config = state.config.lock().unwrap().clone();
    config.selected_skills = profile.selected_skills;
    config.selected_tools = profile.selected_tools;

    apply_config(&app, &state, config)?;

    // Save agents config
    let agents_config = agents::AgentsConfig {
        agents: profile.agents,
        orchestration: profile.orchestration,
    };
    let agents_json = serde_json::to_string_pretty(&agents_config).map_err(|e| e.to_string())?;
    fs::write(&state.agents_config_path, agents_json).map_err(|e| e.to_string())?;

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
                if let Ok(data_dir) = app.path().app_data_dir() {
                    let _ = app.opener().open_path(data_dir.to_string_lossy().into_owned(), None::<&str>);
                }
            } else if event.id() == SAVE_PROFILE_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Profile", &["zip"])
                    .set_file_name("ai-chat.profile.zip")
                    .blocking_save_file()
                {
                    let profile_path = path.into_path().map_err(|_| "unsupported save path").ok();
                    if let Some(profile_path) = profile_path {
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
                    let profile_path = path.into_path().map_err(|_| "unsupported profile path").ok();
                    if let Some(profile_path) = profile_path {
                        if let Err(err) = backup::import_profile(app, &profile_path) {
                            app.state::<AppState>()
                                .logger
                                .lock()
                                .unwrap()
                                .log("ERROR", &format!("Restore profile failed: {err}"));
                        }
                    }
                }
            } else if event.id() == MARKDOWN_EDIT_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"]) 
                    .blocking_pick_file()
                {
                    let profile_path = path.into_path().map_err(|_| "unsupported markdown path").ok();
                    if let Some(markdown_path) = profile_path {
                        match fs::read_to_string(&markdown_path) {
                            Ok(content) => {
                                let payload = MarkdownEditPayload {
                                    path: markdown_path.to_string_lossy().to_string(),
                                    content,
                                };
                                let _ = app.emit(MARKDOWN_EDIT_OPEN_EVENT, payload);
                            }
                            Err(err) => {
                                let msg = format!("Failed to open markdown file: {err}");
                                app.state::<AppState>().logger.lock().unwrap().log("ERROR", &msg);
                                let _ = app.emit(MARKDOWN_EDIT_ERROR_EVENT, msg);
                            }
                        }
                    }
                }
            } else if event.id() == ABOUT_MENU_ID {
                app.dialog()
                    .message(format!(
                        "About AI Chat\n\nAI Chat\nOpenAI-compatible desktop assistant\nVersion {}",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .buttons(MessageDialogButtons::Ok)
                    .show(|_| {});
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

            let skills_dir = data_dir.join("skills");
            fs::create_dir_all(&skills_dir).ok();

            let mcp_servers_path = data_dir.join("mcp_servers.json");
            let agents_config_path = data_dir.join("sub_agents.json");
            let profiles_path = data_dir.join("profiles.json");

            let db_path = data_dir.join("chat.db");
            let db = Connection::open(&db_path).unwrap();
            db.execute("CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp DATETIME DEFAULT CURRENT_TIMESTAMP)", []).unwrap();
            let _ = db.execute("ALTER TABLE history ADD COLUMN tool_calls TEXT", []);
            let _ = db.execute("ALTER TABLE history ADD COLUMN reasoning_content TEXT", []);
            // Attachments column: JSON-serialized `Attachment[]` for user
            // messages. The column is added lazily via ALTER TABLE so older
            // databases (without the column) upgrade in place; new rows
            // always include it.
            let _ = db.execute("ALTER TABLE history ADD COLUMN attachments TEXT", []);
            let _ = db.execute("ALTER TABLE api_requests ADD COLUMN reasoning_content TEXT", []);
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
            ).unwrap();
            db.execute(
                "CREATE TABLE IF NOT EXISTS session_summaries (\
                    session_id TEXT PRIMARY KEY, \
                    summary TEXT NOT NULL, \
                    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP\
                )",
                [],
            ).unwrap();
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
                    duration_ms INTEGER DEFAULT 0, \
                    error TEXT\
                )",
                [],
            ).unwrap();
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
            ).unwrap();
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
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP, \
                    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP\
                )",
                [],
            ).unwrap();
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
            ).unwrap();
            db.execute(
                "CREATE INDEX IF NOT EXISTS idx_agent_tasks_mission_status \
                 ON agent_tasks (mission_id, status)",
                [],
            ).unwrap();

            let config_path = data_dir.join("config.json");
            let config = if config_path.exists() {
                fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default()
            } else {
                AppConfig::default()
            };

            let app_logger = AppLogger::new(
                cfg!(debug_assertions),
                config.logger_output.clone(),
                data_dir.join("app.log"),
            );
            app_logger.log("INFO", "Logger initialized");

            // Resolve workspace_dir from config, or fall back to default
            let workspace_dir = match config.workspace_dir.as_deref().filter(|s| !s.is_empty()) {
                Some(dir) => PathBuf::from(dir),
                None => data_dir.join("workspace"),
            };
            fs::create_dir_all(&workspace_dir).ok();
            // workspace/skills is created on demand (e.g. by self-evolution or the
            // user); skill listing tolerates its absence.

            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                workspace_dir: Mutex::new(workspace_dir.clone()),
                db: Mutex::new(db),
                db_path,
                logger: Mutex::new(app_logger),
                skills_dir,
                mcp_servers_path: mcp_servers_path.clone(),
                agents_config_path,
                profiles_path,
                chat_cancelled: AtomicBool::new(false),
                confirm_sender: Mutex::new(None),
                mcp_logs: std::sync::Arc::new(Mutex::new(Default::default())),
                mcp_cancelled_tests: std::sync::Arc::new(Mutex::new(Default::default())),
                session_memories: Mutex::new(Default::default()),
            });

            // Set window title to show current workspace directory
            if let Some(win) = app.get_webview_window("main") {
                let title = format!("AI Chat — {}", workspace_dir.display());
                let _ = win.set_title(&title);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            llm_complete::chat_completion,
            stop_chat_completion,
            confirm_command,
            get_config,
            save_config,
            fetch_models,
            get_workspace_dir,
            save_markdown_file,
            skills::list_skills,
            skills::save_skill,
            skills::delete_skill,
            skills::filter_existing_skills,
            db::save_history,
            db::load_history,
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
            db::list_interactions,
            db::get_interaction,
            db::clear_interactions,
            mcp::list_mcp_servers,
            mcp::save_mcp_server,
            mcp::delete_mcp_server,
            mcp::test_mcp_server,
            mcp::cancel_mcp_test,
            mcp::get_mcp_logs,
            mcp::clear_mcp_logs,
            agents::list_sub_agents,
            agents::save_sub_agent,
            agents::delete_sub_agent,
            agents::get_agent_orchestration,
            agents::save_agent_orchestration,
            agents::list_agent_missions,
            todos::get_session_todo,
            todos::get_todo_list,
            todos::create_session_todo,
            todos::update_todo_status_cmd,
            todos::update_todo_text_cmd,
            todos::delete_todo_cmd,
            todos::clear_completed_todos,
            todos::archive_todo_list,
            todos::create_todo_list,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
