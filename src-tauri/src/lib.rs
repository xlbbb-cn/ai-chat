use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use rusqlite::Connection;
use tauri::{menu::{Menu, MenuItem, Submenu}, AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_dialog::DialogExt;

mod db;
mod logger;
mod llm_complete;
pub mod mcp;
mod skills;
mod tools;
pub mod neo4j_db;

use logger::{AppLogger, LoggerOutput};

const OPEN_APP_DATA_DIR_MENU_ID: &str = "open-app-data-dir";
const SAVE_PROFILE_MENU_ID: &str = "save-profile";
const RESTORE_PROFILE_MENU_ID: &str = "restore-profile";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileData {
    version: u32,
    config: AppConfig,
    #[serde(default)]
    mcp_servers: Vec<mcp::McpServer>,
    #[serde(default)]
    skills: Vec<skills::Skill>,
}

fn resolve_workspace_path(app: &AppHandle, workspace_dir: Option<&str>) -> Result<PathBuf, String> {
    Ok(match workspace_dir.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => app.path().app_data_dir().map_err(|e| e.to_string())?.join("workspace"),
    })
}

fn apply_config(app: &AppHandle, state: &AppState, config: AppConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&state.config_path, json).map_err(|e| e.to_string())?;

    let new_workspace_path = resolve_workspace_path(app, config.workspace_dir.as_deref())?;
    fs::create_dir_all(&new_workspace_path).ok();
    *state.workspace_dir.lock().unwrap() = new_workspace_path.clone();

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

fn collect_local_skills(skills_dir: &PathBuf) -> Vec<skills::Skill> {
    fs::read_dir(skills_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let skill_path = e.path().join("skill.md");
                    let content = fs::read_to_string(skill_path).ok()?;
                    skills::parse_skill_md(&content).ok()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn export_profile(app: &AppHandle, profile_path: &PathBuf) -> Result<(), String> {
    let state = app.state::<AppState>();
    let profile = ProfileData {
        version: 1,
        config: state.config.lock().unwrap().clone(),
        mcp_servers: mcp::load_servers(&state.mcp_servers_path),
        skills: collect_local_skills(&state.skills_dir),
    };

    if let Some(parent) = profile_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let json = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?;
    fs::write(profile_path, json).map_err(|e| e.to_string())?;
    state
        .logger
        .lock()
        .unwrap()
        .log("INFO", &format!("Profile saved to {}", profile_path.display()));
    Ok(())
}

fn import_profile(app: &AppHandle, profile_path: &PathBuf) -> Result<(), String> {
    let state = app.state::<AppState>();
    let content = fs::read_to_string(profile_path).map_err(|e| e.to_string())?;
    let profile: ProfileData = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    apply_config(app, &state, profile.config)?;

    let mcp_file = serde_json::json!({ "servers": profile.mcp_servers });
    let mcp_json = serde_json::to_string_pretty(&mcp_file).map_err(|e| e.to_string())?;
    fs::write(&state.mcp_servers_path, mcp_json).map_err(|e| e.to_string())?;

    fs::create_dir_all(&state.skills_dir).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(&state.skills_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    for skill in &profile.skills {
        let md = skills::skill_to_md(skill)?;
        let skill_dir = state.skills_dir.join(&skill.name);
        fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
        fs::write(skill_dir.join("skill.md"), md).map_err(|e| e.to_string())?;
    }

    for server in mcp::load_servers(&state.mcp_servers_path).into_iter().filter(|s| s.enabled) {
        mcp::spawn_warmup(server);
    }

    let _ = app.emit("profile-restored", ());
    state
        .logger
        .lock()
        .unwrap()
        .log("INFO", &format!("Profile restored from {}", profile_path.display()));
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
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            temperature: None,
            top_p: None,
            reasoning_effort: String::new(),
            max_tokens: None,
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
    #[serde(default)]
    pub system_message: String,
    #[serde(default)]
    pub selected_tools: Vec<String>,
    #[serde(default)]
    pub selected_skills: Vec<String>,
    pub kg_engine: Option<String>,
    pub neo4j_uri: Option<String>,
    pub neo4j_user: Option<String>,
    pub neo4j_password: Option<String>,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    #[serde(default)]
    pub logger_output: LoggerOutput,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            model_catalog: vec!["gpt-4o-mini".to_string()],
            model_settings: ModelSettings::default(),
            system_message: String::new(),
            selected_tools: vec![],
            selected_skills: vec![],
            kg_engine: None,
            neo4j_uri: Some("bolt://localhost:7687".to_string()),
            neo4j_user: Some("neo4j".to_string()),
            neo4j_password: Some(String::new()),
            workspace_dir: None,
            logger_output: LoggerOutput::default(),
        }
    }
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    pub workspace_dir: Mutex<PathBuf>,
    pub skills_dir: PathBuf,
    pub mcp_servers_path: PathBuf,
    pub db: Mutex<Connection>,
    pub logger: Mutex<AppLogger>,
    pub chat_cancelled: AtomicBool,
    /// One-shot channel sender used to relay the user's confirm/deny response
    /// back to a waiting `execute_tool` call.
    pub confirm_sender: Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
}

// ─── Config commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_config(app: AppHandle, state: State<'_, AppState>, config: AppConfig) -> Result<(), String> {
    apply_config(&app, &state, config)
}

