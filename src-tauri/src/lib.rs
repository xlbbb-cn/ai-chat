use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use rusqlite::Connection;
use tauri::{menu::{Menu, MenuItem, Submenu}, AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

mod db;
mod llm_complete;
pub mod mcp;
mod skills;
mod tools;
pub mod neo4j_db;

const OPEN_APP_DATA_DIR_MENU_ID: &str = "open-app-data-dir";

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
    pub kg_engine: Option<String>,
    pub neo4j_uri: Option<String>,
    pub neo4j_user: Option<String>,
    pub neo4j_password: Option<String>,
    #[serde(default)]
    pub workspace_dir: Option<String>,
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
            kg_engine: None,
            neo4j_uri: Some("bolt://localhost:7687".to_string()),
            neo4j_user: Some("neo4j".to_string()),
            neo4j_password: Some(String::new()),
            workspace_dir: None,
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
    pub chat_cancelled: AtomicBool,
}

// ─── Config commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_config(app: AppHandle, state: State<'_, AppState>, config: AppConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&state.config_path, json).map_err(|e| e.to_string())?;

    // Update workspace_dir in state and refresh window title
    let new_workspace_path = match config.workspace_dir.as_deref().filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => app.path().app_data_dir().map_err(|e| e.to_string())?.join("workspace"),
    };
    fs::create_dir_all(&new_workspace_path).ok();
    *state.workspace_dir.lock().unwrap() = new_workspace_path.clone();
    if let Some(win) = app.get_webview_window("main") {
        let title = format!("AI Chat — {}", new_workspace_path.display());
        let _ = win.set_title(&title);
    }

    *state.config.lock().unwrap() = config;
    Ok(())
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

#[tauri::command]
fn get_workspace_dir(state: State<'_, AppState>) -> String {
    state.workspace_dir.lock().unwrap().to_string_lossy().to_string()
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .on_menu_event(|app, event| {
            if event.id() == OPEN_APP_DATA_DIR_MENU_ID {
                if let Ok(data_dir) = app.path().app_data_dir() {
                    let _ = app.opener().open_path(data_dir.to_string_lossy().into_owned(), None::<&str>);
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
            let file_menu = Submenu::with_items(app, "File", true, &[&open_app_data_dir_item])
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
                skills_dir,
                mcp_servers_path: mcp_servers_path.clone(),
                chat_cancelled: AtomicBool::new(false),
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
