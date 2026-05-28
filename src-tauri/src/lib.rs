use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem, Submenu},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;
use zip::{write::FileOptions, ZipArchive, ZipWriter};

pub mod agents;
mod db;
mod llm_complete;
mod logger;
pub mod mcp;
pub mod neo4j_db;
mod skills;
mod tools;

use logger::{AppLogger, LoggerOutput};

const OPEN_APP_DATA_DIR_MENU_ID: &str = "open-app-data-dir";
const SAVE_PROFILE_MENU_ID: &str = "save-profile";
const RESTORE_PROFILE_MENU_ID: &str = "restore-profile";
const MARKDOWN_EDIT_MENU_ID: &str = "markdown-edit";
const ABOUT_MENU_ID: &str = "about";
const PROFILE_EXPORT_START_EVENT: &str = "profile-export-start";
const PROFILE_EXPORT_STATUS_EVENT: &str = "profile-export-status";
const PROFILE_EXPORT_DONE_EVENT: &str = "profile-export-done";
const PROFILE_EXPORT_ERROR_EVENT: &str = "profile-export-error";
const MARKDOWN_EDIT_OPEN_EVENT: &str = "markdown-edit-open";
const MARKDOWN_EDIT_ERROR_EVENT: &str = "markdown-edit-error";

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

fn add_path_to_zip(
    zip: &mut ZipWriter<fs::File>,
    base_dir: &Path,
    path: &Path,
) -> Result<(), String> {
    if path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            add_path_to_zip(zip, base_dir, &entry.path())?;
        }
        return Ok(());
    }

    if path.is_file() {
        let relative_path = path.strip_prefix(base_dir).map_err(|e| e.to_string())?;
        let entry_name = format!(
            "skills/{}",
            relative_path.to_string_lossy().replace('\\', "/")
        );
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(entry_name, options)
            .map_err(|e| e.to_string())?;

        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, zip).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn emit_profile_export_status(app: &AppHandle, status: &str) {
    let _ = app.emit(PROFILE_EXPORT_STATUS_EVENT, status.to_string());
}

fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}

fn backup_chat_db(state: &AppState) -> Result<PathBuf, String> {
    let backup_path = state
        .db_path
        .with_file_name(format!("chat-export-{}.db", Uuid::new_v4()));
    let escaped_path = escape_sql_string(&backup_path.to_string_lossy());

    let db = state.db.lock().unwrap();
    db.execute_batch(&format!("VACUUM INTO '{}';", escaped_path))
        .map_err(|e| e.to_string())?;

    Ok(backup_path)
}

fn apply_config(app: &AppHandle, state: &AppState, config: AppConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&state.config_path, json).map_err(|e| e.to_string())?;

    let new_workspace_path = resolve_workspace_path(app, config.workspace_dir.as_deref())?;
    fs::create_dir_all(&new_workspace_path).ok();
    fs::create_dir_all(new_workspace_path.join("skills")).ok();
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

