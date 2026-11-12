use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

use self::paths::{build_workspace_scoped_shell_code, with_root_header};
use self::risk::{generate_risk_report, score_to_risk_level};
use self::search::{run_integrated_search, SearchOptions, SEARCH_MAX_MATCHES};

mod execute;
mod file_actions;
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
/// The main chat owns the `tool-call` transcript event: its listener settles
/// the running step and appends a row. A sub-agent mission must never write
/// there — that would settle the task's *own* row to "done" while it is still
/// working and add a stray main-chat row. Mission progress goes out as
/// `agent-task-tool`, which the UI applies to the task row without touching its
/// status.
pub(crate) fn emit_tool_progress(
    app: &AppHandle,
    mission_id: Option<&str>,
    text: impl Into<String>,
) {
    match mission_id {
        Some(task_id) => {
            let _ = app.emit(
                "agent-task-tool",
                json!({ "task_id": task_id, "phase": "progress", "text": text.into() }),
            );
        }
        None => {
            let _ = app.emit("tool-call", text.into());
        }
    }
}

#[cfg(test)]
mod tests;
