use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::{AppConfig, AppState, tools, llm_complete::{StreamOptions, stream_llm_request}};

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_skills: Vec<String>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_max_iterations() -> u32 { 10 }
fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOrchestration {
    #[serde(default)]
    pub use_agents: bool,
    #[serde(default)]
    pub auto_configure: bool,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_max_concurrent() -> usize { 3 }
fn default_mode() -> String { "parallel".to_string() }

impl Default for AgentOrchestration {
    fn default() -> Self {
        Self { use_agents: false, auto_configure: false, max_concurrent: 3, mode: "parallel".to_string() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    #[serde(default)]
    pub agents: Vec<SubAgent>,
    #[serde(default)]
    pub orchestration: AgentOrchestration,
}

// ─── Internal planning types ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub agent_id: String,
    pub description: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TaskPlan {
    tasks: Vec<Task>,
    #[serde(default)]
    execution_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskResult {
    pub task_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub status: String,
    pub content: String,
    pub tool_calls_count: u32,
    pub tokens_used: u32,
}

// ─── Config I/O ───────────────────────────────────────────────────────────────

pub fn load_agents_config(path: &Path) -> AgentsConfig {
    if !path.exists() {
        return AgentsConfig::default();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_agents_config_file(path: &Path, config: &AgentsConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_sub_agents(state: State<'_, AppState>) -> Vec<SubAgent> {
    load_agents_config(&state.agents_config_path).agents
}

#[tauri::command]
pub fn save_sub_agent(state: State<'_, AppState>, agent: SubAgent) -> Result<(), String> {
    let mut config = load_agents_config(&state.agents_config_path);
    let mut a = agent;
    if a.id.is_empty() {
        a.id = Uuid::new_v4().to_string();
    }
    if let Some(pos) = config.agents.iter().position(|x| x.id == a.id) {
        config.agents[pos] = a;
    } else {
        config.agents.push(a);
    }
    save_agents_config_file(&state.agents_config_path, &config)
}

#[tauri::command]
pub fn delete_sub_agent(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut config = load_agents_config(&state.agents_config_path);
    config.agents.retain(|a| a.id != id);
    save_agents_config_file(&state.agents_config_path, &config)
}

#[tauri::command]
pub fn get_agent_orchestration(state: State<'_, AppState>) -> AgentOrchestration {
    load_agents_config(&state.agents_config_path).orchestration
}

#[tauri::command]
pub fn save_agent_orchestration(
    state: State<'_, AppState>,
    orchestration: AgentOrchestration,
) -> Result<(), String> {
    let mut config = load_agents_config(&state.agents_config_path);
    config.orchestration = orchestration;
    save_agents_config_file(&state.agents_config_path, &config)
}

// ─── LLM helpers ──────────────────────────────────────────────────────────────

/// Non-streaming LLM call; returns the raw text content.
async fn call_llm_once(
    client: &Client,
    url: &str,
    api_key: &str,
    messages: Vec<Value>,
    model: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    let res = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(res.text().await.unwrap_or_default());
    }
    let parsed: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(parsed["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

// ─── Orchestration entry point ────────────────────────────────────────────────

/// Main orchestration entry point called from llm_complete.rs.
pub async fn orchestrate(
    app: &AppHandle,
    state: &AppState,
    config: &AppConfig,
    messages: &[crate::llm_complete::ChatMessage],
    _session_id: &str,
) -> Result<String, String> {
    let agents_config = load_agents_config(&state.agents_config_path);
    let enabled: Vec<SubAgent> = agents_config
        .agents
        .iter()
        .filter(|a| a.enabled)
        .cloned()
        .collect();

    if enabled.is_empty() {
        return Err("No enabled sub-agents configured".to_string());
    }

    let client = Client::new();
    let url = format!("{}/chat/completions", config.api_base_url.trim_end_matches('/'));
    let model = config.model.clone();
    let orchestration = agents_config.orchestration.clone();

    // Step 1: Optionally auto-configure temporary agents
    let working_agents: Vec<SubAgent> = if orchestration.auto_configure {
        auto_configure_agents(&client, &url, &config.api_key, &model, messages, &enabled)
            .await
            .unwrap_or(enabled)
    } else {
        enabled
    };

    // Step 2: Plan
    let _ = app.emit("agent-plan-start", json!({ "task_count": 0 }));
    let plan =
        plan_tasks(&client, &url, &config.api_key, &model, messages, &working_agents).await?;

    if plan.tasks.is_empty() {
        return Err("Planner produced no tasks".to_string());
    }
    let _ = app.emit("agent-plan-start", json!({ "task_count": plan.tasks.len() }));

    // Step 3: Execute
    let use_parallel = orchestration.mode != "sequential"
        && plan.execution_mode != "sequential";
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let results = execute_tasks(
        app,
        &client,
        &url,
        config,
        &working_agents,
        plan.tasks,
        workspace_dir,
        use_parallel,
        orchestration.max_concurrent,
        &state.chat_cancelled,
    )
    .await;

    // Step 4: Aggregate
    let _ = app.emit("agent-aggregate-start", ());
    aggregate_results(app, &client, &url, &config.api_key, &model, messages, results).await
}

// ─── Auto-configure ───────────────────────────────────────────────────────────

async fn auto_configure_agents(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    existing: &[SubAgent],
) -> Result<Vec<SubAgent>, String> {
    let existing_desc = existing
        .iter()
        .map(|a| format!("  - id={:?} name={:?} description={:?}", a.id, a.name, a.description))
        .collect::<Vec<_>>()
        .join("\n");

    let user_query = messages.last().map(|m| m.content.as_str()).unwrap_or("");
    let system = format!(
        "You are an AI orchestrator. Based on the user request, decide which agents to use.\n\
        Existing agents:\n{existing_desc}\n\n\
        Option A — use existing: return JSON: {{\"strategy\":\"existing\",\"ids\":[\"id1\",\"id2\"]}}\n\
        Option B — define new temporary agents: return JSON:\n\
        {{\"strategy\":\"new\",\"agents\":[\
          {{\"id\":\"tmp-1\",\"name\":\"...\",\"description\":\"...\",\"system_prompt\":\"...\",\
            \"allowed_tools\":[\"file_actions\"],\"max_iterations\":5,\"enabled\":true}}\
        ]}}\n\
        Return ONLY the JSON object, no markdown, no extra text.",
    );

    let raw = call_llm_once(
        client, url, api_key,
        vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": user_query}),
        ],
        model, 2048,
    ).await?;

    // Strip markdown fences if present
    let json_str = raw.trim();
    let json_str = json_str
        .strip_prefix("```json").unwrap_or(json_str)
        .strip_prefix("```").unwrap_or(json_str)
        .strip_suffix("```").unwrap_or(json_str)
        .trim();

    let parsed: Value = serde_json::from_str(json_str).map_err(|e| e.to_string())?;
    match parsed["strategy"].as_str() {
        Some("existing") => {
            let ids: Vec<&str> = parsed["ids"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            Ok(existing.iter().filter(|a| ids.contains(&a.id.as_str())).cloned().collect())
        }
        Some("new") => {
            let agents: Vec<SubAgent> = parsed["agents"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| serde_json::from_value::<SubAgent>(v.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            if agents.is_empty() { Ok(existing.to_vec()) } else { Ok(agents) }
        }
        _ => Ok(existing.to_vec()),
    }
}

// ─── Planning ─────────────────────────────────────────────────────────────────

async fn plan_tasks(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    agents: &[SubAgent],
) -> Result<TaskPlan, String> {
    let agents_desc = agents
        .iter()
        .map(|a| format!("  - id={:?} name={:?} description={:?}", a.id, a.name, a.description))
        .collect::<Vec<_>>()
        .join("\n");

    // Build a short conversation summary (last 3 messages, max 600 chars each)
    let conversation = messages
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|m| {
            let limit = m.content.len().min(600);
            let mut end = limit;
            while end > 0 && !m.content.is_char_boundary(end) {
                end -= 1;
            }
            let preview = &m.content[..end];
            format!("[{}]: {}", m.role, preview)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system = format!(
        "You are a task planning expert. Decompose the user request into tasks for sub-agents.\n\
        Available agents:\n{agents_desc}\n\n\
        Return ONLY valid JSON — no markdown, no extra text:\n\
        {{\"tasks\":[{{\"id\":\"t1\",\"agent_id\":\"<id>\",\"description\":\"...\",\
          \"context\":\"...\",\"dependencies\":[]}}],\
          \"execution_mode\":\"parallel\"}}\n\
        Rules:\n\
        - execution_mode: \"parallel\" when tasks are independent, \"sequential\" when order matters\n\
        - dependencies: list of task ids that must finish before this one\n\
        - Every agent_id must match one of the available agent ids above\n\
        - Keep descriptions brief and actionable"
    );

    let raw = call_llm_once(
        client, url, api_key,
        vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": format!("Conversation:\n{conversation}\n\nCreate a task plan.")}),
        ],
        model, 2048,
    ).await?;

    let json_str = raw.trim();
    let json_str = json_str
        .strip_prefix("```json").unwrap_or(json_str)
        .strip_prefix("```").unwrap_or(json_str)
        .strip_suffix("```").unwrap_or(json_str)
        .trim();

    serde_json::from_str::<TaskPlan>(json_str)
        .map_err(|e| format!("Plan parse error: {e}\nRaw: {raw}"))
}

// ─── Execution ────────────────────────────────────────────────────────────────

async fn execute_tasks(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    use_parallel: bool,
    max_concurrent: usize,
    cancelled: &AtomicBool,
) -> Vec<TaskResult> {
    if use_parallel {
        execute_parallel(app, client, url, config, agents, tasks, workspace_dir, max_concurrent, cancelled).await
    } else {
        execute_sequential(app, client, url, config, agents, tasks, workspace_dir, cancelled).await
    }
}

async fn execute_parallel(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    max_concurrent: usize,
    cancelled: &AtomicBool,
) -> Vec<TaskResult> {
    use std::collections::HashMap;
    use tokio::task::JoinSet;

    let mut completed: HashMap<String, ()> = HashMap::new();
    let mut pending = tasks;
    let mut results: Vec<TaskResult> = Vec::new();

    while !pending.is_empty() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        // Collect tasks whose dependencies are all satisfied
        let ready: Vec<Task> = pending
            .iter()
            .filter(|t| t.dependencies.iter().all(|dep| completed.contains_key(dep)))
            .cloned()
            .collect();

        if ready.is_empty() {
            // Dependency deadlock — run all remaining to avoid hanging
            break;
        }

        pending.retain(|t| !ready.iter().any(|r| r.id == t.id));

        // Process ready tasks in batches of max_concurrent
        for batch in ready.chunks(max_concurrent.max(1)) {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }

            let mut join_set: JoinSet<TaskResult> = JoinSet::new();

            for task in batch {
                let agent = match agents.iter().find(|a| a.id == task.agent_id) {
                    Some(a) => a.clone(),
                    None => continue,
                };

                let app_c = app.clone();
                let client_c = client.clone();
                let url_c = url.to_string();
                let config_c = config.clone();
                let task_c = task.clone();
                let wd_c = workspace_dir.clone();
                let is_cancelled = cancelled.load(Ordering::SeqCst);

                join_set.spawn(async move {
                    if is_cancelled {
                        return TaskResult {
                            task_id: task_c.id,
                            agent_id: task_c.agent_id,
                            agent_name: agent.name,
                            status: "skipped".to_string(),
                            content: "Cancelled".to_string(),
                            tool_calls_count: 0,
                            tokens_used: 0,
                        };
                    }
                    run_sub_agent(&app_c, &client_c, &url_c, &config_c, &agent, &task_c, wd_c).await
                });
            }

            while let Some(Ok(res)) = join_set.join_next().await {
                completed.insert(res.task_id.clone(), ());
                results.push(res);
            }
        }
    }

    results
}

async fn execute_sequential(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    cancelled: &AtomicBool,
) -> Vec<TaskResult> {
    let mut results = Vec::new();
    for task in tasks {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let agent = match agents.iter().find(|a| a.id == task.agent_id) {
            Some(a) => a,
            None => continue,
        };
        let res = run_sub_agent(app, client, url, config, agent, &task, workspace_dir.clone()).await;
        results.push(res);
    }
    results
}

// ─── Sub-agent loop ───────────────────────────────────────────────────────────

pub async fn run_sub_agent(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agent: &SubAgent,
    task: &Task,
    workspace_dir: PathBuf,
) -> TaskResult {
    let _ = app.emit(
        "agent-task-start",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "description": task.description,
        }),
    );

    let model = agent.model.as_deref().unwrap_or(&config.model);
    let max_tokens = agent.max_tokens.unwrap_or(8192);
    let tools_list = tools::get_all_tools(&agent.allowed_tools);
    let cancelled = AtomicBool::new(false);
    let mut total_tokens: u32 = 0;
    let mut tool_calls_count: u32 = 0;

    let task_system = format!(
        "{}\n\nYou are executing a specific task. Be concise and thorough.\nTask: {}\nContext: {}",
        agent.system_prompt, task.description, task.context
    );

    let mut messages: Vec<Value> = vec![
        json!({"role": "system", "content": task_system}),
        json!({"role": "user", "content": format!("Execute this task:\n{}\n\n{}", task.description, task.context)}),
    ];

    for _iteration in 0..agent.max_iterations {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        let mut req_body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(temp) = agent.temperature {
            req_body["temperature"] = json!(temp);
        }
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
            req_body["tool_choice"] = json!("auto");
        }

        let stream_result = stream_llm_request(
            app, client, url, &config.api_key, req_body, &cancelled,
            StreamOptions {
                token_event: "agent-task-token",
                task_id: Some(&task.id),
                emit_reasoning: false,
                emit_usage: false,
            },
        )
        .await
        .map(|sr| (sr.content, sr.tool_calls, sr.prompt_tokens + sr.completion_tokens));

        match stream_result {
            Err(e) => {
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": e }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: format!("Error: {e}"),
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
            Ok((content, agent_tool_calls, tokens)) => {
                total_tokens += tokens;

                if agent_tool_calls.is_empty() {
                    // Done — no more tool calls
                    let summary: String = content.chars().take(200).collect();
                    let _ = app.emit(
                        "agent-task-done",
                        json!({
                            "task_id": task.id,
                            "agent_id": agent.id,
                            "agent_name": agent.name,
                            "summary": summary,
                        }),
                    );
                    return TaskResult {
                        task_id: task.id.clone(),
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        status: "success".to_string(),
                        content,
                        tool_calls_count,
                        tokens_used: total_tokens,
                    };
                }

                // Append assistant turn with tool calls
                let assistant_tcs: Vec<Value> = agent_tool_calls
                    .iter()
                    .map(|(id, name, args)| json!({
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": args },
                    }))
                    .collect();
                messages.push(json!({ "role": "assistant", "content": null, "tool_calls": assistant_tcs }));

                // Execute each tool call
                for (id, name, args) in &agent_tool_calls {
                    tool_calls_count += 1;
                    let result = tools::execute_tool(
                        app, name, args,
                        workspace_dir.clone(),
                        config,
                        &[],
                        &[],
                    )
                    .await;
                    messages.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
                }
            }
        }
    }

    // Max iterations reached — return whatever was last generated
    let last_content = messages
        .iter()
        .rev()
        .find_map(|m| m["content"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()))
        .unwrap_or_else(|| "Max iterations reached without final answer.".to_string());

    let _ = app.emit(
        "agent-task-done",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "summary": "Max iterations reached",
        }),
    );
    TaskResult {
        task_id: task.id.clone(),
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        status: "success".to_string(),
        content: last_content,
        tool_calls_count,
        tokens_used: total_tokens,
    }
}

// ─── Aggregation ──────────────────────────────────────────────────────────────

async fn aggregate_results(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    original_messages: &[crate::llm_complete::ChatMessage],
    results: Vec<TaskResult>,
) -> Result<String, String> {
    if results.is_empty() {
        return Ok("No results returned from sub-agents.".to_string());
    }

    let results_text = results
        .iter()
        .map(|r| {
            format!(
                "### {} (Agent: {}, Status: {}, Tools used: {}, Tokens: {})\n\n{}",
                r.task_id, r.agent_name, r.status, r.tool_calls_count, r.tokens_used, r.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let user_query = original_messages
        .last()
        .map(|m| m.content.as_str())
        .unwrap_or("the user request");

    let agg_messages = vec![
        json!({
            "role": "system",
            "content": "You are a results synthesizer. Multiple specialized agents have completed their tasks. \
                Synthesize their outputs into one coherent, well-structured final response. \
                Avoid redundancy. Present insights clearly."
        }),
        json!({
            "role": "user",
            "content": format!(
                "Original request: {user_query}\n\nSub-agent results:\n\n{results_text}\n\n\
                Please synthesize these into a comprehensive final answer."
            )
        }),
    ];

    let req_body = json!({
        "model": model,
        "max_tokens": 4096,
        "messages": agg_messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    let cancelled = AtomicBool::new(false);
    let sr = stream_llm_request(app, client, url, api_key, req_body, &cancelled, StreamOptions {
        token_event: "chat-token",
        task_id: None,
        emit_reasoning: false,
        emit_usage: false,
    }).await?;

    Ok(sr.content)
}
