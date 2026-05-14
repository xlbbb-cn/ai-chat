use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use dirs::home_dir;

// ─── App State ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
        }
    }
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub config_path: PathBuf,
    pub skills_dir: PathBuf,
}

// ─── Skill ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
}

// ─── Chat ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Stream a chat completion from any OpenAI-compatible endpoint.
/// Emits "chat-token" events with each chunk, then "chat-done" or "chat-error".
#[tauri::command]
async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let config = state.config.lock().unwrap().clone();

    // Build the message list, prepending the skill's system prompt if active
    let mut all_messages: Vec<Value> = vec![];
    if let Some(ref sid) = skill_id {
        if let Ok(skill) = load_skill_by_id(&state.skills_dir, sid) {
            all_messages.push(json!({ "role": "system", "content": skill.system_prompt }));
        }
    }
    for m in &messages {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!("{}/chat/completions", config.api_base_url.trim_end_matches('/'));
    let client = Client::new();

    let res = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&json!({
            "model": config.model,
            "messages": all_messages,
            "stream": true
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        let _ = app.emit("chat-error", err.clone());
        return Err(err);
    }

    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);

        for line in text.lines() {
            let line = line.trim();
            if line == "data: [DONE]" {
                let _ = app.emit("chat-done", ());
                return Ok(());
            }
            if let Some(json_str) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
                    if let Some(delta) = parsed
                        .get("choices")
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("delta"))
                        .and_then(|d| d.get("content"))
                        .and_then(|v| v.as_str())
                    {
                        let _ = app.emit("chat-token", delta.to_string());
                    }
                }
            }
        }
    }

    let _ = app.emit("chat-done", ());
    Ok(())
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

// ─── Skills commands ──────────────────────────────────────────────────────────

fn load_skill_by_id(skills_dir: &PathBuf, id: &str) -> Result<Skill, String> {
    let path = skills_dir.join(format!("{}.json", id));
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_skills(state: State<'_, AppState>) -> Vec<Skill> {
    let mut skills = Vec::new();

    // Load skills from the app's skills directory
    let app_skills = fs::read_dir(&state.skills_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|e| {
                    let content = fs::read_to_string(e.path()).ok()?;
                    serde_json::from_str::<Skill>(&content).ok()
                })
                .collect::<Vec<Skill>>()
        })
        .unwrap_or_default();
    skills.extend(app_skills);

    // Load skills from the user's ~/.skills directory
    if let Some(home_dir) = home_dir() {
        let user_skills_dir = home_dir.join(".skills");
        let user_skills = fs::read_dir(&user_skills_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                    .filter_map(|e| {
                        let content = fs::read_to_string(e.path()).ok()?;
                        serde_json::from_str::<Skill>(&content).ok()
                    })
                    .collect::<Vec<Skill>>()
            })
            .unwrap_or_default();
        skills.extend(user_skills);
    }

    skills
}

#[tauri::command]
fn save_skill(state: State<'_, AppState>, skill: Skill) -> Result<(), String> {
    let path = state.skills_dir.join(format!("{}.json", skill.id));
    let json = serde_json::to_string_pretty(&skill).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_skill(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let path = state.skills_dir.join(format!("{}.json", id));
    fs::remove_file(path).map_err(|e| e.to_string())
}

// ─── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");
            fs::create_dir_all(&data_dir).ok();

            let skills_dir = data_dir.join("skills");
            fs::create_dir_all(&skills_dir).ok();

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
                skills_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            chat_completion,
            get_config,
            save_config,
            list_skills,
            save_skill,
            delete_skill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
