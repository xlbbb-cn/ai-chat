use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

use self::paths::{build_workspace_scoped_shell_code, with_root_header};
use self::risk::{generate_risk_report, score_to_risk_level};
use self::search::{run_integrated_search, SearchOptions, SEARCH_MAX_MATCHES};

pub(crate) mod agent_admin;
pub(crate) mod batch;
pub(crate) mod calculator;
mod execute;
pub(crate) mod experience;
mod file_actions;
pub(crate) mod file_locks;
pub(crate) mod memory;
pub(crate) mod mcp_self;
pub(crate) mod skill_admin;
mod patch;
pub(crate) mod paths;
mod process;
mod risk;
mod schema;
mod search;
pub(crate) mod scripts;
pub(crate) mod serve;
pub(crate) mod timer;
pub(crate) mod todos;
pub(crate) mod background;

// Re-exports for the agent runners (`crate::agents::cli` / `cloud`) and tests.
pub(crate) use self::paths::resolve_safe_path;
pub(crate) use self::execute::*;
pub(crate) use self::process::{decode_process_bytes, OutputDecodeHint};
pub(crate) use self::schema::*;

// Re-exports for the test module below: `mod tests` still reaches the moved
// items through `super::name`, so those names must exist in this namespace.
#[cfg(all(test, windows))]
use self::process::decode_windows_process_bytes_with_code_pages;
#[cfg(test)]
use self::{
    file_actions::build_file_diff,
    patch::apply_unified_patch,
    paths::ensure_mutation_target_allowed,
    risk::{calculate_risk_score, syntax_gate_check, RiskLevel, REJECT_THRESHOLD, RISK_THRESHOLDS},
    search::{contains_ignore_case, SEARCH_MAX_FILE_BYTES},
};

/// Emits one tool-progress line, scoped to the caller.
///
/// The main chat owns the `tool-call` transcript event: its listener applies
/// the line to the row of `call_id` (creating it on the first event) and the
/// call's `done` event settles it. A sub-agent mission must never write there —
/// that would settle the task's *own* row to "done" while it is still working
/// and add a stray main-chat row. Mission progress goes out as
/// `agent-task-tool`, which the UI applies to the task row without touching its
/// status.
///
/// `session_id` routes both payloads: a winding-down turn must not settle rows
/// inside a chat the user already switched to. `impl Into<Option<&str>>` lets
/// every caller pass what it has — `Option<&str>` at the dispatch layer, a bare
/// `&str` inside handlers that already unwrapped it — without per-callsite
/// wrapping. `call_id` is the model's `tool_call.id`; `None` keeps the legacy
/// "append a row and settle the previous one" behaviour for emitters that have
/// no call of their own (background tasks).
pub(crate) fn emit_tool_progress<'a>(
    app: &AppHandle,
    mission_id: Option<&str>,
    session_id: impl Into<Option<&'a str>>,
    call_id: Option<&str>,
    text: impl Into<String>,
) {
    let session_id = session_id.into();
    match mission_id {
        Some(task_id) => {
            let _ = app.emit(
                crate::events::AGENT_TASK_TOOL,
                crate::events::AgentTaskTool {
                    task_id: task_id.to_string(),
                    session_id: session_id.map(str::to_string),
                    phase: "progress".to_string(),
                    tool: None,
                    detail: None,
                    duration_ms: None,
                    ok: None,
                    text: Some(text.into()),
                },
            );
        }
        None => {
            // Structured so the main-chat listener can drop another
            // conversation's steps (a plain string carries no session) and
            // apply the line to the row of *this* call: with a parallel round
            // several rows are open at once, and "the newest running row" is
            // no longer necessarily the one that emitted this step.
            let _ = app.emit(
                crate::events::TOOL_CALL,
                json!({
                    "session_id": session_id,
                    "call_id": call_id,
                    "phase": "progress",
                    "text": text.into()
                }),
            );
        }
    }
}

/// Settles the main-chat row of one tool call.
///
/// `call_id` is the model's `tool_call.id` — the same value the call's progress
/// events carried — so the frontend closes exactly that row. A tool that emits
/// no progress at all (a plain `memory` lookup, `timer_list`, …) still gets a
/// visible row: the frontend creates it from this event when it does not exist
/// yet. A sub-agent mission never reaches here (its row is settled by
/// `agent-task-done`), and neither does a blocked call: those emit their own
/// "not executed" line.
pub(crate) fn emit_tool_call_done(
    app: &AppHandle,
    session_id: Option<&str>,
    call_id: &str,
    name: &str,
    ok: bool,
) {
    let _ = app.emit(
        crate::events::TOOL_CALL,
        json!({
            "session_id": session_id,
            "call_id": call_id,
            "phase": "done",
            "ok": ok,
            "text": format!("*{name}*"),
        }),
    );
}

#[cfg(test)]
mod tests;
