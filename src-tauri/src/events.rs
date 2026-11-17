//! Backend → frontend event contract.
//!
//! Every name and payload shape the webview listens to lives here instead of at
//! the emit call site. `src/api.ts` (`listen`) and `src/types.ts` are the other
//! half of the contract: a field renamed here must be renamed there too.
//!
//! Payloads are structs wherever the shape is fixed. A few events still carry
//! an ad-hoc `serde_json::Value` because their body is assembled incrementally
//! (`chat-usage` cache fields, `agent-task-tool` optional details) or nests a
//! domain type (`todo-state`, `risk-assessment`, `syntax-error`,
//! `agent-selection`, planner/mbus events). Those keep using `json!` at the
//! call site, but their names still come from this module so a typo cannot
//! silently split an event into two.
//!
//! This module deliberately has no dependency on any domain module: it must be
//! safe to import from every layer, including from `ipc/`.

use serde::Serialize;

// ─── Chat / turn ─────────────────────────────────────────────────────────────

pub const CHAT_TOKEN: &str = "chat-token";
pub const CHAT_REASONING_TOKEN: &str = "chat-reasoning-token";
pub const CHAT_DONE: &str = "chat-done";
pub const CHAT_ERROR: &str = "chat-error";
pub const CHAT_RETRY: &str = "chat-retry";
pub const CHAT_USAGE: &str = "chat-usage";
pub const CHAT_NOTICE: &str = "chat-notice";
pub const CHAT_INJECTED: &str = "chat-injected";
pub const TOOL_CALL: &str = "tool-call";
pub const SKILL_LOADED: &str = "skill-loaded";
pub const CONFIRM_REQUIRED: &str = "confirm-required";
pub const RISK_ASSESSMENT: &str = "risk-assessment";
pub const SYNTAX_ERROR: &str = "syntax-error";

/// One streamed content delta of the user's own chat turn.
#[derive(Debug, Clone, Serialize)]
pub struct ChatToken {
    /// Absent for internal requests that are not a user chat turn.
    pub turn_id: Option<String>,
    pub text: String,
}

/// The turn finished (final answer, or an early stop that still closes it).
#[derive(Debug, Clone, Serialize)]
pub struct ChatTurn {
    pub turn_id: String,
}

/// The turn failed terminally.
#[derive(Debug, Clone, Serialize)]
pub struct ChatError {
    pub turn_id: Option<String>,
    pub error: String,
}

/// A streamed attempt failed and is about to be retried. The UI rewinds the
/// partial text it already received to `content` / `reasoning`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRetry {
    pub turn_id: Option<String>,
    pub content: String,
    pub reasoning: String,
}

/// Token accounting of one request (`chat-usage`).
///
/// `max_tokens` and `session_id` are always serialised (possibly `null`) to
/// match the historical shape; the cache fields are only present when the
/// provider actually reported a cache dialect.
#[derive(Debug, Clone, Serialize)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub max_tokens: Option<u32>,
    pub usage_ratio: f64,
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_ratio: Option<f64>,
}

/// A turn-guard notice shown as a warning in the transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum ChatNotice {
    /// `tool_failure_limit` reached: the tool is disabled for this session.
    #[serde(rename = "tool_disabled")]
    ToolDisabled {
        session_id: String,
        tool: String,
        failures: u32,
        limit: u32,
    },
    /// The round / tool-call budget is spent: the model answers with what it has.
    #[serde(rename = "budget_exhausted")]
    BudgetExhausted {
        session_id: String,
        reason: String,
        rounds: u32,
        tool_calls: u32,
    },
}

/// One item delivered into a running turn (mid-turn injection or a background
/// task result).
#[derive(Debug, Clone, Serialize)]
pub struct ChatInjectedItem {
    pub id: String,
    pub text: String,
    pub created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatInjected {
    pub session_id: String,
    pub items: Vec<ChatInjectedItem>,
}

/// One tool-progress line. `call_id` / `phase` / `ok` are absent on the legacy
/// "append a line and settle the previous row" form.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCall {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillLoaded {
    pub session_id: String,
    pub call_id: String,
    pub skill_name: String,
}

/// A risky command awaiting the user's confirmation.
#[derive(Debug, Clone, Serialize)]
pub struct ConfirmRequired {
    pub request_id: String,
    pub call_id: Option<String>,
    pub reason: String,
    pub cmd_type: String,
    pub code: String,
    pub confirm_kind: String,
    pub requires_auth: String,
}

