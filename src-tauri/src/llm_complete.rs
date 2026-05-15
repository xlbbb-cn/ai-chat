use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, tools, skills};

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

            if let Some(usage) = parsed.get("usage") {
                if !usage.is_null() {
                    let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let _ = app.emit("chat-usage", json!({
                        "prompt_tokens": prompt_tokens,
                        "completion_tokens": completion_tokens
                    }));
                }
            }

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

#[tauri::command]
pub async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let config = state.config.lock().unwrap().clone();

    let mut allow_commands = false;
    let mut all_messages: Vec<Value> = vec![];
    let mut skill_dir_path: Option<PathBuf> = None;

    if !config.system_message.is_empty() {
        all_messages.push(json!({ "role": "system", "content": config.system_message }));
    }

    for skill_name in &skill_ids {
        if let Ok(skill) = skills::load_skill_by_name(&state.skills_dir, skill_name) {
            all_messages.push(json!({ "role": "system", "content": skill.system_prompt }));
            if skill.allowed_tools.iter().any(|t| t.eq_ignore_ascii_case("bash")) {
                allow_commands = true;
            }
            if skill_dir_path.is_none() {
                skill_dir_path = Some(state.skills_dir.join(skill_name));
            }
        } else if let Some(home_dir) = dirs::home_dir() {
            let user_skills_dir = home_dir.join(".skills");
            if let Ok(skill) = skills::load_skill_by_name(&user_skills_dir, skill_name) {
                all_messages.push(json!({ "role": "system", "content": skill.system_prompt }));
                if skill.allowed_tools.iter().any(|t| t.eq_ignore_ascii_case("bash")) {
                    allow_commands = true;
                }
                if skill_dir_path.is_none() {
                    skill_dir_path = Some(user_skills_dir.join(skill_name));
                }
            }
        }
    }

    for m in &messages {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!("{}/chat/completions", config.api_base_url.trim_end_matches('/'));
    let client = Client::new();

    if config.selected_tools.iter().any(|t| t == "execute_command") {
        allow_commands = true;
    }
    
    let tools_list = tools::get_all_tools(&config.selected_tools, allow_commands, skill_dir_path.as_deref());

    loop {
        let mut req_body = json!({
            "model": config.model,
            "messages": all_messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
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
