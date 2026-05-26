use futures_util::StreamExt;
use os_info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

use crate::{agents, db, mcp, skills, tools, AppState};

fn log_event(state: &State<'_, AppState>, level: &str, message: String) {
    if let Ok(logger) = state.logger.lock() {
        logger.log(level, &message);
    }
}

fn merge_allowed_commands(allowed_commands: &mut Vec<String>, incoming: &[String]) {
    for command in incoming {
        if !allowed_commands
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(command))
        {
            allowed_commands.push(command.clone());
        }
    }
}

// ─── Chat ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub struct StreamResult {
    pub finish_reason: String,
    pub tool_calls: Vec<(String, String, String)>,
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Options controlling how a streaming LLM request behaves.
/// Main agent: emit reasoning + usage, token_event="chat-token", no task_id.
/// Sub-agent:  no reasoning/usage, token_event="agent-task-token", task_id=Some(...).
pub struct StreamOptions<'a> {
    /// Event name for content delta tokens.
    pub token_event: &'a str,
    /// If set, include `{"task_id": ..., "token": ...}` payload instead of a plain string.
    pub task_id: Option<&'a str>,
    /// Emit `chat-reasoning-token` for DeepSeek/Qwen reasoning_content.
    pub emit_reasoning: bool,
    /// Emit `chat-usage` and populate prompt/completion token counts.
    pub emit_usage: bool,
    /// Optional max token ceiling used for usage ratio reporting.
    pub usage_max_tokens: Option<u32>,
}

const CONTEXT_COMPRESSION_THRESHOLD: f32 = 0.9;
const CONTEXT_KEEP_RECENT_MESSAGES: usize = 12;
const CONTEXT_SUMMARY_MARKER: &str = "INTERNAL CONTEXT - SESSION SUMMARY";
const CONTEXT_SUMMARY_MAX_LINES: usize = 120;
const DEFAULT_MODEL_MAX_TOKENS: u32 = 131_072;
const STREAM_MAX_RETRIES: usize = 3;
const STREAM_RETRY_BACKOFF_MS: [u64; 2] = [300, 900];

fn retry_backoff_ms(attempt: usize) -> u64 {
    let idx = attempt.saturating_sub(1).min(STREAM_RETRY_BACKOFF_MS.len());
    STREAM_RETRY_BACKOFF_MS
        .get(idx)
        .copied()
        .unwrap_or(1500)
}

fn is_retryable_network_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    let patterns = [
        "socket connection was closed unexpectedly",
        "connection reset",
        "broken pipe",
        "connection aborted",
        "timed out",
        "timeout",
        "unexpected eof",
        "incomplete message",
        "stream error",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

fn extract_upstream_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.to_string())
}

fn is_retryable_http_error_body(body: &str) -> bool {
    is_retryable_network_error(&extract_upstream_error_message(body))
}

fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(300))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect::<String>()
}

fn summarize_message_for_context(msg: &Value) -> Option<String> {
    let role = msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    if role == "tool" {
        return None;
    }

    if msg
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|calls| !calls.is_empty())
        .unwrap_or(false)
    {
        return None;
    }

    let content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .replace('\n', " ");

    if content.is_empty() {
        return None;
    }

    Some(format!("- {role}: {}", truncate_text(&content, 260)))
}

fn is_pinned_context_message(msg: &Value) -> bool {
    if msg.get("role").and_then(|v| v.as_str()) == Some("system") {
        return true;
    }
    msg.get("content")
        .and_then(|v| v.as_str())
        .map(|content| {
            content.starts_with("INTERNAL CONTEXT - ACTIVE SKILLS")
                || content.starts_with("INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL")
        })
        .unwrap_or(false)
}

fn extract_summary_body(content: &str) -> Option<String> {
    if !content.starts_with(CONTEXT_SUMMARY_MARKER) {
        return None;
    }
    let body = content
        .split_once('\n')
        .map(|(_, rest)| rest.trim().to_string())
        .unwrap_or_default();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

fn merge_summary_lines(existing_summary: Option<&str>, new_lines: &[String]) -> String {
    let mut merged: Vec<String> = Vec::new();

    if let Some(existing) = existing_summary {
        merged.extend(
            existing
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string()),
        );
    }

    merged.extend(new_lines.iter().cloned());

    if merged.len() > CONTEXT_SUMMARY_MAX_LINES {
        merged = merged.split_off(merged.len() - CONTEXT_SUMMARY_MAX_LINES);
    }

    merged.join("\n")
}