// ─── Todos ───────────────────────────────────────────────────────────────────

pub const TODO_STATE: &str = "todo-state";
pub const TODO_ASK: &str = "todo-ask";
pub const TODO_ASK_CLOSED: &str = "todo-ask-closed";

// ─── Background tasks / timers / sleep lock ──────────────────────────────────

pub const BACKGROUND_TASK_STATE: &str = "background-task-state";
pub const BACKGROUND_WAITING: &str = "background-waiting";
pub const TIMER_STATE: &str = "timer-state";
pub const TIMER_FIRED: &str = "timer-fired";
pub const SLEEP_LOCK: &str = "sleep-lock";

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundWaiting {
    pub session_id: String,
    pub pending: usize,
    /// Only present while the turn is being held open for background work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
}

// ─── Sub-agents / mission bus ────────────────────────────────────────────────

pub const AGENT_SELECTION: &str = "agent-selection";
pub const AGENT_PLAN_START: &str = "agent-plan-start";
pub const AGENT_TASK_START: &str = "agent-task-start";
pub const AGENT_TASK_TOKEN: &str = "agent-task-token";
pub const AGENT_TASK_REASONING: &str = "agent-task-reasoning";
pub const AGENT_TASK_TOOL: &str = "agent-task-tool";
pub const AGENT_TASK_DONE: &str = "agent-task-done";
pub const AGENT_TASK_ERROR: &str = "agent-task-error";
pub const AGENT_TASK_SKIPPED: &str = "agent-task-skipped";
pub const AGENT_TASK_STATE: &str = "agent-task-state";
pub const AGENT_AGGREGATE_DONE: &str = "agent-aggregate-done";
pub const AGENT_PLANNER_STATE: &str = "agent-planner-state";
pub const AGENT_PLANNER_ACTION: &str = "agent-planner-action";
pub const AGENT_PLANNER_TOKEN: &str = "agent-planner-token";
pub const AGENT_MBUS_MESSAGE: &str = "agent-mbus-message";
pub const AGENT_MBUS_DELIVERED: &str = "agent-mbus-delivered";

#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskStart {
    pub task_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub description: String,
    /// Workspace-relative working directory; `null` = workspace root.
    pub cwd: Option<String>,
}

