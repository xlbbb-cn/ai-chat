use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use rusqlite::Connection;
use tauri::{menu::{Menu, MenuItem, Submenu}, Manager, State};
use tauri_plugin_opener::OpenerExt;

mod db;
mod llm_complete;
mod search;
mod skills;
mod tools;

const OPEN_APP_DATA_DIR_MENU_ID: &str = "open-app-data-dir";

// ─── App State ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub reasoning_effort: String,
    #[serde(default)]
    pub system_message: String,
    #[serde(default)]
    pub selected_tools: Vec<String>,
    #[serde(default = "default_search_engine")]
    pub search_engine: String,
}

fn default_search_engine() -> String {
    "duckduckgo".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            temperature: None,
            reasoning_effort: String::new(),
            system_message: String::new(),
            selected_tools: vec!["web_search".to_string()],
            search_engine: default_search_engine(),
        }
    }
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    pub workspace_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub db: Mutex<Connection>,
}

// ─── Config commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_config(state: State<'_, AppState>, config: AppConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&state.config_path, json).map_err(|e| e.to_string())?;
    *state.config.lock().unwrap() = config;
    Ok(())
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
            let workspace_dir = data_dir.join("workspace");
            fs::create_dir_all(&workspace_dir).ok();
            
            let skills_dir = data_dir.join("skills");
            fs::create_dir_all(&skills_dir).ok();

            let db_path = data_dir.join("chat.db");
            let db = Connection::open(db_path).unwrap();
            db.execute("CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp DATETIME DEFAULT CURRENT_TIMESTAMP)", []).unwrap();

            let config_path = data_dir.join("config.json");
            let config = if config_path.exists() {
                fs::read_to_string(&config_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default()
            } else {
                AppConfig::default()
            };

            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                workspace_dir,
                db: Mutex::new(db),
                skills_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            llm_complete::chat_completion,
            get_config,
            save_config,
            skills::list_skills,
            skills::save_skill,
            skills::delete_skill,
            search::search_by,
            db::save_history,
            db::load_history,
            db::delete_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
