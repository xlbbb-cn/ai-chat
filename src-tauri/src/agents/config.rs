//! Agent configuration: the `SubAgent` roster types, their defaults and
//! the JSON I/O around `agents.json`.

use super::*;

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

pub(crate) fn default_agent_type() -> String {
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
pub(crate) fn default_max_iterations() -> u32 {
    0
}

/// Upper bound accepted for `max_iterations`. Anything larger is clamped on
/// load/save so a hand-edited `sub_agents.json` cannot pin a runaway loop.
pub const MAX_AGENT_ITERATIONS: u32 = 500;
pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn is_false(value: &bool) -> bool {
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

pub(crate) fn default_prompt_via() -> String {
    "arg".to_string()
}

pub(crate) fn default_cli_timeout_seconds() -> u32 {
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

pub(crate) fn default_cloud_body_template() -> String {
    "{\"messages\":[{\"role\":\"user\",\"content\":\"{prompt}\"}]}".to_string()
}
pub(crate) fn default_cloud_result_path() -> String {
    "choices.0.message.content".to_string()
}
pub(crate) fn default_cloud_timeout_seconds() -> u32 {
    1_800
}
pub(crate) fn default_poll_id_path() -> String {
    "id".to_string()
}
pub(crate) fn default_poll_status_path() -> String {
    "status".to_string()
}
pub(crate) fn default_poll_done_values() -> Vec<String> {
    ["completed", "succeeded", "success", "done", "finished"]
        .iter()
        .map(|value| value.to_string())
        .collect()
}
pub(crate) fn default_poll_failed_values() -> Vec<String> {
    ["failed", "error", "cancelled", "canceled"]
        .iter()
        .map(|value| value.to_string())
        .collect()
}
pub(crate) fn default_poll_interval_seconds() -> u32 {
    5
}

/// Definition of a standalone CLI agent written by builds that predate agent
/// types. Only used to migrate `sub_agents.json` / old backup archives forward.
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

pub(crate) fn default_config_version() -> u32 {
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

pub(crate) fn default_max_concurrent() -> usize {
    3
}
pub(crate) fn default_mbus_pair_limit() -> usize {
    mbus::DEFAULT_PAIR_LIMIT
}
pub(crate) fn default_mode() -> String {
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