/// Terminal task outcome. `status` is `success` or `stopped` — an error settles
/// through [`AgentTaskError`] instead.
#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskDone {
    pub task_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub summary: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskError {
    pub task_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskSkipped {
    pub task_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub reason: String,
}

/// Streamed delta of a sub-agent task (content or reasoning, depending on the
/// event name).
#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskToken {
    pub task_id: String,
    pub session_id: Option<String>,
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskTool {
    pub task_id: String,
    pub session_id: Option<String>,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAggregateDone {
    pub task_count: usize,
    pub session_id: String,
}

// ─── Evolution maintenance ───────────────────────────────────────────────────

pub const EVOLUTION_MAINTENANCE_STATE: &str = "evolution-maintenance-state";
pub const EVOLUTION_MAINTENANCE_TOKEN: &str = "evolution-maintenance-token";

#[derive(Debug, Clone, Serialize)]
pub struct EvolutionMaintenanceState {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
    #[serde(rename = "toolCalls", skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ─── Backup / update / workspace / markdown / notify / MCP ───────────────────

pub const BACKUP_EXPORT_START: &str = "backup-export-start";
pub const BACKUP_EXPORT_STATUS: &str = "backup-export-status";
pub const BACKUP_EXPORT_DONE: &str = "backup-export-done";
pub const BACKUP_EXPORT_ERROR: &str = "backup-export-error";
pub const BACKUP_RESTORED: &str = "backup-restored";

pub const UPDATE_DOWNLOAD_PROGRESS: &str = "update-download-progress";
pub const WORKSPACE_CHANGED: &str = "workspace-changed";
pub const MARKDOWN_EDIT_OPEN: &str = "markdown-edit-open";
pub const MARKDOWN_EDIT_ERROR: &str = "markdown-edit-error";
pub const NOTIFY_SHOW: &str = "notify-show";
pub const MCP_SERVERS_CHANGED: &str = "mcp-servers-changed";

/// Payload of `markdown-edit-open`: the file the OS opened (or the titlebar
/// menu picked) and its full text.
#[derive(Debug, Clone, Serialize)]
pub struct MarkdownEditPayload {
    pub path: String,
    pub content: String,
}

/// One self-built MCP server whose registry entry changed.
#[derive(Debug, Clone, Serialize)]
pub struct McpServersChanged {
    pub id: String,
    pub name: String,
    pub action: String,
    pub enabled: bool,
    pub source: String,
    pub definition_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every event name in the registry must be unique: a duplicate would split
    /// one channel into two and silently drop half the payloads.
    #[test]
    fn event_names_are_unique() {
        let names = [
            CHAT_TOKEN,
            CHAT_REASONING_TOKEN,
            CHAT_DONE,
            CHAT_ERROR,
            CHAT_RETRY,
            CHAT_USAGE,
            CHAT_NOTICE,
            CHAT_INJECTED,
            TOOL_CALL,
            SKILL_LOADED,
            CONFIRM_REQUIRED,
            RISK_ASSESSMENT,
            SYNTAX_ERROR,
            TODO_STATE,
            TODO_ASK,
            TODO_ASK_CLOSED,
            BACKGROUND_TASK_STATE,
            BACKGROUND_WAITING,
            TIMER_STATE,
            TIMER_FIRED,
            SLEEP_LOCK,
            AGENT_SELECTION,
            AGENT_PLAN_START,
            AGENT_TASK_START,
            AGENT_TASK_TOKEN,
            AGENT_TASK_REASONING,
            AGENT_TASK_TOOL,
            AGENT_TASK_DONE,
            AGENT_TASK_ERROR,
            AGENT_TASK_SKIPPED,
            AGENT_TASK_STATE,
            AGENT_AGGREGATE_DONE,
            AGENT_PLANNER_STATE,
            AGENT_PLANNER_ACTION,
            AGENT_PLANNER_TOKEN,
            AGENT_MBUS_MESSAGE,
            AGENT_MBUS_DELIVERED,
            EVOLUTION_MAINTENANCE_STATE,
            EVOLUTION_MAINTENANCE_TOKEN,
            BACKUP_EXPORT_START,
            BACKUP_EXPORT_STATUS,
            BACKUP_EXPORT_DONE,
            BACKUP_EXPORT_ERROR,
            BACKUP_RESTORED,
            UPDATE_DOWNLOAD_PROGRESS,
            WORKSPACE_CHANGED,
            MARKDOWN_EDIT_OPEN,
            MARKDOWN_EDIT_ERROR,
            NOTIFY_SHOW,
            MCP_SERVERS_CHANGED,
        ];
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "duplicate event name");
        assert_eq!(names.len(), 50);
    }

    #[test]
    fn chat_usage_keeps_the_historical_null_fields() {
        let payload = ChatUsage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
            max_tokens: None,
            usage_ratio: 0.0,
            session_id: None,
            cached_tokens: None,
            hit_ratio: None,
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "prompt_tokens": 10,
                "completion_tokens": 2,
                "total_tokens": 12,
                "max_tokens": null,
                "usage_ratio": 0.0,
                "session_id": null,
            })
        );
    }

    #[test]
    fn chat_notice_is_tagged_by_kind() {
        assert_eq!(
            serde_json::to_value(ChatNotice::ToolDisabled {
                session_id: "s".into(),
                tool: "run_cmd".into(),
                failures: 3,
                limit: 3,
            })
            .unwrap(),
            json!({
                "kind": "tool_disabled",
                "session_id": "s",
                "tool": "run_cmd",
                "failures": 3,
                "limit": 3,
            })
        );
    }

    #[test]
    fn background_waiting_only_carries_the_round_while_holding() {
        let idle = BackgroundWaiting {
            session_id: "s".into(),
            pending: 0,
            round: None,
        };
        assert_eq!(
            serde_json::to_value(idle).unwrap(),
            json!({ "session_id": "s", "pending": 0 })
        );
    }

    #[test]
    fn tool_call_omits_the_legacy_optional_fields() {
        let legacy = ToolCall {
            session_id: "s".into(),
            call_id: None,
            phase: None,
            ok: None,
            text: "done".into(),
        };
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            json!({ "session_id": "s", "text": "done" })
        );
    }
}
