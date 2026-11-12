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
    db::EmbeddedRole,
    llm_complete::{
        apply_completion_token_limit, apply_request_headers, build_stream_client, content_to_text,
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

mod config;
mod execution;
mod helper;
mod mission;
mod planning;

pub use self::config::*;
pub(crate) use self::execution::*;
pub(crate) use self::helper::*;
pub use self::mission::*;
pub(crate) use self::planning::*;

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
pub(crate) struct SelectionResult {
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
pub(crate) struct TaskPlan {
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
pub(crate) enum PlanDecision {
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

// ─── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_sub_agents(state: State<'_, AppState>) -> Vec<SubAgent> {
    load_sub_agent_list(&state.agents_config_path)
}

// ─── Composer `+agent` mentions ───────────────────────────────────────────────

/// One row of the composer's `+agent` popup.
///
/// It deliberately carries no `system_prompt`: the popup only needs a label,
/// while the full roster would send hundreds of kilobytes to the frontend. The
/// prompt is snapshotted in the backend when the session's first message is
/// sent (`snapshot_embedded_roles`).
#[derive(Debug, Clone, Serialize)]
pub struct AgentMention {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// The token inserted after `+` in the composer.
    pub token: String,
}

/// Whether `value` can be typed as one mention token.
///
/// The composer grammar (`src/agentMention.ts`) ends a token at whitespace and
/// at `+ @ / # < >`, so a name containing any of them would split itself.
fn is_mention_token_safe(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '+' | '@' | '/' | '#' | '<' | '>'))
}

/// The token offered for `agent`, before roster-wide deduplication.
///
/// The display name wins when it is token-safe (Chinese names usually are), so
/// `+嵌入式固件工程师` matches what the panel shows. Otherwise the markdown file
/// stem is used, which is the stable English handle for imported agents, and a
/// short id fragment is the last resort.
fn agent_mention_base_token(agent: &SubAgent) -> String {
    let name = agent.name.trim();
    if is_mention_token_safe(name) {
        return name.to_string();
    }
    if let Some(file) = agent.file.as_deref() {
        let stem = Path::new(file)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_mention_token_safe(&stem) {
            return stem;
        }
    }
    let fallback: String = agent
        .id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(8)
        .collect();
    if fallback.is_empty() {
        "agent".to_string()
    } else {
        fallback
    }
}

/// The `+agent` popup roster: enabled `llm` agents that have a real system
/// prompt (a `cli` / `cloud` agent has none to embed). Tokens are unique within
/// the roster, with `-2`, `-3` suffixes on a collision.
#[tauri::command]
pub fn list_agent_mentions(state: State<'_, AppState>) -> Vec<AgentMention> {
    let roster = load_configured_agents(&state.agents_config_path);
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut mentions = Vec::new();

    for agent in roster.agents {
        if agent.agent_type != AGENT_TYPE_LLM
            || !agent.enabled
            || agent.system_prompt.trim().is_empty()
        {
            continue;
        }
        let base = agent_mention_base_token(&agent);
        let mut token = base.clone();
        let mut counter = 2u32;
        while used.contains(&token.to_lowercase()) {
            token = format!("{base}-{counter}");
            counter += 1;
        }
        used.insert(token.to_lowercase());
        mentions.push(AgentMention {
            id: agent.id,
            name: agent.name,
            description: agent.description,
            emoji: agent.emoji,
            token,
        });
    }

    mentions
}

/// Freeze the roles the composer embedded into a session.
///
/// Only `llm` agents with a non-empty prompt contribute one. `enabled` is
/// deliberately **not** checked: the popup lists enabled agents, but disabling
/// one between selection and the first send must not silently drop the role the
/// user picked. Unknown ids are skipped and duplicates keep their first
/// position, so the prompt order matches the composer order.
pub fn snapshot_embedded_roles(path: &Path, ids: &[String]) -> Vec<EmbeddedRole> {
    if ids.is_empty() {
        return Vec::new();
    }
    let roster = load_configured_agents(path);
    let mut roles: Vec<EmbeddedRole> = Vec::new();
    for id in ids {
        if roles.iter().any(|role| &role.id == id) {
            continue;
        }
        let Some(agent) = roster.agents.iter().find(|agent| &agent.id == id) else {
            continue;
        };
        let prompt = agent.system_prompt.trim();
        if agent.agent_type != AGENT_TYPE_LLM || prompt.is_empty() {
            continue;
        }
        roles.push(EmbeddedRole {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            prompt: prompt.to_string(),
        });
    }
    roles
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

#[cfg(test)]
mod tests;
