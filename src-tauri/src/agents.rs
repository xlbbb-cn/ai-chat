use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tiktoken_rs::cl100k_base;
use uuid::Uuid;

const LLM_DEFAULT_MAX_TOKENS: u32 = 131_072;
const LLM_DEFAULT_MAX_COMLETE_TOKENS: u32 = 8_192;

use crate::{
    llm_complete::{
        apply_completion_token_limit, apply_request_headers, content_to_text,
        extract_upstream_error_message, has_authorization_header, resolve_reasoning_effort,
        sanitize_tool_pairs, stream_llm_request, upstream_request_headers, StreamOptions,
    },
    mcp, tools, AppConfig, AppState,
};

pub mod cli;
pub mod cloud;
pub mod exchange;
pub mod files;
pub mod mbus;
pub mod supervisor;

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
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_agent_type")]
    pub agent_type: String,
    // ── `llm` backend ────────────────────────────────────────────────────────
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_complete_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub allowed_skills: Vec<String>,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// `llm` backend only: inject the tools of every enabled MCP server.
    /// Each task owns its own server processes and stops them when it ends.
    #[serde(default, skip_serializing_if = "is_false")]
    pub enable_mcp: bool,
    // ── file-backed definition (`llm` only) ─────────────────────────────────
    /// Display emoji from the md frontmatter (content, not a UI icon).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Author color from the md frontmatter. Stored, never painted as a second
    /// accent (see DESIGN.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Relative path under `<app_data>/agents` that holds this agent's
    /// definition. `None` for cli/cloud agents and temporary planner agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Set when a discovered file exists but its frontmatter is broken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
    /// Runtime-only: whether `sub_agents.json` already has a config entry for
    /// this agent. A discovered-but-unconfigured file is returned with
    /// `configured = false` and defaults; it is never persisted until the user
    /// enables or edits it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub configured: bool,
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

/// Iteration cap for `llm` agents. `0` means "no limit": the default, because
/// mission completion is driven by the external task state, not by a round
/// count. The panel keeps a 500-round ceiling for typo protection.
fn default_max_iterations() -> u32 {
    0
}

/// Upper bound accepted for `max_iterations`. Anything larger is clamped on
/// load/save so a hand-edited `sub_agents.json` cannot pin a runaway loop.
pub const MAX_AGENT_ITERATIONS: u32 = 500;
fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
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
    pub model_args: Vec<String>,
    #[serde(default)]
    pub env: Vec<KeyValue>,
    /// Run timeout in seconds. `0` means no timeout.
    #[serde(default = "default_cli_timeout_seconds")]
    pub default_timeout_seconds: u32,
}

impl Default for CliAgentSettings {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            prompt_via: default_prompt_via(),
            model_args: Vec::new(),
            env: Vec::new(),
            default_timeout_seconds: default_cli_timeout_seconds(),
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
    /// Overall timeout in seconds, covering polling. `0` means no timeout.
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
    pub model_args: Vec<String>,
    #[serde(default)]
    pub env: Vec<KeyValue>,
    #[serde(default = "default_cli_timeout_seconds")]
    pub default_timeout_seconds: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    /// Schema version. Version 2 moved `llm` agent definitions to
    /// `<app_data>/agents/**/*.md`; v3 turned smart selection on by default;
    /// v4 made `max_iterations` unlimited by default; v5 enabled the shared
    /// `memory` tool for every `llm` sub-agent. This file is configuration only.
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub agents: Vec<SubAgent>,
    #[serde(default)]
    pub orchestration: AgentOrchestration,
    /// Legacy standalone CLI agents — migrated into `agents` (type `cli`) on
    /// load and never written back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cli_agents: Vec<LegacyCliAgent>,
}

fn default_config_version() -> u32 {
    5
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            version: default_config_version(),
            agents: Vec::new(),
            orchestration: AgentOrchestration::default(),
            cli_agents: Vec::new(),
        }
    }
}

/// Normalizes a loaded config in place: agent types are canonicalized and any
/// legacy standalone CLI agents become `cli`-type agents.
pub fn normalize_config(config: &mut AgentsConfig) {
    // v3 turned the one-call agent selection (`auto_configure`) on by default.
    // Older files always carried an explicit `false` because the panel saves the
    // whole orchestration object, and the old default was off — adopt the new
    // default exactly once, then persist v3 so a later "off" sticks.
    if config.version < 3 {
        config.orchestration.auto_configure = true;
        config.version = 3;
    }

    // v4 made `max_iterations` unlimited by default. Older files always
    // serialized the old default (10), so exactly 10 is the old default rather
    // than a deliberate choice — reset it once, then persist v4 so a later
    // explicit 10 sticks.
    if config.version < 4 {
        for agent in &mut config.agents {
            if agent.max_iterations == 10 {
                agent.max_iterations = 0;
            }
        }
        config.version = 4;
    }

    // v5 gives every `llm` sub-agent the shared `memory` tool by default.
    // Before v5 the panel did not offer it, so no agent could have disabled it
    // deliberately; after the migration the user can uncheck it and the choice
    // sticks (the version guard runs once).
    if config.version < 5 {
        for agent in &mut config.agents {
            if normalize_agent_type(&agent.agent_type) == AGENT_TYPE_LLM
                && !agent.allowed_tools.iter().any(|tool| tool == "memory")
            {
                agent.allowed_tools.push("memory".to_string());
            }
        }
        config.version = 5;
    }

    for agent in &mut config.agents {
        agent.agent_type = normalize_agent_type(&agent.agent_type);
        // `max_tokens` was the pre-2.8 name of the per-agent output cap. The
        // panel wrote it but the backend only reads `max_complete_tokens`, so
        // migrate the old value once and clear the dead field. `0` means unset.
        if agent
            .max_complete_tokens
            .filter(|limit| *limit > 0)
            .is_none()
        {
            agent.max_complete_tokens = agent.max_tokens.take().filter(|limit| *limit > 0);
        }
        agent.max_tokens = None;
        agent.max_iterations = agent.max_iterations.min(MAX_AGENT_ITERATIONS);
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
            enable_mcp: false,
            emoji: None,
            color: None,
            file: None,
            parse_error: None,
            configured: true,
            cli: CliAgentSettings {
                command: entry.command,
                args: entry.args,
                prompt_via: entry.prompt_via,
                model_args: entry.model_args,
                env: entry.env,
                default_timeout_seconds: entry.default_timeout_seconds,
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
    /// Run the one-call selection step before planning (default on). Turning it
    /// off means every enabled agent takes part in the mission.
    #[serde(default = "default_true")]
    pub auto_configure: bool,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Mission message bus (mbus): let the planner supervise dispatched tasks
    /// and let sub-agents exchange messages. Default on; the Agents panel keeps
    /// a visible switch because supervision adds planner model calls.
    #[serde(default = "default_true")]
    pub mbus_enabled: bool,
    /// Max mbus messages between any two sub-agents, both directions combined.
    /// When reached, the pair is blocked and the planner is asked to mediate.
    /// `0` disables the per-pair cap.
    #[serde(default = "default_mbus_pair_limit")]
    pub mbus_pair_limit: usize,
}

fn default_max_concurrent() -> usize {
    3
}
fn default_mbus_pair_limit() -> usize {
    mbus::DEFAULT_PAIR_LIMIT
}
fn default_mode() -> String {
    "parallel".to_string()
}

impl Default for AgentOrchestration {
    fn default() -> Self {
        Self {
            use_agents: false,
            auto_configure: true,
            max_concurrent: 3,
            mode: "parallel".to_string(),
            mbus_enabled: true,
            mbus_pair_limit: mbus::DEFAULT_PAIR_LIMIT,
        }
    }
}

// ─── Selection & orchestration outcome ────────────────────────────────────────

/// Why one agent was picked for this turn. Serialized straight into the
/// `agent-selection` event so the UI chip and the main session agree.
#[derive(Debug, Clone, Serialize)]
pub struct AgentChoice {
    pub id: String,
    pub name: String,
    /// Short role label ("code changes", "research", …).
    pub role: String,
    /// One-sentence justification produced by the selector.
    pub why: String,
}

/// The selector's decision for the current turn.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentSelection {
    /// `false` means the request needs no sub-agents: the planner is skipped
    /// and the main agent answers directly.
    pub delegate: bool,
    /// Selector confidence in `0.0..=1.0`.
    pub confidence: f32,
    pub choices: Vec<AgentChoice>,
}

/// Everything `orchestrate` hands back to the chat turn.
#[derive(Debug, Clone, Default)]
pub struct OrchestrationOutcome {
    /// Markdown mission report; `None` when no sub-agent ran.
    pub report: Option<String>,
    /// Selection details for the main session's system prompt and the UI chip.
    pub selection: Option<AgentSelection>,
}

/// Internal: the subset handed to the planner plus its public description.
struct SelectionResult {
    agents: Vec<SubAgent>,
    selection: AgentSelection,
}

/// Upper bound for a single plan. A runaway planner must not spawn hundreds of
/// missions (each one has its own mission row, MCP session and process tree).
const MAX_PLAN_TASKS: usize = 16;
/// Upper bound for the selected roster; the planner is told 0–4 as well, but a
/// model answer is not a contract.
const MAX_SELECTED_AGENTS: usize = 4;

// ─── Prompt budgets ───────────────────────────────────────────────────────────
// Every byte below is re-sent on every request, so the prompts clip what the
// model does not need. The limits are deliberately generous: they only bite on
// pathological input (a 10 KB user prompt, a planner that writes essays into
// `context`), never on normal usage.

/// User text handed to the selector. It only decides *who*, not *how*, so a
/// very long request can be shortened without losing the routing signal.
const SELECTION_QUERY_MAX_BYTES: usize = 2_000;
/// One-line capability summary per agent in the selector catalogue.
const ROSTER_DESC_MAX_BYTES: usize = 120;
/// Tools/skills listed per agent in the selector catalogue.
const ROSTER_LIST_MAX_ITEMS: usize = 6;
/// Conversation preview handed to the planner (per message).
const PLANNER_CONVERSATION_MAX_BYTES: usize = 400;
/// Root task context repeated in every sub-agent mission snapshot.
const SNAPSHOT_CONTEXT_MAX_CHARS: usize = 2_000;
/// Per-task description repeated in every sub-agent mission snapshot.
const SNAPSHOT_TASK_DESC_MAX_CHARS: usize = 300;
/// Tasks rendered in the snapshot; the rest stay reachable via `get_active_tasks`.
const SNAPSHOT_MAX_TASKS: usize = 30;

// ─── Internal planning types ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub agent_id: String,
    pub description: String,
    #[serde(default)]
    pub context: String,
    /// Workspace-relative directory the agent must start in. `None` (or an
    /// empty/"." value) means the workspace root. Supplied per task by the
    /// planner or by the main agent's `call_subagent` call — never by the
    /// agent definition, so one agent can work on different projects.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TaskPlan {
    #[serde(default)]
    tasks: Vec<Task>,
    #[serde(default)]
    execution_mode: String,
    /// Planner's own answer to "does this request need sub-agents at all?".
    /// `None` keeps the old behaviour (delegate when tasks are present).
    #[serde(default)]
    delegate: Option<bool>,
    #[serde(default)]
    why: String,
}

/// The planner's first decision: answer directly, or execute a mission.
#[derive(Debug)]
enum PlanDecision {
    Direct { why: String },
    Mission(TaskPlan),
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

impl TaskResult {
    /// A task that never ran (unknown agent, unmet dependency, user stop).
    /// Reported instead of dropped so the mission report and the UI can show
    /// *why* a planned task produced nothing.
    fn skipped(task: &Task, agent_name: &str, reason: impl Into<String>) -> Self {
        Self {
            task_id: task.id.clone(),
            agent_id: task.agent_id.clone(),
            agent_name: agent_name.to_string(),
            status: "skipped".to_string(),
            content: reason.into(),
            tool_calls_count: 0,
            tokens_used: 0,
        }
    }
}

const AGENT_CONTEXT_COMPRESSION_THRESHOLD: f32 = 0.8;
/// Safety ceiling: a long tool loop compresses even if the model window is
/// huge. The token budget is the primary trigger; this only bounds pathological
/// rounds. It is deliberately well above the 12-message window kept on
/// compression, so normal work is never summarised early.
const AGENT_WORKING_MEMORY_MESSAGES: usize = 60;
/// Messages kept verbatim after a compression (same as the main chat).
const AGENT_MIN_RECENT_MESSAGES: usize = 12;
const AGENT_SUMMARY_MAX_TOKENS: u32 = 768;
const AGENT_DEFAULT_CONTEXT_WINDOW: usize = 131_072;
const AGENT_CONTINUE_PROMPT: &str = "Continue autonomously. Re-read the injected mission snapshot and active task list. Use the external task tools to add, update, or complete tasks, and only call mark_mission_accomplished when the mission is truly done.";

// ─── Main-agent help (`ask_main_agent`) ──────────────────────────────────────
/// Tool rounds the helper may run before it must answer.
const MAIN_HELPER_MAX_ITERATIONS: u32 = 8;
/// Wall-clock budget for one `ask_main_agent` exchange.
const MAIN_HELPER_TIMEOUT_SECS: u64 = 180;
/// How many times one sub-agent task may ask the main agent.
const MAIN_HELPER_MAX_CALLS: u32 = 3;
/// Session history handed to the helper: entries, per-entry bytes, total bytes.
const MAIN_HELPER_HISTORY_MESSAGES: usize = 12;
const MAIN_HELPER_HISTORY_MESSAGE_BYTES: usize = 2_000;
const MAIN_HELPER_HISTORY_BUDGET_BYTES: usize = 12_000;
/// Hard cap for the material returned to the sub-agent.
const MAIN_HELPER_ANSWER_MAX_CHARS: usize = 20_000;
/// Cap for one tool result inside the helper loop, so a huge file read or MCP
/// response cannot blow up the helper's context before it answers.
const MAIN_HELPER_TOOL_RESULT_MAX_BYTES: usize = 32_000;

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
    /// Mission message bus of the orchestration batch that produced this
    /// mission. `None` for direct `call_subagent` runs and old rows.
    pub bus_id: Option<String>,
    pub root_task_description: String,
    pub root_task_context: String,
    pub status: String,
    pub mission_accomplished: bool,
    pub episodic_summary: String,
    pub final_report: String,
    /// One line per `ask_main_agent` exchange, newest last.
    pub help_notes: String,
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
    bus_id: Option<&str>,
) -> Result<(), String> {
    let mission_id = mission_id_for_task(task);
    // Planner task ids (`t1`, `t2`, …) repeat across turns in one session, so
    // the upsert must reset the previous run's state: otherwise a new task
    // would start "accomplished" with the old final report and help log.
    db.execute(
        "INSERT INTO agent_missions (mission_id, session_id, agent_id, root_task_description, root_task_context, bus_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(mission_id) DO UPDATE SET \
           session_id = excluded.session_id, \
           agent_id = excluded.agent_id, \
           root_task_description = excluded.root_task_description, \
           root_task_context = excluded.root_task_context, \
           bus_id = excluded.bus_id, \
           status = 'running', \
           mission_accomplished = 0, \
           episodic_summary = '', \
           final_report = '', \
           help_notes = '', \
           created_at = CURRENT_TIMESTAMP, \
           updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![mission_id, session_id, agent.id, task.description, task.context, bus_id],
    )
    .map_err(|e| e.to_string())?;

    // A planner task id repeats across turns (`t1`, `t2`, …). The root task is
    // re-seeded below; stale sub-tasks from the previous run must not leak into
    // the new mission's active list (the planner supervisor reads it too).
    db.execute(
        "DELETE FROM agent_tasks WHERE mission_id = ?1 AND task_id != ?1",
        rusqlite::params![mission_id],
    )
    .map_err(|e| e.to_string())?;

    db.execute(
        "INSERT INTO agent_tasks (task_id, mission_id, name, description, status) \
         VALUES (?1, ?2, ?3, ?4, 'in_progress') \
         ON CONFLICT(task_id) DO UPDATE SET \
           mission_id = excluded.mission_id, \
           name = excluded.name, \
           description = excluded.description, \
           status = 'in_progress', \
           created_at = CURRENT_TIMESTAMP, \
           updated_at = CURRENT_TIMESTAMP, \
           completed_at = NULL",
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
             WHERE mission_id = ?1 AND status NOT IN ('completed', 'failed', 'stopped') \
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
    let updated = db.execute(
        "UPDATE agent_missions SET status = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1 AND mission_accomplished = 0 AND status = 'running'",
        rusqlite::params![mission_id, status],
    );
    // A failed/stopped mission must not leave its root task (or any internal
    // sub-task) showing as `in_progress` in the Mission panel. `agent_tasks`
    // only knows pending/in_progress/completed in the tool API, so the terminal
    // failed/stopped values are written internally and excluded by
    // `get_active_mission_tasks`.
    if updated.unwrap_or(0) > 0 {
        let _ = db.execute(
            "UPDATE agent_tasks SET status = ?2, updated_at = CURRENT_TIMESTAMP \
             WHERE mission_id = ?1 \
               AND status NOT IN ('completed', 'failed', 'stopped')",
            rusqlite::params![mission_id, status],
        );
    }
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

    let mut lines: Vec<String> = tasks
        .iter()
        .take(SNAPSHOT_MAX_TASKS)
        .map(|task| {
            format!(
                "- [{}] {} (id: {})\n  {}",
                task.status,
                task.name,
                task.task_id,
                preview_text(&task.description, SNAPSHOT_TASK_DESC_MAX_CHARS)
            )
        })
        .collect();
    if tasks.len() > SNAPSHOT_MAX_TASKS {
        // The snapshot is re-sent every iteration; a long list is pure weight.
        // The full list stays one `get_active_tasks` call away.
        lines.push(format!(
            "- … {} more task(s) — call get_active_tasks for the full list",
            tasks.len() - SNAPSHOT_MAX_TASKS
        ));
    }
    lines.join("\n")
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
    let self_evolution_block = if self_evolution_context.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n{}", self_evolution_context)
    };
    format!(
        "{}\n\nAutonomous execution sub-agent. External mission state is authoritative for task tracking.\n\
         Rules: track tasks via add_task/update_task_status/get_active_tasks/mark_mission_accomplished \
         (not chat history); keep outputs execution-focused; create tasks for branches; \
         clear active tasks before finishing. \
         The latest mission snapshot is appended as the final system message every turn — \
         always treat it as authoritative.{}",
        agent.system_prompt, self_evolution_block,
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
        preview_text(&mission.root_task_context, SNAPSHOT_CONTEXT_MAX_CHARS),
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
    headers: &[(String, String)],
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
        headers,
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
        None,
    )
    .await?;

    Ok(raw.trim().to_string())
}

