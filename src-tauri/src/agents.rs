use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State};
use tiktoken_rs::cl100k_base;
use uuid::Uuid;

const LLM_DEFAULT_MAX_TOKENS: u32 = 131_072;
const LLM_DEFAULT_MAX_COMLETE_TOKENS: u32 = 4_096;

use crate::{
    llm_complete::{
        apply_completion_token_limit, content_to_text, conversation_id_header_value,
        extract_upstream_error_message, resolve_reasoning_effort, sanitize_tool_pairs,
        stream_llm_request, StreamOptions, CONVERSATION_ID_HEADER,
    },
    tools, AppConfig, AppState,
};

pub mod cli;
pub mod cloud;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A sub-agent. `agent_type` decides which execution backend runs it:
///
/// - `llm`   — the built-in autonomous LLM loop (tools + mission state);
/// - `cli`   — a headless coding CLI installed on this machine (opencode,
///             Claude Code, Codex, …) driven through `cli`;
/// - `cloud` — a remote HTTP agent endpoint, driven through `cloud`.
///
/// The type-specific settings live in `cli` / `cloud`; the LLM fields are
/// ignored by the other two backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default = "default_agent_type")]
    pub agent_type: String,
    // ── `llm` backend ────────────────────────────────────────────────────────
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_complete_tokens: Option<u32>,
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
    // ── `cli` backend ────────────────────────────────────────────────────────
    #[serde(default)]
    pub cli: CliAgentSettings,
    // ── `cloud` backend ──────────────────────────────────────────────────────
    #[serde(default)]
    pub cloud: CloudAgentSettings,
    /// `cli` / `cloud` agents only: ask the user before the first run of this
    /// agent in each app session. Turn off to treat the panel switch as the
    /// standing authorization.
    #[serde(default = "default_true")]
    pub require_approval: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

pub const AGENT_TYPE_LLM: &str = "llm";
pub const AGENT_TYPE_CLI: &str = "cli";
pub const AGENT_TYPE_CLOUD: &str = "cloud";

fn default_agent_type() -> String {
    AGENT_TYPE_LLM.to_string()
}

/// Normalizes a user-supplied agent type, falling back to `llm`.
pub fn normalize_agent_type(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        AGENT_TYPE_CLI => AGENT_TYPE_CLI.to_string(),
        AGENT_TYPE_CLOUD => AGENT_TYPE_CLOUD.to_string(),
        _ => AGENT_TYPE_LLM.to_string(),
    }
}

fn default_max_iterations() -> u32 {
    10
}
fn default_true() -> bool {
    true
}

/// A key/value pair: an environment variable for `cli` agents, or an extra
/// header for `cloud` agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    #[serde(default)]
    pub value: String,
}

/// Settings of a `cli` agent: how to launch the external executable.
///
/// `command` + `args` form the argv template: `{prompt}`, `{model}`, `{cwd}`
/// and `{agent_name}` are substituted before spawning. With
/// `prompt_via = "stdin"` the prompt is written to the child's stdin instead of
/// appearing in argv (use it for CLIs that read piped input, and on Windows
/// `.cmd` shims where cmd.exe could expand `%VAR%` text).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliAgentSettings {
    /// Executable to launch: a bare name (resolved through PATH) or a path.
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_prompt_via")]
    pub prompt_via: String,
    #[serde(default)]
    pub continue_args: Vec<String>,
    #[serde(default)]
    pub model_args: Vec<String>,
    #[serde(default)]
    pub env: Vec<KeyValue>,
    #[serde(default = "default_cli_timeout_seconds")]
    pub default_timeout_seconds: u32,
    /// Optional workspace-relative directory to run in (default: workspace root).
    #[serde(default)]
    pub cwd: String,
}

impl Default for CliAgentSettings {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            prompt_via: default_prompt_via(),
            continue_args: Vec::new(),
            model_args: Vec::new(),
            env: Vec::new(),
            default_timeout_seconds: default_cli_timeout_seconds(),
            cwd: String::new(),
        }
    }
}

fn default_prompt_via() -> String {
    "arg".to_string()
}

fn default_cli_timeout_seconds() -> u32 {
    600
}