fn compress_session_context(all_messages: &mut Vec<Value>) -> Option<String> {
    if all_messages.len() <= CONTEXT_KEEP_RECENT_MESSAGES + 2 {
        return None;
    }

    let mut pinned: Vec<Value> = Vec::new();
    let mut compressible: Vec<Value> = Vec::new();
    let mut existing_summary_body: Option<String> = None;

    for msg in all_messages.drain(..) {
        let existing_summary = msg
            .get("content")
            .and_then(|v| v.as_str())
            .and_then(extract_summary_body);
        if let Some(summary_body) = existing_summary {
            existing_summary_body = Some(summary_body);
            continue;
        }

        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or_default();
        let has_tool_calls = msg
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .map(|calls| !calls.is_empty())
            .unwrap_or(false);

        if role == "tool" || has_tool_calls {
            // Explicitly drop tool invocation context from compression strategy.
            continue;
        }

        if is_pinned_context_message(&msg) {
            pinned.push(msg);
        } else {
            compressible.push(msg);
        }
    }

    if compressible.len() <= CONTEXT_KEEP_RECENT_MESSAGES + 2 {
        let mut rebuilt = Vec::with_capacity(pinned.len() + compressible.len());
        rebuilt.extend(pinned);
        rebuilt.extend(compressible);

        if let Some(existing_summary_body) = existing_summary_body {
            rebuilt.insert(
                rebuilt.len().min(1),
                json!({
                    "role": "system",
                    "content": format!("{CONTEXT_SUMMARY_MARKER}\n{existing_summary_body}")
                }),
            );
        }

        *all_messages = rebuilt;
        return None;
    }

    let split_idx = compressible
        .len()
        .saturating_sub(CONTEXT_KEEP_RECENT_MESSAGES);
    let (older, recent) = compressible.split_at(split_idx);

    let summary_lines: Vec<String> = older
        .iter()
        .filter_map(summarize_message_for_context)
        .collect();

    if summary_lines.is_empty() {
        let mut rebuilt = Vec::with_capacity(pinned.len() + compressible.len());
        rebuilt.extend(pinned);
        rebuilt.extend(compressible);

        if let Some(existing_summary_body) = existing_summary_body {
            rebuilt.insert(
                rebuilt.len().min(1),
                json!({
                    "role": "system",
                    "content": format!("{CONTEXT_SUMMARY_MARKER}\n{existing_summary_body}")
                }),
            );
        }

        *all_messages = rebuilt;
        return None;
    }

    let merged_summary = merge_summary_lines(existing_summary_body.as_deref(), &summary_lines);

    let summary_message = json!({
        "role": "system",
        "content": format!("{CONTEXT_SUMMARY_MARKER}\n{merged_summary}")
    });

    let mut rebuilt = Vec::with_capacity(pinned.len() + 1 + recent.len());
    rebuilt.extend(pinned);
    rebuilt.push(summary_message);
    rebuilt.extend(recent.iter().cloned());
    *all_messages = rebuilt;
    Some(merged_summary)
}

// ─── Unified streaming helper ─────────────────────────────────────────────────

