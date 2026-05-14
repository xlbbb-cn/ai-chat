use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use rusqlite::Connection;
mod search_db;
mod tools;
use tauri::{AppHandle, Emitter, Manager, State};
use dirs::home_dir;

// ─── App State ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub enable_thinking: bool,
    #[serde(default)]
    pub reasoning_effort: String,
    #[serde(default)]
    pub system_message: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            temperature: None,
            enable_thinking: false,
            reasoning_effort: String::new(),
            system_message: String::new(),
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

/// YAML frontmatter parsed from a SKILL.md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillMeta {
    name: String,
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(rename = "allowed-tools", default, skip_serializing_if = "Vec::is_empty")]
    allowed_tools: Vec<String>,
}

/// Full skill representation passed to/from the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    pub system_prompt: String,
}

fn parse_skill_md(content: &str) -> Result<Skill, String> {
    let content = content.trim_start_matches('\u{feff}');
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or("missing YAML frontmatter")?;
    let end = rest.find("\n---").ok_or("unclosed frontmatter")?;
    let frontmatter = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').trim_start_matches('\r').trim();
    let meta: SkillMeta = serde_yaml::from_str(frontmatter).map_err(|e| e.to_string())?;
    Ok(Skill {
        name: meta.name,
        description: meta.description,
        version: meta.version,
        author: meta.author,
        allowed_tools: meta.allowed_tools,
        system_prompt: body.to_string(),
    })
}

fn skill_to_md(skill: &Skill) -> Result<String, String> {
    let meta = SkillMeta {
        name: skill.name.clone(),
        description: skill.description.clone(),
        version: skill.version.clone(),
        author: skill.author.clone(),
        allowed_tools: skill.allowed_tools.clone(),
    };
    let frontmatter = serde_yaml::to_string(&meta).map_err(|e| e.to_string())?;
    Ok(format!("---\n{}---\n\n{}", frontmatter, skill.system_prompt))
}

// ─── Chat ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// ─── Tool helpers ─────────────────────────────────────────────────────────────

/// Stream one completion request. Emits "chat-token" for content chunks.
/// Returns (finish_reason, tool_calls: vec of (id, name, accumulated_args)).
async fn stream_request(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    req_body: Value,
) -> Result<(String, Vec<(String, String, String)>), String> {
    let res = client
        .post(url)
        .bearer_auth(api_key)
        .json(&req_body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        let _ = app.emit("chat-error", err.clone());
        return Err(err);
    }

    let mut finish_reason = String::new();
    // Each entry: (id, name, accumulated_args)
    let mut tool_calls: Vec<(String, String, String)> = Vec::new();

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
                if !fr.is_empty() {
                    finish_reason = fr.to_string();
                }
            }

            // Accumulate streamed tool call chunks by index
            if let Some(tcs) = delta["tool_calls"].as_array() {
                for tc in tcs {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while tool_calls.len() <= idx {
                        tool_calls.push((String::new(), String::new(), String::new()));
                    }
                    if let Some(id) = tc["id"].as_str() {
                        tool_calls[idx].0 = id.to_string();
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        tool_calls[idx].1 = name.to_string();
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        tool_calls[idx].2.push_str(args);
                    }
                }
            }

            // Emit regular content tokens
            if let Some(content) = delta["content"].as_str() {
                let _ = app.emit("chat-token", content.to_string());
            }
        }
    }

    Ok((finish_reason, tool_calls))
}

// ─── Chat command ─────────────────────────────────────────────────────────────