/// Settings of a `cloud` agent: a remote HTTP endpoint that accepts a task and
/// returns its result, optionally asynchronously through a polling URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudAgentSettings {
    /// POST target that accepts the task.
    #[serde(default)]
    pub endpoint: String,
    /// Sent as `Authorization: Bearer <token>`. Leave empty for none.
    #[serde(default)]
    pub token: String,
    /// Extra request headers.
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    /// JSON request body template; `{prompt}`, `{agent_name}` and `{cwd}` are
    /// substituted (JSON-escaped where they appear inside string literals).
    #[serde(default = "default_cloud_body_template")]
    pub body_template: String,
    /// Dot path into the JSON response that holds the agent's text
    /// (e.g. `choices.0.message.content`, `result.output`).
    #[serde(default = "default_cloud_result_path")]
    pub result_path: String,
    /// Overall timeout in seconds, covering polling.
    #[serde(default = "default_cloud_timeout_seconds")]
    pub timeout_seconds: u32,
    /// Async mode: poll this URL (supports the `{id}` placeholder) until the
    /// status matches one of the done/failed values. Empty = single response.
    #[serde(default)]
    pub poll_url: String,
    /// Dot path in the POST response holding the job id used for `{id}`.
    #[serde(default = "default_poll_id_path")]
    pub poll_id_path: String,
    /// Dot path in the poll response holding the job status.
    #[serde(default = "default_poll_status_path")]
    pub poll_status_path: String,
    /// Status values that mean "done, read the result".
    #[serde(default = "default_poll_done_values")]
    pub poll_done_values: Vec<String>,
    /// Status values that mean "the job failed".
    #[serde(default = "default_poll_failed_values")]
    pub poll_failed_values: Vec<String>,
    #[serde(default = "default_poll_interval_seconds")]
    pub poll_interval_seconds: u32,
}

impl Default for CloudAgentSettings {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            token: String::new(),
            headers: Vec::new(),
            body_template: default_cloud_body_template(),
            result_path: default_cloud_result_path(),
            timeout_seconds: default_cloud_timeout_seconds(),
            poll_url: String::new(),
            poll_id_path: default_poll_id_path(),
            poll_status_path: default_poll_status_path(),
            poll_done_values: default_poll_done_values(),
            poll_failed_values: default_poll_failed_values(),
            poll_interval_seconds: default_poll_interval_seconds(),
        }
    }
}

fn default_cloud_body_template() -> String {
    "{\"messages\":[{\"role\":\"user\",\"content\":\"{prompt}\"}]}".to_string()
}
fn default_cloud_result_path() -> String {
    "choices.0.message.content".to_string()
}
fn default_cloud_timeout_seconds() -> u32 {
    1_800
}
fn default_poll_id_path() -> String {
    "id".to_string()
}
fn default_poll_status_path() -> String {
    "status".to_string()
}
fn default_poll_done_values() -> Vec<String> {
    ["completed", "succeeded", "success", "done", "finished"]
        .iter()
        .map(|value| value.to_string())
        .collect()
}
fn default_poll_failed_values() -> Vec<String> {
    ["failed", "error", "cancelled", "canceled"]
        .iter()
        .map(|value| value.to_string())
        .collect()
}
fn default_poll_interval_seconds() -> u32 {
    5
}

/// Definition of a standalone CLI agent written by builds that predate agent
/// types. Only used to migrate `sub_agents.json` / profiles forward.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyCliAgent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_prompt_via")]
    pub prompt_via: String,
    #[serde(default)]
    pub continue_args: Vec<String>,
    #[serde(default)]
    pub model_args: Vec<String>,
    #[serde(default)]
    pub env: Vec<KeyValue>,
    #[serde(default = "default_cli_timeout_seconds")]
    pub default_timeout_seconds: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    #[serde(default)]
    pub agents: Vec<SubAgent>,
    #[serde(default)]
    pub orchestration: AgentOrchestration,
    /// Legacy standalone CLI agents — migrated into `agents` (type `cli`) on
    /// load and never written back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cli_agents: Vec<LegacyCliAgent>,
}