async fn compress_working_memory_if_needed(
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
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

    // Always keep the recent window verbatim, whether the trigger was the token
    // budget or the message ceiling. Summarising down to the last 60 messages
    // after a 61-message runaway would leave the summary far too large.
    let keep_count = AGENT_MIN_RECENT_MESSAGES.min(working_memory.len());

    if working_memory.len() <= keep_count || keep_count == 0 {
        return Ok(false);
    }

    let split_index = working_memory.len() - keep_count;
    let older_messages: Vec<Value> = working_memory.drain(..split_index).collect();
    let merged_summary = summarize_agent_memory(
        client,
        url,
        api_key,
        headers,
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
    // Everything read from the file is, by definition, configured. The
    // resolver marks discovered-but-unconfigured agents with `false`.
    for agent in &mut config.agents {
        agent.configured = true;
    }
    config
}

/// Load the **configured** roster with definitions from the files they
/// reference. This never scans `agents/`: discovery happens only in
/// `refresh_agent_index` (the panel's Refresh button), which writes
/// `agents/index.yaml`.
pub fn load_configured_agents(path: &Path) -> AgentsConfig {
    let mut config = load_agents_config(path);
    let report = files::resolve_configured(&mut config, &files::agents_root(path));
    if report.changed {
        let _ = save_agents_config_file(path, &config);
    }
    config
}

/// Full panel roster: configured agents plus the discovered entries stored in
/// `agents/index.yaml`. Reading one indexed file is not discovery; a missing
/// index simply means only configured agents are shown until Refresh.
pub fn load_sub_agent_list(path: &Path) -> Vec<SubAgent> {
    let mut config = load_configured_agents(path);
    let root = files::agents_root(path);
    if let Some(index) = files::load_index(&root) {
        files::append_index_agents(&mut config.agents, &index, &root);
    }
    config.agents
}

pub(crate) fn save_agents_config_file(path: &Path, config: &AgentsConfig) -> Result<(), String> {
    let mut config = config.clone();
    normalize_config(&mut config);
    config.version = default_config_version();

    // Virtual agents exist only in the resolved view: they have a file but no
    // config entry yet. A legacy inline entry (no file) is kept until it can
    // be migrated, so a read-only agents directory never loses the prompt.
    config.agents.retain(|agent| {
        if agent.agent_type != AGENT_TYPE_LLM {
            return true;
        }
        !(agent.file.is_some() && !agent.configured)
    });

    for agent in &mut config.agents {
        agent.configured = false;
        agent.parse_error = None;
        if agent.agent_type == AGENT_TYPE_LLM && agent.file.is_some() {
            // The definition lives in the markdown file. Only the runtime
            // configuration and the file reference belong in this file.
            agent.name.clear();
            agent.description.clear();
            agent.system_prompt.clear();
            agent.emoji = None;
            agent.color = None;
        } else if agent.agent_type != AGENT_TYPE_LLM {
            // MCP injection is an `llm` capability.
            agent.enable_mcp = false;
        }
    }

    let json = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    let mut json = json;
    // Empty definition fields are not config: strip them from `llm` entries so
    // the file stays readable. The frontend keeps receiving real strings from
    // the resolved view, which is why this is done here and not in a serde
    // `skip_serializing_if` (that would omit `description: ""` from the panel
    // payloads too).
    if let Some(agents) = json
        .get_mut("agents")
        .and_then(|value| value.as_array_mut())
    {
        for agent in agents {
            let Some(object) = agent.as_object_mut() else {
                continue;
            };
            if object.get("agent_type").and_then(|value| value.as_str()) != Some(AGENT_TYPE_LLM) {
                continue;
            }
            for key in ["name", "description", "system_prompt"] {
                if object
                    .get(key)
                    .and_then(|value| value.as_str())
                    .map(str::is_empty)
                    .unwrap_or(false)
                {
                    object.remove(key);
                }
            }
            // Backend settings that only exist for cli/cloud agents.
            for key in ["cli", "cloud", "require_approval"] {
                object.remove(key);
            }
        }
    }
    let json = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_sub_agents(state: State<'_, AppState>) -> Vec<SubAgent> {
    load_sub_agent_list(&state.agents_config_path)
}

/// The panel's Refresh action: migrate configured inline prompts, scan
/// `agents/**/*.md`, **replace** `agents/index.yaml`, and return the merged
/// roster. This is the only path that performs discovery.
#[tauri::command]
pub fn refresh_agent_index(state: State<'_, AppState>) -> Result<Vec<SubAgent>, String> {
    let path = state.agents_config_path.clone();
    let root = files::agents_root(&path);

    // Resolve configured entries first: a legacy inline prompt may create a
    // new markdown file that this refresh should discover.
    let mut config = load_agents_config(&path);
    let report = files::resolve_configured(&mut config, &root);
    if report.changed {
        save_agents_config_file(&path, &config)?;
    }

    let index = files::build_index(&root);
    files::write_index(&root, &index)?;

    let mut agents = config.agents;
    files::append_index_agents(&mut agents, &index, &root);
    Ok(agents)
}

#[tauri::command]
pub fn save_sub_agent(state: State<'_, AppState>, agent: SubAgent) -> Result<SubAgent, String> {
    let path = state.agents_config_path.clone();
    let mut config = load_agents_config(&path);
    let mut a = agent;
    if a.name.trim().is_empty() {
        return Err("The agent name cannot be empty.".to_string());
    }
    a.name = a.name.trim().to_string();
    a.description = a.description.trim().to_string();
    a.agent_type = normalize_agent_type(&a.agent_type);

    // A file-backed `llm` agent must not be converted: the markdown file would
    // be stranded as a second, disabled agent. cli/cloud agents have no file,
    // so their type can still be changed.
    if !a.id.is_empty() {
        if let Some(existing) = config.agents.iter().find(|x| x.id == a.id) {
            let existing_type = normalize_agent_type(&existing.agent_type);
            if existing_type == AGENT_TYPE_LLM && existing_type != a.agent_type {
                return Err(
                    "The agent type cannot be changed after the agent is created.".to_string(),
                );
            }
        }
    }

    match a.agent_type.as_str() {
        AGENT_TYPE_LLM => {
            let root = files::agents_root(&path);
            let existing_file = config
                .agents
                .iter()
                .find(|x| x.id == a.id)
                .and_then(|x| x.file.clone());
            // A configured agent's file is stable: prefer the path recorded in
            // the config so a stale client cannot strand the old markdown file.
            let relative = match existing_file {
                Some(value) => files::normalize_relative(&value)?,
                None => match a.file.clone().filter(|value| !value.trim().is_empty()) {
                    Some(value) => files::normalize_relative(&value)?,
                    None => files::unique_agent_path(&root, files::CUSTOM_CATEGORY, &a.name),
                },
            };

            if let Some(other) = config
                .agents
                .iter()
                .find(|x| x.id != a.id && x.file.as_deref() == Some(relative.as_str()))
            {
                let label = if other.name.trim().is_empty() {
                    other.id.as_str()
                } else {
                    other.name.as_str()
                };
                return Err(format!(
                    "Another agent ('{}') already uses {}.",
                    label, relative
                ));
            }

            let definition = files::AgentDefinition {
                name: a.name.clone(),
                description: a.description.clone(),
                emoji: a
                    .emoji
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                color: a
                    .color
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                prompt: a.system_prompt.clone(),
            };
            files::write_definition(&root, &relative, &definition)?;

            a.file = Some(relative.clone());
            a.configured = true;
            a.parse_error = None;
            if a.id.trim().is_empty() {
                a.id = files::id_for_relative(&relative);
            }
        }
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
            a.file = None;
            a.emoji = None;
            a.color = None;
            a.parse_error = None;
            a.enable_mcp = false;
            a.configured = true;
        }
        AGENT_TYPE_CLOUD => {
            a.cloud.endpoint = a.cloud.endpoint.trim().to_string();
            if a.cloud.endpoint.is_empty() {
                return Err("A cloud agent needs an endpoint URL.".to_string());
            }
            if a.cloud.poll_interval_seconds == 0 {
                a.cloud.poll_interval_seconds = default_poll_interval_seconds();
            }
            a.file = None;
            a.emoji = None;
            a.color = None;
            a.parse_error = None;
            a.enable_mcp = false;
            a.configured = true;
        }
        _ => {}
    }
    if a.id.is_empty() {
        a.id = Uuid::new_v4().to_string();
    }
    if let Some(pos) = config.agents.iter().position(|x| x.id == a.id) {
        config.agents[pos] = a.clone();
    } else {
        config.agents.push(a.clone());
    }
    save_agents_config_file(&path, &config)?;
    Ok(a)
}

#[tauri::command]
pub fn delete_sub_agent(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let path = state.agents_config_path.clone();
    // Resolve first: a discovered (virtual) custom agent has no config entry
    // but still owns a file the user may want to remove. The panel list includes
    // index.yaml entries, so the same merged view is used here.
    let resolved = load_sub_agent_list(&path);
    let found = resolved.iter().find(|agent| agent.id == id).cloned();

    if let Some(agent) = found {
        if agent.agent_type == AGENT_TYPE_LLM {
            if let Some(relative) = agent.file.clone() {
                if !files::is_custom_path(&relative) {
                    return Err(
                        "Library agent files cannot be deleted. Disable the agent instead."
                            .to_string(),
                    );
                }
                files::delete_definition(&files::agents_root(&path), &relative)?;
            }
        }
    }

    let mut config = load_agents_config(&path);
    config.agents.retain(|a| a.id != id);
    save_agents_config_file(&path, &config)
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
        bus_id: Option<String>,
        root_task_description: String,
        root_task_context: String,
        status: String,
        mission_accomplished: bool,
        episodic_summary: String,
        final_report: String,
        help_notes: String,
        created_at: String,
        updated_at: String,
    }

    let agent_names = load_configured_agents(&state.agents_config_path)
        .agents
        .into_iter()
        .map(|agent| (agent.id, agent.name))
        .collect::<std::collections::HashMap<_, _>>();

    let db = state.db.lock().unwrap();
    let rows: Vec<MissionRow> = {
        let mut stmt = db
            .prepare(
                "SELECT mission_id, COALESCE(session_id, ''), agent_id, bus_id, \
                        root_task_description, root_task_context, status, \
                        mission_accomplished, episodic_summary, final_report, \
                        COALESCE(help_notes, ''), \
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
                    bus_id: row.get(3)?,
                    root_task_description: row.get(4)?,
                    root_task_context: row.get(5)?,
                    status: row.get(6)?,
                    mission_accomplished: row.get::<_, i64>(7)? != 0,
                    episodic_summary: row.get(8)?,
                    final_report: row.get(9)?,
                    help_notes: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
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
                bus_id: row.bus_id,
                root_task_description: row.root_task_description,
                root_task_context: row.root_task_context,
                status: row.status,
                mission_accomplished: row.mission_accomplished,
                episodic_summary: row.episodic_summary,
                final_report: row.final_report,
                help_notes: row.help_notes,
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
    headers: &[(String, String)],
    mut messages: Vec<Value>,
    model: &str,
    max_complete_tokens: u32,
    temperature: Option<f32>,
) -> Result<String, String> {
    sanitize_tool_pairs(&mut messages);
    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    if let Some(temperature) = temperature {
        // Planner/selector calls want the same answer every time. Some
        // OpenAI-compatible gateways ignore this field; harmless when they do.
        body["temperature"] = json!(temperature);
    }
    // No DS-Format normalization needed here: these prompts only ever carry
    // system/user messages plus (for a repair retry) the model's own previous
    // text, never a streamed assistant turn with tool calls.
    apply_completion_token_limit(
        &mut body,
        Some(max_complete_tokens),
        Some(LLM_DEFAULT_MAX_TOKENS),
    );
    let mut request = client.post(url);
    if !has_authorization_header(headers) {
        request = request.bearer_auth(api_key);
    }
    let res = apply_request_headers(request, headers)
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

// ─── Planning helpers (pure, unit-tested) ─────────────────────────────────────

/// App-log writer for orchestration diagnostics. `plan_tasks` and the selector
/// run outside the chat turn, so they cannot use `llm_complete::log_event`.
fn log_agent_event(app: &AppHandle, level: &str, message: String) {
    let state = app.state::<AppState>();
    let logger = state.logger.lock();
    if let Ok(logger) = logger {
        logger.log(level, &message);
    }
}

/// Announces one tool call on a sub-agent's task row (`agent-task-tool`).
///
/// The row must keep its `running` status while the tool executes: the generic
/// `tool-call` event belongs to the main chat, whose listener settles every
/// running step and appends a row. This event only updates the task's activity
/// line, so the UI shows what the agent is doing during rounds that produce no
/// content at all (`text_chars=0`, tool_calls only).
fn emit_agent_tool_state(
    app: &AppHandle,
    task_id: &str,
    phase: &str,
    tool: &str,
    detail: Option<String>,
    duration_ms: Option<u128>,
    ok: Option<bool>,
) {
    let mut payload = json!({
        "task_id": task_id,
        "phase": phase,
        "tool": tool,
    });
    if let Some(detail) = detail {
        payload["detail"] = json!(detail);
    }
    if let Some(duration_ms) = duration_ms {
        payload["duration_ms"] = json!(duration_ms.min(u64::MAX as u128) as u64);
    }
    if let Some(ok) = ok {
        payload["ok"] = json!(ok);
    }
    let _ = app.emit("agent-task-tool", payload);
}

/// Provider balance/quota failures are terminal for this account: retrying or
/// spending more iterations on the same request only burns the turn. The text
/// is provider-specific, so match the common 402/quota phrasings.
fn is_provider_balance_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("insufficient account funds")
        || lower.contains("insufficient_quota")
        || lower.contains("free usage limit")
        || lower.contains("requires more credits")
        || lower.contains("credit balance")
        || lower.contains("billing")
        || lower.contains("http 402")
        || lower.contains(" 402 ")
}

/// UTF-8-safe preview for log lines and prompts (`limit` is bytes).
fn preview_text(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len() + c.len_utf8() > limit {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

/// Joins a capability list for the selector catalogue. Only the first few
/// entries matter for routing; the rest would be pure prompt weight.
fn compact_list(items: &[String]) -> String {
    if items.is_empty() {
        return "-".to_string();
    }
    let mut text = items
        .iter()
        .take(ROSTER_LIST_MAX_ITEMS)
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    if items.len() > ROSTER_LIST_MAX_ITEMS {
        text.push_str(",…");
    }
    text
}

/// Returns the first balanced `{…}` object in `raw`, ignoring braces inside
/// JSON strings. Models routinely wrap the payload in a markdown fence, a
/// preamble or a trailing sentence; a strict `from_str` would throw all of that
/// away and waste the whole mission.
fn extract_json_object(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);

    let start = body.find('{')?;
    let bytes = body.as_bytes();
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return body.get(start..offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Conservative repair of the near-JSON models actually emit: trailing commas,
/// typographic quotes and `//` line comments. Characters inside string literals
/// are preserved (except a typographic closing quote, which is normalized).
fn repair_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' if !escaped => {
                    escaped = true;
                    out.push(c);
                }
                '"' if !escaped => {
                    in_string = false;
                    out.push(c);
                }
                '”' if !escaped => {
                    in_string = false;
                    out.push('"');
                }
                _ => {
                    escaped = false;
                    out.push(c);
                }
            }
            continue;
        }

        match c {
            '"' | '“' => {
                in_string = true;
                out.push('"');
            }
            '”' => out.push('"'),
            '/' if chars.peek() == Some(&'/') => {
                // Drop the comment but keep its newline so tokens stay apart.
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ',' => {
                // A trailing comma: only whitespace may follow before a closer.
                let mut look = chars.clone();
                while matches!(look.peek(), Some(next) if next.is_whitespace()) {
                    look.next();
                }
                if matches!(look.peek(), Some('}') | Some(']')) {
                    continue;
                }
                out.push(',');
            }
            _ => out.push(c),
        }
    }
    out
}

/// Finds one dependency edge that closes a cycle, if any: `(task, dependency)`.
/// Removing that edge (repeatedly) guarantees the executor can make progress.
fn find_cycle_edge(tasks: &[Task]) -> Option<(String, String)> {
    use std::collections::HashMap;

    let index: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, task)| (task.id.as_str(), i))
        .collect();
    // 0 = unvisited, 1 = on the current DFS stack, 2 = finished.
    let mut state = vec![0u8; tasks.len()];

    fn dfs(
        i: usize,
        tasks: &[Task],
        index: &std::collections::HashMap<&str, usize>,
        state: &mut [u8],
    ) -> Option<(String, String)> {
        state[i] = 1;
        for dep in &tasks[i].dependencies {
            let Some(&j) = index.get(dep.as_str()) else {
                continue;
            };
            if state[j] == 1 {
                return Some((tasks[i].id.clone(), dep.clone()));
            }
            if state[j] == 0 {
                if let Some(edge) = dfs(j, tasks, index, state) {
                    return Some(edge);
                }
            }
        }
        state[i] = 2;
        None
    }

    for i in 0..tasks.len() {
        if state[i] == 0 {
            if let Some(edge) = dfs(i, tasks, &index, &mut state) {
                return Some(edge);
            }
        }
    }
    None
}

