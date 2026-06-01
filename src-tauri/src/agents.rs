use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tiktoken_rs::cl100k_base;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::{
    llm_complete::{
        apply_completion_token_limit, extract_upstream_error_message, stream_llm_request,
        StreamOptions,
    },
    tools, AppConfig, AppState,
};

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

fn default_max_iterations() -> u32 {
    10
}
fn default_true() -> bool {
    true
}

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

fn default_max_concurrent() -> usize {
    3
}
fn default_mode() -> String {
    "parallel".to_string()
}

impl Default for AgentOrchestration {
    fn default() -> Self {
        Self {
            use_agents: false,
            auto_configure: false,
            max_concurrent: 3,
            mode: "parallel".to_string(),
        }
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

const AGENT_CONTEXT_COMPRESSION_THRESHOLD: f32 = 0.8;
const AGENT_WORKING_MEMORY_MESSAGES: usize = 8;
const AGENT_MIN_RECENT_MESSAGES: usize = 4;
const AGENT_SUMMARY_MAX_TOKENS: u32 = 768;
const AGENT_DEFAULT_CONTEXT_WINDOW: usize = 131_072;
const AGENT_CONTINUE_PROMPT: &str = "Continue autonomously. Re-read the injected mission snapshot and active task list. Use the external task tools to add, update, or complete tasks, and only call mark_mission_accomplished when the mission is truly done.";

#[derive(Debug, Clone, Serialize)]
pub struct MissionTaskRecord {
    pub task_id: String,
    pub name: String,
    pub description: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissionStateView {
    pub mission_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub root_task_description: String,
    pub root_task_context: String,
    pub status: String,
    pub mission_accomplished: bool,
    pub episodic_summary: String,
    pub final_report: String,
    pub active_tasks: Vec<MissionTaskRecord>,
    pub active_task_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
struct MissionStateSnapshot {
    mission_id: String,
    root_task_description: String,
    root_task_context: String,
    episodic_summary: String,
    mission_accomplished: bool,
    final_report: String,
    active_tasks: Vec<MissionTaskRecord>,
}

fn mission_id_for_task(task: &Task) -> &str {
    &task.id
}

fn normalize_task_name(name: &str, fallback: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        fallback.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_task_status(status: &str) -> Result<&'static str, String> {
    match status.trim() {
        "pending" => Ok("pending"),
        "in_progress" => Ok("in_progress"),
        "completed" => Ok("completed"),
        other => Err(format!(
            "Unsupported task status '{}'. Expected one of: pending, in_progress, completed.",
            other
        )),
    }
}

pub fn initialize_mission_state(
    db: &rusqlite::Connection,
    session_id: &str,
    agent: &SubAgent,
    task: &Task,
) -> Result<(), String> {
    let mission_id = mission_id_for_task(task);
    db.execute(
        "INSERT INTO agent_missions (mission_id, session_id, agent_id, root_task_description, root_task_context) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(mission_id) DO UPDATE SET \
           session_id = excluded.session_id, \
           agent_id = excluded.agent_id, \
           root_task_description = excluded.root_task_description, \
           root_task_context = excluded.root_task_context, \
           updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![mission_id, session_id, agent.id, task.description, task.context],
    )
    .map_err(|e| e.to_string())?;

    db.execute(
        "INSERT INTO agent_tasks (task_id, mission_id, name, description, status) \
         VALUES (?1, ?2, ?3, ?4, 'in_progress') \
         ON CONFLICT(task_id) DO UPDATE SET \
           mission_id = excluded.mission_id, \
           name = excluded.name, \
           description = excluded.description, \
           status = CASE WHEN agent_tasks.status = 'completed' THEN agent_tasks.status ELSE 'in_progress' END, \
           updated_at = CURRENT_TIMESTAMP, \
           completed_at = CASE WHEN agent_tasks.status = 'completed' THEN agent_tasks.completed_at ELSE NULL END",
        rusqlite::params![
            mission_id,
            mission_id,
            normalize_task_name(&task.description, "Primary task"),
            task.description,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

fn get_mission_task(
    db: &rusqlite::Connection,
    mission_id: &str,
    task_id: &str,
) -> Result<MissionTaskRecord, String> {
    let mut stmt = db
        .prepare(
            "SELECT task_id, name, description, status \
             FROM agent_tasks WHERE mission_id = ?1 AND task_id = ?2 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_row(rusqlite::params![mission_id, task_id], |row| {
        Ok(MissionTaskRecord {
            task_id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            status: row.get(3)?,
        })
    })
    .map_err(|e| e.to_string())
}

pub fn add_mission_task(
    db: &rusqlite::Connection,
    mission_id: &str,
    name: &str,
    description: &str,
) -> Result<MissionTaskRecord, String> {
    let trimmed_description = description.trim();
    if trimmed_description.is_empty() {
        return Err("Task description cannot be empty.".to_string());
    }

    let task = MissionTaskRecord {
        task_id: Uuid::new_v4().to_string(),
        name: normalize_task_name(name, trimmed_description),
        description: trimmed_description.to_string(),
        status: "pending".to_string(),
    };

    db.execute(
        "INSERT INTO agent_tasks (task_id, mission_id, name, description, status) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            task.task_id,
            mission_id,
            task.name,
            task.description,
            task.status,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(task)
}

pub fn update_mission_task_status(
    db: &rusqlite::Connection,
    mission_id: &str,
    task_id: &str,
    status: &str,
) -> Result<MissionTaskRecord, String> {
    let normalized_status = normalize_task_status(status)?;
    let updated = db
        .execute(
            "UPDATE agent_tasks SET \
               status = ?1, \
               updated_at = CURRENT_TIMESTAMP, \
               completed_at = CASE WHEN ?1 = 'completed' THEN CURRENT_TIMESTAMP ELSE NULL END \
             WHERE mission_id = ?2 AND task_id = ?3",
            rusqlite::params![normalized_status, mission_id, task_id],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Err(format!(
            "Task '{}' was not found in mission '{}'.",
            task_id, mission_id
        ));
    }

    get_mission_task(db, mission_id, task_id)
}

pub fn get_active_mission_tasks(
    db: &rusqlite::Connection,
    mission_id: &str,
) -> Result<Vec<MissionTaskRecord>, String> {
    let mut stmt = db
        .prepare(
            "SELECT task_id, name, description, status \
             FROM agent_tasks \
             WHERE mission_id = ?1 AND status != 'completed' \
             ORDER BY CASE status WHEN 'in_progress' THEN 0 ELSE 1 END, created_at, task_id",
        )
        .map_err(|e| e.to_string())?;

    let tasks = stmt
        .query_map(rusqlite::params![mission_id], |row| {
            Ok(MissionTaskRecord {
                task_id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(tasks)
}

pub fn save_mission_summary(
    db: &rusqlite::Connection,
    mission_id: &str,
    summary: &str,
) -> Result<(), String> {
    db.execute(
        "UPDATE agent_missions \
         SET episodic_summary = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1",
        rusqlite::params![mission_id, summary.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn save_mission_final_report(
    db: &rusqlite::Connection,
    mission_id: &str,
    final_report: &str,
) -> Result<(), String> {
    db.execute(
        "UPDATE agent_missions \
         SET final_report = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1",
        rusqlite::params![mission_id, final_report.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn mark_mission_accomplished(
    db: &rusqlite::Connection,
    mission_id: &str,
    final_report: Option<&str>,
) -> Result<(), String> {
    let report = final_report.unwrap_or("").trim().to_string();
    let updated = db
        .execute(
            "UPDATE agent_missions SET \
               status = 'completed', \
               mission_accomplished = 1, \
               final_report = CASE WHEN ?2 != '' THEN ?2 ELSE final_report END, \
               updated_at = CURRENT_TIMESTAMP \
             WHERE mission_id = ?1",
            rusqlite::params![mission_id, report],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Err(format!("Mission '{}' was not found.", mission_id));
    }

    let _ = db.execute(
        "UPDATE agent_tasks SET \
           status = 'completed', \
           updated_at = CURRENT_TIMESTAMP, \
           completed_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1 AND task_id = ?1 AND status != 'completed'",
        rusqlite::params![mission_id],
    );

    Ok(())
}

fn load_mission_state(
    db: &rusqlite::Connection,
    mission_id: &str,
) -> Result<MissionStateSnapshot, String> {
    let mut stmt = db
        .prepare(
            "SELECT mission_id, root_task_description, root_task_context, episodic_summary, \
                    mission_accomplished, final_report \
             FROM agent_missions WHERE mission_id = ?1 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;

    let mut snapshot = stmt
        .query_row(rusqlite::params![mission_id], |row| {
            Ok(MissionStateSnapshot {
                mission_id: row.get(0)?,
                root_task_description: row.get(1)?,
                root_task_context: row.get(2)?,
                episodic_summary: row.get(3)?,
                mission_accomplished: row.get::<_, i64>(4)? != 0,
                final_report: row.get(5)?,
                active_tasks: Vec::new(),
            })
        })
        .map_err(|e| e.to_string())?;

    snapshot.active_tasks = get_active_mission_tasks(db, mission_id)?;
    Ok(snapshot)
}

fn build_active_tasks_snapshot(tasks: &[MissionTaskRecord]) -> String {
    if tasks.is_empty() {
        return "- none".to_string();
    }

    tasks.iter()
        .map(|task| {
            format!(
                "- [{}] {} (id: {})\\n  {}",
                task.status, task.name, task.task_id, task.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(max_chars).collect::<String>()
}

fn summarize_message_for_memory(message: &Value) -> Option<String> {
    let role = message.get("role").and_then(|value| value.as_str()).unwrap_or("unknown");

    if let Some(tool_calls) = message.get("tool_calls").and_then(|value| value.as_array()) {
        let tool_names = tool_calls
            .iter()
            .filter_map(|call| call.get("function"))
            .filter_map(|function| function.get("name"))
            .filter_map(|name| name.as_str())
            .collect::<Vec<_>>();
        if !tool_names.is_empty() {
            return Some(format!("- assistant requested tools: {}", tool_names.join(", ")));
        }
    }

    let content = message
        .get("content")
        .and_then(|value| value.as_str())
        .map(|value| truncate_for_summary(value, 400))
        .unwrap_or_default();

    if content.is_empty() {
        None
    } else {
        Some(format!("- {role}: {content}"))
    }
}

fn format_memory_excerpt(messages: &[Value]) -> String {
    messages
        .iter()
        .filter_map(summarize_message_for_memory)
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_agent_system_prompt(
    agent: &SubAgent,
    mission: &MissionStateSnapshot,
    self_evolution_context: &str,
) -> String {
    let episodic_summary = if mission.episodic_summary.trim().is_empty() {
        "(empty)".to_string()
    } else {
        mission.episodic_summary.clone()
    };

    let completion_state = if mission.mission_accomplished {
        "Mission completion flag is already set. Unless a final cleanup step is still required, produce the final answer."
    } else {
        "Mission completion flag is not set. Keep executing until you explicitly call mark_mission_accomplished."
    };

    format!(
        "{}\n\nAutonomous execution sub-agent. External mission state is authoritative for task tracking.\n\
         Mission ID: {}\nPrimary task: {}\nContext: {}\n{}\n\
         Active tasks:\n{}\nEpisodic memory:\n{}\n\
         Rules: track tasks via add_task/update_task_status/get_active_tasks/mark_mission_accomplished \
         (not chat history); keep outputs execution-focused; create tasks for branches; \
         clear active tasks before finishing.{}",
        agent.system_prompt,
        mission.mission_id,
        mission.root_task_description,
        mission.root_task_context,
        completion_state,
        build_active_tasks_snapshot(&mission.active_tasks),
        episodic_summary,
        self_evolution_context,
    )
}

fn build_agent_messages(system_prompt: &str, working_memory: &[Value]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(working_memory.len() + 1);
    messages.push(json!({ "role": "system", "content": system_prompt }));
    messages.extend(working_memory.iter().cloned());
    messages
}

fn estimate_message_tokens(messages: &[Value]) -> usize {
    if let Ok(encoding) = cl100k_base() {
        return messages
            .iter()
            .map(|message| {
                let serialized = serde_json::to_string(message).unwrap_or_default();
                encoding.encode_with_special_tokens(&serialized).len() + 4
            })
            .sum::<usize>()
            + 2;
    }

    messages
        .iter()
        .map(|message| serde_json::to_string(message).unwrap_or_default().len() / 4)
        .sum()
}

async fn summarize_agent_memory(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    current_summary: &str,
    messages_to_compress: &[Value],
) -> Result<String, String> {
    let excerpt = format_memory_excerpt(messages_to_compress);
    if excerpt.trim().is_empty() {
        return Ok(current_summary.trim().to_string());
    }

    let raw = call_llm_once(
        client,
        url,
        api_key,
        vec![
            json!({
                "role": "system",
                "content": "You compress autonomous agent working memory. Preserve durable facts, decisions, blockers, evidence, file paths, and unresolved threads. Do not reproduce full tool transcripts or exact TODO lists because tasks are tracked externally. Return plain text bullet points only."
            }),
            json!({
                "role": "user",
                "content": format!(
                    "Current episodic summary:\n{}\n\nNew interaction excerpt to compress:\n{}\n\nMerge them into one concise episodic memory.",
                    if current_summary.trim().is_empty() {
                        "(empty)"
                    } else {
                        current_summary.trim()
                    },
                    excerpt
                )
            }),
        ],
        model,
        AGENT_SUMMARY_MAX_TOKENS,
    )
    .await?;

    Ok(raw.trim().to_string())
}

async fn compress_working_memory_if_needed(
    client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    working_memory: &mut Vec<Value>,
    episodic_summary: &mut String,
    context_budget: usize,
) -> Result<bool, String> {
    let candidate_messages = build_agent_messages(system_prompt, working_memory);
    let estimated_tokens = estimate_message_tokens(&candidate_messages);
    let threshold = (context_budget as f32 * AGENT_CONTEXT_COMPRESSION_THRESHOLD) as usize;
    let should_compress = working_memory.len() > AGENT_WORKING_MEMORY_MESSAGES
        || estimated_tokens >= threshold.max(1);

    if !should_compress {
        return Ok(false);
    }

    let keep_count = if working_memory.len() > AGENT_WORKING_MEMORY_MESSAGES {
        AGENT_WORKING_MEMORY_MESSAGES
    } else {
        working_memory.len().saturating_sub(AGENT_MIN_RECENT_MESSAGES)
    };

    if working_memory.len() <= keep_count || keep_count == 0 {
        return Ok(false);
    }

    let split_index = working_memory.len() - keep_count;
    let older_messages: Vec<Value> = working_memory.drain(..split_index).collect();
    let merged_summary = summarize_agent_memory(
        client,
        url,
        api_key,
        model,
        episodic_summary,
        &older_messages,
    )
    .await?;

    *episodic_summary = merged_summary;
    Ok(true)
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

#[tauri::command]
pub fn list_agent_missions(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<MissionStateView>, String> {
    #[derive(Debug)]
    struct MissionRow {
        mission_id: String,
        session_id: String,
        agent_id: String,
        root_task_description: String,
        root_task_context: String,
        status: String,
        mission_accomplished: bool,
        episodic_summary: String,
        final_report: String,
        created_at: String,
        updated_at: String,
    }

    let agent_names = load_agents_config(&state.agents_config_path)
        .agents
        .into_iter()
        .map(|agent| (agent.id, agent.name))
        .collect::<std::collections::HashMap<_, _>>();

    let db = state.db.lock().unwrap();
    let rows: Vec<MissionRow> = {
        let mut stmt = db
            .prepare(
                "SELECT mission_id, COALESCE(session_id, ''), agent_id, \
                        root_task_description, root_task_context, status, \
                        mission_accomplished, episodic_summary, final_report, \
                        COALESCE(created_at, ''), COALESCE(updated_at, '') \
                 FROM agent_missions \
                 WHERE session_id = ?1 \
                 ORDER BY updated_at DESC, created_at DESC, mission_id DESC",
            )
            .map_err(|e| e.to_string())?;

        let mapped_rows = stmt.query_map(rusqlite::params![session_id], |row| {
            Ok(MissionRow {
                mission_id: row.get(0)?,
                session_id: row.get(1)?,
                agent_id: row.get(2)?,
                root_task_description: row.get(3)?,
                root_task_context: row.get(4)?,
                status: row.get(5)?,
                mission_accomplished: row.get::<_, i64>(6)? != 0,
                episodic_summary: row.get(7)?,
                final_report: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;

        mapped_rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };

    rows.into_iter()
        .map(|row| {
            let active_tasks = get_active_mission_tasks(&db, &row.mission_id)?;
            Ok(MissionStateView {
                mission_id: row.mission_id,
                session_id: row.session_id,
                agent_id: row.agent_id.clone(),
                agent_name: agent_names
                    .get(&row.agent_id)
                    .cloned()
                    .unwrap_or_else(|| row.agent_id.clone()),
                root_task_description: row.root_task_description,
                root_task_context: row.root_task_context,
                status: row.status,
                mission_accomplished: row.mission_accomplished,
                episodic_summary: row.episodic_summary,
                final_report: row.final_report,
                active_task_count: active_tasks.len(),
                active_tasks,
                created_at: row.created_at,
                updated_at: row.updated_at,
            })
        })
        .collect()
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
    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    apply_completion_token_limit(&mut body, max_tokens);
    let res = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(extract_upstream_error_message(&body));
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
    session_id: &str,
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
    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
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
    let plan = plan_tasks(
        &client,
        &url,
        &config.api_key,
        &model,
        messages,
        &working_agents,
    )
    .await?;

    if plan.tasks.is_empty() {
        return Err("Planner produced no tasks".to_string());
    }
    let _ = app.emit(
        "agent-plan-start",
        json!({ "task_count": plan.tasks.len() }),
    );

    // Step 3: Execute
    let use_parallel = orchestration.mode != "sequential" && plan.execution_mode != "sequential";
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let skill_access_roots =
        crate::skills::collect_self_evolution_roots(&state.skills_dir, Some(&workspace_dir), config.self_evolution_mode);
    let results = execute_tasks(
        app,
        &client,
        &url,
        config,
        session_id,
        &working_agents,
        plan.tasks,
        workspace_dir,
        skill_access_roots,
        use_parallel,
        orchestration.max_concurrent,
        &state.chat_cancelled,
    )
    .await;

    // Step 4: Aggregate
    let _ = app.emit("agent-aggregate-start", ());
    aggregate_results(
        app,
        &client,
        &url,
        &config.api_key,
        &model,
        messages,
        results,
    )
    .await
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
        .map(|a| {
            format!(
                "  - id={:?} name={:?} description={:?}",
                a.id, a.name, a.description
            )
        })
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
        client,
        url,
        api_key,
        vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": user_query}),
        ],
        model,
        2048,
    )
    .await?;

    // Strip markdown fences if present
    let json_str = raw.trim();
    let json_str = json_str
        .strip_prefix("```json")
        .or_else(|| json_str.strip_prefix("```"))
        .unwrap_or(json_str)
        .trim();
    let json_str = json_str.strip_suffix("```").unwrap_or(json_str).trim();

    let parsed: Value = serde_json::from_str(json_str).map_err(|e| e.to_string())?;
    match parsed["strategy"].as_str() {
        Some("existing") => {
            let ids: Vec<&str> = parsed["ids"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            Ok(existing
                .iter()
                .filter(|a| ids.contains(&a.id.as_str()))
                .cloned()
                .collect())
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
            if agents.is_empty() {
                Ok(existing.to_vec())
            } else {
                Ok(agents)
            }
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
        .map(|a| {
            format!(
                "  - id={:?} name={:?} description={:?}",
                a.id, a.name, a.description
            )
        })
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
        .strip_prefix("```json")
        .or_else(|| json_str.strip_prefix("```"))
        .unwrap_or(json_str)
        .trim();
    let json_str = json_str.strip_suffix("```").unwrap_or(json_str).trim();

    serde_json::from_str::<TaskPlan>(json_str)
        .map_err(|e| format!("Plan parse error: {e}\nRaw: {raw}"))
}

// ─── Execution ────────────────────────────────────────────────────────────────

async fn execute_tasks(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    skill_access_roots: Vec<PathBuf>,
    use_parallel: bool,
    max_concurrent: usize,
    cancelled: &AtomicBool,
) -> Vec<TaskResult> {
    if use_parallel {
        execute_parallel(
            app,
            client,
            url,
            config,
            session_id,
            agents,
            tasks,
            workspace_dir,
            skill_access_roots,
            max_concurrent,
            cancelled,
        )
        .await
    } else {
        execute_sequential(
            app,
            client,
            url,
            config,
            session_id,
            agents,
            tasks,
            workspace_dir,
            skill_access_roots,
            cancelled,
        )
        .await
    }
}

async fn execute_parallel(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    skill_access_roots: Vec<PathBuf>,
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
                let session_id_c = session_id.to_string();
                let task_c = task.clone();
                let wd_c = workspace_dir.clone();
                let skill_roots_c = skill_access_roots.clone();
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
                    run_sub_agent(
                        &app_c,
                        &client_c,
                        &url_c,
                        &config_c,
                        &session_id_c,
                        &agent,
                        &task_c,
                        wd_c,
                        skill_roots_c,
                    )
                    .await
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
    session_id: &str,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    skill_access_roots: Vec<PathBuf>,
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
        let res = run_sub_agent(
            app,
            client,
            url,
            config,
            session_id,
            agent,
            &task,
            workspace_dir.clone(),
            skill_access_roots.clone(),
        )
        .await;
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
    session_id: &str,
    agent: &SubAgent,
    task: &Task,
    workspace_dir: PathBuf,
    skill_access_roots: Vec<PathBuf>,
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
    let mut tools_list = tools::get_all_tools(&agent.allowed_tools);
    tools_list.extend(tools::get_agent_task_tools());
    let cancelled = AtomicBool::new(false);
    let mut total_tokens: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mission_id = mission_id_for_task(task).to_string();
    let context_budget = config
        .model_settings
        .max_tokens
        .filter(|limit| *limit > 0)
        .map(|limit| limit as usize)
        .unwrap_or(AGENT_DEFAULT_CONTEXT_WINDOW);

    let self_evolution_context = if skill_access_roots.is_empty() {
        String::new()
    } else {
        let roots = skill_access_roots
            .iter()
            .map(|root| format!("- {}", root.display()))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\n\nSelf-evolution: skill roots available:\n{roots}\n\
             Use `file_actions` for skill edits (auto-creates `.bak.<n>` backups). \
             Workspace is the default root; use skill paths (`./skills/<name>/...` or \
             `app_data/skills/<name>/...`) only when the task requires it. \
             Reuse exact paths returned by tools."
        )
    };

    let task_system = format!(
        "Execute this mission.\nPrimary task: {}\nContext: {}\nTreat the external mission snapshot as authoritative and keep the explicit task list updated through tools.",
        task.description, task.context
    );

    {
        let state = app.state::<AppState>();
        let db = match state.db.lock() {
            Ok(db) => db,
            Err(err) => {
                let error = format!("Failed to lock database for mission initialization: {err}");
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": error }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: error,
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
        };

        if let Err(err) = initialize_mission_state(&db, session_id, agent, task) {
            let _ = app.emit(
                "agent-task-error",
                json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
            );
            return TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: format!("Error: {}", err),
                tool_calls_count,
                tokens_used: total_tokens,
            };
        }
    }
    let _ = app.emit(
        "agent-task-state",
        json!({ "mission_id": mission_id, "session_id": session_id, "status": "running", "event": "initialized" }),
    );

    let mut working_memory: Vec<Value> = vec![json!({
        "role": "user",
        "content": task_system,
    })];
    let max_iterations = (agent.max_iterations > 0).then_some(agent.max_iterations);
    let mut iterations: u32 = 0;

    loop {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        if let Some(limit) = max_iterations {
            if iterations >= limit {
                break;
            }
        }
        iterations = iterations.saturating_add(1);

        let mut mission_snapshot = {
            let state = app.state::<AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => {
                    let error = format!("Failed to lock database while loading mission state: {err}");
                    let _ = app.emit(
                        "agent-task-error",
                        json!({ "task_id": task.id, "agent_id": agent.id, "error": error }),
                    );
                    return TaskResult {
                        task_id: task.id.clone(),
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        status: "error".to_string(),
                        content: error,
                        tool_calls_count,
                        tokens_used: total_tokens,
                    };
                }
            };

            match load_mission_state(&db, &mission_id) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    let _ = app.emit(
                        "agent-task-error",
                        json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
                    );
                    return TaskResult {
                        task_id: task.id.clone(),
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        status: "error".to_string(),
                        content: format!("Error: {}", err),
                        tool_calls_count,
                        tokens_used: total_tokens,
                    };
                }
            }
        };

        let system_prompt = build_agent_system_prompt(agent, &mission_snapshot, &self_evolution_context);

        match compress_working_memory_if_needed(
            client,
            url,
            &config.api_key,
            model,
            &system_prompt,
            &mut working_memory,
            &mut mission_snapshot.episodic_summary,
            context_budget,
        )
        .await
        {
            Ok(true) => {
                let state = app.state::<AppState>();
                if let Ok(db) = state.db.lock() {
                    let _ = save_mission_summary(&db, &mission_id, &mission_snapshot.episodic_summary);
                };
                let _ = app.emit(
                    "agent-task-state",
                    json!({ "mission_id": mission_id, "session_id": session_id, "summary_updated": true }),
                );
            }
            Ok(false) => {}
            Err(err) => {
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: format!("Error: {}", err),
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
        }

        let system_prompt = build_agent_system_prompt(agent, &mission_snapshot, &self_evolution_context);
        let messages = build_agent_messages(&system_prompt, &working_memory);

        let mut req_body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        apply_completion_token_limit(&mut req_body, max_tokens);
        if let Some(temp) = agent.temperature {
            req_body["temperature"] = json!(temp);
        }
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
            req_body["tool_choice"] = json!("auto");
        }

        let stream_result = stream_llm_request(
            app,
            client,
            url,
            &config.api_key,
            req_body,
            &cancelled,
            StreamOptions {
                token_event: "agent-task-token",
                task_id: Some(&task.id),
                emit_reasoning: false,
                emit_usage: false,
                usage_max_tokens: None,
            },
        )
        .await
        .map(|sr| {
            (
                sr.content,
                sr.tool_calls,
                sr.prompt_tokens + sr.completion_tokens,
            )
        });

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
                    if !content.trim().is_empty() {
                        working_memory.push(json!({ "role": "assistant", "content": content.clone() }));
                    }

                    let mission_snapshot = {
                        let state = app.state::<AppState>();
                        let db = match state.db.lock() {
                            Ok(db) => db,
                            Err(err) => {
                                let error = format!("Failed to lock database after agent turn: {err}");
                                let _ = app.emit(
                                    "agent-task-error",
                                    json!({ "task_id": task.id, "agent_id": agent.id, "error": error }),
                                );
                                return TaskResult {
                                    task_id: task.id.clone(),
                                    agent_id: agent.id.clone(),
                                    agent_name: agent.name.clone(),
                                    status: "error".to_string(),
                                    content: error,
                                    tool_calls_count,
                                    tokens_used: total_tokens,
                                };
                            }
                        };

                        match load_mission_state(&db, &mission_id) {
                            Ok(snapshot) => snapshot,
                            Err(err) => {
                                let _ = app.emit(
                                    "agent-task-error",
                                    json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
                                );
                                return TaskResult {
                                    task_id: task.id.clone(),
                                    agent_id: agent.id.clone(),
                                    agent_name: agent.name.clone(),
                                    status: "error".to_string(),
                                    content: format!("Error: {}", err),
                                    tool_calls_count,
                                    tokens_used: total_tokens,
                                };
                            }
                        }
                    };

                    if mission_snapshot.mission_accomplished {
                        let final_content = if !content.trim().is_empty() {
                            content.clone()
                        } else if !mission_snapshot.final_report.trim().is_empty() {
                            mission_snapshot.final_report.clone()
                        } else {
                            "Mission accomplished.".to_string()
                        };

                        let state = app.state::<AppState>();
                        if let Ok(db) = state.db.lock() {
                            let _ = save_mission_final_report(&db, &mission_id, &final_content);
                        }

                        let summary: String = final_content.chars().take(200).collect();
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
                            content: final_content,
                            tool_calls_count,
                            tokens_used: total_tokens,
                        };
                    }

                    working_memory.push(json!({ "role": "user", "content": AGENT_CONTINUE_PROMPT }));
                    continue;
                }

                // Append assistant turn with tool calls
                let assistant_tcs: Vec<Value> = agent_tool_calls
                    .iter()
                    .map(|(id, name, args)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": args },
                        })
                    })
                    .collect();
                let assistant_content = if content.trim().is_empty() {
                    Value::Null
                } else {
                    Value::String(content.clone())
                };
                working_memory.push(
                    json!({ "role": "assistant", "content": assistant_content, "tool_calls": assistant_tcs }),
                );

                // Execute each tool call
                for (id, name, args) in &agent_tool_calls {
                    tool_calls_count += 1;
                    let result = tools::execute_tool(
                        app,
                        name,
                        args,
                        workspace_dir.clone(),
                        config,
                        &[],
                        &skill_access_roots,
                        &[],
                        Some(&mission_id),
                    )
                    .await;
                    working_memory.push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
                }
            }
        }
    }

    // Max iterations reached — return whatever was last generated
    let last_content = working_memory
        .iter()
        .rev()
        .find_map(|m| {
            m["content"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| {
            if max_iterations.is_some() {
                "Max iterations reached without final answer.".to_string()
            } else {
                "Autonomous loop stopped without a final answer.".to_string()
            }
        });

    let mission_summary = {
        let state = app.state::<AppState>();
        state
            .db
            .lock()
            .ok()
            .and_then(|db| load_mission_state(&db, &mission_id).ok())
    };

    let final_content = mission_summary
        .as_ref()
        .and_then(|mission| {
            if mission.mission_accomplished && !mission.final_report.trim().is_empty() {
                Some(mission.final_report.clone())
            } else {
                None
            }
        })
        .unwrap_or(last_content);

    let _ = app.emit(
        "agent-task-done",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "summary": if max_iterations.is_some() { "Max iterations reached" } else { "Loop stopped" },
        }),
    );
    TaskResult {
        task_id: task.id.clone(),
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        status: if mission_summary.as_ref().map(|mission| mission.mission_accomplished).unwrap_or(false) {
            "success".to_string()
        } else {
            "stopped".to_string()
        },
        content: final_content,
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

    let mut req_body = json!({
        "model": model,
        "messages": agg_messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    apply_completion_token_limit(&mut req_body, 4096);

    let cancelled = AtomicBool::new(false);
    let sr = stream_llm_request(
        app,
        client,
        url,
        api_key,
        req_body,
        &cancelled,
        StreamOptions {
            token_event: "chat-token",
            task_id: None,
            emit_reasoning: false,
            emit_usage: false,
            usage_max_tokens: None,
        },
    )
    .await?;

    Ok(sr.content)
}
