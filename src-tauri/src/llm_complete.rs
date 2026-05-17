use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, tools, skills, mcp};

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
    cancelled: &AtomicBool,
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
    let mut buffer = String::new();

    'stream_loop: while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(("cancelled".to_string(), vec![]));
        }
        let bytes = chunk.map_err(|e| e.to_string())?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(idx) = buffer.find('\n') {
            let line = buffer[..idx].to_string();
            buffer.drain(..=idx);

            if cancelled.load(Ordering::SeqCst) {
                return Ok(("cancelled".to_string(), vec![]));
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line == "data: [DONE]" {
                break 'stream_loop;
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

            // Consume reasoning content (DeepSeek/Qwen style)
            if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                let _ = app.emit("chat-reasoning-token", reasoning.to_string());
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
    state.chat_cancelled.store(false, Ordering::SeqCst);
    let config = state.config.lock().unwrap().clone();

    let mut allow_commands = false;
    let mut all_messages: Vec<Value> = vec![];
    let mut skill_dir_path: Option<PathBuf> = None;

    // 1. Determine which skills have already been loaded in this session
    // We scan the assistant messages for "🧠 *Loading skill: xxx*"
    let mut activated_skills: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &messages {
        if m.role == "assistant" {
            let marker = "🧠 *Loading skill: ";
            let mut start_idx = 0;
            while let Some(idx) = m.content[start_idx..].find(marker) {
                let actual_start = start_idx + idx + marker.len();
                if let Some(end) = m.content[actual_start..].find('*') {
                    let skill_name = m.content[actual_start..actual_start+end].trim().to_string();
                    activated_skills.insert(skill_name);
                }
                start_idx = actual_start;
            }
        }
    }

    let mut system_content = config.system_message.clone();
    let mut loaded_skills_content = String::new();
    let mut available_skills_info = String::new();

    for skill_name in &skill_ids {
        let skill_opt = if let Ok(skill) = skills::load_skill_by_name(&state.skills_dir, skill_name) {
            Some((skill, state.skills_dir.join(skill_name)))
        } else if let Some(home_dir) = dirs::home_dir() {
            let user_skills_dir = home_dir.join(".skills");
            if let Ok(skill) = skills::load_skill_by_name(&user_skills_dir, skill_name) {
                Some((skill, user_skills_dir.join(skill_name)))
            } else { None }
        } else { None };

        if let Some((skill, spath)) = skill_opt {
            if skill.allowed_tools.iter().any(|t| t.eq_ignore_ascii_case("bash")) {
                allow_commands = true;
            }
            if skill_dir_path.is_none() {
                skill_dir_path = Some(spath);
            }
            
            if activated_skills.contains(&skill.name) {
                // Already active, keep its full details in a non-system context message.
                loaded_skills_content.push_str(&format!("\n\n--- Skill: {} ---\n{}", skill.name, skill.system_prompt));
            } else {
                // Not active, just list description so it can be loaded with use_skill
                available_skills_info.push_str(&format!("- Name: {}\n  Description: {}\n", skill.name, skill.description));
            }
        }
    }

    if !available_skills_info.is_empty() {
        let skills_sys_msg = format!(
            "You have access to the following skills. You currently only see their descriptions. \
            If you decide that a skill is relevant to the user's request, you MUST call the `use_skill` \
            tool with the skill's name to load its detailed instructions. Once loaded, the instructions will be appended as a dedicated context message for the rest of the session.\n\n\
            Available skills:\n{}",
            available_skills_info
        );
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&skills_sys_msg);
    }

    // Push the SINGLE combined system message.
    if !system_content.is_empty() {
        all_messages.push(json!({ "role": "system", "content": system_content }));
    }

    if !loaded_skills_content.is_empty() {
        let active_skills_context = format!(
            "INTERNAL CONTEXT - ACTIVE SKILLS (not a user request):\n\
IMPORTANT SKILL PATH ISOLATION RULE: Except for explicitly requested paths, any operation executed by a skill MUST use the directory containing the skill's SKILL.md as its root path. Operating on or referencing paths outside this root is STRICTLY FORBIDDEN. All paths referenced within a skill (e.g. read_file, write_file, list_dir, execute_command) are automatically evaluated relative to this root path.\n\n\
The following skills are CURRENTLY ACTIVE and their detailed instructions are provided below:{}",
            loaded_skills_content
        );
        all_messages.push(json!({ "role": "user", "content": active_skills_context }));
    }

    for m in &messages {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!("{}/chat/completions", config.api_base_url.trim_end_matches('/'));
    let client = Client::new();

    if config.selected_tools.iter().any(|t| t == "execute_command") {
        allow_commands = true;
    }

    let mut tools_list = tools::get_all_tools(&config.selected_tools, allow_commands, skill_dir_path.as_deref());

    // ── MCP tools ───────────────────────────────────────────────────────────
    // Load enabled MCP servers and collect their tools into tools_list.
    // mcp_tool_map: function_name -> (server, actual_mcp_tool_name)
    let mut mcp_tool_map: std::collections::HashMap<String, (mcp::McpServer, String)> =
        std::collections::HashMap::new();
    {
        let enabled_servers: Vec<mcp::McpServer> = mcp::load_servers(&state.mcp_servers_path)
            .into_iter()
            .filter(|s| s.enabled)
            .collect();

        for (idx, server) in enabled_servers.iter().enumerate() {
            match mcp::get_server_tools(server).await {
                Ok(server_tools) => {
                    for tool in server_tools {
                        let Some(actual_name) = tool["function"]["name"].as_str() else { continue };
                        let safe_name = mcp::sanitize_fn_name(actual_name);
                        // Prefix: mcp_{idx}_{safe_tool_name}, truncated to 64 chars
                        let raw_fn = format!("mcp_{idx}_{safe_name}");
                        let fn_name: String = raw_fn.chars().take(64).collect();

                        let mut openai_tool = tool.clone();
                        openai_tool["function"]["name"] = serde_json::json!(fn_name);
                        // Prepend server name to description for clarity
                        let desc = tool["function"]["description"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        openai_tool["function"]["description"] =
                            serde_json::json!(format!("[MCP: {}] {}", server.name, desc));

                        tools_list.push(openai_tool);
                        mcp_tool_map.insert(fn_name, (server.clone(), actual_name.to_string()));
                    }
                }
                Err(e) => {
                    // Non-fatal: log via a system message the LLM won't see
                    eprintln!("MCP server '{}' tools/list failed: {e}", server.name);
                }
            }
        }
    }

    if !skill_ids.is_empty() {
        tools_list.push(json!({
            "type": "function",
            "function": {
                "name": "use_skill",
                "description": "Load detailed instructions for a specific skill. You MUST call this before using a skill's capabilities.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "skill_name": { "type": "string", "description": "The name of the skill to load details for" }
                    },
                    "required": ["skill_name"]
                }
            }
        }));
    }

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
        if !config.reasoning_effort.is_empty() {
            req_body["reasoning_effort"] = json!(config.reasoning_effort);
        }

        let (finish_reason, tool_calls) =
            stream_request(
                &app,
                &client,
                &url,
                &config.api_key,
                req_body,
                &state.chat_cancelled,
            )
            .await?;

        if finish_reason == "cancelled" {
            break;
        }

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

        let mut pending_skill_context_messages: Vec<Value> = vec![];

        for (id, name, args) in &tool_calls {
            if state.chat_cancelled.load(Ordering::SeqCst) {
                break;
            }
            let result = if name == "use_skill" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let skill_name = args_json["skill_name"].as_str().unwrap_or("");
                
                let skill_opt = if let Ok(skill) = skills::load_skill_by_name(&state.skills_dir, skill_name) {
                    Some(skill)
                } else if let Some(home_dir) = dirs::home_dir() {
                    let user_skills_dir = home_dir.join(".skills");
                    skills::load_skill_by_name(&user_skills_dir, skill_name).ok()
                } else {
                    None
                };

                if let Some(skill) = skill_opt {
                    let dyn_marker = format!("--- Skill (Dynamically Loaded): {} ---", skill.name);
                    let static_marker = format!("--- Skill: {} ---", skill.name);

                    let already_loaded = all_messages.iter().any(|msg| {
                        msg["content"]
                            .as_str()
                            .map(|content| content.contains(&dyn_marker) || content.contains(&static_marker))
                            .unwrap_or(false)
                    }) || pending_skill_context_messages.iter().any(|msg| {
                        msg["content"]
                            .as_str()
                            .map(|content| content.contains(&dyn_marker) || content.contains(&static_marker))
                            .unwrap_or(false)
                    });

                    if !already_loaded {
                        let skill_context = format!(
                            "INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL (not a user request):\n\
{}\n{}",
                            dyn_marker,
                            skill.system_prompt
                        );
                        pending_skill_context_messages.push(json!({
                            "role": "user",
                            "content": skill_context
                        }));
                        let _ = app.emit("chat-token", format!("🧠 *Loading skill: {}*\n\n", skill_name));
                        format!("Skill '{}' detailed instructions have been successfully loaded and appended to context messages. You can now follow its instructions to fulfill the user's request. There is no need to call use_skill for this skill again.", skill_name)
                    } else {
                        format!("Skill '{}' is already loaded in context messages. There is no need to call use_skill for this skill again.", skill_name)
                    }
                } else {
                    format!("Error: Skill '{}' not found.", skill_name)
                }
            } else if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let _ = app.emit("chat-token", format!("🔌 *MCP [{}]: {}*\n\n", mcp_server.name, actual_tool_name));
                match mcp::invoke_mcp_tool(mcp_server, actual_tool_name, args_json).await {
                    Ok(result) => result,
                    Err(e) => format!("MCP tool error: {e}"),
                }
            } else {
                tools::execute_tool(
                    &app,
                    name,
                    args,
                    skill_dir_path.clone(),
                    state.workspace_dir.clone(),
                    &config,
                )
                .await
            };
            
            all_messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }

        all_messages.extend(pending_skill_context_messages);
    }

    state.chat_cancelled.store(false, Ordering::SeqCst);
    let _ = app.emit("chat-done", ());
    Ok(())
}