#[tauri::command]
async fn fetch_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let config = state.config.lock().unwrap().clone();
    let url = format!("{}/models", config.api_base_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .bearer_auth(config.api_key)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(res.text().await.unwrap_or_else(|_| "failed to fetch models".to_string()));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Invalid models response: missing data array".to_string())?;

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
        .collect();

    names.sort();
    names.dedup();
    Ok(names)
}

#[tauri::command]
fn stop_chat_completion(state: State<'_, AppState>) {
    state
        .chat_cancelled
        .store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Called by the frontend to confirm or deny a pending dangerous-command execution.
#[tauri::command]
fn confirm_command(state: State<'_, AppState>, confirmed: bool) {
    let mut guard = state.confirm_sender.lock().unwrap();
    if let Some(tx) = guard.take() {
        let _ = tx.send(confirmed);
    }
}

#[tauri::command]
fn get_workspace_dir(state: State<'_, AppState>) -> String {
    state.workspace_dir.lock().unwrap().to_string_lossy().to_string()
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .on_menu_event(|app, event| {
            if event.id() == OPEN_APP_DATA_DIR_MENU_ID {
                if let Ok(data_dir) = app.path().app_data_dir() {
                    let _ = app.opener().open_path(data_dir.to_string_lossy().into_owned(), None::<&str>);
                }
            } else if event.id() == SAVE_PROFILE_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Profile", &["json"])
                    .set_file_name("ai-chat.profile.json")
                    .blocking_save_file()
                {
                    let profile_path = path.into_path().map_err(|_| "unsupported save path").ok();
                    if let Some(profile_path) = profile_path {
                        if let Err(err) = export_profile(app, &profile_path) {
                            app.state::<AppState>()
                                .logger
                                .lock()
                                .unwrap()
                                .log("ERROR", &format!("Save profile failed: {err}"));
                        }
                    }
                }
            } else if event.id() == RESTORE_PROFILE_MENU_ID {
                if let Some(path) = app
                    .dialog()
                    .file()
                    .add_filter("Profile", &["json"])
                    .blocking_pick_file()
                {
                    let profile_path = path.into_path().map_err(|_| "unsupported profile path").ok();
                    if let Some(profile_path) = profile_path {
                        if let Err(err) = import_profile(app, &profile_path) {
                            app.state::<AppState>()
                                .logger
                                .lock()
                                .unwrap()
                                .log("ERROR", &format!("Restore profile failed: {err}"));
                        }
                    }
                }
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
            let file_menu = Submenu::with_items(
                app,
                "File",
                true,
                &[&save_profile_item, &restore_profile_item, &open_app_data_dir_item],
            )
                .expect("failed to create app menu");
            let menu = Menu::with_items(app, &[&file_menu]).expect("failed to create app menu");
            app.set_menu(menu).expect("failed to set app menu");
            
            let skills_dir = data_dir.join("skills");
            fs::create_dir_all(&skills_dir).ok();

            let mcp_servers_path = data_dir.join("mcp_servers.json");

            let db_path = data_dir.join("chat.db");
            let db = Connection::open(db_path).unwrap();
            db.execute("CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp DATETIME DEFAULT CURRENT_TIMESTAMP)", []).unwrap();
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

            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                workspace_dir: Mutex::new(workspace_dir.clone()),
                db: Mutex::new(db),
                logger: Mutex::new(app_logger),
                skills_dir,
                mcp_servers_path: mcp_servers_path.clone(),
                chat_cancelled: AtomicBool::new(false),
                confirm_sender: Mutex::new(None),
            });

            // Warm up enabled MCP servers as soon as the app starts.
            for server in mcp::load_servers(&mcp_servers_path).into_iter().filter(|s| s.enabled) {
                mcp::spawn_warmup(server);
            }

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
            skills::list_skills,
            skills::save_skill,
            skills::delete_skill,
            db::save_history,
            db::load_history,
            db::delete_history,
            db::list_api_requests,
            db::get_api_request,
            db::delete_api_request,
            db::clear_api_requests,
            mcp::list_mcp_servers,
            mcp::save_mcp_server,
            mcp::delete_mcp_server,
            mcp::test_mcp_server,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