/// Turns raw planner output into an executable plan.
///
/// Models return fences, preambles, trailing commas, hallucinated agent ids,
/// duplicate task ids and cyclic dependencies. None of those should waste the
/// turn: the plan is extracted, repaired and sanitized here, and every fix is
/// reported back as an issue for the log. Only an unusable answer fails.
fn parse_task_plan(raw: &str, roster: &[String]) -> Result<(PlanDecision, Vec<String>), String> {
    use std::collections::HashSet;

    let json = extract_json_object(raw)
        .ok_or_else(|| "no JSON object in the planner output".to_string())?;
    let plan: TaskPlan = serde_json::from_str(&repair_json(json)).map_err(|e| e.to_string())?;

    // The planner is allowed to look at the request and decline delegation
    // before planning anything. A missing `delegate` keeps the old contract:
    // tasks mean delegation.
    let delegate = plan.delegate.unwrap_or(!plan.tasks.is_empty());
    if !delegate {
        let mut issues: Vec<String> = Vec::new();
        if !plan.tasks.is_empty() {
            issues.push(format!(
                "planner returned delegate=false with {} task(s); ignored them",
                plan.tasks.len()
            ));
        }
        return Ok((
            PlanDecision::Direct {
                why: plan.why.trim().to_string(),
            },
            issues,
        ));
    }

    let mut issues: Vec<String> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut used_ids: HashSet<String> = HashSet::new();

    for mut task in plan.tasks {
        if task.description.trim().is_empty() {
            issues.push(format!("dropped task '{}': empty description", task.id));
            continue;
        }
        if !roster.iter().any(|id| id == &task.agent_id) {
            issues.push(format!(
                "dropped task '{}': unknown agent '{}'",
                task.id, task.agent_id
            ));
            continue;
        }

        // `mission_id == task.id`, so duplicates would corrupt mission rows and
        // dependency bookkeeping. Rename instead of dropping the work. `planner`
        // is reserved for the UI/supervisor row.
        let original = task.id.trim().to_string();
        if original == mbus::PLANNER_ID {
            issues.push("renamed reserved task id 'planner' to 'task'".to_string());
        }
        let base = if original.is_empty() || original == mbus::PLANNER_ID {
            "task"
        } else {
            original.as_str()
        }
        .to_string();
        let mut unique = base.clone();
        let mut suffix = 1usize;
        while used_ids.contains(&unique) {
            suffix += 1;
            unique = format!("{base}-{suffix}");
        }
        if unique != base {
            issues.push(format!("renamed duplicate task id '{base}' to '{unique}'"));
        }
        used_ids.insert(unique.clone());
        task.id = unique;
        task.description = task.description.trim().to_string();
        task.context = task.context.trim().to_string();
        // The planner may write "", "." or "./" for "the workspace root".
        task.cwd = normalize_task_cwd(task.cwd.as_deref());
        tasks.push(task);
    }

    if tasks.len() > MAX_PLAN_TASKS {
        issues.push(format!(
            "truncated plan from {} to {} tasks",
            tasks.len(),
            MAX_PLAN_TASKS
        ));
        tasks.truncate(MAX_PLAN_TASKS);
    }

    let known: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();

    // Dependencies: only ids that actually exist, never self-references.
    for task in &mut tasks {
        let before = task.dependencies.len();
        let mut seen = HashSet::new();
        task.dependencies
            .retain(|dep| dep != &task.id && known.contains(dep) && seen.insert(dep.clone()));
        if task.dependencies.len() != before {
            issues.push(format!(
                "dropped {} invalid dependency(ies) from task '{}'",
                before - task.dependencies.len(),
                task.id
            ));
        }
    }

    // Cycles: break one edge at a time so every remaining task stays reachable
    // by the wave scheduler instead of hanging it on an empty ready set.
    for _ in 0..=known.len() {
        let Some((task_id, dep_id)) = find_cycle_edge(&tasks) else {
            break;
        };
        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
            task.dependencies.retain(|dep| dep != &dep_id);
        }
        issues.push(format!(
            "dropped dependency '{task_id}' -> '{dep_id}' to break a cycle"
        ));
    }

    if tasks.is_empty() {
        return Err(if issues.is_empty() {
            "the planner produced no tasks".to_string()
        } else {
            format!(
                "the planner produced no usable tasks: {}",
                issues.join("; ")
            )
        });
    }

    Ok((
        PlanDecision::Mission(TaskPlan {
            tasks,
            execution_mode: plan.execution_mode,
            delegate: Some(true),
            why: plan.why,
        }),
        issues,
    ))
}

/// Announces the selector's decision to the UI (chip above the answer).
fn emit_agent_selection(app: &AppHandle, selection: &AgentSelection) {
    let _ = app.emit(
        "agent-selection",
        json!({
            "delegate": selection.delegate,
            "confidence": selection.confidence,
            "agents": &selection.choices,
        }),
    );
}

// ─── Orchestration entry point ────────────────────────────────────────────────

/// Main orchestration entry point called from llm_complete.rs.
///
/// `cancelled` is THIS turn's cancel flag (the per-turn flag swapped in by
/// `chat_completion`), so the mission observes the Stop of its own turn only.
pub async fn orchestrate(
    app: &AppHandle,
    state: &AppState,
    config: &AppConfig,
    messages: &[crate::llm_complete::ChatMessage],
    session_id: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<OrchestrationOutcome, String> {
    let agents_config = load_configured_agents(&state.agents_config_path);
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
    // Same headers as the main chat: configured conversation-id header plus
    // the user's custom headers, so planning calls reach the same gateway.
    let headers = upstream_request_headers(config, session_id);

    // Step 1: one selection call decides who takes part in this turn. With the
    // switch off every enabled agent takes part, exactly like before.
    let (working_agents, selection) = if orchestration.auto_configure {
        match select_agents(
            app,
            &client,
            &url,
            &config.api_key,
            &headers,
            &model,
            messages,
            &enabled,
        )
        .await
        {
            Ok(result) => (result.agents, Some(result.selection)),
            Err(err) => {
                // A failed selection must never lose the mission: fall back to
                // the full roster and let the planner decide.
                log_agent_event(
                    app,
                    "WARN",
                    format!("agent selection failed, using every enabled agent: {err}"),
                );
                (enabled.clone(), None)
            }
        }
    } else {
        (enabled.clone(), None)
    };

    // The selector can decide the request needs no sub-agents at all. Skipping
    // planning here saves a full LLM round trip and keeps simple questions on
    // the direct-answer path.
    if matches!(&selection, Some(selection) if !selection.delegate) {
        return Ok(OrchestrationOutcome {
            report: None,
            selection,
        });
    }

    // Step 2: Plan. Stop pressed between steps aborts the mission here, before
    // any sub-agent is launched.
    let _ = app.emit("agent-plan-start", json!({ "task_count": 0 }));
    if cancelled.load(Ordering::SeqCst) {
        return Err("stopped by the user".to_string());
    }
    // The planner needs the workspace root to name task directories: the main
    // agent decides *where* each delegated task runs, not the agent definition.
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    // The batch root is reserved before planning so the planner can reference
    // the deliverable exchange area in task context. The directory itself is
    // created only when the planner actually delegates.
    let exchange_root = exchange::ExchangeArea::new_batch_root(&workspace_dir);
    let decision = plan_tasks(
        app,
        &client,
        &url,
        &config.api_key,
        &headers,
        &model,
        messages,
        &working_agents,
        &workspace_dir,
        &exchange_root,
    )
    .await?;

    if cancelled.load(Ordering::SeqCst) {
        return Err("stopped by the user".to_string());
    }

    // The planner has the final say: a request that one direct answer can
    // handle exits here, before a mission, bus or supervisor is created.
    let plan = match decision {
        PlanDecision::Direct { why } => {
            let why = if why.trim().is_empty() {
                "the planner decided this request needs no sub-agents".to_string()
            } else {
                why
            };
            log_agent_event(app, "INFO", format!("planner declined delegation: {why}"));
            let declined = AgentSelection {
                delegate: false,
                confidence: selection
                    .as_ref()
                    .map(|selection| selection.confidence)
                    .unwrap_or(1.0),
                choices: Vec::new(),
            };
            if enabled.len() > 1 {
                emit_agent_selection(app, &declined);
            }
            return Ok(OrchestrationOutcome {
                report: None,
                selection: Some(declined),
            });
        }
        PlanDecision::Mission(plan) => plan,
    };

    // Emit the selector chip only after the planner confirmed the mission, so a
    // declined plan never leaves a stale "agents selected" row in the UI.
    if let Some(selection) = &selection {
        if enabled.len() > 1 {
            emit_agent_selection(app, selection);
        }
    }

    let exchange_area = match exchange::ExchangeArea::create_at(
        exchange_root,
        &workspace_dir,
        &plan
            .tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>(),
    ) {
        Ok(area) => Some(area),
        Err(err) => {
            log_agent_event(
                app,
                "WARN",
                format!("deliverable exchange area unavailable: {err}"),
            );
            None
        }
    };

    // Step 3: Create the mission bus for this batch. A batch with no in-app LLM
    // agent has no peer that can call mbus tools, so it keeps the old behaviour.
    let mission_bus: Option<Arc<mbus::MissionBus>> = if orchestration.mbus_enabled {
        let bus = Arc::new(
            mbus::MissionBus::new(session_id, &plan.tasks, &working_agents)
                .with_pair_limit(orchestration.mbus_pair_limit),
        );
        if bus.has_llm_peer() {
            Some(bus)
        } else {
            None
        }
    } else {
        None
    };
    let _ = app.emit(
        "agent-plan-start",
        json!({
            "task_count": plan.tasks.len(),
            "mbus": mission_bus.is_some(),
            "bus_id": mission_bus.as_ref().map(|bus| bus.bus_id()),
        }),
    );

    // Step 4: Execute. The planner supervisor runs concurrently with the tasks;
    // a oneshot from the executor cancels a slow supervisor call the moment the
    // last task settles, so supervision never delays the final answer.
    let use_parallel = orchestration.mode != "sequential" && plan.execution_mode != "sequential";
    if let Some(bus) = &mission_bus {
        let _ = app.emit(
            "agent-planner-action",
            json!({
                "session_id": session_id,
                "bus_id": bus.bus_id(),
                "task_id": "planner",
                "action_id": "planner-schedule",
                "activation": 0,
                "round": 0,
                "action": "schedule",
                "status": "done",
                "target": "plan",
                "detail": format!(
                    "{} task(s) · {} · max concurrent {}",
                    plan.tasks.len(),
                    if use_parallel { "parallel" } else { "sequential" },
                    orchestration.max_concurrent
                ),
                "created_at_ms": chrono::Local::now().timestamp_millis(),
            }),
        );
    }
    let skill_access_root =
        crate::skills::self_evolution_skill_root(Some(&workspace_dir), config.self_evolution_mode);
    let supervisor_tasks = plan.tasks.clone();
    let bus_for_executor = mission_bus.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let executor_client = client.clone();
    let executor_url = url.clone();
    let executor_cancelled = cancelled.clone();
    let executor_workspace = workspace_dir.clone();
    let executor_exchange = exchange_area.clone();
    let executor = async move {
        let results = execute_tasks(
            app,
            &executor_client,
            &executor_url,
            config,
            session_id,
            &working_agents,
            plan.tasks,
            executor_workspace,
            executor_exchange,
            skill_access_root,
            use_parallel,
            orchestration.max_concurrent,
            &executor_cancelled,
            bus_for_executor,
        )
        .await;
        let _ = done_tx.send(());
        results
    };
    let supervisor_bus = mission_bus.clone();
    let supervisor_client = client.clone();
    let supervisor_url = url.clone();
    let supervisor_model = model.clone();
    let supervisor_cancelled = cancelled.clone();
    let supervisor = async move {
        if let Some(bus) = supervisor_bus {
            supervisor::run_planner_supervisor(
                app,
                &supervisor_client,
                &supervisor_url,
                config,
                session_id,
                &supervisor_model,
                messages,
                &supervisor_tasks,
                bus,
                supervisor_cancelled,
                done_rx,
            )
            .await;
        }
    };
    let (results, _) = tokio::join!(executor, supervisor);
    if let Some(bus) = &mission_bus {
        bus.close();
    }

    // Step 5: Hand the mission record to the caller. Sub-agents are execution
    // workers — they never write the user-facing answer: the main agent (full
    // chat context, skills and tools) defines the final result, so
    // llm_complete injects this report and runs its normal completion flow.
    //
    // The aggregation itself is instantaneous (it only renders the report
    // below), so the event reports a *completed* step: a "start" event here
    // would leave the UI spinning until the whole final answer is streamed.
    let _ = app.emit(
        "agent-aggregate-done",
        json!({ "task_count": results.len() }),
    );
    Ok(OrchestrationOutcome {
        report: Some(build_mission_results_block(
            &results,
            exchange_area.as_ref(),
        )),
        selection,
    })
}

// ─── Selection ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SelectionResponse {
    #[serde(default)]
    delegate: Option<bool>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    agents: Vec<SelectionChoice>,
    #[serde(default)]
    why: String,
}

#[derive(Debug, Deserialize)]
struct SelectionChoice {
    #[serde(default)]
    id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    why: String,
}

impl SelectionChoice {
    fn role_or(&self, fallback: &str) -> String {
        let role = self.role.trim();
        if role.is_empty() {
            fallback.chars().take(60).collect()
        } else {
            role.to_string()
        }
    }
}

/// Every enabled agent takes part. Used both by the deterministic single-agent
/// path and by every "the model answer cannot be trusted" fallback.
fn full_roster_selection(existing: &[SubAgent], confidence: f32, why: &str) -> SelectionResult {
    SelectionResult {
        agents: existing.to_vec(),
        selection: AgentSelection {
            delegate: !existing.is_empty(),
            confidence,
            choices: existing
                .iter()
                .map(|agent| AgentChoice {
                    id: agent.id.clone(),
                    name: agent.name.clone(),
                    role: agent.description.chars().take(60).collect(),
                    why: why.to_string(),
                })
                .collect(),
        },
    }
}