fn export_profile(app: &AppHandle, profile_path: &PathBuf) -> Result<(), String> {
    let state = app.state::<AppState>();
    if let Some(parent) = profile_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let file = fs::File::create(profile_path).map_err(|e| e.to_string())?;
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    emit_profile_export_status(app, "Writing config.json");
    let config_json = serde_json::to_string_pretty(&state.config.lock().unwrap().clone())
        .map_err(|e| e.to_string())?;
    zip.start_file("config.json", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(config_json.as_bytes())
        .map_err(|e| e.to_string())?;

    emit_profile_export_status(app, "Writing mcp_servers.json");
    let mcp_json = serde_json::json!({ "servers": mcp::load_servers(&state.mcp_servers_path) });
    let mcp_json = serde_json::to_string_pretty(&mcp_json).map_err(|e| e.to_string())?;
    zip.start_file("mcp_servers.json", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(mcp_json.as_bytes())
        .map_err(|e| e.to_string())?;

    emit_profile_export_status(app, "Writing sub_agents.json");
    let sub_agents_json = if state.agents_config_path.exists() {
        fs::read_to_string(&state.agents_config_path).map_err(|e| e.to_string())?
    } else {
        serde_json::to_string_pretty(&agents::AgentsConfig::default()).map_err(|e| e.to_string())?
    };
    zip.start_file("sub_agents.json", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(sub_agents_json.as_bytes())
        .map_err(|e| e.to_string())?;

    emit_profile_export_status(app, "Packing skills directory");
    zip.add_directory("skills/", options)
        .map_err(|e| e.to_string())?;
    add_path_to_zip(&mut zip, &state.skills_dir, &state.skills_dir)?;

    emit_profile_export_status(app, "Exporting chat.db (sending disabled during export)");
    let chat_db_backup = backup_chat_db(&state)?;
    let db_result = (|| -> Result<(), String> {
        zip.start_file("chat.db", options)
            .map_err(|e| e.to_string())?;
        let mut db_file = fs::File::open(&chat_db_backup).map_err(|e| e.to_string())?;
        std::io::copy(&mut db_file, &mut zip).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(&chat_db_backup);
    db_result?;

    zip.finish().map_err(|e| e.to_string())?;
    state.logger.lock().unwrap().log(
        "INFO",
        &format!("Profile saved to {}", profile_path.display()),
    );
    Ok(())
}

fn spawn_profile_export(app: AppHandle, profile_path: PathBuf) {
    std::thread::spawn(move || {
        let _ = app.emit(PROFILE_EXPORT_START_EVENT, ());
        emit_profile_export_status(&app, "Preparing to export profile...");

        match export_profile(&app, &profile_path) {
            Ok(()) => {
                let _ = app.emit(PROFILE_EXPORT_DONE_EVENT, ());
            }
            Err(err) => {
                let _ = app.emit(PROFILE_EXPORT_ERROR_EVENT, err);
            }
        }
    });
}

fn import_profile(app: &AppHandle, profile_path: &PathBuf) -> Result<(), String> {
    let state = app.state::<AppState>();
    let file = fs::File::open(profile_path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;

    let config: AppConfig = {
        let mut config_file = archive
            .by_name("config.json")
            .map_err(|_| "missing config.json in profile archive".to_string())?;
        let mut config_json = String::new();
        config_file
            .read_to_string(&mut config_json)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&config_json).map_err(|e| e.to_string())?
    };

    apply_config(app, &state, config)?;

    if let Ok(mut agents_file) = archive.by_name("sub_agents.json") {
        let mut sub_agents_json = String::new();
        agents_file
            .read_to_string(&mut sub_agents_json)
            .map_err(|e| e.to_string())?;
        fs::write(&state.agents_config_path, sub_agents_json).map_err(|e| e.to_string())?;
    }

    let _ = app.emit("profile-restored", ());
    state.logger.lock().unwrap().log(
        "INFO",
        &format!("Profile restored from {}", profile_path.display()),
    );
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
            self_evolution_mode: false,
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
    pub db_path: PathBuf,
    pub skills_dir: PathBuf,
    pub mcp_servers_path: PathBuf,
    pub agents_config_path: PathBuf,
    pub db: Mutex<Connection>,
    pub logger: Mutex<AppLogger>,
    pub chat_cancelled: AtomicBool,
    /// One-shot channel sender used to relay the user's confirm/deny response
    /// back to a waiting `execute_tool` call.
    pub confirm_sender: Mutex<Option<tokio::sync::oneshot::Sender<ToolConfirmation>>>,
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
        return Err(res
            .text()
            .await
            .unwrap_or_else(|_| "failed to fetch models".to_string()));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Invalid models response: missing data array".to_string())?;

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| {
            m.get("id")
                .and_then(|id| id.as_str())
                .map(|s| s.to_string())
        })
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
fn confirm_command(
    state: State<'_, AppState>,
    confirmed: bool,
    username: Option<String>,
    password: Option<String>,
) {
    let mut guard = state.confirm_sender.lock().unwrap();
    if let Some(tx) = guard.take() {
        let _ = tx.send(ToolConfirmation {
            confirmed,
            username,
            password,
        });
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
fn get_skill_roots(state: State<'_, AppState>) -> Vec<String> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let mut paths: Vec<String> =
        skills::collect_self_evolution_roots(&state.skills_dir, Some(&workspace_dir), true)
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
    paths.push(state.agents_config_path.to_string_lossy().to_string());
    paths
}

#[tauri::command]
fn save_markdown_file(path: String, content: String) -> Result<(), String> {
    fs::write(PathBuf::from(path), content).map_err(|e| e.to_string())
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
                    .add_filter("Profile", &["zip"])
                    .set_file_name("ai-chat.profile.zip")
                    .blocking_save_file()
                {
                    let profile_path = path.into_path().map_err(|_| "unsupported save path").ok();
                    if let Some(profile_path) = profile_path {
                        spawn_profile_export(app.clone(), profile_path);
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
                        if let Err(err) = import_profile(app, &profile_path) {
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
            let menu = Menu::with_items(app, &[&file_menu, &tools_menu, &about_menu])
                .expect("failed to create app menu");
            app.set_menu(menu).expect("failed to set app menu");

            let skills_dir = data_dir.join("skills");
            fs::create_dir_all(&skills_dir).ok();

            let mcp_servers_path = data_dir.join("mcp_servers.json");
            let agents_config_path = data_dir.join("sub_agents.json");

            let db_path = data_dir.join("chat.db");
            let db = Connection::open(&db_path).unwrap();
            db.execute("CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp DATETIME DEFAULT CURRENT_TIMESTAMP)", []).unwrap();
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
            fs::create_dir_all(workspace_dir.join("skills")).ok();

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
            get_skill_roots,
            save_markdown_file,
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
            db::list_interactions,
            db::get_interaction,
            db::clear_interactions,
            mcp::list_mcp_servers,
            mcp::save_mcp_server,
            mcp::delete_mcp_server,
            mcp::test_mcp_server,
            agents::list_sub_agents,
            agents::save_sub_agent,
            agents::delete_sub_agent,
            agents::get_agent_orchestration,
            agents::save_agent_orchestration,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