/// Normalizes a loaded config in place: agent types are canonicalized and any
/// legacy standalone CLI agents become `cli`-type agents.
pub fn normalize_config(config: &mut AgentsConfig) {
    for agent in &mut config.agents {
        agent.agent_type = normalize_agent_type(&agent.agent_type);
    }

    if config.cli_agents.is_empty() {
        return;
    }

    let legacy = std::mem::take(&mut config.cli_agents);
    for entry in legacy {
        let already_present = config.agents.iter().any(|agent| agent.id == entry.id);
        if already_present {
            continue;
        }
        config.agents.push(SubAgent {
            id: entry.id,
            name: entry.name,
            description: entry.description,
            agent_type: AGENT_TYPE_CLI.to_string(),
            system_prompt: String::new(),
            model: None,
            max_complete_tokens: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Vec::new(),
            allowed_skills: Vec::new(),
            max_iterations: default_max_iterations(),
            cli: CliAgentSettings {
                command: entry.command,
                args: entry.args,
                prompt_via: entry.prompt_via,
                continue_args: entry.continue_args,
                model_args: entry.model_args,
                env: entry.env,
                default_timeout_seconds: entry.default_timeout_seconds,
                cwd: String::new(),
            },
            cloud: CloudAgentSettings::default(),
            require_approval: true,
            enabled: entry.enabled,
        });
    }
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

/// Closes a mission that ended without `mark_mission_accomplished` (max
/// iterations reached, user cancellation, hard error, …) so the mission
/// monitor never keeps showing a stale "running" status.
fn close_mission(app: &AppHandle, mission_id: &str, status: &str) {
    let state = app.state::<AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(_) => return,
    };
    let _ = db.execute(
        "UPDATE agent_missions SET status = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1 AND mission_accomplished = 0 AND status = 'running'",
        rusqlite::params![mission_id, status],
    );
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

    tasks
        .iter()
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
    let role = message
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");

    if let Some(tool_calls) = message.get("tool_calls").and_then(|value| value.as_array()) {
        let tool_names = tool_calls
            .iter()
            .filter_map(|call| call.get("function"))
            .filter_map(|function| function.get("name"))
            .filter_map(|name| name.as_str())
            .collect::<Vec<_>>();
        if !tool_names.is_empty() {
            return Some(format!(
                "- assistant requested tools: {}",
                tool_names.join(", ")
            ));
        }
    }

    let content = message
        .get("content")
        .map(content_to_text)
        .map(|value| truncate_for_summary(&value, 400))
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

/// Static part of the sub-agent prompt — computed once per mission and reused
/// across iterations so the LLM prompt cache keeps hitting on the prefix.
fn build_agent_static_system_prompt(agent: &SubAgent, self_evolution_context: &str) -> String {
    format!(
        "{}\n\nAutonomous execution sub-agent. External mission state is authoritative for task tracking.\n\
         Rules: track tasks via add_task/update_task_status/get_active_tasks/mark_mission_accomplished \
         (not chat history); keep outputs execution-focused; create tasks for branches; \
         clear active tasks before finishing. \
         The latest mission snapshot is appended as the final system message every turn — \
         always treat it as authoritative.{}",
        agent.system_prompt, self_evolution_context,
    )
}

/// Dynamic mission snapshot — refreshed every iteration and appended as the
/// LAST message so changes never invalidate the cached prefix before it.
fn build_mission_snapshot_prompt(mission: &MissionStateSnapshot) -> String {
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
        "MISSION SNAPSHOT (refreshed each turn):\n\
         Mission ID: {}\nPrimary task: {}\nContext: {}\n{}\n\
         Active tasks:\n{}\nEpisodic memory:\n{}",
        mission.mission_id,
        mission.root_task_description,
        mission.root_task_context,
        completion_state,
        build_active_tasks_snapshot(&mission.active_tasks),
        episodic_summary,
    )
}

fn build_agent_messages(
    static_system_prompt: &str,
    working_memory: &[Value],
    snapshot_prompt: &str,
) -> Vec<Value> {
    let mut messages = Vec::with_capacity(working_memory.len() + 2);
    messages.push(json!({ "role": "system", "content": static_system_prompt }));
    messages.extend(working_memory.iter().cloned());
    messages.push(json!({ "role": "system", "content": snapshot_prompt }));
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
    session_id: &str,
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
        session_id,
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
    session_id: &str,
    model: &str,
    static_system_prompt: &str,
    snapshot_prompt: &str,
    working_memory: &mut Vec<Value>,
    episodic_summary: &mut String,
    context_budget: usize,
) -> Result<bool, String> {
    let candidate_messages =
        build_agent_messages(static_system_prompt, working_memory, snapshot_prompt);
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
        working_memory
            .len()
            .saturating_sub(AGENT_MIN_RECENT_MESSAGES)
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
        session_id,
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
    let mut config = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<AgentsConfig>(&s).ok())
        .unwrap_or_default();
    normalize_config(&mut config);
    config
}

