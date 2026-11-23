//! Explicit sub-agent creation from the main agent (self-evolution mode).
//!
//! While the sub-agent system AND self-evolution mode are both on, the main
//! agent may create, update, list or delete sub-agents itself — the same
//! roster the Agents panel edits, going through the exact same cores
//! (`agents::save_sub_agent_core` / `agents::delete_sub_agent_core`), so a
//! self-created agent is indistinguishable from a panel-created one.
//!
//! Prompt injection follows the manuals design (`manuals.rs`): the tool
//! description stays short and points at the `create-subagent` manual, which
//! the model reads before its first `save` of the session. The tool is
//! advertised only to the main chat — the sub-agent helper builds its tool
//! list from `get_all_tools` without this module, and the dispatcher refuses
//! the name anywhere else, so a sub-agent can never re-create the roster.

use serde_json::{json, Value};
use tauri::AppHandle;

use super::emit_tool_progress;
use crate::agents;
use crate::AppState;

/// The `agent_admin` tool definition. The full text lives in `prompt.rs`.
pub(crate) fn definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "agent_admin",
            "description": crate::prompt::agent_admin_description(),
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "save", "delete"],
                        "description": "'list' (default) returns the roster; 'save' creates or updates one agent from `agent`; 'delete' removes the agent with `id`."
                    },
                    "agent": {
                        "type": "object",
                        "description": "Required for 'save'. Shape per `agent_type` is documented in `.data/manuals/create-subagent.md`: name, description, agent_type (\"llm\"|\"cli\"|\"cloud\"), plus type-specific fields (system_prompt/model/allowed_tools/enable_mcp for llm; cli.command/cli.args/prompt_via for cli; cloud.endpoint for cloud). Omit `id` to create.",
                    },
                    "id": {
                        "type": "string",
                        "description": "Required for 'delete'. The agent id from 'list'."
                    }
                }
            }
        }
    })
}

/// Execute one `agent_admin` call and return the tool-message text.
pub(crate) async fn handle(app: &AppHandle, state: &AppState, call_id: Option<&str>, parsed: &Value) -> String {
    let action = parsed
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("list")
        .trim()
        .to_ascii_lowercase();

    let result = match action.as_str() {
        "list" | "" => {
            let roster = agents::load_sub_agent_list(&state.agents_config_path);
            let lines: Vec<String> = roster
                .iter()
                .map(|a| {
                    format!(
                        "- {} [{}] {} — {}{}",
                        a.id,
                        agents::normalize_agent_type(&a.agent_type),
                        a.name,
                        a.description,
                        if a.enabled { "" } else { " (disabled)" }
                    )
                })
                .collect();
            Ok(if lines.is_empty() {
                "The roster is empty.".to_string()
            } else {
                format!("Sub-agent roster:\n{}", lines.join("\n"))
            })
        }
        "save" => {
            let Some(agent_value) = parsed.get("agent") else {
                return "Error: action 'save' requires an `agent` object — read \
                        .data/manuals/create-subagent.md for the field shapes."
                    .to_string();
            };
            match serde_json::from_value::<agents::SubAgent>(agent_value.clone()) {
                Ok(agent) => match agents::save_sub_agent_core(&state.agents_config_path, agent) {
                    Ok(saved) => Ok(format!(
                        "Saved agent '{}' [{}] (id `{}`, type {}). It is selectable with \
                         `call_subagent` from your next request.",
                        saved.name,
                        saved.agent_type,
                        saved.id,
                        if saved.enabled { "enabled" } else { "disabled" }
                    )),
                    Err(err) => Err(err),
                },
                Err(err) => Err(format!(
                    "The `agent` object does not match the schema: {err}. Read \
                     .data/manuals/create-subagent.md for the exact field shapes."
                )),
            }
        }
        "delete" => {
            let Some(id) = parsed.get("id").and_then(Value::as_str) else {
                return "Error: action 'delete' requires the agent `id`.".to_string();
            };
            match agents::delete_sub_agent_core(&state.agents_config_path, id.trim()) {
                Ok(()) => Ok(format!("Deleted agent '{}'.", id.trim())),
                Err(err) => Err(err),
            }
        }
        other => Err(format!(
            "Unknown agent_admin action '{other}' — use 'list', 'save' or 'delete'."
        )),
    };

    if let Ok(logger) = state.logger.lock() {
        logger.log(
            "INFO",
            &format!("agent_admin action={action} ok={}", result.is_ok()),
        );
    }
    emit_tool_progress(
        app,
        None,
        None::<&str>,
        call_id,
        format!("agent_admin {action}"),
    );
    match result {
        Ok(text) => text,
        Err(err) => format!("Error: {err}"),
    }
}