/// Stream a chat completion from any OpenAI-compatible endpoint.
/// Emits "chat-token" events with each chunk, then "chat-done" or "chat-error".
/// Supports tool calling: web_search (when web_search=true) and
/// execute_command (when the active skill has "Bash" in allowed-tools).
#[tauri::command]
async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_id: Option<String>,
    web_search: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let config = state.config.lock().unwrap().clone();

    let mut allow_commands = false;
    let mut all_messages: Vec<Value> = vec![];
    let mut skill_dir_path: Option<PathBuf> = None;

    // Global system message from config (base context)
    if !config.system_message.is_empty() {
        all_messages.push(json!({ "role": "system", "content": config.system_message }));
    }

    if let Some(ref skill_name) = skill_id {
        if let Ok(skill) = load_skill_by_name(&state.skills_dir, skill_name) {
            all_messages.push(json!({ "role": "system", "content": skill.system_prompt }));
            allow_commands = skill.allowed_tools.iter().any(|t| t.eq_ignore_ascii_case("bash"));
            skill_dir_path = Some(state.skills_dir.join(skill_name));
        } else if let Some(home_dir) = dirs::home_dir() {
            let user_skills_dir = home_dir.join(".skills");
            if let Ok(skill) = load_skill_by_name(&user_skills_dir, skill_name) {
                all_messages.push(json!({ "role": "system", "content": skill.system_prompt }));
                allow_commands = skill.allowed_tools.iter().any(|t| t.eq_ignore_ascii_case("bash"));
                skill_dir_path = Some(user_skills_dir.join(skill_name));
            }
        }
    }
    for m in &messages {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!("{}/chat/completions", config.api_base_url.trim_end_matches('/'));
    let client = Client::new();

    // Build tools list from enabled capabilities
    let tools = tools::get_all_tools(web_search, allow_commands, skill_dir_path.as_deref());

    // Tool calling loop — repeat until the model stops calling tools
    loop {
        let mut req_body = json!({
            "model": config.model,
            "messages": all_messages,
            "stream": true
        });
        if !tools.is_empty() {
            req_body["tools"] = json!(tools);
            req_body["tool_choice"] = json!("auto");
        }
        if let Some(temp) = config.temperature {
            req_body["temperature"] = json!(temp);
        }
        if config.enable_thinking {
            req_body["thinking"] = json!({ "type": "enabled" });
        }
        if !config.reasoning_effort.is_empty() {
            req_body["reasoning_effort"] = json!(config.reasoning_effort);
        }

        let (finish_reason, tool_calls) =
            stream_request(&app, &client, &url, &config.api_key, req_body).await?;

        if finish_reason != "tool_calls" || tool_calls.is_empty() {
            break;
        }

        // Append assistant message with tool_calls
        let assistant_tcs: Vec<Value> = tool_calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect();
        all_messages.push(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": assistant_tcs
        }));

        // Execute each tool and append its result
        for (id, name, args) in &tool_calls {
            let result = tools::execute_tool(&app, name, args, skill_dir_path.clone()).await;
            all_messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
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

fn load_skill_by_name(skills_dir: &PathBuf, name: &str) -> Result<Skill, String> {
    let path = skills_dir.join(name).join("skill.md");
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    parse_skill_md(&content)
}

#[tauri::command]
fn list_skills(state: State<'_, AppState>) -> Vec<Skill> {
    let mut skills = Vec::new();

    let read_dir_skills = |dir: &PathBuf| -> Vec<Skill> {
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| {
                        let skill_path = e.path().join("skill.md");
                        let content = fs::read_to_string(skill_path).ok()?;
                        parse_skill_md(&content).ok()
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    skills.extend(read_dir_skills(&state.skills_dir));

    if let Some(home_dir) = home_dir() {
        let user_skills_dir = home_dir.join(".skills");
        skills.extend(read_dir_skills(&user_skills_dir));
    }

    skills
}

#[tauri::command]
fn save_skill(state: State<'_, AppState>, skill: Skill) -> Result<(), String> {
    let md = skill_to_md(&skill)?;
    let skill_dir = state.skills_dir.join(&skill.name);
    fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    fs::write(skill_dir.join("skill.md"), md).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_skill(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let skill_dir = state.skills_dir.join(&name);
    fs::remove_dir_all(skill_dir).map_err(|e| e.to_string())
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