pub(crate) fn save_agents_config_file(path: &Path, config: &AgentsConfig) -> Result<(), String> {
    let mut config = config.clone();
    normalize_config(&mut config);
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
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
    if a.name.trim().is_empty() {
        return Err("The agent name cannot be empty.".to_string());
    }
    a.agent_type = normalize_agent_type(&a.agent_type);
    match a.agent_type.as_str() {
        AGENT_TYPE_CLI => {
            a.cli.command = a.cli.command.trim().to_string();
            if a.cli.command.is_empty() {
                return Err("A CLI agent needs a command to launch.".to_string());
            }
            a.cli.prompt_via = if a.cli.prompt_via.trim().eq_ignore_ascii_case("stdin") {
                "stdin".to_string()
            } else {
                "arg".to_string()
            };
        }
        AGENT_TYPE_CLOUD => {
            a.cloud.endpoint = a.cloud.endpoint.trim().to_string();
            if a.cloud.endpoint.is_empty() {
                return Err("A cloud agent needs an endpoint URL.".to_string());
            }
            if a.cloud.timeout_seconds == 0 {
                a.cloud.timeout_seconds = default_cloud_timeout_seconds();
            }
            if a.cloud.poll_interval_seconds == 0 {
                a.cloud.poll_interval_seconds = default_poll_interval_seconds();
            }
        }
        _ => {}
    }
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

// ─── Agent probes (panel "Test" buttons) ─────────────────────────────────────

/// Resolve a `cli` agent's command (PATH + PATHEXT) and run it with the given
/// arguments, so the panel can verify it before the model ever uses it.
#[tauri::command]
pub async fn probe_agent_cli(
    command: String,
    args: Option<Vec<String>>,
) -> Result<cli::CliProbe, String> {
    cli::probe(&command, &args.unwrap_or_default()).await
}

/// Check that a `cloud` agent's endpoint is reachable with the given token.
#[tauri::command]
pub async fn probe_agent_cloud(
    endpoint: String,
    token: Option<String>,
    headers: Option<Vec<KeyValue>>,
) -> Result<cloud::CloudProbe, String> {
    cloud::probe(
        &endpoint,
        token.as_deref().unwrap_or(""),
        &headers.unwrap_or_default(),
    )
    .await
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

        let mapped_rows = stmt
            .query_map(rusqlite::params![session_id], |row| {
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
    session_id: &str,
    mut messages: Vec<Value>,
    model: &str,
    max_complete_tokens: u32,
) -> Result<String, String> {
    sanitize_tool_pairs(&mut messages);
    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    // No DS-Format normalization needed here: these prompts only ever carry
    // system/user messages, never replayed assistant turns.
    apply_completion_token_limit(
        &mut body,
        Some(max_complete_tokens),
        Some(LLM_DEFAULT_MAX_TOKENS),
    );
    let res = client
        .post(url)
        .header(
            CONVERSATION_ID_HEADER,
            conversation_id_header_value(session_id),
        )
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
        auto_configure_agents(
            &client,
            &url,
            &config.api_key,
            session_id,
            &model,
            messages,
            &enabled,
        )
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
        session_id,
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
    let skill_access_roots = crate::skills::collect_self_evolution_roots(
        Some(&workspace_dir),
        config.self_evolution_mode,
    );
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

    // Step 4: Hand the mission record to the caller. Sub-agents are execution
    // workers — they never write the user-facing answer: the main agent (full
    // chat context, skills and tools) defines the final result, so
    // llm_complete injects this report and runs its normal completion flow.
    let _ = app.emit("agent-aggregate-start", ());
    Ok(build_mission_results_block(&results))
}

// ─── Auto-configure ───────────────────────────────────────────────────────────

async fn auto_configure_agents(
    client: &Client,
    url: &str,
    api_key: &str,
    session_id: &str,
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    existing: &[SubAgent],
) -> Result<Vec<SubAgent>, String> {
    let existing_desc = existing
        .iter()
        .map(|a| {
            format!(
                "  - id={} name={} description={}",
                a.id, a.name, a.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user_query = messages
        .last()
        .map(|m| content_to_text(&m.content))
        .unwrap_or_default();
    let system = format!(
        "You are an AI orchestrator. Based on the user request, decide which agents to use.\n\
        Existing agents:\n{existing_desc}\n\n\
        Option A — use existing: return JSON: {{\"strategy\":\"existing\",\"ids\":[\"id1\",\"id2\"]}}\n\
        Option B — define new temporary agents: return JSON:\n\
        {{\"strategy\":\"new\",\"agents\":[\
          {{\"id\":\"tmp-1\",\"name\":\"...\",\"description\":\"...\",\"system_prompt\":\"...\",\
            \"allowed_tools\":[\"file_actions\"],\"max_iterations\":5,\"enabled\":true}}\
        ]}}\n\
        allowed_tools values must be a subset of: [\"file_actions\",\"run_cmd\",\"run_shell\"].\n\
        Return ONLY the JSON object, no markdown, no extra text.",
    );

    let raw = call_llm_once(
        client,
        url,
        api_key,
        session_id,
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
    session_id: &str,
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    agents: &[SubAgent],
) -> Result<TaskPlan, String> {
    let agents_desc = agents
        .iter()
        .map(|a| {
            format!(
                "  - id={} name={} description={}",
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
            let text = content_to_text(&m.content);
            let limit = text.len().min(600);
            let mut end = limit;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            let preview = &text[..end];
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
        client, url, api_key, session_id,
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
            // Maps each spawned job to its task so an aborted or panicked job can
            // be attributed and reported instead of silently dropping its result.
            let mut spawned_jobs: HashMap<tokio::task::Id, (String, String, String)> =
                HashMap::new();

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
                let job_meta = (task.id.clone(), agent.id.clone(), agent.name.clone());

                let handle = join_set.spawn(async move {
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
                spawned_jobs.insert(handle.id(), job_meta);
            }

            while let Some(joined) = join_set.join_next().await {
                match joined {
                    Ok(res) => {
                        completed.insert(res.task_id.clone(), ());
                        results.push(res);
                    }
                    Err(err) => {
                        // A panicked or aborted job must not take down the rest of
                        // the batch: report it as an error result and mark it as
                        // finished so dependent tasks can still run.
                        let (task_id, agent_id, agent_name) =
                            spawned_jobs.remove(&err.id()).unwrap_or_else(|| {
                                (
                                    "unknown-task".to_string(),
                                    "unknown-agent".to_string(),
                                    "unknown-agent".to_string(),
                                )
                            });
                        let reason = if err.is_panic() {
                            "panicked"
                        } else {
                            "was aborted"
                        };
                        let result = TaskResult {
                            task_id,
                            agent_id,
                            agent_name,
                            status: "error".to_string(),
                            content: format!("Sub-agent task {reason}: {err}"),
                            tool_calls_count: 0,
                            tokens_used: 0,
                        };
                        // task_id == mission_id: never leave a panicked mission
                        // behind in the "running" state.
                        close_mission(app, &result.task_id, "failed");
                        let _ = app.emit(
                            "agent-task-error",
                            json!({
                                "task_id": &result.task_id,
                                "agent_id": &result.agent_id,
                                "error": &result.content,
                            }),
                        );
                        completed.insert(result.task_id.clone(), ());
                        results.push(result);
                    }
                }
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
    let mission_id = mission_id_for_task(task).to_string();
    let agent_type = normalize_agent_type(&agent.agent_type);
    let result = match agent_type.as_str() {
        AGENT_TYPE_CLI => {
            run_external_agent(app, agent, task, session_id, workspace_dir, ExternalBackend::Cli)
                .await
        }
        AGENT_TYPE_CLOUD => {
            run_external_agent(app, agent, task, session_id, workspace_dir, ExternalBackend::Cloud)
                .await
        }
        _ => {
            run_sub_agent_inner(
                app,
                client,
                url,
                config,
                session_id,
                agent,
                task,
                workspace_dir,
                skill_access_roots,
            )
            .await
        }
    };

    // Close the mission unless the sub-agent completed it explicitly: without
    // this, a stopped, failed or cancelled mission keeps reporting "running"
    // in the mission monitor forever.
    if result.status != "success" {
        close_mission(
            app,
            &mission_id,
            if result.status == "error" {
                "failed"
            } else {
                "stopped"
            },
        );
    }

    result
}

// ─── External agents (cli / cloud) ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalBackend {
    Cli,
    Cloud,
}

impl ExternalBackend {
    fn label(self) -> &'static str {
        match self {
            ExternalBackend::Cli => "CLI",
            ExternalBackend::Cloud => "Cloud",
        }
    }
}

/// Runs a `cli` / `cloud` agent. The agent has no LLM loop of its own inside the
/// app: it receives one self-contained prompt, runs to completion (or the
/// timeout) and its output becomes the mission report.
async fn run_external_agent(
    app: &AppHandle,
    agent: &SubAgent,
    task: &Task,
    session_id: &str,
    workspace_dir: PathBuf,
    backend: ExternalBackend,
) -> TaskResult {
    let mission_id = mission_id_for_task(task).to_string();
    let _ = app.emit(
        "agent-task-start",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "description": task.description,
        }),
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
                    tool_calls_count: 0,
                    tokens_used: 0,
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
                tool_calls_count: 0,
                tokens_used: 0,
            };
        }
    }
    let _ = app.emit(
        "agent-task-state",
        json!({ "mission_id": mission_id, "session_id": session_id, "status": "running", "event": "initialized" }),
    );

    let prompt = build_external_prompt(agent, task);

    if agent.require_approval {
        if let Some(rejection) =
            request_external_agent_approval(app, agent, &prompt, &workspace_dir, backend).await
        {
            let _ = app.emit(
                "agent-task-error",
                json!({ "task_id": task.id, "agent_id": agent.id, "error": rejection.clone() }),
            );
            return TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: rejection,
                tool_calls_count: 0,
                tokens_used: 0,
            };
        }
    }

    let outcome: Result<(String, String), String> = match backend {
        ExternalBackend::Cli => {
            let cwd = match resolve_agent_cwd(agent, &workspace_dir) {
                Ok(cwd) => cwd,
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
                        content: format!("Error: {err}"),
                        tool_calls_count: 0,
                        tokens_used: 0,
                    };
                }
            };
            cli::run(app, agent, &task.id, &prompt, cwd)
                .await
                .map(|run| (run.output, run.summary))
                .map_err(|err| format!("CLI agent \"{}\" failed: {err}", agent.name))
        }
        ExternalBackend::Cloud => cloud::run(app, agent, &task.id, &prompt, &workspace_dir)
            .await
            .map(|run| (run.result, run.summary))
            .map_err(|err| format!("Cloud agent \"{}\" failed: {err}", agent.name)),
    };

    match outcome {
        Ok((content, summary)) => {
            let state = app.state::<AppState>();
            if let Ok(db) = state.db.lock() {
                let _ = mark_mission_accomplished(&db, &mission_id, Some(&content));
            }
            let _ = app.emit(
                "agent-task-done",
                json!({
                    "task_id": task.id,
                    "agent_id": agent.id,
                    "agent_name": agent.name,
                    "summary": summary.chars().take(200).collect::<String>(),
                }),
            );
            TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "success".to_string(),
                content,
                tool_calls_count: 0,
                tokens_used: 0,
            }
        }
        Err(err) => {
            let _ = app.emit(
                "agent-task-error",
                json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
            );
            TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: format!("Error: {err}"),
                tool_calls_count: 0,
                tokens_used: 0,
            }
        }
    }
}

/// The single prompt handed to a `cli` / `cloud` agent: the task plus whatever
/// context the caller attached. External agents never see the chat history.
fn build_external_prompt(agent: &SubAgent, task: &Task) -> String {
    let mut prompt = String::new();
    if !agent.system_prompt.trim().is_empty() {
        prompt.push_str(agent.system_prompt.trim());
        prompt.push_str("\n\n");
    }
    prompt.push_str(task.description.trim());
    if !task.context.trim().is_empty() {
        prompt.push_str("\n\nContext:\n");
        prompt.push_str(task.context.trim());
    }
    prompt
}

/// Workspace-relative `cwd` configured on a CLI agent (defaults to the
/// workspace root). The path is checked against the workspace sandbox.
fn resolve_agent_cwd(agent: &SubAgent, workspace_dir: &Path) -> Result<PathBuf, String> {
    let configured = agent.cli.cwd.trim();
    if configured.is_empty() {
        return Ok(workspace_dir.to_path_buf());
    }
    let path = crate::tools::resolve_safe_path(workspace_dir, configured)?;
    if !path.is_dir() {
        return Err(format!(
            "the agent's working directory '{}' does not exist",
            path.display()
        ));
    }
    Ok(path)
}

/// Approval gate for `cli` / `cloud` agents: asked once per agent per app run
/// (unless `require_approval` is off, in which case the panel switch is the
/// standing authorization).
async fn request_external_agent_approval(
    app: &AppHandle,
    agent: &SubAgent,
    prompt: &str,
    workspace_dir: &Path,
    backend: ExternalBackend,
) -> Option<String> {
    let state = app.state::<AppState>();
    if state
        .external_agent_confirmed
        .lock()
        .map(|set| set.contains(&agent.id))
        .unwrap_or(false)
    {
        return None;
    }

    // Only one confirmation dialog can be pending at a time (single
    // `confirm_sender`), so parallel agents must queue here instead of
    // overwriting each other's request.
    let _guard = state.external_agent_confirm_lock.lock().await;

    if state
        .external_agent_confirmed
        .lock()
        .map(|set| set.contains(&agent.id))
        .unwrap_or(false)
    {
        return None;
    }

    let target = match backend {
        ExternalBackend::Cli => {
            // The approval text must show `{cwd}` as the directory the agent
            // will actually start in, not the workspace root.
            let cwd = resolve_agent_cwd(agent, workspace_dir)
                .unwrap_or_else(|_| workspace_dir.to_path_buf());
            cli::describe_run(agent, prompt, &cwd)
                .map(|command| format!("It runs with your user permissions and can read and write files in the workspace.\n\n{}", command))
                .unwrap_or_else(|err| format!("Its command could not be resolved yet: {err}"))
        }
        ExternalBackend::Cloud => format!(
            "It sends the task to a remote endpoint and can read and write files there.\n\nPOST {}",
            agent.cloud.endpoint.trim()
        ),
    };

    let confirm = crate::tools::request_tool_confirmation(
        app,
        format!(
            "The {} agent '{}' is about to run.\n\n{}",
            backend.label(),
            agent.name,
            target
        ),
        "external_agent".to_string(),
        target.clone(),
        "external_agent",
        "none",
    )
    .await;

    if !confirm.confirmed {
        return Some(format!(
            "⛔ The user rejected running the {} agent '{}'. Do not retry it; ask the user how to proceed or do the work with the available tools.",
            backend.label(),
            agent.name
        ));
    }

    if let Ok(mut set) = state.external_agent_confirmed.lock() {
        set.insert(agent.id.clone());
    }
    None
}

async fn run_sub_agent_inner(
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
    let max_complete_tokens = agent
        .max_complete_tokens
        .unwrap_or(LLM_DEFAULT_MAX_COMLETE_TOKENS);
    // `timer` is a chat-session tool (it resumes the conversation later), so it
    // is never handed to a sub-agent even if an agent config lists it.
    let agent_tool_ids: Vec<String> = agent
        .allowed_tools
        .iter()
        .filter(|tool| tool.as_str() != "timer")
        .cloned()
        .collect();
    let mut tools_list = tools::get_all_tools(&agent_tool_ids);
    tools_list.extend(tools::get_agent_task_tools());
    let cancelled = AtomicBool::new(false);
    let mut total_tokens: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mission_id = mission_id_for_task(task).to_string();
    // Context budget resolution order:
    // 1. Per-model context window from config (auto-detected from the
    //    provider's `/models` response or entered manually in Settings).
    // 2. `max_complete_tokens` (legacy approximation).
    // 3. Conservative global default.
    let context_budget = config
        .model_context_lengths
        .get(model)
        .copied()
        .filter(|limit| *limit > 0)
        .map(|limit| limit as usize)
        .or_else(|| {
            config
                .model_settings
                .max_complete_tokens
                .filter(|limit| *limit > 0)
                .map(|limit| limit as usize)
        })
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
             Use `file_actions` only with workspace-relative paths. \
             Skill edits are limited to `./.data/skills/...` under the workspace and create `.bak.<n>` backups automatically. \
             Do not reference app-data skill paths or any path outside the workspace root. \
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
    // Static system prompt: built once, reused every iteration (cache-friendly).
    let static_system_prompt = build_agent_static_system_prompt(agent, &self_evolution_context);
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
                    let error =
                        format!("Failed to lock database while loading mission state: {err}");
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

        let snapshot_prompt = build_mission_snapshot_prompt(&mission_snapshot);

        match compress_working_memory_if_needed(
            client,
            url,
            &config.api_key,
            session_id,
            model,
            &static_system_prompt,
            &snapshot_prompt,
            &mut working_memory,
            &mut mission_snapshot.episodic_summary,
            context_budget,
        )
        .await
        {
            Ok(true) => {
                let state = app.state::<AppState>();
                if let Ok(db) = state.db.lock() {
                    let _ =
                        save_mission_summary(&db, &mission_id, &mission_snapshot.episodic_summary);
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

        // Rebuild the snapshot after a possible compression (episodic summary
        // may have changed), then append it as the final message.
        let snapshot_prompt = build_mission_snapshot_prompt(&mission_snapshot);
        let mut messages =
            build_agent_messages(&static_system_prompt, &working_memory, &snapshot_prompt);
        sanitize_tool_pairs(&mut messages);

        let mut req_body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        apply_completion_token_limit(
            &mut req_body,
            Some(max_complete_tokens),
            Some(LLM_DEFAULT_MAX_TOKENS),
        );
        if let Some(temp) = agent.temperature {
            req_body["temperature"] = json!(temp);
        }
        // Same thinking depth as the chat toolbar / config, resolved for the
        // model this agent runs on.
        if let Some(effort) = resolve_reasoning_effort(config, model) {
            req_body["reasoning_effort"] = json!(effort);
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
                ds_format: config.ds_format,
                conversation_id: Some(session_id),
            },
        )
        .await
        .map(|sr| {
            (
                sr.content,
                sr.reasoning_content,
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
            Ok((content, reasoning_content, agent_tool_calls, tokens)) => {
                total_tokens += tokens;

                if agent_tool_calls.is_empty() {
                    if !content.trim().is_empty() {
                        // Keep the round's reasoning trace: thinking-mode APIs
                        // require it on replayed assistant messages.
                        let mut assistant_msg =
                            json!({ "role": "assistant", "content": content.clone() });
                        if !reasoning_content.is_empty() {
                            assistant_msg["reasoning_content"] = json!(reasoning_content.as_str());
                        }
                        working_memory.push(assistant_msg);
                    }

                    let mission_snapshot = {
                        let state = app.state::<AppState>();
                        let db = match state.db.lock() {
                            Ok(db) => db,
                            Err(err) => {
                                let error =
                                    format!("Failed to lock database after agent turn: {err}");
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

                    working_memory
                        .push(json!({ "role": "user", "content": AGENT_CONTINUE_PROMPT }));
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
                // Thinking-mode APIs require the round's `reasoning_content` to
                // ride along with tool-call turns, unmodified.
                let mut assistant_msg = json!({
                    "role": "assistant",
                    "content": assistant_content,
                    "tool_calls": assistant_tcs,
                });
                if !reasoning_content.is_empty() {
                    assistant_msg["reasoning_content"] = json!(reasoning_content.as_str());
                }
                working_memory.push(assistant_msg);

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
                        Some(&mission_id),
                        Some(session_id),
                        &[], // sub-agents don't track active skill dirs for skill_read
                    )
                    .await;
                    working_memory
                        .push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
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
        status: if mission_summary
            .as_ref()
            .map(|mission| mission.mission_accomplished)
            .unwrap_or(false)
        {
            "success".to_string()
        } else {
            "stopped".to_string()
        },
        content: final_content,
        tool_calls_count,
        tokens_used: total_tokens,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's pre-typed config: an `llm` agent plus the legacy standalone
    /// CLI-agent array. Loading it must yield two typed agents and drop the
    /// legacy array.
    #[test]
    fn legacy_cli_agents_are_migrated_into_typed_agents() {
        let raw = r#"{
            "agents": [
                {
                    "id": "code-writer",
                    "name": "code-writer",
                    "description": "reviews code",
                    "system_prompt": "be precise",
                    "allowed_tools": ["file_actions"],
                    "max_iterations": 10,
                    "enabled": true
                }
            ],
            "orchestration": { "use_agents": true, "auto_configure": false, "max_concurrent": 3, "mode": "parallel" },
            "cli_agents": [
                {
                    "id": "opencode-agent",
                    "name": "opencode",
                    "description": "",
                    "command": "C:\\Users\\x\\AppData\\Roaming\\npm\\opencode2.ps1",
                    "args": ["run", "{prompt}"],
                    "prompt_via": "arg",
                    "continue_args": [],
                    "model_args": [],
                    "env": [],
                    "default_timeout_seconds": 600,
                    "enabled": true
                }
            ]
        }"#;

        let mut config: AgentsConfig = serde_json::from_str(raw).expect("legacy config parses");
        normalize_config(&mut config);

        assert_eq!(config.agents.len(), 2);
        assert!(config.cli_agents.is_empty());

        let llm_agent = config
            .agents
            .iter()
            .find(|agent| agent.id == "code-writer")
            .expect("llm agent kept");
        assert_eq!(llm_agent.agent_type, AGENT_TYPE_LLM);
        assert!(llm_agent.require_approval);

        let cli_agent = config
            .agents
            .iter()
            .find(|agent| agent.id == "opencode-agent")
            .expect("legacy CLI agent migrated");
        assert_eq!(cli_agent.agent_type, AGENT_TYPE_CLI);
        assert_eq!(cli_agent.cli.args, vec!["run".to_string(), "{prompt}".to_string()]);
        assert_eq!(cli_agent.cli.default_timeout_seconds, 600);
        assert!(cli_agent.cli.command.ends_with("opencode2.ps1"));

        // The legacy array must not come back when the config is serialized.
        let serialized = serde_json::to_string(&config).expect("serializes");
        assert!(!serialized.contains("\"cli_agents\""));
    }

    #[test]
    fn unknown_agent_types_fall_back_to_llm() {
        assert_eq!(normalize_agent_type("CLI"), AGENT_TYPE_CLI);
        assert_eq!(normalize_agent_type(" cloud "), AGENT_TYPE_CLOUD);
        assert_eq!(normalize_agent_type("banana"), AGENT_TYPE_LLM);
        assert_eq!(normalize_agent_type(""), AGENT_TYPE_LLM);
    }
}

// ─── Final result hand-off ──────────────────────────────────────────────────

/// Renders the finished mission for injection into the main agent's context.
/// The main agent — not a separate synthesizer prompt — defines the final
/// user-facing answer from these reports.
fn build_mission_results_block(results: &[TaskResult]) -> String {
    let mut body = String::from(
        "All sub-agent tasks have finished. Sub-agents are execution workers and do not \
         write the user-facing answer: define and deliver the final result yourself. \
         Verify the reports below, reconcile conflicting findings, flag anything that \
         failed or looks incomplete, and answer the original request completely.\n\n",
    );

    if results.is_empty() {
        body.push_str("No task results were produced.");
        return body;
    }

    for result in results {
        body.push_str(&format!(
            "### {} (agent: {}, status: {}, tool calls: {}, tokens: {})\n\n{}\n\n",
            result.task_id,
            result.agent_name,
            result.status,
            result.tool_calls_count,
            result.tokens_used,
            result.content
        ));
    }

    body
}