/// Stream one completion request according to `opts`.
/// Returns StreamResult with finish_reason, accumulated tool_calls, content, and token counts.
pub async fn stream_llm_request(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    req_body: Value,
    cancelled: &AtomicBool,
    opts: StreamOptions<'_>,
) -> Result<StreamResult, String> {
    let mut last_err: Option<String> = None;

    for attempt in 1..=STREAM_MAX_RETRIES {
        let res = match client
            .post(url)
            .bearer_auth(api_key)
            .json(&req_body)
            .send()
            .await
        {
            Ok(res) => res,
            Err(err) => {
                let err = err.to_string();
                let retryable = is_retryable_network_error(&err) && attempt < STREAM_MAX_RETRIES;
                last_err = Some(err.clone());
                if retryable {
                    tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
                    continue;
                }
                return Err(err);
            }
        };

        if !res.status().is_success() {
            let err = res.text().await.unwrap_or_default();
            let retryable =
                is_retryable_http_error_body(&err) && attempt < STREAM_MAX_RETRIES;
            last_err = Some(err.clone());
            if retryable {
                tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
                continue;
            }
            let _ = app.emit("chat-error", err.clone());
            return Err(err);
        }

        let mut finish_reason = String::new();
        let mut tool_calls: Vec<(String, String, String)> = Vec::new();
        let mut content = String::new();
        let mut prompt_tokens: u32 = 0;
        let mut completion_tokens: u32 = 0;
        let mut got_tool_calls = false;
        let mut stream_error: Option<String> = None;

        let mut stream = res.bytes_stream();
        let mut buffer = String::new();

        'stream_loop: while let Some(chunk) = stream.next().await {
            if cancelled.load(Ordering::SeqCst) {
                return Ok(StreamResult {
                    finish_reason: "cancelled".into(),
                    tool_calls: vec![],
                    content,
                    prompt_tokens,
                    completion_tokens,
                });
            }
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(err) => {
                    let err = err.to_string();
                    let can_retry = is_retryable_network_error(&err)
                        && content.is_empty()
                        && tool_calls.is_empty()
                        && attempt < STREAM_MAX_RETRIES;
                    if can_retry {
                        stream_error = Some(err);
                        break 'stream_loop;
                    }
                    return Err(err);
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].to_string();
                buffer.drain(..=idx);

                if cancelled.load(Ordering::SeqCst) {
                    return Ok(StreamResult {
                        finish_reason: "cancelled".into(),
                        tool_calls: vec![],
                        content,
                        prompt_tokens,
                        completion_tokens,
                    });
                }
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "data: [DONE]" {
                    break 'stream_loop;
                }
                let Some(json_str) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<Value>(json_str) else {
                    continue;
                };

                if opts.emit_usage {
                    if let Some(usage) = parsed.get("usage").filter(|v| !v.is_null()) {
                        prompt_tokens = usage
                            .get("prompt_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        completion_tokens = usage
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        let total_tokens = prompt_tokens.saturating_add(completion_tokens);
                        let usage_ratio = opts
                            .usage_max_tokens
                            .filter(|max| *max > 0)
                            .map(|max| total_tokens as f64 / max as f64)
                            .unwrap_or(0.0);
                        let _ = app.emit(
                            "chat-usage",
                            json!({
                                "prompt_tokens": prompt_tokens,
                                "completion_tokens": completion_tokens,
                                "total_tokens": total_tokens,
                                "max_tokens": opts.usage_max_tokens,
                                "usage_ratio": usage_ratio
                            }),
                        );
                    }
                }

                let Some(choice) = parsed["choices"].get(0) else {
                    continue;
                };
                let delta = &choice["delta"];

                if let Some(fr) = choice["finish_reason"].as_str() {
                    if !fr.is_empty() {
                        finish_reason = fr.to_string();
                        if fr == "tool_calls" {
                            got_tool_calls = true;
                        }
                    }
                }

                // DeepSeek/Qwen reasoning tokens — main agent only
                if opts.emit_reasoning {
                    if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                        let _ = app.emit("chat-reasoning-token", reasoning.to_string());
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

                // Emit content token
                if let Some(token) = delta["content"].as_str() {
                    content.push_str(token);
                    if let Some(task_id) = opts.task_id {
                        let _ = app.emit(
                            opts.token_event,
                            json!({ "task_id": task_id, "token": token }),
                        );
                    } else {
                        let _ = app.emit(opts.token_event, token.to_string());
                    }
                }
            }
        }

        if let Some(err) = stream_error {
            last_err = Some(err);
            tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
            continue;
        }

        // Sub-agent mode: clear tool_calls if the LLM never signalled tool use
        if !got_tool_calls && opts.task_id.is_some() {
            tool_calls.clear();
        }

        return Ok(StreamResult {
            finish_reason,
            tool_calls,
            content,
            prompt_tokens,
            completion_tokens,
        });
    }

    Err(last_err.unwrap_or_else(|| "stream request failed after retries".to_string()))
}

/// Thin wrapper used by the main agent (preserves existing call sites).
async fn stream_request(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    req_body: Value,
    cancelled: &AtomicBool,
    usage_max_tokens: Option<u32>,
) -> Result<StreamResult, String> {
    stream_llm_request(
        app,
        client,
        url,
        api_key,
        req_body,
        cancelled,
        StreamOptions {
            token_event: "chat-token",
            task_id: None,
            emit_reasoning: true,
            emit_usage: true,
            usage_max_tokens,
        },
    )
    .await
}

/// Log interaction to database
fn log_interaction(
    db: &rusqlite::Connection,
    session_id: &str,
    interaction_type: &str,
    actor: &str,
    action_name: &str,
    input_data: String,
    output_data: String,
    error_message: Option<String>,
    duration_ms: i64,
) {
    let _ = db::save_interaction_log(
        db,
        session_id,
        interaction_type,
        actor,
        action_name,
        &input_data,
        &output_data,
        error_message.as_deref(),
        duration_ms,
        None,
    );
}

