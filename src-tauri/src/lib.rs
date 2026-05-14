use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use rusqlite::Connection;
mod search_db;
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
    pub db: Mutex<Connection>,
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
/// When web_search=true, includes a web_search tool and handles tool_calls automatically.
#[tauri::command]
async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_id: Option<String>,
    web_search: bool,
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

    let mut req_body = json!({
        "model": config.model,
        "messages": all_messages,
        "stream": true
    });

    if web_search {
        req_body["tools"] = json!([{
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for current information about a topic. Use this when the user asks about recent events, current data, or anything that might require up-to-date information.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query to look up"
                        }
                    },
                    "required": ["query"]
                }
            }
        }]);
        req_body["tool_choice"] = json!("auto");
    }

    let res = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&req_body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        let _ = app.emit("chat-error", err.clone());
        return Err(err);
    }

    // Stream first response — emit content tokens and detect tool calls
    let mut tool_call_id = String::new();
    let mut tool_call_args = String::new();
    let mut finish_reason = String::new();

    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);

        for line in text.lines() {
            let line = line.trim();
            if line == "data: [DONE]" {
                break;
            }
            let Some(json_str) = line.strip_prefix("data: ") else { continue };
            let Ok(parsed) = serde_json::from_str::<Value>(json_str) else { continue };
            let Some(choice) = parsed["choices"].get(0) else { continue };
            let delta = &choice["delta"];

            if let Some(fr) = choice["finish_reason"].as_str() {
                finish_reason = fr.to_string();
            }
            // Accumulate tool call id and arguments from streamed chunks
            if let Some(tcs) = delta["tool_calls"].as_array() {
                for tc in tcs {
                    if let Some(id) = tc["id"].as_str() {
                        tool_call_id = id.to_string();
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        tool_call_args.push_str(args);
                    }
                }
            }
            // Emit regular content tokens
            if let Some(content) = delta["content"].as_str() {
                let _ = app.emit("chat-token", content.to_string());
            }
        }
    }

    // If the model issued a tool call, execute the search and do a second streaming call
    if finish_reason == "tool_calls" && !tool_call_id.is_empty() {
        let query = serde_json::from_str::<Value>(&tool_call_args)
            .ok()
            .and_then(|v| v["query"].as_str().map(String::from))
            .unwrap_or_default();

        let _ = app.emit("chat-token", format!("🔍 *Searching: {}...*\n\n", query));

        let search_result = search_db::search_duckduckgo(query).await
            .unwrap_or_else(|e| format!("Search failed: {}", e));

        all_messages.push(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": tool_call_id,
                "type": "function",
                "function": { "name": "web_search", "arguments": tool_call_args }
            }]
        }));
        all_messages.push(json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": search_result
        }));

        let res2 = client
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

        if !res2.status().is_success() {
            let err = res2.text().await.unwrap_or_default();
            let _ = app.emit("chat-error", err.clone());
            return Err(err);
        }

        let mut stream2 = res2.bytes_stream();
        while let Some(chunk) = stream2.next().await {
            let bytes = chunk.map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                let line = line.trim();
                if line == "data: [DONE]" {
                    let _ = app.emit("chat-done", ());
                    return Ok(());
                }
                let Some(json_str) = line.strip_prefix("data: ") else { continue };
                let Ok(parsed) = serde_json::from_str::<Value>(json_str) else { continue };
                if let Some(content) = parsed["choices"].get(0)
                    .and_then(|c| c["delta"]["content"].as_str())
                {
                    let _ = app.emit("chat-token", content.to_string());
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
    fs::write(&state.config_path,
 json).map_err(|e| e.to_string())?;
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
                db: Mutex::new(db),
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
            search_db::search_duckduckgo,
            search_db::save_history,
            search_db::load_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