/// One non-streaming LLM call that decides which agents take part in this turn.
///
/// The answer is a *proposal*: ids are validated against the live roster, the
/// list is capped, and every failure widens back to the full roster instead of
/// failing the mission. `delegate: false` is a first-class outcome — it skips
/// planning entirely.
async fn select_agents(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    existing: &[SubAgent],
) -> Result<SelectionResult, String> {
    // One agent means there is nothing to choose: skip the request entirely.
    if existing.len() <= 1 {
        return Ok(full_roster_selection(
            existing,
            1.0,
            "the only enabled agent",
        ));
    }

    let catalogue = existing
        .iter()
        .map(|agent| {
            format!(
                "- id={} name={} type={} tools={} skills={} mcp={} desc={}",
                agent.id,
                agent.name,
                normalize_agent_type(&agent.agent_type),
                compact_list(&agent.allowed_tools),
                compact_list(&agent.allowed_skills),
                if agent.enable_mcp { "enabled" } else { "none" },
                preview_text(&agent.description, ROSTER_DESC_MAX_BYTES)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user_query = preview_text(
        &messages
            .last()
            .map(|m| content_to_text(&m.content))
            .unwrap_or_default(),
        SELECTION_QUERY_MAX_BYTES,
    );

    // An attachment-only turn has no text for the selector to judge. Narrowing
    // on an empty query would wrongly decline delegation, so skip the call.
    if user_query.trim().is_empty() {
        return Ok(full_roster_selection(existing, 1.0, "no text to analyse"));
    }

    let system = format!(
        "You are an AI orchestrator. Decide which sub-agents should work on the user's request.\n\
         Existing agents:\n{catalogue}\n\n\
         Rules:\n\
         - Pick the smallest set that can do the job: 0 to {MAX_SELECTED_AGENTS} agents.\n\
         - Use `llm` agents for reasoning, analysis and writing; `cli` agents for large code changes in the workspace; `cloud` agents for long remote jobs.\n\
         - Match capabilities to the task. A live web/browser/API data task needs an agent with `mcp=enabled` or browser/network tools. Do not pick a file-only analyst merely because its domain matches. If no agent has the required data channel, return delegate=false so the main agent (which has MCP/browser tools) handles the live fetch.\n\
         - If no agent fits, or the request can be answered directly, return delegate=false.\n\
         - Copy every id exactly from the list above. Never invent ids.\n\n\
         Return ONLY one JSON object — no markdown, no commentary:\n\
         {{\"delegate\":true,\"confidence\":0.0,\"agents\":[{{\"id\":\"<id>\",\"role\":\"<short role>\",\"why\":\"<one short sentence>\"}}]}}\n\
         or {{\"delegate\":false,\"confidence\":0.0,\"why\":\"<one short sentence>\"}}",
    );

    let raw = call_llm_once(
        client,
        url,
        api_key,
        headers,
        vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": user_query}),
        ],
        model,
        1024,
        Some(0.0),
    )
    .await?;

    let json = extract_json_object(&raw).ok_or_else(|| {
        format!(
            "no JSON in the selection output: {}",
            preview_text(&raw, 300)
        )
    })?;
    let parsed: SelectionResponse = serde_json::from_str(&repair_json(json))
        .map_err(|e| format!("selection parse error: {e}"))?;
    // A model that lists agents but forgets `delegate` still meant to delegate;
    // an explicit `false` always wins. A missing confidence is treated as
    // "reasonably sure" — otherwise every terse answer would widen the roster.
    let delegate = parsed.delegate.unwrap_or(!parsed.agents.is_empty());
    let confidence = parsed.confidence.unwrap_or(0.7).clamp(0.0, 1.0);

    if !delegate {
        log_agent_event(
            app,
            "INFO",
            format!(
                "agent selection declined delegation (confidence {confidence:.2}): {}",
                preview_text(parsed.why.trim(), 200)
            ),
        );
        return Ok(SelectionResult {
            agents: Vec::new(),
            selection: AgentSelection {
                delegate: false,
                confidence,
                choices: Vec::new(),
            },
        });
    }

    let mut choices: Vec<AgentChoice> = Vec::new();
    let mut agents: Vec<SubAgent> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for choice in parsed.agents {
        let id = choice.id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let Some(agent) = existing.iter().find(|agent| agent.id == id) else {
            // Hallucinated id: never hand it to the planner.
            log_agent_event(
                app,
                "WARN",
                format!("agent selection named unknown agent id '{id}'"),
            );
            continue;
        };
        choices.push(AgentChoice {
            id: agent.id.clone(),
            name: agent.name.clone(),
            role: choice.role_or(&agent.description),
            why: choice.why.trim().to_string(),
        });
        agents.push(agent.clone());
        if agents.len() == MAX_SELECTED_AGENTS {
            break;
        }
    }

    // A selection that matches nothing, or one the model is not confident
    // about, is worse than no narrowing at all: widen to the full roster so the
    // planner still sees every option.
    if agents.is_empty() || confidence < 0.5 {
        let reason = if agents.is_empty() {
            "matched no known agent"
        } else {
            "confidence below 0.5"
        };
        log_agent_event(
            app,
            "WARN",
            format!("agent selection {reason}; falling back to every enabled agent"),
        );
        return Ok(full_roster_selection(
            existing,
            confidence,
            "fallback: full roster",
        ));
    }

    Ok(SelectionResult {
        agents,
        selection: AgentSelection {
            delegate: true,
            confidence,
            choices,
        },
    })
}

// ─── Planning ─────────────────────────────────────────────────────────────────

async fn plan_tasks(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    agents: &[SubAgent],
    workspace_dir: &Path,
    exchange_root: &Path,
) -> Result<PlanDecision, String> {
    let agents_desc = agents
        .iter()
        .map(|a| {
            format!(
                "  - id={} name={} type={} tools={} skills={} mcp={} desc={}",
                a.id,
                a.name,
                normalize_agent_type(&a.agent_type),
                compact_list(&a.allowed_tools),
                compact_list(&a.allowed_skills),
                if a.enable_mcp { "enabled" } else { "none" },
                preview_text(&a.description, ROSTER_DESC_MAX_BYTES)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let roster = agents
        .iter()
        .map(|a| a.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    // Build a short conversation summary (last 3 messages, budgeted per
    // message): the planner only needs the intent and recent corrections.
    let conversation = messages
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|m| {
            let text = content_to_text(&m.content);
            format!(
                "[{}]: {}",
                m.role,
                preview_text(&text, PLANNER_CONVERSATION_MAX_BYTES)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system = format!(
        "You are a task planning expert. FIRST decide whether the request needs sub-agents at all.\n\n\
        Step 1 — delegation decision:\n\
        - Return {{\"delegate\":false,\"why\":\"<one short sentence>\"}} when one direct answer from \
        the main agent is enough: a simple question, a single lookup, a small edit, or a writing task.\n\
        - Return {{\"delegate\":true,...}} only when the work clearly benefits from sub-agents: \
        parallel research, large code changes, long remote jobs, or independent workstreams.\n\
        - When unsure, delegate. A wrong decline loses capability; a needless mission only costs one task.\n\
        - A single task is a valid plan when one agent is enough.\n\n\
        Step 2 — only for delegate:true, plan the tasks.\n\
        Available agents:\n{agents_desc}\n\n\
        Workspace root: {workspace_root}\n\
        Deliverable exchange area: {exchange_root}\n\
        Each task gets its own directory under the exchange area (named after its task id); shared \
        files go to {exchange_root}/shared. In `context`, refer to a dependency's artifact as \
        `<task-id>/<filename>` relative to the exchange root; never paste large artifacts into task \
        descriptions.\n\n\
        Every task may name one working directory with `cwd` (workspace-relative, for example \
        \"apps/web\"). The sub-agent starts in that directory, so use it whenever the request \
        names or clearly targets a specific project/folder; omit it (or use \".\") for the \
        workspace root. Never invent a directory that the conversation does not mention.\n\n\
        Return ONLY valid JSON — no markdown, no extra text:\n\
        {{\"delegate\":false,\"why\":\"...\"}}\n\
        or\n\
        {{\"delegate\":true,\"tasks\":[{{\"id\":\"t1\",\"agent_id\":\"<id>\",\"description\":\"...\",\
          \"context\":\"...\",\"cwd\":\"<workspace-relative dir, omit for root>\",\"dependencies\":[]}}],\
          \"execution_mode\":\"parallel\"}}\n\
        Rules:\n\
        - execution_mode: \"parallel\" when tasks are independent, \"sequential\" when order matters\n\
        - dependencies: list of task ids that must finish before this one\n\
        - Every agent_id must be exactly one of: {roster}\n\
        - Match capabilities to the task: live web/browser/API data collection needs `mcp=enabled` or browser/network tools. Never assign that work to a file-only agent (mcp=none); it will burn iterations scanning local snapshots. If no listed agent can fetch the data, leave the live fetch to the main agent and plan only analysis tasks.\n\
        - Keep descriptions brief and actionable",
        workspace_root = workspace_dir.display(),
        exchange_root = exchange_root.display(),
    );
    let user_prompt = format!("Conversation:\n{conversation}\n\nCreate a task plan.");
    let roster_ids: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();

    let mut last_error = String::new();
    let mut last_raw = String::new();

    // Two attempts: the second one hands the model its own broken answer plus
    // the parse error. A malformed plan must not waste the whole mission, and
    // one repair round is enough — anything beyond that is a model problem.
    for attempt in 0..2 {
        let mut request = vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": user_prompt}),
        ];
        if attempt > 0 {
            request.push(json!({"role":"assistant","content": last_raw}));
            request.push(json!({"role":"user","content": format!(
                "Your previous answer could not be used: {last_error}\nReturn ONLY the JSON object described above — no markdown fence, no commentary."
            )}));
        }

        let raw = call_llm_once(
            client,
            url,
            api_key,
            headers,
            request,
            model,
            2048,
            Some(0.0),
        )
        .await?;

        match parse_task_plan(&raw, &roster_ids) {
            Ok((plan, issues)) => {
                for issue in issues {
                    log_agent_event(app, "WARN", format!("planner: {issue}"));
                }
                return Ok(plan);
            }
            Err(err) => {
                log_agent_event(
                    app,
                    "WARN",
                    format!(
                        "planner attempt {} failed: {err}; output={}",
                        attempt + 1,
                        preview_text(&raw, 500)
                    ),
                );
                last_error = err;
                last_raw = raw;
            }
        }
    }

    Err(format!("Plan parse error after 2 attempts: {last_error}"))
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
    exchange: Option<exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    use_parallel: bool,
    max_concurrent: usize,
    cancelled: &Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
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
            exchange,
            skill_access_root,
            max_concurrent,
            cancelled,
            mbus,
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
            exchange,
            skill_access_root,
            cancelled,
            mbus,
        )
        .await
    }
}

/// Reports a task that never ran. The mission row (if any) is closed so the
/// monitor cannot keep showing it as running, and the UI gets a neutral event
/// instead of a silent gap.
fn finish_skipped(app: &AppHandle, mbus: Option<&mbus::MissionBus>, result: &TaskResult) {
    close_mission(app, &result.task_id, "stopped");
    if let Some(bus) = mbus {
        bus.mark_skipped(result);
    }
    let _ = app.emit(
        "agent-task-skipped",
        json!({
            "task_id": &result.task_id,
            "agent_id": &result.agent_id,
            "agent_name": &result.agent_name,
            "reason": &result.content,
        }),
    );
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
    exchange: Option<exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    max_concurrent: usize,
    cancelled: &Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> Vec<TaskResult> {
    use std::collections::{HashMap, HashSet};
    use tokio::task::JoinSet;

    // `settled` = a result exists (success or error, the latter still lets
    // dependents run, as before). `blocked` = the task never produced a result,
    // so every task depending on it is skipped instead of waiting forever.
    let mut settled: HashSet<String> = HashSet::new();
    let mut blocked: HashSet<String> = HashSet::new();
    let mut pending: Vec<Task> = Vec::new();
    let mut results: Vec<TaskResult> = Vec::new();

    // Whole-batch pre-flight: an unknown agent can never run. Report it here
    // rather than letting its dependents deadlock on a missing result.
    for task in tasks {
        if agents.iter().any(|agent| agent.id == task.agent_id) {
            pending.push(task);
        } else {
            let result = TaskResult::skipped(
                &task,
                &task.agent_id,
                format!("agent '{}' is not available", task.agent_id),
            );
            finish_skipped(app, mbus.as_deref(), &result);
            blocked.insert(result.task_id.clone());
            results.push(result);
        }
    }

    while !pending.is_empty() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        // Cascade skips to a fixpoint: skipping one task can invalidate a whole
        // dependency chain, and those tasks must not be scheduled at all.
        loop {
            let newly: Vec<Task> = pending
                .iter()
                .filter(|task| task.dependencies.iter().any(|dep| blocked.contains(dep)))
                .cloned()
                .collect();
            if newly.is_empty() {
                break;
            }
            for task in newly {
                let missing: Vec<&str> = task
                    .dependencies
                    .iter()
                    .filter(|dep| blocked.contains(*dep))
                    .map(|dep| dep.as_str())
                    .collect();
                let result = TaskResult::skipped(
                    &task,
                    &task.agent_id,
                    format!("skipped because {} did not run", missing.join(", ")),
                );
                finish_skipped(app, mbus.as_deref(), &result);
                blocked.insert(result.task_id.clone());
                results.push(result);
                pending.retain(|candidate| candidate.id != task.id);
            }
        }

        let ready: Vec<Task> = pending
            .iter()
            .filter(|task| task.dependencies.iter().all(|dep| settled.contains(dep)))
            .cloned()
            .collect();

        if ready.is_empty() {
            // Only reachable with a cycle the planner validation missed: never
            // hang the mission and never drop tasks silently.
            for task in pending.drain(..) {
                let missing: Vec<&str> = task
                    .dependencies
                    .iter()
                    .filter(|dep| !settled.contains(*dep))
                    .map(|dep| dep.as_str())
                    .collect();
                let reason = if missing.is_empty() {
                    "dependencies could not be satisfied".to_string()
                } else {
                    format!(
                        "dependencies could not be satisfied: {}",
                        missing.join(", ")
                    )
                };
                let result = TaskResult::skipped(&task, &task.agent_id, reason);
                finish_skipped(app, mbus.as_deref(), &result);
                results.push(result);
            }
            break;
        }

        pending.retain(|task| !ready.iter().any(|candidate| candidate.id == task.id));

        // Process ready tasks in batches of max_concurrent. `next` tracks how
        // far the batch window got, so a Stop between batches can put the
        // unscheduled remainder back into `pending` for the tail below.
        let limit = max_concurrent.max(1);
        let mut next = 0;
        while next < ready.len() {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            let end = (next + limit).min(ready.len());
            let batch = ready[next..end].to_vec();
            next = end;

            let mut join_set: JoinSet<TaskResult> = JoinSet::new();
            // Maps each spawned job to its task so an aborted or panicked job can
            // be attributed and reported instead of silently dropping its result.
            let mut spawned_jobs: HashMap<tokio::task::Id, (String, String, String)> =
                HashMap::new();

            for task in batch {
                let Some(agent) = agents.iter().find(|a| a.id == task.agent_id).cloned() else {
                    // Pre-flight already handled this; keep the guard cheap.
                    continue;
                };

                let app_c = app.clone();
                let client_c = client.clone();
                let url_c = url.to_string();
                let config_c = config.clone();
                let session_id_c = session_id.to_string();
                let task_c = task.clone();
                let wd_c = workspace_dir.clone();
                let exchange_c = exchange.clone();
                let skill_roots_c = skill_access_root.clone();
                // Arc snapshot of the app-global cancel flag: the spawned job
                // and every sub-agent inside it observe the same flag, so Stop
                // tears down mid-flight agents instead of only skipping future
                // tasks.
                let cancelled_c = cancelled.clone();
                let is_cancelled = cancelled.load(Ordering::SeqCst);
                let mbus_c = mbus.clone();
                let job_meta = (task.id.clone(), agent.id.clone(), agent.name.clone());

                let handle = join_set.spawn(async move {
                    if is_cancelled {
                        return TaskResult::skipped(&task_c, &agent.name, "cancelled by user");
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
                        exchange_c.as_ref(),
                        skill_roots_c,
                        cancelled_c,
                        mbus_c,
                    )
                    .await
                });
                spawned_jobs.insert(handle.id(), job_meta);
            }

            while let Some(joined) = join_set.join_next().await {
                match joined {
                    Ok(res) => {
                        // Only a real outcome satisfies a dependency; a stopped
                        // or skipped run must cascade instead.
                        if matches!(res.status.as_str(), "success" | "error") {
                            settled.insert(res.task_id.clone());
                        } else {
                            // A pre-cancelled batch slot returns "skipped" without
                            // going through `run_sub_agent`, so it still needs its
                            // UI event.
                            if res.status == "skipped" {
                                finish_skipped(app, mbus.as_deref(), &res);
                            }
                            blocked.insert(res.task_id.clone());
                        }
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
                        if let Some(bus) = &mbus {
                            bus.finish_task(&result);
                        }
                        let _ = app.emit(
                            "agent-task-error",
                            json!({
                                "task_id": &result.task_id,
                                "agent_id": &result.agent_id,
                                "error": &result.content,
                            }),
                        );
                        settled.insert(result.task_id.clone());
                        results.push(result);
                    }
                }
            }
        }

        if next < ready.len() {
            pending.extend(ready[next..].iter().cloned());
        }
    }

    // Stop pressed: everything that never started is still reported.
    if cancelled.load(Ordering::SeqCst) {
        for task in pending {
            let result = TaskResult::skipped(&task, &task.agent_id, "cancelled by user");
            finish_skipped(app, mbus.as_deref(), &result);
            results.push(result);
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
    exchange: Option<exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: &Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> Vec<TaskResult> {
    use std::collections::HashSet;

    let mut results = Vec::new();
    // Sequential mode never reorders by dependency, but a task whose dependency
    // was skipped still cannot produce a correct result.
    let mut blocked: HashSet<String> = HashSet::new();

    for task in tasks {
        if cancelled.load(Ordering::SeqCst) {
            let result = TaskResult::skipped(&task, &task.agent_id, "cancelled by user");
            finish_skipped(app, mbus.as_deref(), &result);
            results.push(result);
            continue;
        }

        let missing: Vec<&str> = task
            .dependencies
            .iter()
            .filter(|dep| blocked.contains(*dep))
            .map(|dep| dep.as_str())
            .collect();
        if !missing.is_empty() {
            let result = TaskResult::skipped(
                &task,
                &task.agent_id,
                format!("skipped because {} did not run", missing.join(", ")),
            );
            finish_skipped(app, mbus.as_deref(), &result);
            blocked.insert(task.id.clone());
            results.push(result);
            continue;
        }

        let agent = match agents.iter().find(|a| a.id == task.agent_id) {
            Some(agent) => agent,
            None => {
                let result = TaskResult::skipped(
                    &task,
                    &task.agent_id,
                    format!("agent '{}' is not available", task.agent_id),
                );
                finish_skipped(app, mbus.as_deref(), &result);
                blocked.insert(task.id.clone());
                results.push(result);
                continue;
            }
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
            exchange.as_ref(),
            skill_access_root.clone(),
            cancelled.clone(),
            mbus.clone(),
        )
        .await;
        if !matches!(res.status.as_str(), "success" | "error") {
            blocked.insert(res.task_id.clone());
        }
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
    exchange: Option<&exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> TaskResult {
    let mission_id = mission_id_for_task(task).to_string();
    let agent_type = normalize_agent_type(&agent.agent_type);
    // `llm` tasks are running as soon as their loop starts. External agents are
    // marked running after their prompt absorbed the pre-start inbox, so the
    // planner can still reach a task that was dispatched but not launched yet.
    if agent_type == AGENT_TYPE_LLM {
        if let Some(bus) = &mbus {
            bus.mark_running(&task.id);
        }
    }

    // Resolve the task's working directory once, before anything starts. It is
    // supplied per delegation (planner / `call_subagent`) and never by the
    // agent definition, so one agent can work on different projects. The path
    // is sandboxed to the workspace and created when it does not exist yet; a
    // bad path fails the task with a message the caller can act on instead of
    // silently running somewhere else.
    let task_cwd = match resolve_task_cwd(task.cwd.as_deref(), &workspace_dir) {
        Ok(cwd) => cwd,
        Err(err) => {
            let _ = app.emit(
                "agent-task-start",
                json!({
                    "task_id": task.id,
                    "agent_id": agent.id,
                    "agent_name": agent.name,
                    "description": task.description,
                    "cwd": task.cwd,
                }),
            );
            let _ = app.emit(
                "agent-task-error",
                json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
            );
            let result = TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: format!("Error: {err}"),
                tool_calls_count: 0,
                tokens_used: 0,
            };
            if let Some(bus) = &mbus {
                bus.finish_task(&result);
            }
            return result;
        }
    };

    let result = match agent_type.as_str() {
        AGENT_TYPE_CLI => {
            run_external_agent(
                app,
                agent,
                task,
                session_id,
                task_cwd,
                exchange,
                ExternalBackend::Cli,
                cancelled,
                mbus.as_deref(),
            )
            .await
        }
        AGENT_TYPE_CLOUD => {
            run_external_agent(
                app,
                agent,
                task,
                session_id,
                task_cwd,
                exchange,
                ExternalBackend::Cloud,
                cancelled,
                mbus.as_deref(),
            )
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
                task_cwd,
                exchange,
                skill_access_root,
                &cancelled,
                mbus.as_deref(),
            )
            .await
        }
    };

    if let Some(bus) = &mbus {
        bus.finish_task(&result);
    }

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

/// What a `cli` run needs beyond the agent settings: where it starts.
struct CliRunContext {
    cwd: PathBuf,
}

/// Upper bound for any external agent run's timeout.
pub(crate) const MAX_AGENT_TIMEOUT_SECS: u64 = 7_200;

/// Resolves a configured agent timeout: `0` disables it, while positive values
/// are capped at [`MAX_AGENT_TIMEOUT_SECS`].
pub(crate) fn resolve_agent_timeout(configured: u32) -> Option<Duration> {
    let seconds = (configured as u64).min(MAX_AGENT_TIMEOUT_SECS);
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// Maps a finished run to the task/mission status. Only an exit-0 run is a
/// success: a user stop closes the mission as "stopped", while a non-zero exit
/// or a timeout is an "error" (it must not mark the mission accomplished).
fn task_status_for_run(run_status: &str) -> &'static str {
    match run_status {
        "success" => "success",
        "cancelled" => "stopped",
        _ => "error",
    }
}

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
    task_cwd: PathBuf,
    exchange: Option<&exchange::ExchangeArea>,
    backend: ExternalBackend,
    cancelled: Arc<AtomicBool>,
    mbus: Option<&mbus::MissionBus>,
) -> TaskResult {
    let mission_id = mission_id_for_task(task).to_string();
    let _ = app.emit(
        "agent-task-start",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "description": task.description,
            "cwd": task.cwd,
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

        if let Err(err) =
            initialize_mission_state(&db, session_id, agent, task, mbus.map(|bus| bus.bus_id()))
        {
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

    let mut prompt = build_external_prompt(
        agent,
        task,
        exchange,
        matches!(backend, ExternalBackend::Cli),
    );
    // External agents cannot call app tools, but a message queued before the
    // process starts can still be absorbed into its one-shot prompt.
    if let Some(bus) = mbus {
        let mut queued = bus.take_inbox(&task.id, mbus::MAX_DELIVERIES_PER_ROUND);
        // Close the race between reading the inbox and publishing `running`:
        // a sender that saw `pending` just before this point may have queued
        // one more message, so absorb it before the prompt is fixed.
        bus.mark_running(&task.id);
        queued.extend(bus.take_inbox(&task.id, mbus::MAX_DELIVERIES_PER_ROUND));
        if !queued.is_empty() {
            prompt.push_str("\n\nMISSION BUS MESSAGES (queued before you started):\n");
            for message in queued {
                prompt.push_str(&format!("- from {}: {}\n", message.from_name, message.body));
            }
        }
    }

    // CLI agents run in the task's working directory. Each run starts a
    // fresh CLI session; no session state is carried across agent calls.
    let cli_run = match backend {
        ExternalBackend::Cli => Some(CliRunContext {
            cwd: task_cwd.clone(),
        }),
        ExternalBackend::Cloud => None,
    };

    if agent.require_approval {
        if let Some(rejection) =
            request_external_agent_approval(app, agent, &prompt, backend, cli_run.as_ref()).await
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

    let outcome: Result<(&'static str, String, String), String> = match backend {
        ExternalBackend::Cli => {
            let cli_run = cli_run.as_ref().expect("CLI run context");
            cli::run(
                app,
                agent,
                &task.id,
                &prompt,
                cli_run.cwd.clone(),
                cancelled,
            )
            .await
            // The run status distinguishes "user stopped it" from a real
            // success so the mission is not marked accomplished on cancel.
            .map(|run| (run.status, run.output, run.summary))
            .map_err(|err| format!("CLI agent \"{}\" failed: {err}", agent.name))
        }
        ExternalBackend::Cloud => cloud::run(app, agent, &task.id, &prompt, &task_cwd, cancelled)
            .await
            .map(|run| ("success", run.result, run.summary))
            .map_err(|err| format!("Cloud agent \"{}\" failed: {err}", agent.name)),
    };

    match outcome {
        Ok((run_status, content, summary)) => {
            let status = task_status_for_run(run_status);
            // A run the user stopped is not a success: the mission closes as
            // "stopped" (via run_sub_agent) instead of "accomplished". The same
            // holds for a non-zero exit or a timeout, which close it as
            // "failed" — only a real success records an accomplishment.
            if status == "success" {
                let state = app.state::<AppState>();
                let db = state.db.lock();
                if let Ok(db) = db {
                    let _ = mark_mission_accomplished(&db, &mission_id, Some(&content));
                }
            }
            // A failed/timeout run must show as an error in the agent panel and
            // the chat's tool-call group; the captured output still travels back
            // to the caller in `content`.
            if status == "error" {
                let _ = app.emit(
                    "agent-task-error",
                    json!({
                        "task_id": task.id,
                        "agent_id": agent.id,
                        "error": summary.chars().take(200).collect::<String>(),
                    }),
                );
            } else {
                let _ = app.emit(
                    "agent-task-done",
                    json!({
                        "task_id": task.id,
                        "agent_id": agent.id,
                        "agent_name": agent.name,
                        "summary": summary.chars().take(200).collect::<String>(),
                        // "success" | "stopped": the row must never show a
                        // completed checkmark for a run the user cancelled.
                        "status": status,
                    }),
                );
            }
            TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: status.to_string(),
                content,
                tool_calls_count: 0,
                tokens_used: 0,
            }
        }
        Err(err) => {
            // Cloud agents report a stop as an error message; surface the
            // mission as "stopped" rather than "failed" for those.
            let status = if err.contains("cancel") {
                "stopped"
            } else {
                "error"
            };
            let _ = app.emit(
                "agent-task-error",
                json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
            );
            TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: status.to_string(),
                content: format!("Error: {err}"),
                tool_calls_count: 0,
                tokens_used: 0,
            }
        }
    }
}

/// The single prompt handed to a `cli` / `cloud` agent: the task plus whatever
/// context the caller attached. External agents never see the chat history.
fn build_external_prompt(
    agent: &SubAgent,
    task: &Task,
    exchange: Option<&exchange::ExchangeArea>,
    local_files: bool,
) -> String {
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
    if let Some(area) = exchange {
        if local_files {
            prompt.push_str(&area.prompt_block(&task.id, true));
        } else {
            prompt.push_str(
                "\n\nDELIVERABLE EXCHANGE AREA: not available — a cloud agent cannot access the \
                 local workspace. Return the deliverable inline in your answer.",
            );
        }
    }
    prompt
}

/// Trims a task-level `cwd` and drops the "workspace root" spellings the
/// planner may emit (`""`, `"."`, `"./"`, `".\\"`).
pub(crate) fn normalize_task_cwd(cwd: Option<&str>) -> Option<String> {
    let value = cwd?.trim();
    let value = value.trim_end_matches(['/', '\\']);
    if value.is_empty() || value == "." {
        return None;
    }
    Some(value.to_string())
}

/// Resolves the task's workspace-relative working directory. An unset `cwd`
/// means the workspace root; anything else is sandbox-checked with
/// `resolve_safe_path` and created when it does not exist yet (a task may
/// legitimately target a folder it is about to scaffold). This is the single
/// source of truth for where a sub-agent starts — the agent definition never
/// carries a directory of its own.
fn resolve_task_cwd(cwd: Option<&str>, workspace_dir: &Path) -> Result<PathBuf, String> {
    let Some(configured) = normalize_task_cwd(cwd) else {
        return Ok(workspace_dir.to_path_buf());
    };
    let path = crate::tools::resolve_safe_path(workspace_dir, &configured)?;
    if !path.is_dir() {
        std::fs::create_dir_all(&path).map_err(|err| {
            format!(
                "could not create the task's working directory '{}': {err}",
                path.display()
            )
        })?;
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
    backend: ExternalBackend,
    cli_run: Option<&CliRunContext>,
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
        ExternalBackend::Cli => match cli_run {
            // The approval text must show the directory the agent will actually
            // start in.
            Some(cli_run) => cli::describe_run(agent, prompt, &cli_run.cwd)
                .map(|command| format!("It runs with your user permissions and can read and write files in the workspace.\n\n{}", command))
                .unwrap_or_else(|err| format!("Its command could not be resolved yet: {err}")),
            None => "Its working directory could not be resolved yet.".to_string(),
        },
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
        None,
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
    task_cwd: PathBuf,
    exchange: Option<&exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: &AtomicBool,
    mbus: Option<&mbus::MissionBus>,
) -> TaskResult {
    // Every task gets a private MCP process pool. The pool is shut down before
    // the task result leaves this function, so a finished/stopped/failed task
    // never leaves an MCP server behind. `Drop` covers unwind paths.
    let state = app.state::<AppState>();
    let mut mcp_session = mcp::AgentMcpSession::new();
    let result = run_llm_agent_body(
        app,
        client,
        url,
        config,
        session_id,
        agent,
        task,
        workspace_dir,
        task_cwd,
        exchange,
        skill_access_root,
        cancelled,
        &mut mcp_session,
        mbus,
    )
    .await;
    mcp_session.shutdown(&state);
    result
}

// ─── Main-agent help (`ask_main_agent`) ──────────────────────────────────────
//
// A sub-agent cannot ask the *live* main chat turn for help: `call_subagent`
// awaits this whole function, so the main agent's loop is blocked. Instead the
// request is answered by a self-contained helper run that has the main model,
// the session history, the enabled skills, the main tool set and the enabled
// MCP servers. From the sub-agent's point of view it is still an ordinary tool
// call: the material comes back and the autonomous loop continues.

struct MainHelperOutcome {
    content: String,
    tokens: u32,
    tool_calls: u32,
}

/// Main-agent tools for the helper: the configured main selection minus
/// `timer` (a chat-session tool that would resume a conversation later).
fn main_helper_tool_ids(config: &AppConfig) -> Vec<String> {
    config
        .effective_tools()
        .into_iter()
        .filter(|tool| tool.as_str() != "timer")
        .collect()
}

/// Drop the self-evolution `mcp_self` tool from a sub-agent / helper list.
///
/// Building an MCP server is a main-agent capability: parallel tasks would race
/// on `mcp_servers.json`, an `AgentMcpSession` only loads servers at task start
/// (so a task's own creation would never reach its tool list), and registering
/// one asks the user for confirmation — a blocking dialog inside a mission.
/// The main agent owns the registry; sub-agents consume what it enabled.
fn drop_mcp_self(tools_list: &mut Vec<Value>) {
    let name = tools::mcp_self::TOOL_NAME;
    tools_list.retain(|tool| tool.pointer("/function/name").and_then(Value::as_str) != Some(name));
}

/// Compact transcript of the real conversation for the helper. Newest entries
/// win when the budget is exhausted; tool-call rounds are skipped because only
/// the user/assistant text is useful as context.
fn build_main_helper_history(records: &[crate::db::HistoryRecord]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;

    for record in records.iter().rev() {
        if lines.len() >= MAIN_HELPER_HISTORY_MESSAGES || used >= MAIN_HELPER_HISTORY_BUDGET_BYTES {
            break;
        }
        let text = record.content.trim();
        if text.is_empty() {
            continue;
        }
        let text = preview_text(text, MAIN_HELPER_HISTORY_MESSAGE_BYTES);
        let line = format!("[{}]: {}", record.role, text);
        used += line.len();
        lines.push(line);
    }

    lines.reverse();
    lines.join("\n")
}

/// Keeps a single tool result within the helper's context budget.
fn cap_helper_tool_result(result: String) -> String {
    if result.len() <= MAIN_HELPER_TOOL_RESULT_MAX_BYTES {
        return result;
    }
    let mut clipped = preview_text(&result, MAIN_HELPER_TOOL_RESULT_MAX_BYTES);
    clipped.push_str("\n\n[truncated: tool output exceeded the helper limit]");
    clipped
}

/// Appends one exchange to the mission's visible help log. Best effort: the
/// helper must still answer when the database write fails.
fn append_mission_help_note(app: &AppHandle, mission_id: &str, note: &str) {
    let state = app.state::<AppState>();
    let Ok(db) = state.db.lock() else {
        return;
    };
    let stamp = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{stamp}] {note}");
    let _ = db.execute(
        "UPDATE agent_missions SET help_notes = \
         CASE WHEN help_notes = '' THEN ?2 ELSE help_notes || char(10) || ?2 END, \
         updated_at = CURRENT_TIMESTAMP WHERE mission_id = ?1",
        rusqlite::params![mission_id, line],
    );
}

/// Loads every enabled MCP server into `session` and returns the OpenAI tool
/// definitions plus the name → (server, actual tool) map. Shared by a task's own
/// `enable_mcp` tools and its main-agent helper, so both reuse one process pool.
async fn load_enabled_mcp_tools(
    app: &AppHandle,
    session: &mut mcp::AgentMcpSession,
    owner: &str,
) -> (
    Vec<Value>,
    std::collections::HashMap<String, (mcp::McpServer, String)>,
) {
    let state = app.state::<AppState>();
    let mut tools_out: Vec<Value> = Vec::new();
    let mut tool_map: std::collections::HashMap<String, (mcp::McpServer, String)> =
        std::collections::HashMap::new();

    // Merged roster: panel servers plus the assistant's own (`.data/mcp.conf`).
    let enabled_servers: Vec<mcp::McpServer> = mcp::load_effective_servers(&state)
        .into_iter()
        .filter(|server| server.enabled)
        .collect();

    for server in enabled_servers {
        match mcp::get_server_tools_for_agent(&state, session, &server, owner).await {
            Ok(server_tools) => {
                for tool in server_tools {
                    if let Some((fn_name, actual_name, openai_tool)) =
                        mcp::adapt_tool_for_llm(&server, &tool, false)
                    {
                        tools_out.push(openai_tool);
                        tool_map.insert(fn_name, (server.clone(), actual_name));
                    }
                }
            }
            Err(err) => {
                if let Ok(logger) = state.logger.lock() {
                    logger.log(
                        "ERROR",
                        &format!(
                            "MCP server '{}' tools/list failed for {owner}: {err}",
                            server.name
                        ),
                    );
                }
            }
        }
    }

    // Prompt-cache stability: the tools payload opens the request prefix, so
    // it must not depend on the `tools/list` enumeration order.
    mcp::sort_adapted_tools(&mut tools_out);

    (tools_out, tool_map)
}

/// Answers one `ask_main_agent` request, bounded by [`MAIN_HELPER_TIMEOUT_SECS`].
#[allow(clippy::too_many_arguments)]
async fn run_main_agent_helper(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agent: &SubAgent,
    task: &Task,
    session_id: &str,
    workspace_dir: &Path,
    task_cwd: &Path,
    skill_access_root: Option<&Path>,
    request: &str,
    context: &str,
    cancelled: &AtomicBool,
    mcp_session: &mut mcp::AgentMcpSession,
) -> Result<MainHelperOutcome, String> {
    match tokio::time::timeout(
        Duration::from_secs(MAIN_HELPER_TIMEOUT_SECS),
        run_main_agent_helper_inner(
            app,
            client,
            url,
            config,
            agent,
            task,
            session_id,
            workspace_dir,
            task_cwd,
            skill_access_root,
            request,
            context,
            cancelled,
            mcp_session,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "main-agent help timed out after {} seconds",
            MAIN_HELPER_TIMEOUT_SECS
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_main_agent_helper_inner(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agent: &SubAgent,
    task: &Task,
    session_id: &str,
    workspace_dir: &Path,
    task_cwd: &Path,
    skill_access_root: Option<&Path>,
    request: &str,
    context: &str,
    cancelled: &AtomicBool,
    mcp_session: &mut mcp::AgentMcpSession,
) -> Result<MainHelperOutcome, String> {
    // Tool progress raised by the helper belongs to the sub-agent's task row,
    // not to the main chat: pass the mission id down to the tool executor.
    let mission_id = mission_id_for_task(task);
    // ── Conversation history (locks the DB, so drop it before any await) ─────
    let history = {
        let state = app.state::<AppState>();
        let db = state
            .db
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        crate::db::query_recent_session_messages(&db, session_id, MAIN_HELPER_HISTORY_MESSAGES * 2)
            .map_err(|e| e.to_string())?
    };
    let transcript = build_main_helper_history(&history);

    // ── Enabled skills (main capability) ─────────────────────────────────────
    let skills = crate::skills::enabled_skills(workspace_dir);
    let mut skills_info = String::new();
    for skill in &skills {
        skills_info.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }

    // Self-evolution mode: the facts the main assistant verified in earlier
    // sessions are part of its capability and belong in the helper context.
    let verified_facts = if config.self_evolution_mode {
        crate::tools::memory::render_evolution_context(
            workspace_dir,
            crate::tools::memory::EVOLUTION_CONTEXT_BUDGET_CHARS,
        )
    } else {
        String::new()
    };

    // ── Tools: main selection + skills + enabled MCP ─────────────────────────
    let tool_ids = main_helper_tool_ids(config);
    let mut tools_list = tools::get_all_tools(&tool_ids, config.self_evolution_mode, false);
    drop_mcp_self(&mut tools_list);
    let has_skills = !skills.is_empty() || config.self_evolution_mode;
    if has_skills {
        tools_list.push(tools::get_use_skill_tool());
        tools_list.push(tools::get_skill_read_tool());
    }
    let (mcp_tools, mcp_tool_map) = load_enabled_mcp_tools(
        app,
        mcp_session,
        &format!("agent:{}:main-helper", agent.name),
    )
    .await;
    tools_list.extend(mcp_tools);

    // ── Prompt ───────────────────────────────────────────────────────────────
    let os_info = os_info::get();
    let base_prompt = config.system_message.trim();
    let base_block = if base_prompt.is_empty() {
        String::new()
    } else {
        format!("{base_prompt}\n\n")
    };
    let skills_block = if skills_info.is_empty() {
        String::new()
    } else {
        format!("Enabled skills (call `use_skill` to load one):\n{skills_info}\n")
    };
    let facts_block = if verified_facts.trim().is_empty() {
        String::new()
    } else {
        format!(
            "VERIFIED FACTS (self-evolution memory, already known — reuse them):\n{verified_facts}\n\n"
        )
    };
    let system = format!(
        "{base}You are the MAIN assistant of this app, temporarily helping the sub-agent \"{agent_name}\" \
         (id {agent_id}) with its autonomous mission. The sub-agent cannot see this conversation.\n\
         Your only job is to collect and return material for its request: concrete findings, exact file \
         paths, excerpts, command output or links. Do not write the final user-facing answer and do not \
         ask the user questions. If the material cannot be obtained, state exactly what is missing.\n\
         User's machine: {os} {os_version}\n\
         Sub-agent working directory: {cwd} (`run_cmd` / `run_shell` start here)\n\
         Workspace root: {workspace_root} (`file_actions` paths are relative to it)\n\n\
         {skills}\
         {facts}\
         Main conversation (for context):\n{transcript}",
        base = base_block,
        agent_name = agent.name,
        agent_id = agent.id,
        os = os_info.os_type(),
        os_version = os_info.version(),
        cwd = task_cwd.display(),
        workspace_root = workspace_dir.display(),
        skills = skills_block,
        facts = facts_block,
        transcript = if transcript.is_empty() {
            "(no prior conversation)"
        } else {
            transcript.as_str()
        },
    );

    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": system }),
        json!({
            "role": "user",
            "content": format!(
                "Sub-agent \"{}\" is executing this task:\n{}\n\nIts request:\n{}\n\nWhy it needs it:\n{}",
                agent.name,
                task.description.trim(),
                request.trim(),
                if context.trim().is_empty() { "(not specified)" } else { context.trim() },
            ),
        }),
    ];

    let model = config.model.trim();
    if model.is_empty() {
        return Err("no main model is configured".to_string());
    }
    let headers = upstream_request_headers(config, session_id);
    let max_complete_tokens = config
        .model_settings
        .max_complete_tokens
        .filter(|value| *value > 0)
        .unwrap_or(LLM_DEFAULT_MAX_COMLETE_TOKENS);

    let mut loaded_skills: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut active_skill_dirs: Vec<(String, PathBuf)> = Vec::new();
    let mut allowed_commands: Vec<String> = Vec::new();
    let mut tokens_used: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mut last_content = String::new();

    for _ in 0..MAIN_HELPER_MAX_ITERATIONS {
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled by the user".to_string());
        }

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
        if let Some(effort) = resolve_reasoning_effort(config, model) {
            req_body["reasoning_effort"] = json!(effort);
        }
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
            req_body["tool_choice"] = json!("auto");
        }

        let stream = stream_llm_request(
            app,
            client,
            url,
            &config.api_key,
            req_body,
            cancelled,
            StreamOptions {
                token_event: "agent-help-token",
                task_id: Some(&task.id),
                reasoning_event: None,
                emit_usage: false,
                emit_tokens: true,
                // The helper runs inside the sub-agent's turn: an upstream
                // failure must surface on the task row, never as a terminal
                // `chat-error` that closes the user's chat turn.
                emit_errors: false,
                usage_max_tokens: None,
                ds_format: config.ds_format_for(model),
                headers: &headers,
            },
        )
        .await?;

        tokens_used = tokens_used.saturating_add(stream.prompt_tokens + stream.completion_tokens);
        let content = stream.content;
        let reasoning_content = stream.reasoning_content;
        let tool_calls = stream.tool_calls;

        if tool_calls.is_empty() {
            let content = content.trim().to_string();
            let content = if content.is_empty() {
                "(the main agent returned no material)".to_string()
            } else if content.chars().count() > MAIN_HELPER_ANSWER_MAX_CHARS {
                let clipped: String = content.chars().take(MAIN_HELPER_ANSWER_MAX_CHARS).collect();
                format!("{clipped}\n\n[truncated: the material exceeded the size limit]")
            } else {
                content
            };
            return Ok(MainHelperOutcome {
                content,
                tokens: tokens_used,
                tool_calls: tool_calls_count,
            });
        }

        if !content.trim().is_empty() {
            last_content = content.clone();
        }
        tool_calls_count = tool_calls_count.saturating_add(tool_calls.len() as u32);

        let assistant_tcs: Vec<Value> = tool_calls
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
            Value::String(content)
        };
        let mut assistant_msg = json!({
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": assistant_tcs,
        });
        if !reasoning_content.is_empty() {
            assistant_msg["reasoning_content"] = json!(reasoning_content);
        }
        messages.push(assistant_msg);

        // Skill bodies are queued and appended AFTER all tool results of this
        // round: an `assistant` message carrying tool_calls must stay adjacent
        // to its `tool` results, or strict providers reject the next request.
        let mut pending_skill_context: Vec<Value> = Vec::new();
        for (id, name, args) in &tool_calls {
            let tool_started = std::time::Instant::now();
            emit_agent_tool_state(
                app,
                &task.id,
                "start",
                name,
                Some(preview_text(args, 200)),
                None,
                None,
            );
            let result = if name == "use_skill" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let skill_name = args_json["skill_name"].as_str().unwrap_or("");
                let skills_dir = crate::workspace::skills_root(workspace_dir);
                match crate::skills::resolve_skill_by_name(&skills_dir, skill_name) {
                    Ok(resolved) => {
                        let skill = resolved.skill;
                        if loaded_skills.insert(skill.name.clone()) {
                            for command in &skill.allowed_commands {
                                if !allowed_commands.iter().any(|existing| existing == command) {
                                    allowed_commands.push(command.clone());
                                }
                            }
                            if !active_skill_dirs
                                .iter()
                                .any(|(name, _)| name == &skill.name)
                            {
                                active_skill_dirs
                                    .push((skill.name.clone(), resolved.skill_dir.clone()));
                            }
                            let cmd_constraint = if skill.allowed_commands.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    "\n[Allowed commands for this skill: {}]\n",
                                    skill.allowed_commands.join(", ")
                                )
                            };
                            pending_skill_context.push(json!({
                                "role": "user",
                                "content": format!(
                                    "INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL (not a user request):\n\
                                     --- Skill: {} ---\n[Skill root: {}]\n{}{}",
                                    skill.name,
                                    resolved.skill_dir.display(),
                                    cmd_constraint,
                                    skill.system_prompt,
                                ),
                            }));
                            format!("Skill '{}' loaded. Follow its instructions.", skill.name)
                        } else {
                            format!("Skill '{}' already loaded.", skill.name)
                        }
                    }
                    Err(err) => format!("Error: {err}"),
                }
            } else if let Some((server, actual_name)) = mcp_tool_map.get(name) {
                let state = app.state::<AppState>();
                let arguments: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                let owner = format!("agent:{}:main-helper", agent.name);
                match mcp::invoke_mcp_tool_for_agent(
                    &state,
                    mcp_session,
                    server,
                    actual_name,
                    arguments,
                    &owner,
                )
                .await
                {
                    Ok(text) => format!("MCP [{}] {} result:\n{}", server.name, actual_name, text),
                    Err(err) => format!("MCP tool error: {err}"),
                }
            } else {
                tools::execute_tool(
                    app,
                    name,
                    args,
                    workspace_dir.to_path_buf(),
                    config,
                    &allowed_commands,
                    skill_access_root,
                    Some(mission_id),
                    Some(session_id),
                    &active_skill_dirs,
                    None,
                    Some(task_cwd),
                )
                .await
            };
            emit_agent_tool_state(
                app,
                &task.id,
                "done",
                name,
                Some(preview_text(&result, 160)),
                Some(tool_started.elapsed().as_millis()),
                Some(!result.trim_start().starts_with("Error")),
            );

            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": cap_helper_tool_result(result),
            }));
        }
        messages.extend(pending_skill_context);
    }

    let fallback = if last_content.trim().is_empty() {
        format!(
            "Main-agent helper reached its {}-round tool limit without a final answer. Tighten the request or collect the material directly.",
            MAIN_HELPER_MAX_ITERATIONS
        )
    } else {
        format!(
            "Main-agent helper reached its {}-round tool limit. Last partial material:\n{}",
            MAIN_HELPER_MAX_ITERATIONS,
            preview_text(&last_content, 4_000)
        )
    };
    Ok(MainHelperOutcome {
        content: fallback,
        tokens: tokens_used,
        tool_calls: tool_calls_count,
    })
}

/// Output-token cap for one `llm` sub-agent request.
///
/// `max_complete_tokens` is the current field; `max_tokens` is the pre-2.8 name
/// and is only read as a fallback for configurations that were not migrated
/// yet. `0` means "unset" for either field.
fn resolve_completion_limit(agent: &SubAgent) -> u32 {
    agent
        .max_complete_tokens
        .filter(|limit| *limit > 0)
        .or_else(|| agent.max_tokens.filter(|limit| *limit > 0))
        .unwrap_or(LLM_DEFAULT_MAX_COMLETE_TOKENS)
}

async fn run_llm_agent_body(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agent: &SubAgent,
    task: &Task,
    workspace_dir: PathBuf,
    task_cwd: PathBuf,
    exchange: Option<&exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: &AtomicBool,
    mcp_session: &mut mcp::AgentMcpSession,
    mbus: Option<&mbus::MissionBus>,
) -> TaskResult {
    let _ = app.emit(
        "agent-task-start",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "description": task.description,
            "cwd": task.cwd,
        }),
    );

    let model = agent.model.as_deref().unwrap_or(&config.model);
    // Same configured conversation-id header and custom headers as the main
    // chat: a sub-agent is still part of the same gateway conversation.
    let headers = upstream_request_headers(config, session_id);
    let max_complete_tokens = resolve_completion_limit(agent);
    // `timer` is a chat-session tool (it resumes the conversation later), so it
    // is never handed to a sub-agent even if an agent config lists it.
    let mut agent_tool_ids: Vec<String> = agent
        .allowed_tools
        .iter()
        .filter(|tool| tool.as_str() != "timer")
        .cloned()
        .collect();
    // Self-evolution mode owns `file_actions` and `memory`: a sub-agent that
    // may evolve skills must also be able to read and write them, and to
    // remember what it learned.
    if config.self_evolution_mode {
        for required in crate::SELF_EVOLUTION_REQUIRED_TOOLS {
            if !agent_tool_ids.iter().any(|tool| tool == required) {
                agent_tool_ids.push(required.to_string());
            }
        }
    }
    let mut tools_list = tools::get_all_tools(&agent_tool_ids, config.self_evolution_mode, false);
    drop_mcp_self(&mut tools_list);
    tools_list.extend(tools::get_agent_task_tools());
    // The mission bus is available only inside a planner-dispatched batch.
    // `call_subagent` (a direct main-agent delegation) has no bus, so the tool
    // is neither advertised nor callable there.
    if mbus.is_some() {
        tools_list.push(mbus::tool_definition(false));
    }
    // Main-agent assistance: only `llm` sub-agents run an in-app loop, so only
    // they can ask for it. The switch is global (Settings → Runtime & Debug).
    if config.sub_agent_main_help {
        tools_list.push(tools::get_main_agent_help_tool());
    }

    // ── MCP tools (`enable_mcp`) ─────────────────────────────────────────────
    // The agent gets the same enabled MCP servers as the main agent, but its
    // stdio processes live in this task's private session — never the shared
    // `AppState` pool — and the wrapper kills them when the task ends.
    let (mcp_tools, mcp_tool_map) = if agent.enable_mcp {
        load_enabled_mcp_tools(app, mcp_session, &format!("agent:{}", agent.name)).await
    } else {
        (Vec::new(), std::collections::HashMap::new())
    };
    tools_list.extend(mcp_tools);
    // `cancelled` is the caller's flag — the turn's own cancel flag when the
    // agent runs inside a chat turn, so pressing Stop in the chat tears the
    // autonomous loop down as well. It is never a fresh local flag: a detached
    // flag can never become true, and Stop would be ignored.
    let mut total_tokens: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mut main_help_calls: u32 = 0;
    let mission_id = mission_id_for_task(task).to_string();
    // Context budget: the per-model window from config (auto-detected from the
    // provider's `/models` response, the metadata catalogue, or entered for the
    // model in the Agents panel), otherwise a conservative default. Output-token
    // caps are a different quantity and must never shrink the context budget.
    let context_budget = crate::llm_complete::resolve_context_window(
        config,
        model,
        AGENT_DEFAULT_CONTEXT_WINDOW as u32,
    ) as usize;

    let mut self_evolution_context =
        crate::skills::self_evolution_prompt(skill_access_root.as_deref());

    // The verified facts the assistant recorded in earlier sessions (the
    // default interpreter and runtime environment, the correct way to operate a
    // tool, the file locations it uses often) are injected here so a
    // sub-agent reuses them instead of rediscovering them.
    if !self_evolution_context.is_empty() {
        let evolution_facts = crate::tools::memory::render_evolution_context(
            &workspace_dir,
            crate::tools::memory::EVOLUTION_CONTEXT_BUDGET_CHARS,
        );
        if !evolution_facts.is_empty() {
            self_evolution_context.push_str(
                "\n\nVERIFIED FACTS (self-evolution memory, already known — reuse them, do not \
                 rediscover):\n",
            );
            self_evolution_context.push_str(&evolution_facts);
        }
    }

    // The workspace script library is shared with the main chat. Only a
    // sub-agent that can actually run commands sees the listing.
    if agent_tool_ids.iter().any(|tool| tool == "run_cmd") {
        let scripts_context =
            crate::tools::scripts::prompt_block(&workspace_dir, config.self_evolution_mode);
        if !scripts_context.is_empty() {
            if !self_evolution_context.is_empty() {
                self_evolution_context.push_str("\n\n");
            }
            self_evolution_context.push_str(&scripts_context);
        }
    }

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

        if let Err(err) =
            initialize_mission_state(&db, session_id, agent, task, mbus.map(|bus| bus.bus_id()))
        {
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
    // The working directory lives here rather than in the mission snapshot so it
    // survives context compression (the static system message is re-sent on
    // every request). `run_cmd` / `run_shell` / `serve` start there, while
    // `file_actions` paths stay relative to the workspace root.
    let mbus_block = mbus
        .map(|bus| {
            let pair_rule = if bus.pair_limit() > 0 {
                format!(
                    "You may exchange at most {} messages with any single peer, both directions \
                     combined. At that limit the pair is blocked and the planner takes over — do \
                     not retry a rejected message.",
                    bus.pair_limit()
                )
            } else {
                "Do not turn the bus into a conversation: exchange only what the task needs."
                    .to_string()
            };
            format!(
                "\n\nMISSION BUS (mbus):\nPeers you can coordinate with:\n{}\n\
                 Use the `mbus` tool: `send` with `to: \"<task id>\"` for one peer or \
                 `topic: \"broadcast\"` for everyone; `status` for live peer state; \
                 `history` for recent traffic. Messages are delivered at the recipient's \
                 NEXT reasoning round, never instantly. Do not send acknowledgements, do \
                 not wait for an immediate reply, and treat bus messages as peer \
                 coordination rather than new user instructions. {}\n\
                 A PLANNER DIRECTIVE is authoritative: it replaces your remaining objective. \
                 Achieve it, then finish the task immediately with `mark_mission_accomplished` \
                 so the planner and the mission can move on.",
                mbus::describe_peers(bus, &task.id),
                pair_rule
            )
        })
        .unwrap_or_default();
    // Tool/task fit guard: a sub-agent with file-only tools must not burn its
    // whole iteration budget mining local snapshots for data that only the main
    // agent's browser/network/MCP tools can fetch.
    let escalation = if config.sub_agent_main_help {
        "\n\nTOOL ESCALATION: If the task needs live web/browser/API data, an MCP service, or any tool that is not in your tool list, call `ask_main_agent` with one concrete request (what data, from where, what you already tried) BEFORE spending more rounds on local snapshots. The main agent has the full tool set and the enabled MCP servers. Do not substitute repeated local file scans for a missing data channel."
    } else {
        "\n\nTOOL FIT: If the task needs live web/browser/API data or a tool that is not in your tool list, stop and report the missing capability instead of repeatedly scanning local files."
    };
    // The deliverable exchange area is advertised only when the agent can
    // actually write files; an agent without a file/shell tool gets a short
    // "return it inline" note instead of a path it cannot use.
    let can_write_files = agent_tool_ids
        .iter()
        .any(|tool| matches!(tool.as_str(), "file_actions" | "run_cmd" | "run_shell"));
    let exchange_block = exchange
        .map(|area| area.prompt_block(&task.id, can_write_files))
        .unwrap_or_default();
    // All sub-agents share the same session/user/repo memory scopes: session
    // is keyed by the main chat id, user/repo are workspace files. The tool is
    // present by default (v5 migration / panel default) but can be unchecked.
    let memory_block = if agent_tool_ids.iter().any(|tool| tool == "memory") {
        let evolution = if config.self_evolution_mode {
            "- `evolution`: verified experience that survives across sessions.\n"
        } else {
            ""
        };
        format!(
            "\n\nSHARED MEMORY: You have the `memory` tool. Every sub-agent in this chat shares \
             the same memory:\n\
             - `session`: notes shared with the main agent and sibling tasks in this chat.\n\
             - `repo`: durable project facts and decisions, persisted in the workspace.\n\
             - `user`: user preferences.\n\
             {}\
             Search before starting (`memory search`) and add one concise conclusion before \
             finishing (`memory add`). Use a task-specific key prefix for mission notes \
             (for example `{}-...`). Never store credentials. Do not run `consolidate`; leave \
             skill consolidation to the main agent.",
            evolution, task.id,
        )
    } else {
        String::new()
    };
    let static_system_prompt = format!(
        "{}\n\nWorking directory: {}\n`run_cmd`, `run_shell` and `run_shell` serve processes start in this directory. \
         `file_actions` paths are always relative to the workspace root ({}), not to the working directory.{}{}{}{}",
        build_agent_static_system_prompt(agent, &self_evolution_context),
        task_cwd.display(),
        workspace_dir.display(),
        mbus_block,
        exchange_block,
        memory_block,
        escalation,
    );
    let max_iterations = (agent.max_iterations > 0).then_some(agent.max_iterations);
    let mut iterations: u32 = 0;

    loop {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        // The planner can end a task at any reasoning boundary (`mbus`
        // `end_task`). Stop before spending another request: report what the
        // task has produced so far and let the planner collect the result.
        if let Some(bus) = mbus {
            if let Some(note) = bus.conclusion(&task.id) {
                let content = last_assistant_content(&working_memory)
                    .unwrap_or_else(|| format!("Ended by the planner: {}", note));
                return finalize_mission_task(
                    app,
                    &mission_id,
                    task,
                    agent,
                    content,
                    tool_calls_count,
                    total_tokens,
                );
            }
        }

        // `mark_mission_accomplished` set the flag in the previous round's tool
        // call. Finish now instead of asking the model for a wrap-up round.
        let accomplished = {
            let state = app.state::<AppState>();
            state
                .db
                .lock()
                .ok()
                .and_then(|db| load_mission_state(&db, &mission_id).ok())
                .filter(|snapshot| snapshot.mission_accomplished)
        };
        if let Some(snapshot) = accomplished {
            let content = if !snapshot.final_report.trim().is_empty() {
                snapshot.final_report.clone()
            } else {
                last_assistant_content(&working_memory)
                    .unwrap_or_else(|| "Mission accomplished.".to_string())
            };
            return finalize_mission_task(
                app,
                &mission_id,
                task,
                agent,
                content,
                tool_calls_count,
                total_tokens,
            );
        }

        if let Some(limit) = max_iterations {
            if iterations >= limit {
                break;
            }
        }
        iterations = iterations.saturating_add(1);
        if let Some(bus) = mbus {
            bus.mark_iteration(&task.id, iterations, total_tokens);
        }

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
            &headers,
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

        // ── Mission-bus messages ─────────────────────────────────────────────
        // A request boundary is the only safe insertion point: an assistant
        // message carrying tool_calls must stay adjacent to its tool results.
        // This mirrors the main agent's `drain_turn_injections`; the injected
        // text uses the `user` role (some gateways reject mid-conversation
        // system messages) and is clearly marked as peer coordination.
        if let Some(bus) = mbus {
            let queued = bus.take_inbox(&task.id, mbus::MAX_DELIVERIES_PER_ROUND);
            if !queued.is_empty() {
                for message in &queued {
                    working_memory.push(json!({
                        "role": "user",
                        "content": format!(
                            "{}\nfrom: {} ({})\nvia: {}\n\n{}",
                            mbus::MBUS_INJECTION_PREFIX,
                            message.from_name,
                            message.from_id,
                            if message.target_kind == "topic" {
                                format!("#{}", message.topic)
                            } else {
                                "direct".to_string()
                            },
                            message.body,
                        ),
                    }));
                }
                let _ = app.emit(
                    "agent-mbus-delivered",
                    json!({
                        "bus_id": bus.bus_id(),
                        "session_id": session_id,
                        "mission_id": mission_id,
                        "task_id": task.id,
                        "count": queued.len(),
                        "messages": queued
                            .iter()
                            .map(|message| json!({
                                "from_id": message.from_id,
                                "from_name": message.from_name,
                                "topic": message.topic,
                                "body": message.body,
                                "created_at_ms": message.created_at_ms,
                            }))
                            .collect::<Vec<_>>(),
                    }),
                );
            }
        }

        // Rebuild the snapshot after a possible compression (episodic summary
        // may have changed), then append it as the final message.
        let mut snapshot_prompt = build_mission_snapshot_prompt(&mission_snapshot);
        if let Some(bus) = mbus {
            if let Some((objective, done_when)) = bus.directive(&task.id) {
                snapshot_prompt.push_str(&format!(
                    "\n\n{}\nObjective: {objective}",
                    mbus::PLANNER_DIRECTIVE_PREFIX
                ));
                if let Some(done) = done_when {
                    snapshot_prompt.push_str(&format!(
                        "\nExit condition (finish the task as soon as it is met): {done}"
                    ));
                }
                snapshot_prompt.push_str(
                    "\nThis objective overrides your earlier plan. Do not exchange further \
                     coordination messages unless strictly required; produce the result and \
                     call mark_mission_accomplished.",
                );
            }
        }
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
        log_agent_event(
            app,
            "INFO",
            format!(
                "agent [{}] {} round {} request: model={}, messages={}, tools={}, \
                 context_budget={}, max_completion_tokens={}, max_iterations={} (0 = unlimited)",
                task.id,
                agent.name,
                iterations,
                model,
                working_memory.len(),
                tools_list.len(),
                context_budget,
                max_complete_tokens,
                max_iterations.unwrap_or(0),
            ),
        );

        let stream_result = stream_llm_request(
            app,
            client,
            url,
            &config.api_key,
            req_body,
            cancelled,
            StreamOptions {
                token_event: "agent-task-token",
                task_id: Some(&task.id),
                // Live "thinking" line on the task row. The reasoning text is
                // never persisted in the mission report; only the UI shows it.
                reasoning_event: Some("agent-task-reasoning"),
                emit_usage: false,
                emit_tokens: true,
                // A sub-agent request failure must not emit `chat-error`: the
                // frontend treats that event as the whole turn failing, closes
                // the transcript and drops later task events. The task reports
                // it through `agent-task-error` instead.
                emit_errors: false,
                usage_max_tokens: None,
                ds_format: config.ds_format_for(model),
                headers: &headers,
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
                let balance_error = is_provider_balance_error(&e);
                let error = if balance_error {
                    format!("Upstream account balance/quota exhausted: {e}")
                } else {
                    e
                };
                log_agent_event(
                    app,
                    "ERROR",
                    format!(
                        "agent [{}] {} round {} request failed{}: {error}",
                        task.id,
                        agent.name,
                        iterations,
                        if balance_error {
                            " (provider balance/quota)"
                        } else {
                            ""
                        },
                    ),
                );
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": error.clone() }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: format!("Error: {error}"),
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
            Ok((content, reasoning_content, agent_tool_calls, tokens)) => {
                total_tokens += tokens;
                log_agent_event(
                    app,
                    "INFO",
                    format!(
                        "agent [{}] {} round {} response: tokens={}, tool_calls={}, text_chars={}",
                        task.id,
                        agent.name,
                        iterations,
                        tokens,
                        agent_tool_calls.len(),
                        content.chars().count()
                    ),
                );

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
                                "status": "success",
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
                    if let Some(bus) = mbus {
                        bus.mark_tool(&task.id, name);
                    }
                    log_agent_event(
                        app,
                        "INFO",
                        format!(
                            "agent [{}] {} tool {}: {}",
                            task.id,
                            agent.name,
                            name,
                            preview_text(args, 240)
                        ),
                    );
                    let tool_started = std::time::Instant::now();
                    emit_agent_tool_state(
                        app,
                        &task.id,
                        "start",
                        name,
                        Some(preview_text(args, 200)),
                        None,
                        None,
                    );
                    let result = if name == "mbus" {
                        match mbus {
                            Some(bus) => {
                                let parsed: Value = serde_json::from_str(args).unwrap_or_default();
                                mbus::handle_tool(app, bus, &task.id, &parsed, false)
                            }
                            None => {
                                "Error: the mission bus is not available for this task.".to_string()
                            }
                        }
                    } else if name == "ask_main_agent" {
                        if !config.sub_agent_main_help {
                            "Error: asking the main agent is disabled in Settings → Runtime & Debug.".to_string()
                        } else if main_help_calls >= MAIN_HELPER_MAX_CALLS {
                            format!(
                                "Error: this task already used its {} main-agent help requests. Continue with the material you have.",
                                MAIN_HELPER_MAX_CALLS
                            )
                        } else {
                            let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                            let request = args_json["request"]
                                .as_str()
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let context = args_json["context"]
                                .as_str()
                                .unwrap_or("")
                                .trim()
                                .to_string();

                            if request.is_empty() {
                                "Error: ask_main_agent requires a non-empty 'request'.".to_string()
                            } else {
                                main_help_calls += 1;
                                let _ = app.emit(
                                    "agent-help-start",
                                    json!({
                                        "task_id": task.id,
                                        "mission_id": mission_id,
                                        "agent_id": agent.id,
                                        "agent_name": agent.name,
                                        "request": preview_text(&request, 300),
                                        "cwd": task.cwd,
                                    }),
                                );

                                match run_main_agent_helper(
                                    app,
                                    client,
                                    url,
                                    config,
                                    agent,
                                    task,
                                    session_id,
                                    &workspace_dir,
                                    &task_cwd,
                                    skill_access_root.as_deref(),
                                    &request,
                                    &context,
                                    cancelled,
                                    mcp_session,
                                )
                                .await
                                {
                                    Ok(outcome) => {
                                        total_tokens = total_tokens.saturating_add(outcome.tokens);
                                        tool_calls_count =
                                            tool_calls_count.saturating_add(outcome.tool_calls);
                                        let answer_chars = outcome.content.chars().count();
                                        append_mission_help_note(
                                            app,
                                            &mission_id,
                                            &format!(
                                                "asked main agent: {} → {} chars",
                                                preview_text(&request, 160),
                                                answer_chars
                                            ),
                                        );
                                        let _ = app.emit(
                                            "agent-help-done",
                                            json!({
                                                "task_id": task.id,
                                                "mission_id": mission_id,
                                                "agent_id": agent.id,
                                                "answer_chars": answer_chars,
                                                "tokens": outcome.tokens,
                                                "tool_calls": outcome.tool_calls,
                                            }),
                                        );
                                        outcome.content
                                    }
                                    Err(err) => {
                                        append_mission_help_note(
                                            app,
                                            &mission_id,
                                            &format!(
                                                "asked main agent: {} → error: {}",
                                                preview_text(&request, 160),
                                                preview_text(&err, 200)
                                            ),
                                        );
                                        let _ = app.emit(
                                            "agent-help-error",
                                            json!({
                                                "task_id": task.id,
                                                "mission_id": mission_id,
                                                "agent_id": agent.id,
                                                "error": err,
                                            }),
                                        );
                                        format!("Error: main-agent help failed: {err}")
                                    }
                                }
                            }
                        }
                    } else if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                        let state = app.state::<AppState>();
                        let arguments: Value =
                            serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                        let owner = format!("agent:{}", agent.name);
                        match mcp::invoke_mcp_tool_for_agent(
                            &state,
                            mcp_session,
                            mcp_server,
                            actual_tool_name,
                            arguments,
                            &owner,
                        )
                        .await
                        {
                            Ok(text) => format!(
                                "MCP [{}] {} result:\n{}",
                                mcp_server.name, actual_tool_name, text
                            ),
                            Err(err) => format!("MCP tool error: {err}"),
                        }
                    } else {
                        tools::execute_tool(
                            app,
                            name,
                            args,
                            workspace_dir.clone(),
                            config,
                            &[],
                            skill_access_root.as_deref(),
                            Some(&mission_id),
                            Some(session_id),
                            &[],  // sub-agents don't track active skill dirs for skill_read
                            None, // background tasks are never advertised to sub-agents
                            Some(task_cwd.as_path()),
                        )
                        .await
                    };
                    emit_agent_tool_state(
                        app,
                        &task.id,
                        "done",
                        name,
                        Some(preview_text(&result, 160)),
                        Some(tool_started.elapsed().as_millis()),
                        Some(!result.trim_start().starts_with("Error")),
                    );
                    log_agent_event(
                        app,
                        if result.trim_start().starts_with("Error") {
                            "WARN"
                        } else {
                            "INFO"
                        },
                        format!(
                            "agent [{}] {} tool {} -> {} chars: {}",
                            task.id,
                            agent.name,
                            name,
                            result.chars().count(),
                            preview_text(&result, 160)
                        ),
                    );
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

    let accomplished = mission_summary
        .as_ref()
        .map(|mission| mission.mission_accomplished)
        .unwrap_or(false);
    let _ = app.emit(
        "agent-task-done",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "summary": if max_iterations.is_some() { "Max iterations reached" } else { "Loop stopped" },
            // A loop that ended without the completion flag was stopped, not
            // completed: the row must not show a success checkmark.
            "status": if accomplished { "success" } else { "stopped" },
        }),
    );
    TaskResult {
        task_id: task.id.clone(),
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        status: if accomplished {
            "success".to_string()
        } else {
            "stopped".to_string()
        },
        content: final_content,
        tool_calls_count,
        tokens_used: total_tokens,
    }
}

/// Last non-empty assistant text in the working memory. Used when the planner
/// ends a task before the model produced a dedicated final answer.
fn last_assistant_content(working_memory: &[Value]) -> Option<String> {
    working_memory.iter().rev().find_map(|message| {
        if message["role"].as_str() != Some("assistant") {
            return None;
        }
        message["content"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
    })
}

/// Close a sub-agent mission as a success without another model round. Used
/// when `mark_mission_accomplished` already ran, or when the planner ended the
/// task through `mbus` `end_task`.
fn finalize_mission_task(
    app: &AppHandle,
    mission_id: &str,
    task: &Task,
    agent: &SubAgent,
    content: String,
    tool_calls_count: u32,
    tokens_used: u32,
) -> TaskResult {
    let state = app.state::<AppState>();
    if let Ok(db) = state.db.lock() {
        let _ = mark_mission_accomplished(&db, mission_id, Some(&content));
    }

    let summary: String = content.chars().take(200).collect();
    let _ = app.emit(
        "agent-task-done",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "summary": summary,
            "status": "success",
        }),
    );
    TaskResult {
        task_id: task.id.clone(),
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        status: "success".to_string(),
        content,
        tool_calls_count,
        tokens_used,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_balance_errors_are_recognized() {
        for text in [
            "Upstream request failed: Insufficient account funds",
            "insufficient_quota",
            "Free usage limit reached",
            "This request requires more credits, or fewer max_tokens",
        ] {
            assert!(is_provider_balance_error(text), "{text}");
        }
        assert!(!is_provider_balance_error("Model is unavailable"));
        assert!(!is_provider_balance_error("connection reset by peer"));
    }

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
        assert_eq!(
            cli_agent.cli.args,
            vec!["run".to_string(), "{prompt}".to_string()]
        );
        assert_eq!(cli_agent.cli.default_timeout_seconds, 600);
        assert!(cli_agent.cli.command.ends_with("opencode2.ps1"));

        // The legacy array must not come back when the config is serialized.
        let serialized = serde_json::to_string(&config).expect("serializes");
        assert!(!serialized.contains("\"cli_agents\""));
    }

    #[test]
    fn failed_and_timed_out_runs_are_not_successes() {
        assert_eq!(task_status_for_run("success"), "success");
        assert_eq!(task_status_for_run("cancelled"), "stopped");
        // A non-zero exit or a timeout must not be reported as "success": the
        // mission would be marked accomplished and the panel would show a
        // green "done" badge for a failed run.
        assert_eq!(task_status_for_run("failed"), "error");
        assert_eq!(task_status_for_run("timeout"), "error");
    }

    #[test]
    fn unknown_agent_types_fall_back_to_llm() {
        assert_eq!(normalize_agent_type("CLI"), AGENT_TYPE_CLI);
        assert_eq!(normalize_agent_type(" cloud "), AGENT_TYPE_CLOUD);
        assert_eq!(normalize_agent_type("banana"), AGENT_TYPE_LLM);
        assert_eq!(normalize_agent_type(""), AGENT_TYPE_LLM);
    }

    #[test]
    fn zero_timeout_means_no_timeout() {
        // `0` disables the deadline for CLI and cloud agents.
        assert!(resolve_agent_timeout(0).is_none());
        assert_eq!(resolve_agent_timeout(600), Some(Duration::from_secs(600)));
        // Positive values stay capped at the documented maximum.
        assert_eq!(
            resolve_agent_timeout(u32::MAX),
            Some(Duration::from_secs(MAX_AGENT_TIMEOUT_SECS))
        );
    }

    #[test]
    fn legacy_max_tokens_migrates_to_max_complete_tokens() {
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"agents":[
                {"id":"legacy","max_tokens":16384},
                {"id":"explicit","max_tokens":8192,"max_complete_tokens":2048},
                {"id":"zero","max_tokens":4096,"max_complete_tokens":0}
            ]}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);

        // A legacy-only value is adopted and the dead field is cleared.
        assert_eq!(config.agents[0].max_complete_tokens, Some(16_384));
        assert_eq!(config.agents[0].max_tokens, None);
        // An explicit new value wins.
        assert_eq!(config.agents[1].max_complete_tokens, Some(2_048));
        assert_eq!(config.agents[1].max_tokens, None);
        // `0` is "unset", so the legacy value still applies.
        assert_eq!(config.agents[2].max_complete_tokens, Some(4_096));
        assert_eq!(config.agents[2].max_tokens, None);
    }

    #[test]
    fn max_iterations_defaults_to_unlimited_and_is_capped() {
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"agents":[{"id":"default"},{"id":"huge","max_iterations":99999}]}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);

        // Default changed from 10 to 0: the external task state ends a mission,
        // not an arbitrary round count.
        assert_eq!(config.agents[0].max_iterations, 0);
        // A hand-edited file cannot exceed the documented ceiling.
        assert_eq!(config.agents[1].max_iterations, MAX_AGENT_ITERATIONS);
    }

    #[test]
    fn completion_limit_prefers_the_new_field() {
        let mut agent: SubAgent = serde_json::from_str(r#"{"id":"a"}"#).expect("agent parses");
        assert_eq!(
            resolve_completion_limit(&agent),
            LLM_DEFAULT_MAX_COMLETE_TOKENS
        );

        agent.max_tokens = Some(16_384);
        assert_eq!(resolve_completion_limit(&agent), 16_384);

        agent.max_complete_tokens = Some(2_048);
        assert_eq!(resolve_completion_limit(&agent), 2_048);

        // A zero in the current field means "unset", not "zero output".
        agent.max_complete_tokens = Some(0);
        assert_eq!(resolve_completion_limit(&agent), 16_384);
    }

    #[test]
    fn v2_config_adopts_smart_selection_once() {
        // A v2 file always carried an explicit `false` (old default): adopt the
        // new default once …
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"version":2,"agents":[],"orchestration":{"use_agents":true,"auto_configure":false}}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);
        assert!(config.orchestration.auto_configure);
        assert_eq!(config.version, 5);

        // … while a v3 file keeps whatever the user chose.
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"version":3,"agents":[],"orchestration":{"use_agents":true,"auto_configure":false}}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);
        assert!(!config.orchestration.auto_configure);
    }

    #[test]
    fn v3_config_migrates_the_old_iteration_default_once() {
        // The old panel always serialized `10`, so that is the old default, not
        // a deliberate cap. Any other value is the user's choice and survives.
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"version":3,"agents":[
                {"id":"old-default","max_iterations":10},
                {"id":"explicit","max_iterations":25}
            ]}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);

        assert_eq!(config.agents[0].max_iterations, 0);
        assert_eq!(config.agents[1].max_iterations, 25);
        assert_eq!(config.version, 5);
    }

    #[test]
    fn v4_config_adds_shared_memory_to_llm_agents_once() {
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"version":4,"agents":[
                {"id":"llm","agent_type":"llm","allowed_tools":["file_actions"]},
                {"id":"cli","agent_type":"cli","allowed_tools":[]}
            ]}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);

        assert!(config.agents[0]
            .allowed_tools
            .iter()
            .any(|tool| tool == "memory"));
        assert!(!config.agents[1]
            .allowed_tools
            .iter()
            .any(|tool| tool == "memory"));
        assert_eq!(config.version, 5);

        // A v5 file keeps a deliberate opt-out.
        let mut config: AgentsConfig = serde_json::from_str(
            r#"{"version":5,"agents":[
                {"id":"llm","agent_type":"llm","allowed_tools":["file_actions"]}
            ]}"#,
        )
        .expect("config parses");
        normalize_config(&mut config);
        assert!(!config.agents[0]
            .allowed_tools
            .iter()
            .any(|tool| tool == "memory"));
    }

    fn roster_ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn expect_mission(decision: PlanDecision) -> TaskPlan {
        match decision {
            PlanDecision::Mission(plan) => plan,
            PlanDecision::Direct { why } => {
                panic!("expected a mission plan, got a direct answer: {why}")
            }
        }
    }

    #[test]
    fn plan_parser_strips_fences_and_preamble() {
        let raw = "Here is the plan you asked for:\n```json\n{\"tasks\":[{\"id\":\"t1\",\
                   \"agent_id\":\"a\",\"description\":\"do it\"}],\"execution_mode\":\"parallel\"}\n```\nEnjoy!";
        let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].id, "t1");
        assert_eq!(plan.execution_mode, "parallel");
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn plan_parser_repairs_near_json() {
        let raw = r#"{
            "tasks": [
                { "id": "t1", "agent_id": "a", "description": "one", }, // trailing comma
                { "id": "t2", "agent_id": "a", "description": "two" }
            ],
        }"#;
        let (decision, _) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        assert_eq!(plan.tasks.len(), 2);
    }

    #[test]
    fn plan_parser_drops_unknown_agents_and_renames_duplicates() {
        let raw = r#"{"tasks":[
            {"id":"t1","agent_id":"a","description":"keep me"},
            {"id":"t1","agent_id":"a","description":"duplicate id"},
            {"id":"t2","agent_id":"ghost","description":"unknown agent"},
            {"id":"t3","agent_id":"a","description":"   "}
        ]}"#;
        let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        // t2 (unknown agent) and t3 (empty description) are dropped; the
        // duplicate t1 is renamed so mission ids stay unique.
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[0].id, "t1");
        assert_eq!(plan.tasks[1].id, "t1-2");
        assert!(issues.iter().any(|issue| issue.contains("ghost")));
        assert!(issues.iter().any(|issue| issue.contains("duplicate")));
    }

    #[test]
    fn plan_parser_reserves_the_planner_id() {
        let raw = r#"{"tasks":[
            {"id":"planner","agent_id":"a","description":"model used the reserved id"},
            {"id":"t1","agent_id":"a","description":"normal"}
        ]}"#;
        let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[0].id, "task");
        assert_eq!(plan.tasks[1].id, "t1");
        assert!(issues
            .iter()
            .any(|issue| issue.contains("reserved") || issue.contains("planner")));
    }

    #[test]
    fn plan_parser_breaks_dependency_cycles() {
        let raw = r#"{"tasks":[
            {"id":"a1","agent_id":"a","description":"first","dependencies":["b1"]},
            {"id":"b1","agent_id":"a","description":"second","dependencies":["a1"]}
        ],"execution_mode":"parallel"}"#;
        let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        assert_eq!(plan.tasks.len(), 2);
        // At least one edge is gone, so the wave scheduler can start somewhere.
        assert!(
            plan.tasks.iter().any(|task| task.dependencies.is_empty()),
            "cycle was not broken: {plan:?}"
        );
        assert!(issues.iter().any(|issue| issue.contains("cycle")));
    }

    #[test]
    fn plan_parser_drops_dangling_dependencies() {
        let raw = r#"{"tasks":[
            {"id":"t1","agent_id":"a","description":"only task","dependencies":["missing","t1"]}
        ]}"#;
        let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        assert!(plan.tasks[0].dependencies.is_empty());
        assert!(issues.iter().any(|issue| issue.contains("dependency")));
    }

    #[test]
    fn plan_parser_keeps_and_normalizes_task_cwd() {
        let raw = r#"{"tasks":[
            {"id":"t1","agent_id":"a","description":"one","cwd":"  apps/web  "},
            {"id":"t2","agent_id":"a","description":"two","cwd":"./"},
            {"id":"t3","agent_id":"a","description":"three","cwd":null},
            {"id":"t4","agent_id":"a","description":"four"}
        ]}"#;
        let (decision, _) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
        let plan = expect_mission(decision);
        assert_eq!(plan.tasks[0].cwd.as_deref(), Some("apps/web"));
        assert_eq!(plan.tasks[1].cwd, None, "\"./\" means the workspace root");
        assert_eq!(plan.tasks[2].cwd, None, "null means the workspace root");
        assert_eq!(
            plan.tasks[3].cwd, None,
            "missing cwd means the workspace root"
        );
    }

    #[test]
    fn resolve_task_cwd_is_sandboxed_and_creates_missing_directories() {
        let root = std::env::temp_dir().join(format!("ai-chat-cwd-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp workspace");

        // Unset / "." → the workspace root itself.
        assert_eq!(resolve_task_cwd(None, &root).unwrap(), root);
        assert_eq!(resolve_task_cwd(Some("."), &root).unwrap(), root);

        // An existing subdirectory is returned as-is.
        let existing = root.join("apps").join("web");
        std::fs::create_dir_all(&existing).expect("existing task dir");
        let resolved = resolve_task_cwd(Some("apps/web"), &root).expect("existing cwd");
        assert_eq!(
            resolved.canonicalize().unwrap(),
            existing.canonicalize().unwrap()
        );

        // A directory that does not exist yet is created (scaffolding tasks).
        let resolved = resolve_task_cwd(Some("./apps/new"), &root).expect("new cwd");
        assert!(resolved.is_dir(), "missing task dir was not created");
        assert!(resolved.ends_with("apps/new"));

        // Escapes and outside absolute paths are rejected.
        assert!(resolve_task_cwd(Some("../outside"), &root).is_err());
        let outside = std::env::temp_dir().display().to_string();
        assert!(resolve_task_cwd(Some(&outside), &root).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plan_parser_rejects_unusable_plans() {
        // Not JSON at all.
        assert!(parse_task_plan("no plan here", &roster_ids(&["a"])).is_err());
        // Valid JSON, but every task is unusable.
        let raw = r#"{"tasks":[{"id":"t1","agent_id":"ghost","description":"x"}]}"#;
        let err = parse_task_plan(raw, &roster_ids(&["a"])).expect_err("no usable tasks");
        assert!(err.contains("ghost"), "unexpected error: {err}");
        // `delegate:true` without tasks is unusable.
        assert!(parse_task_plan(
            r#"{"delegate":true,"execution_mode":"parallel"}"#,
            &roster_ids(&["a"])
        )
        .is_err());
    }

    #[test]
    fn plan_parser_honours_a_direct_answer() {
        let (decision, issues) = parse_task_plan(
            r#"{"delegate":false,"why":"simple question"}"#,
            &roster_ids(&["a"]),
        )
        .expect("decline parses");
        match decision {
            PlanDecision::Direct { why } => assert_eq!(why, "simple question"),
            PlanDecision::Mission(_) => panic!("expected a direct answer"),
        }
        assert!(issues.is_empty());

        // An explicit false wins even when the model also emitted tasks.
        let raw = r#"{"delegate":false,"why":"no","tasks":[
            {"id":"t1","agent_id":"a","description":"ignored"}
        ]}"#;
        let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("decline parses");
        assert!(matches!(decision, PlanDecision::Direct { .. }));
        assert!(issues.iter().any(|issue| issue.contains("ignored")));
    }

    #[test]
    fn json_extraction_handles_braces_inside_strings() {
        let raw =
            r#"prefix {"tasks":[{"id":"t1","agent_id":"a","description":"a } brace"}]} suffix"#;
        let extracted = extract_json_object(raw).expect("object found");
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
        assert!(extracted.contains("a } brace"));
    }

    #[test]
    fn repaired_json_roundtrips_through_serde() {
        let raw = r#"{"tasks":[{"id":"t1","agent_id":"a","description":"x",}],}"#;
        let repaired = repair_json(raw);
        let _: Value = serde_json::from_str(&repaired).expect("repaired JSON parses");
    }

    #[test]
    fn task_result_skipped_carries_reason() {
        let task = Task {
            id: "t1".to_string(),
            agent_id: "a".to_string(),
            description: "x".to_string(),
            context: String::new(),
            cwd: None,
            dependencies: Vec::new(),
        };
        let result = TaskResult::skipped(&task, "agent a", "agent 'a' is not available");
        assert_eq!(result.status, "skipped");
        assert_eq!(result.task_id, "t1");
        assert!(result.content.contains("not available"));
    }

    #[test]
    fn compact_list_caps_long_capability_lists() {
        let items: Vec<String> = (0..10).map(|i| format!("tool{i}")).collect();
        let text = compact_list(&items);
        assert!(text.ends_with(",…"), "unexpected list: {text}");
        assert!(text.len() < items.join(",").len());
        assert_eq!(compact_list(&[]), "-");
    }

    #[test]
    fn main_helper_tools_drop_chat_only_tools() {
        let mut config = AppConfig::default();
        config.selected_tools = vec![
            "run_cmd".to_string(),
            "file_actions".to_string(),
            "timer".to_string(),
        ];
        let ids = main_helper_tool_ids(&config);
        assert!(ids.iter().any(|id| id == "run_cmd"));
        assert!(ids.iter().any(|id| id == "file_actions"));
        assert!(
            !ids.iter().any(|id| id == "timer"),
            "timer is a chat-session tool and must not reach the helper"
        );
    }

    #[test]
    fn main_helper_history_is_budgeted_and_keeps_the_newest_entries() {
        let records: Vec<crate::db::HistoryRecord> = (0..20)
            .map(|i| crate::db::HistoryRecord {
                id: i,
                session_id: "s".to_string(),
                role: if i % 2 == 0 {
                    "user".to_string()
                } else {
                    "assistant".to_string()
                },
                content: format!("message {i}"),
                timestamp: String::new(),
                tool_calls: None,
                reasoning_content: None,
                attachments: None,
                injections: None,
                error: None,
                status: None,
            })
            .collect();

        let transcript = build_main_helper_history(&records);
        assert!(
            transcript.starts_with("[user]: message 8"),
            "got: {transcript}"
        );
        assert!(
            transcript.ends_with("[assistant]: message 19"),
            "got: {transcript}"
        );
        assert!(
            !transcript.contains("message 7"),
            "older entry was not dropped"
        );
    }

    #[test]
    fn main_helper_history_respects_the_byte_budget() {
        let records: Vec<crate::db::HistoryRecord> = (0..20)
            .map(|i| crate::db::HistoryRecord {
                id: i,
                session_id: "s".to_string(),
                role: "user".to_string(),
                content: format!("{i}:{}", "x".repeat(5_000)),
                timestamp: String::new(),
                tool_calls: None,
                reasoning_content: None,
                attachments: None,
                injections: None,
                error: None,
                status: None,
            })
            .collect();

        let transcript = build_main_helper_history(&records);
        let limit = MAIN_HELPER_HISTORY_BUDGET_BYTES + MAIN_HELPER_HISTORY_MESSAGE_BYTES + 20;
        assert!(
            transcript.len() <= limit,
            "transcript exceeded the budget: {} bytes",
            transcript.len()
        );
    }

    #[test]
    fn main_helper_tool_results_are_capped() {
        let short = "ok".to_string();
        assert_eq!(cap_helper_tool_result(short.clone()), short);

        let long = "x".repeat(MAIN_HELPER_TOOL_RESULT_MAX_BYTES + 100);
        let capped = cap_helper_tool_result(long);
        assert!(capped.len() <= MAIN_HELPER_TOOL_RESULT_MAX_BYTES + 100);
        assert!(capped.contains("truncated"));
    }

    #[test]
    fn mission_snapshot_clips_pathological_task_lists() {
        let tasks: Vec<MissionTaskRecord> = (0..SNAPSHOT_MAX_TASKS + 5)
            .map(|i| MissionTaskRecord {
                task_id: format!("t{i}"),
                name: format!("task {i}"),
                description: "x".repeat(2_000),
                status: "pending".to_string(),
            })
            .collect();
        let snapshot = build_active_tasks_snapshot(&tasks);
        assert!(snapshot.contains("more task(s)"));
        for line in snapshot.lines() {
            assert!(
                line.chars().count() <= SNAPSHOT_TASK_DESC_MAX_CHARS + 10,
                "line not clipped: {} chars",
                line.chars().count()
            );
        }

        let mission = MissionStateSnapshot {
            mission_id: "m1".to_string(),
            root_task_description: "root".to_string(),
            root_task_context: "c".repeat(10_000),
            episodic_summary: String::new(),
            mission_accomplished: false,
            final_report: String::new(),
            active_tasks: Vec::new(),
        };
        let prompt = build_mission_snapshot_prompt(&mission);
        assert!(
            prompt.chars().count() < SNAPSHOT_CONTEXT_MAX_CHARS + 400,
            "snapshot not clipped: {} chars",
            prompt.chars().count()
        );
    }

    fn temp_agents_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-chat-agents-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn llm_agent(id: &str, name: &str) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            agent_type: AGENT_TYPE_LLM.to_string(),
            system_prompt: String::new(),
            model: None,
            max_complete_tokens: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Vec::new(),
            allowed_skills: Vec::new(),
            max_iterations: 10,
            enable_mcp: false,
            emoji: None,
            color: None,
            file: None,
            parse_error: None,
            configured: true,
            cli: Default::default(),
            cloud: Default::default(),
            require_approval: true,
            enabled: false,
        }
    }

    #[test]
    fn saving_config_keeps_only_runtime_config_for_file_backed_agents() {
        let dir = temp_agents_dir("save");
        let path = dir.join("sub_agents.json");

        let mut virtual_agent = llm_agent("engineering/virtual", "Virtual");
        virtual_agent.file = Some("engineering/virtual.md".to_string());
        virtual_agent.configured = false;

        let mut configured = llm_agent("custom/configured", "Configured");
        configured.file = Some("custom/configured.md".to_string());
        configured.system_prompt = "be precise".to_string();
        configured.description = "reviews code".to_string();
        configured.emoji = Some("🤖".to_string());
        configured.enable_mcp = true;

        let mut cli: SubAgent =
            serde_json::from_str(r#"{"id":"cli","name":"OpenCode","description":"","agent_type":"cli","cli":{"command":"opencode"}}"#)
                .expect("cli agent parses");
        cli.configured = true;

        let config = AgentsConfig {
            version: 99,
            agents: vec![virtual_agent, configured, cli],
            orchestration: AgentOrchestration::default(),
            cli_agents: Vec::new(),
        };
        save_agents_config_file(&path, &config).expect("save");

        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
                .expect("json");
        assert_eq!(saved["version"], 5);
        let agents = saved["agents"].as_array().expect("agents array");
        assert_eq!(agents.len(), 2, "virtual agents are never persisted");

        let llm = agents
            .iter()
            .find(|agent| agent["id"] == "custom/configured")
            .expect("configured llm agent");
        assert!(llm.get("name").is_none());
        assert!(llm.get("description").is_none());
        assert!(llm.get("system_prompt").is_none());
        assert!(llm.get("emoji").is_none());
        assert!(llm.get("cli").is_none());
        assert!(llm.get("cloud").is_none());
        assert!(llm.get("require_approval").is_none());
        assert_eq!(llm["file"], "custom/configured.md");
        assert_eq!(llm["enable_mcp"], true);
        assert!(llm.get("configured").is_none());

        let cli = agents
            .iter()
            .find(|agent| agent["id"] == "cli")
            .expect("cli agent");
        assert_eq!(cli["name"], "OpenCode");
        assert!(cli.get("enable_mcp").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn indexed_agents_serialize_with_the_fields_the_panel_reads() {
        let dir = temp_agents_dir("serialize");
        let path = dir.join("sub_agents.json");
        let root = dir.join("agents");
        std::fs::write(&path, r#"{ "agents": [] }"#).expect("write config");
        std::fs::create_dir_all(root.join("custom")).expect("agents dir");
        std::fs::write(
            root.join("custom/reviewer.md"),
            "---\nname: Reviewer\ndescription: reviews code\nemoji: 👀\n---\n\nbe precise\n",
        )
        .expect("write agent");
        // The panel lists discovered agents from the manifest, not a scan.
        super::files::write_index(&root, &super::files::build_index(&root)).expect("write index");

        let resolved = load_sub_agent_list(&path);
        let value = serde_json::to_value(
            resolved
                .iter()
                .find(|agent| agent.file.as_deref() == Some("custom/reviewer.md"))
                .expect("indexed agent"),
        )
        .expect("serialize");

        // The panel's draft conversion reads `cli`/`cloud` for every type and
        // the enable checkbox reads `require_approval`, so these must stay in
        // the API payload even though the config file omits them.
        assert_eq!(value["name"], "Reviewer");
        assert_eq!(value["description"], "reviews code");
        assert_eq!(value["emoji"], "👀");
        assert!(value["cli"].is_object());
        assert!(value["cloud"].is_object());
        assert_eq!(value["require_approval"], true);
        assert_eq!(value["allowed_skills"], serde_json::json!([]));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn configured_load_migrates_inline_prompts_and_panel_reads_the_index() {
        let dir = temp_agents_dir("resolved");
        let path = dir.join("sub_agents.json");
        let root = dir.join("agents");
        std::fs::write(
            &path,
            r#"{
                "agents": [{
                    "id": "legacy",
                    "name": "Legacy",
                    "description": "old",
                    "system_prompt": "be precise",
                    "enabled": true
                }],
                "orchestration": { "use_agents": false }
            }"#,
        )
        .expect("write config");

        std::fs::create_dir_all(root.join("engineering")).expect("agents dir");
        std::fs::write(
            root.join("engineering/library.md"),
            "---\nname: Library\ndescription: from disk\nemoji: 📚\n---\n\nlibrary prompt\n",
        )
        .expect("write library agent");

        // Runtime resolution sees configured agents only; the library file must
        // not be discovered until Refresh writes the manifest.
        let configured = load_configured_agents(&path);
        assert_eq!(configured.agents.len(), 1);
        let legacy = configured
            .agents
            .iter()
            .find(|agent| agent.id == "legacy")
            .expect("legacy agent");
        assert_eq!(legacy.file.as_deref(), Some("custom/legacy.md"));
        assert!(legacy.system_prompt.is_empty());
        assert!(root.join("custom/legacy.md").is_file());

        // Refresh (index build + replace) makes the file visible to the panel.
        super::files::write_index(&root, &super::files::build_index(&root)).expect("write index");
        let resolved = load_sub_agent_list(&path);
        let library = resolved
            .iter()
            .find(|agent| agent.file.as_deref() == Some("engineering/library.md"))
            .expect("indexed library agent");
        assert!(!library.configured);
        assert!(!library.enabled);
        assert_eq!(library.name, "Library");
        assert_eq!(library.emoji.as_deref(), Some("📚"));
        assert_eq!(
            library.allowed_tools,
            vec!["file_actions".to_string(), "memory".to_string()]
        );
        assert!(!library.enable_mcp);

        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
                .expect("json");
        let agents = saved["agents"].as_array().expect("agents array");
        assert_eq!(
            agents.len(),
            1,
            "only the migrated config entry is persisted"
        );
        assert_eq!(agents[0]["id"], "legacy");
        assert_eq!(agents[0]["file"], "custom/legacy.md");
        assert!(agents[0].get("system_prompt").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}

// ─── Final result hand-off ──────────────────────────────────────────────────

/// Renders the finished mission for injection into the main agent's context.
/// The main agent — not a separate synthesizer prompt — defines the final
/// user-facing answer from these reports.
fn build_mission_results_block(
    results: &[TaskResult],
    exchange: Option<&exchange::ExchangeArea>,
) -> String {
    let mut body = String::from(
        "All sub-agent tasks have finished. Sub-agents are execution workers and do not \
         write the user-facing answer: define and deliver the final result yourself. \
         Verify the reports below, reconcile conflicting findings, flag anything that \
         failed or looks incomplete, and answer the original request completely. \
         Tasks marked `skipped` produced no work at all: never present them as completed — \
         either do that part yourself or state clearly what was not done.\n\n",
    );

    if let Some(area) = exchange {
        body.push_str(&format!(
            "Deliverable exchange area: {} (workspace-relative: {})\n\
             Read deliverables with `file_actions` before answering; do not paste huge files \
             verbatim into the answer.\n\n",
            area.root().display(),
            area.relative_root(),
        ));
    }

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
        if let Some(area) = exchange {
            body.push_str("Deliverables:\n");
            body.push_str(&exchange::render_files(&area.list_files(&result.task_id)));
            body.push_str("\n\n");
        }
    }

    body
}