// ─── Chat command ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_ids: Vec<String>,
    session_id: String,
    model_override: Option<String>,
    use_agents: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.chat_cancelled.store(false, Ordering::SeqCst);
    let config = state.config.lock().unwrap().clone();
    let multi_agent_enabled = use_agents.unwrap_or(false);
    let self_evolution_enabled = config.self_evolution_mode;
    let subagent_evolution_enabled = multi_agent_enabled && config.self_evolution_mode;
    log_event(
        &state,
        "INFO",
        format!(
            "chat_completion started: session_id={}, model={}",
            session_id, config.model
        ),
    );

    // ── Agent orchestration path ──────────────────────────────────────────────
    if multi_agent_enabled {
        let agents_cfg = agents::load_agents_config(&state.agents_config_path);
        let has_enabled = agents_cfg.agents.iter().any(|a| a.enabled);
        if has_enabled {
            let result = agents::orchestrate(&app, &state, &config, &messages, &session_id).await;
            state.chat_cancelled.store(false, Ordering::SeqCst);
            log_event(
                &state,
                "INFO",
                format!("agent orchestration finished: session_id={}", session_id),
            );
            let _ = app.emit("chat-done", ());
            return result.map(|_| ());
        }
    }

    let mut all_messages: Vec<Value> = vec![];
    let mut skill_allowed_commands: Vec<String> = Vec::new();
    let mut active_skill_roots: Vec<std::path::PathBuf> = Vec::new();
    let self_evolution_roots = if self_evolution_enabled {
        skills::collect_self_evolution_roots(&state.skills_dir, true)
    } else {
        Vec::new()
    };
    let self_evolution_files = if subagent_evolution_enabled {
        vec![state.agents_config_path.clone()]
    } else {
        Vec::new()
    };

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
                    let skill_name = m.content[actual_start..actual_start + end]
                        .trim()
                        .to_string();
                    activated_skills.insert(skill_name);
                }
                start_idx = actual_start;
            }
        }
    }

    let mut system_content = config.system_message.clone(); //top-level system instructions from config
    let mut loaded_skills_content = String::new();
    let mut available_skills_info = String::new();

    let os_info = os_info::get();
    let os_sys_msg = format!(
        "System information:\n- OS: {} {}\n- CPU: {:?}\n",
        os_info.os_type(),
        os_info.version(),
        os_info.architecture()
    );

    if !system_content.is_empty() {
        system_content.push_str("\n\n");
    }
    system_content.push_str(&os_sys_msg); //Add system info to system prompt

    for skill_name in &skill_ids {
        let skill_opt = if let Ok(skill) = skills::load_skill_by_name(&state.skills_dir, skill_name)
        {
            Some((skill, state.skills_dir.join(skill_name)))
        } else if let Some(user_skills_dir) = skills::user_skills_dir() {
            if let Ok(skill) = skills::load_skill_by_name(&user_skills_dir, skill_name) {
                Some((skill, user_skills_dir.join(skill_name)))
            } else {
                None
            }
        } else {
            None
        };

        if let Some((skill, spath)) = skill_opt {
            merge_allowed_commands(&mut skill_allowed_commands, &skill.allowed_commands);

            if activated_skills.contains(&skill.name) {
                if !active_skill_roots.iter().any(|p| p == &spath) {
                    active_skill_roots.push(spath.clone());
                }
                let cmd_constraint = if skill.allowed_commands.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n[Allowed commands for this skill: {}]\n",
                        skill.allowed_commands.join(", ")
                    )
                };
                loaded_skills_content.push_str(&format!(
                    "\n\n--- Skill: {} ---\n{}{}",
                    skill.name, cmd_constraint, skill.system_prompt
                ));
            } else {
                available_skills_info.push_str(&format!(
                    "- Name: {}\n  Description: {}\n",
                    skill.name, skill.description
                ));
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

    let enabled_agents: Vec<agents::SubAgent> = if multi_agent_enabled {
        agents::load_agents_config(&state.agents_config_path)
            .agents
            .into_iter()
            .filter(|a| a.enabled)
            .collect()
    } else {
        Vec::new()
    };

    if !enabled_agents.is_empty() {
        let mut available_agents_info = String::new();
        for agent in &enabled_agents {
            available_agents_info.push_str(&format!(
                "- ID: {}\n  Name: {}\n  Description: {}\n",
                agent.id, agent.name, agent.description
            ));
        }
        let agents_sys_msg = format!(
        "You have access to the following specialized sub-agents. To delegate a task to one of them, use the `call_subagent` tool with the corresponding `agent_id`.\n\n\
        Available sub-agents:\n{}",
        available_agents_info
    );
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&agents_sys_msg);
    }

    if !system_content.is_empty() {
        all_messages.push(json!({ "role": "system", "content": system_content }));
    }

    if self_evolution_enabled && !self_evolution_roots.is_empty() {
        let skill_roots_display = self_evolution_roots
            .iter()
            .map(|p| format!("- {}", p.display()))
            .collect::<Vec<String>>()
            .join("\n");
        let subagent_config_section = if subagent_evolution_enabled {
            format!(
                "- You may also inspect and update the sub-agent config file at:\n\\
                 {}\n\\
",
                state.agents_config_path.display()
            )
        } else {
            String::new()
        };
        let backup_scope_line = if subagent_evolution_enabled {
            "- Before modifying any skill file or the sub-agent config file, create a sibling backup with suffix `.bak.<number>`."
        } else {
            "- Before modifying any skill file, create a sibling backup with suffix `.bak.<number>`."
        };
        all_messages.push(json!({
            "role": "system",
            "content": format!(
                "INTERNAL CONTEXT - SELF EVOLUTION MODE (not a user request):\n\
                 Self-evolution mode is ENABLED.\n\
                 - You may inspect, create, and update reusable skills under these skill roots:\n\
                 {skill_roots_display}\n\
                 {subagent_config_section}
                 - When it directly helps the user's request, you may improve existing skills or create new ones for future reuse.\n\
                 {backup_scope_line}\n\
                 - Use `file_actions` for these edits so backup creation is enforced automatically.\n\
                 - Prefer absolute paths when working in skill roots to avoid ambiguity with workspace files.\n\
                 - If you read or list a file inside a skill root, reuse that exact returned path when patching or writing it.\n\
                 - If you use a relative path that starts with a skill root directory name (for example `skills/demo/skill.md` or `.skills/demo/skill.md`), treat it as a path inside that skill root, not inside the workspace.\n\
                 - Unless the user explicitly asks otherwise, do not access paths outside the workspace root, these skill roots, or the sub-agent config file."
            )
        }));
    }

    if let Ok(db_guard) = state.db.lock() {
        if let Ok(Some(saved_summary)) = db::get_session_summary(&db_guard, &session_id) {
            if !saved_summary.trim().is_empty() {
                all_messages.push(json!({
                    "role": "system",
                    "content": format!("{CONTEXT_SUMMARY_MARKER}\n{}", saved_summary)
                }));
            }
        }
    }

    if !loaded_skills_content.is_empty() {
        let workspace_dir = state.workspace_dir.lock().unwrap().clone();
        let workspace_root_display = workspace_dir.display().to_string();
        let active_skill_roots_display = if active_skill_roots.is_empty() {
            "(none)".to_string()
        } else {
            active_skill_roots
                .iter()
                .map(|p| format!("- {}", p.display()))
                .collect::<Vec<String>>()
                .join("\n")
        };
        let active_skills_context = format!(
               "INTERNAL CONTEXT - ACTIVE SKILLS (not a user request):\n\
                IMPORTANT SKILL PATH ISOLATION RULE:\n\
                - The workspace root directory (absolute path on this machine) is: {workspace_root_display}\n\
                - Active skill root directories are:\n\
                {active_skill_roots_display}\n\
                - For workspace files, use paths relative to the workspace root (e.g. \"src/foo.txt\").\n\
                - For skill reference files (e.g. \"ref/index.md\"), you may use a relative path; backend resolves existing files under workspace root first, then active skill roots.\n\
                - When you read a file from a skill root, write back to that same skill-root path instead of recreating it under the workspace root.\n\
                - Relative paths that begin with a skill root directory name (for example \"skills/demo/skill.md\") are interpreted as belonging to that skill root.\n\
                - You may also provide an absolute path under the workspace root or any active skill root; backend will strip the matched root prefix automatically.\n\
                - Except for explicitly requested paths, you MUST NOT access any file or directory outside workspace root or active skill roots.\n\
                - Operating on paths outside these roots is STRICTLY FORBIDDEN.\n\n\
                The following skills are CURRENTLY ACTIVE and their detailed instructions are provided below:{}",
            loaded_skills_content
        );
        all_messages.push(json!({ "role": "user", "content": active_skills_context }));
    }

    for m in &messages {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
    let client = build_http_client()?;

    let mut tools_list = tools::get_all_tools(&config.selected_tools);
    let active_model = model_override
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| config.model.clone());

    // ── MCP tools ───────────────────────────────────────────────────────────
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
                        let Some(actual_name) = tool["function"]["name"].as_str() else {
                            continue;
                        };
                        let safe_name = mcp::sanitize_fn_name(actual_name);
                        let raw_fn = format!("mcp_{idx}_{safe_name}");
                        let fn_name: String = raw_fn.chars().take(64).collect();

                        let mut openai_tool = tool.clone();
                        openai_tool["function"]["name"] = serde_json::json!(fn_name);
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
                    log_event(
                        &state,
                        "ERROR",
                        format!("MCP server '{}' tools/list failed: {e}", server.name),
                    );
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

    if !enabled_agents.is_empty() {
        tools_list.push(json!({
            "type": "function",
            "function": {
                "name": "call_subagent",
                "description": "Delegate a complex or specialized task to an autonomous sub-agent. You must specify the exact agent_id. Wait for its execution fully; it returns the final task result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The exact ID of the sub-agent to invoke"
                        },
                        "task_description": {
                            "type": "string",
                            "description": "Detailed prompt/description of the task for the sub-agent to perform"
                        }
                    },
                    "required": ["agent_id", "task_description"]
                }
            }
        }));
    }

    loop {
        let effective_max_tokens = config
            .model_settings
            .max_tokens
            .filter(|max| *max > 0)
            .unwrap_or(DEFAULT_MODEL_MAX_TOKENS);

        let mut req_body = json!({
            "model": active_model,
            "messages": all_messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
            req_body["tool_choice"] = json!("auto");
        }
        if let Some(temp) = config.model_settings.temperature {
            req_body["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.model_settings.top_p {
            req_body["top_p"] = json!(top_p);
        }
        if !config.model_settings.reasoning_effort.is_empty() {
            req_body["reasoning_effort"] = json!(config.model_settings.reasoning_effort);
        }
        if let Some(max_tokens) = config.model_settings.max_tokens {
            req_body["max_tokens"] = json!(max_tokens);
        }

        let started_at = std::time::Instant::now();
        let request_snapshot = req_body.clone();

        let result = stream_request(
            &app,
            &client,
            &url,
            &config.api_key,
            req_body,
            &state.chat_cancelled,
            Some(effective_max_tokens),
        )
        .await;

        let duration_ms = started_at.elapsed().as_millis() as i64;

        match &result {
            Ok(sr) => {
                // Log successful request
                let tool_calls_json = if sr.tool_calls.is_empty() {
                    String::new()
                } else {
                    serde_json::to_string(
                        &sr.tool_calls
                            .iter()
                            .map(|(id, name, args)| {
                                json!({
                                    "id": id, "name": name, "arguments": args
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap_or_default()
                };
                let db = state.db.lock().unwrap();
                let _ = db.execute(
                    "INSERT INTO api_requests (session_id, model, request_body, response_content, tool_calls, finish_reason, prompt_tokens, completion_tokens, duration_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        session_id,
                        active_model.as_str(),
                        request_snapshot.to_string(),
                        sr.content,
                        tool_calls_json,
                        sr.finish_reason,
                        sr.prompt_tokens,
                        sr.completion_tokens,
                        duration_ms,
                    ],
                );
                // Log to interaction_log table
                let request_body_display = request_snapshot.to_string();
                let response_display = sr.content.clone();
                log_interaction(
                    &db,
                    &session_id,
                    "llm_api",
                    &active_model,
                    "chat_completion",
                    request_body_display,
                    response_display,
                    None,
                    duration_ms,
                );
                drop(db);
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "chat_completion request succeeded: session_id={}, finish_reason={}, duration_ms={}",
                        session_id, sr.finish_reason, duration_ms
                    ),
                );
            }
            Err(err) => {
                // Log failed request
                let db = state.db.lock().unwrap();
                let _ = db.execute(
                    "INSERT INTO api_requests (session_id, model, request_body, finish_reason, duration_ms, error) \
                     VALUES (?1, ?2, ?3, 'error', ?4, ?5)",
                    rusqlite::params![
                        session_id,
                        active_model.as_str(),
                        request_snapshot.to_string(),
                        duration_ms,
                        err,
                    ],
                );
                // Log to interaction_log table
                let request_body_display = request_snapshot.to_string();
                log_interaction(
                    &db,
                    &session_id,
                    "llm_error",
                    &active_model,
                    "chat_completion",
                    request_body_display,
                    String::new(),
                    Some(err.clone()),
                    duration_ms,
                );
                drop(db);
                log_event(
                    &state,
                    "ERROR",
                    format!(
                        "chat_completion request failed: session_id={}, duration_ms={}, error={}",
                        session_id, duration_ms, err
                    ),
                );
            }
        }

        let sr = result?;

        let total_tokens = sr.prompt_tokens.saturating_add(sr.completion_tokens);
        let ratio = total_tokens as f32 / effective_max_tokens as f32;
        if ratio >= CONTEXT_COMPRESSION_THRESHOLD {
            if let Some(merged_summary) = compress_session_context(&mut all_messages) {
                if let Ok(db_guard) = state.db.lock() {
                    let _ = db::save_session_summary(&db_guard, &session_id, &merged_summary);
                }
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "session context compressed: session_id={}, total_tokens={}, max_tokens={}, ratio={:.3}",
                        session_id, total_tokens, effective_max_tokens, ratio
                    ),
                );
                let _ = app.emit("chat-token", "\n\n[System] Context compressed (tool-call traces discarded) and persisted for next turns.\n\n");
            }
        }

        if sr.finish_reason == "cancelled" {
            break;
        }

        if sr.finish_reason != "tool_calls" || sr.tool_calls.is_empty() {
            break;
        }

        let assistant_tcs: Vec<Value> = sr
            .tool_calls
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

        for (id, name, args) in &sr.tool_calls {
            if state.chat_cancelled.load(Ordering::SeqCst) {
                break;
            }
            let result = if name == "use_skill" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let skill_name = args_json["skill_name"].as_str().unwrap_or("");

                let skill_opt =
                    if let Ok(skill) = skills::load_skill_by_name(&state.skills_dir, skill_name) {
                        Some((skill, state.skills_dir.join(skill_name)))
                    } else if let Some(user_skills_dir) = skills::user_skills_dir() {
                        skills::load_skill_by_name(&user_skills_dir, skill_name)
                            .ok()
                            .map(|skill| (skill, user_skills_dir.join(skill_name)))
                    } else {
                        None
                    };

                if let Some((skill, skill_root)) = skill_opt {
                    let dyn_marker = format!("--- Skill (Dynamically Loaded): {} ---", skill.name);
                    let static_marker = format!("--- Skill: {} ---", skill.name);

                    let already_loaded = all_messages.iter().any(|msg| {
                        msg["content"]
                            .as_str()
                            .map(|content| {
                                content.contains(&dyn_marker) || content.contains(&static_marker)
                            })
                            .unwrap_or(false)
                    }) || pending_skill_context_messages.iter().any(|msg| {
                        msg["content"]
                            .as_str()
                            .map(|content| {
                                content.contains(&dyn_marker) || content.contains(&static_marker)
                            })
                            .unwrap_or(false)
                    });

                    if !already_loaded {
                        merge_allowed_commands(
                            &mut skill_allowed_commands,
                            &skill.allowed_commands,
                        );
                        if !active_skill_roots.iter().any(|p| p == &skill_root) {
                            active_skill_roots.push(skill_root);
                        }
                        let cmd_constraint = if skill.allowed_commands.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\n[Allowed commands for this skill: {}]\n",
                                skill.allowed_commands.join(", ")
                            )
                        };
                        let skill_context = format!(
                            "INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL (not a user request):\n\
{}\n{}{}",
                            dyn_marker, cmd_constraint, skill.system_prompt
                        );
                        pending_skill_context_messages.push(json!({
                            "role": "user",
                            "content": skill_context
                        }));
                        let _ = app.emit(
                            "chat-token",
                            format!("🧠 *Loading skill: {}*\n\n", skill_name),
                        );
                        format!("Skill '{}' detailed instructions have been successfully loaded and appended to context messages. You can now follow its instructions to fulfill the user's request. There is no need to call use_skill for this skill again.", skill_name)
                    } else {
                        format!("Skill '{}' is already loaded in context messages. There is no need to call use_skill for this skill again.", skill_name)
                    }
                } else {
                    format!("Error: Skill '{}' not found.", skill_name)
                }
            } else if name == "call_subagent" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let agent_id = args_json["agent_id"].as_str().unwrap_or("");
                let task_desc = args_json["task_description"].as_str().unwrap_or("");

                if let Some(agent) = enabled_agents.iter().find(|a| a.id == agent_id) {
                    let task = agents::Task {
                        id: uuid::Uuid::new_v4().to_string(),
                        agent_id: agent_id.to_string(),
                        description: task_desc.to_string(),
                        context: String::new(),
                        dependencies: vec![],
                    };
                    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                    let url = format!(
                        "{}/chat/completions",
                        config.api_base_url.trim_end_matches('/')
                    );
                    let client = match build_http_client() {
                        Ok(client) => client,
                        Err(err) => return format!("Error: {err}"),
                    };

                    let res = agents::run_sub_agent(
                        &app,
                        &client,
                        &url,
                        &config,
                        agent,
                        &task,
                        workspace_dir,
                        self_evolution_roots.clone(),
                        self_evolution_files.clone(),
                    )
                    .await;

                    format!("Task execution finished with status: {}\nTokens used: {}\nTool calls made: {}\n\nResult:\n{}", 
                        res.status, res.tokens_used, res.tool_calls_count, res.content)
                } else {
                    format!(
                        "Error: Agent with ID '{}' not found or not enabled.",
                        agent_id
                    )
                }
            } else if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                let mut args_json: Value = serde_json::from_str(args).unwrap_or_default();
                // Resolve any relative file paths in arguments to absolute paths
                let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                mcp::resolve_paths_in_args(&mut args_json, &workspace_dir);
                log_event(
                    &state,
                    "INFO",
                    format!("Args for MCP tool '{}': {}", actual_tool_name, args_json),
                );
                let _ = app.emit(
                    "chat-token",
                    format!("🔌 *MCP [{}]: {}*\n\n", mcp_server.name, actual_tool_name),
                );
                let start_time = std::time::Instant::now();
                let mcp_result =
                    mcp::invoke_mcp_tool(mcp_server, actual_tool_name, args_json.clone()).await;
                let duration_ms = start_time.elapsed().as_millis() as i64;
                let result = match &mcp_result {
                    Ok(result) => {
                        // Log successful MCP call
                        let db = state.db.lock().unwrap();
                        log_interaction(
                            &db,
                            &session_id,
                            "mcp_response",
                            &mcp_server.name,
                            actual_tool_name,
                            serde_json::to_string(&args_json).unwrap_or_default(),
                            result.clone(),
                            None,
                            duration_ms,
                        );
                        result.clone()
                    }
                    Err(e) => {
                        // Log MCP error
                        let db = state.db.lock().unwrap();
                        log_interaction(
                            &db,
                            &session_id,
                            "mcp_call",
                            &mcp_server.name,
                            actual_tool_name,
                            serde_json::to_string(&args_json).unwrap_or_default(),
                            String::new(),
                            Some(e.clone()),
                            duration_ms,
                        );
                        format!("MCP tool error: {e}")
                    }
                };
                result
            } else {
                let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                let mut accessible_skill_roots = active_skill_roots.clone();
                for root in &self_evolution_roots {
                    if !accessible_skill_roots
                        .iter()
                        .any(|existing| existing == root)
                    {
                        accessible_skill_roots.push(root.clone());
                    }
                }
                let start_time = std::time::Instant::now();
                let tool_result = tools::execute_tool(
                    &app,
                    name,
                    args,
                    workspace_dir,
                    &config,
                    &skill_allowed_commands,
                    &active_skill_roots,
                    &accessible_skill_roots,
                    &self_evolution_roots,
                    &self_evolution_files,
                )
                .await;
                let duration_ms = start_time.elapsed().as_millis() as i64;

                // Log tool execution
                let db = state.db.lock().unwrap();
                let is_error = tool_result.starts_with("⛔") || tool_result.starts_with("Error");
                log_interaction(
                    &db,
                    &session_id,
                    if is_error {
                        "tool_error"
                    } else {
                        "tool_output"
                    },
                    "tool_executor",
                    name,
                    serde_json::to_string(&serde_json::from_str::<Value>(args).unwrap_or_default())
                        .unwrap_or_default(),
                    tool_result.clone(),
                    if is_error {
                        Some(tool_result.clone())
                    } else {
                        None
                    },
                    duration_ms,
                );
                drop(db);
                tool_result
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
    log_event(
        &state,
        "INFO",
        format!("chat_completion finished: session_id={}", session_id),
    );
    let _ = app.emit("chat-done", ());
    Ok(())
}
