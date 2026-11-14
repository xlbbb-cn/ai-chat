//! `mcp_self` — the assistant builds and runs its own MCP servers.
//!
//! Self-evolution mode only. The tool owns the whole lifecycle of a *local*
//! stdio MCP server the assistant wrote itself:
//!
//! ```text
//! <workspace>/.data/mcp/<slug>/server.py         the script (workspace-owned)
//! <workspace>/.data/mcp/<slug>/server.py.bak.1   previous version
//! <workspace>/.data/mcp.conf                     the roster (YAML, merged with
//!                                                the app-data mcp_servers.json)
//! ```
//!
//! The roster lives *in the workspace*, next to the scripts: it follows the
//! workspace switch, it travels inside the backup's workspace snapshot, and
//! the assistant never touches the user's app-data `mcp_servers.json`. Both
//! files are merged into the effective roster (`mcp::load_effective_servers`),
//! so the panel lists self-built servers like any other entry.
//!
//! `file_actions` could hand-edit `mcp.conf`; the tool exists because turning
//! workspace code into a running server is more than a file write:
//!
//! - the interpreter is chosen by the backend from a fixed whitelist
//!   (`python` / `node` / `shell`), never by the model, so the tool can never
//!   register an arbitrary process;
//! - the script always lives under `<workspace>/.data/mcp/<slug>/`;
//! - the roster entry, the `.bak` copy and the process start/tools-list happen
//!   as one audited step;
//! - creating, enabling and removing ask the user once
//!   (`user_software`), which `auto_accept_confirm_kinds` can pre-approve;
//! - every mutation bumps [`AppState::mcp_revision`], so `chat_completion`
//!   rebuilds its MCP tool segment at the next request boundary and the new
//!   tools are callable inside the *same* turn.
//!
//! The registry entry keeps the script as a **workspace-relative** argument
//! (`command = python3`, `args = [".data/mcp/<slug>/server.py"]`). Every stdio
//! child already starts in the workspace root (`mcp::build_stdio_cmd`), so the
//! relative argument resolves locally and still resolves after a backup
//! restore on another machine.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

use super::paths::{resolve_input_path, resolve_safe_path};
use super::request_tool_confirmation;
use crate::mcp::SOURCE_SELF_EVOLUTION;
use crate::{mcp, AppState};

/// Tool name advertised to the model.
pub const TOOL_NAME: &str = "mcp_self";

/// Emitted after every registry mutation. The panel refreshes its list from it;
/// the chat turn uses `AppState::mcp_revision` instead (a tool can also be
/// called by a background task, which the panel must still see).
pub const CHANGED_EVENT: &str = "mcp-servers-changed";

/// Hard cap on one server script. A generated MCP server is a tool adapter, not
/// a project — anything larger belongs in a normal workspace file.
const MAX_SCRIPT_BYTES: usize = 200_000;

/// How much source code the confirmation dialog shows.
const PREVIEW_CHARS: usize = 1_500;

/// Diagnostic lines returned by `logs` / appended to a failure.
const LOG_TAIL_LINES: usize = 30;

/// The tool owns exactly this workspace-relative directory.
///
/// Derived from the single data-layout constant (`workspace::DATA_DIR`) instead
/// of repeating the literal: the script folder, the roster and
/// `workspace::mcp_root` must never drift apart.
fn self_mcp_rel_dir() -> String {
    format!("{}/mcp", crate::workspace::DATA_DIR)
}

/// The roster file the tool writes (next to the scripts it describes). Merged
/// with the app-data `mcp_servers.json` into the effective server list.
fn roster_rel_path() -> String {
    format!("{}/mcp.conf", crate::workspace::DATA_DIR)
}

// ─── Schema ──────────────────────────────────────────────────────────────────

/// Tool definition advertised to the main agent in self-evolution mode.
pub fn schema() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Build and run your own MCP servers (self-evolution mode only). \
    A server is a small stdio program speaking line-delimited JSON-RPC 2.0: answer `initialize`, \
    then `tools/list` with your schemas, then `tools/call` returning \
    `content: [{type: \"text\", text: ...}]`. Write ONLY JSON-RPC to stdout — nothing else, and \
    log to stderr.\n\n\
    The script lives at `<workspace>/.data/mcp/<slug>/server.<ext>`, registered in \
    `<workspace>/.data/mcp.conf` (YAML, `source: \"self-evolution\"`) and merged with the app-data \
    `mcp_servers.json` that the user's panel owns — never write that file. The backend resolves \
    the interpreter from `runtime`, never an arbitrary command; stdio children start in the \
    workspace root, so workspace-relative paths work — write any scratch data a server needs to \
    `.data/tmp/` (`utf-8` JSON/CSV, never credentials) instead of the workspace root.\n\n\
    Workflow: `write` (create/update, optionally enable, returns the tool names) → use the tools → \
    `logs` on failure (`write` re-tests, so `test` is for diagnosis). `write`, `enable` and `remove` \
    ask the user once (`user_software`); new tools are callable from your next request in the same \
    turn. Rules: no credentials in a script (use the environment or an app tool); one job and a few \
    tools per server; prefer `python` unless the task needs Node; a server whose `tools/list` fails \
    stays disabled so it cannot stall later requests. A running server keeps the code it started \
    with — after editing it with `file_actions`, `restart` (or `write` again).",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "write", "enable", "disable", "test", "restart", "logs", "remove"],
                        "description": "'list' shows your servers; 'write' creates/updates a script and registers it; 'enable'/'disable' toggles one; 'test' runs tools/list; 'restart' kills the process and re-runs the handshake after an edit; 'logs' returns recent diagnostics; 'remove' stops it, deletes its folder and drops the registry entry."
                    },
                    "name": {
                        "type": "string",
                        "description": "Server name ('write'); human readable — a slug is derived for the folder."
                    },
                    "runtime": {
                        "type": "string",
                        "enum": ["python", "node", "shell"],
                        "description": "Interpreter ('write', default 'python'). The backend resolves the concrete command (python3/python, node, bash/powershell); arbitrary executables are refused."
                    },
                    "code": {
                        "type": "string",
                        "description": "Complete script source ('write'). Must implement the handshake plus `tools/list` and `tools/call`."
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional one-line note about the server; stored in the diagnostic log."
                    },
                    "env": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Optional environment variables for the server process. Never put credentials here."
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "For 'write': enable and start it immediately (default true). A failed start leaves it disabled and reports the error plus the log tail."
                    },
                    "id": {
                        "type": "string",
                        "description": "Existing server id ('enable'/'disable'/'test'/'restart'/'logs'/'remove'; optional for 'write' to update in place). Copy it from 'list'."
                    }
                },
                "required": ["action"]
            }
        }
    })
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// Execute one `mcp_self` call and return the tool result text.
pub async fn handle(
    app: &AppHandle,
    args: &Value,
    workspace_dir: &Path,
    cancel: Option<&Arc<AtomicBool>>,
) -> String {
    let action = args["action"].as_str().unwrap_or("list").trim().to_string();
    let result = match action.as_str() {
        "list" => list(app),
        "write" => write(app, args, workspace_dir, cancel).await,
        "enable" => set_enabled(app, args, true, cancel).await,
        "disable" => set_enabled(app, args, false, cancel).await,
        "test" => test(app, args).await,
        "restart" => restart(app, args).await,
        "logs" => read_logs(app, args),
        "remove" => remove(app, args, workspace_dir, cancel).await,
        other => Err(format!(
            "unknown action '{other}'; use list | write | enable | disable | test | restart | logs | remove"
        )),
    };
    match result {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
        Err(err) => format!("Error: {err}"),
    }
}

// ─── Actions ─────────────────────────────────────────────────────────────────

/// Every self-built server, with its script state. Cheap: never spawns.
fn list(app: &AppHandle) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let servers: Vec<Value> = mcp::load_workspace_servers(&workspace_dir)
        .into_iter()
        .map(|server| {
            let script = script_of(&workspace_dir, &server);
            json!({
                "id": server.id,
                "name": server.name,
                "enabled": server.enabled,
                "runtime_command": server.command,
                "args": server.args,
                "definition_path": server.definition_path,
                "script_exists": script.is_some(),
                "script_bytes": script
                    .and_then(|path| std::fs::metadata(path).ok())
                    .map(|meta| meta.len())
                    .unwrap_or(0),
            })
        })
        .collect();

    Ok(json!({
        "root": self_mcp_rel_dir(),
        "roster": roster_rel_path(),
        "count": servers.len(),
        "servers": servers,
        "hint": if servers.is_empty() {
            "no self-built servers yet — call write with a name, runtime and code"
        } else {
            "write | enable | disable | test | restart | logs | remove, with the id above"
        },
    }))
}

/// Create or update a server script, register it, and (by default) start it.
async fn write(
    app: &AppHandle,
    args: &Value,
    workspace_dir: &Path,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Value, String> {
    let name = args["name"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("'name' is required for action 'write'")?
        .to_string();
    let runtime = args["runtime"]
        .as_str()
        .unwrap_or("python")
        .trim()
        .to_lowercase();
    let code = args["code"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or("'code' is required for action 'write'")?
        .to_string();
    if code.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "script is {} bytes; the limit is {MAX_SCRIPT_BYTES} (put reusable code in a workspace file and import it)",
            code.len()
        ));
    }
    let description = args["description"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    let env: std::collections::HashMap<String, String> = args["env"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let start_enabled = args["enabled"].as_bool().unwrap_or(true);

    let state = app.state::<AppState>();
    let spec = resolve_runtime(&runtime)?;

    let existing = find_self_built(&state, args["id"].as_str(), Some(&name));
    // Reuse the existing folder when updating in place, so an edited server
    // keeps its id, its enabled state and its path — only the bytes change.
    // The folder is always re-derived as a workspace-*relative* string, so a
    // legacy entry that stored an absolute or `\`-separated `definition_path`
    // is repaired here instead of being written back into the roster.
    let rel_dir = existing
        .as_ref()
        .and_then(|server| existing_rel_dir(workspace_dir, &server.definition_path))
        .unwrap_or_else(|| unique_rel_dir(workspace_dir, &name));
    let rel_script = format!("{rel_dir}/server.{}", spec.ext);
    let (script_path, rel_script) = resolve_input_path(workspace_dir, &rel_script)?;

    if let Some(server) = &existing {
        let same = write_script(&script_path, &code)?;
        if same {
            mcp::append_mcp_log(
                &state,
                &server.id,
                mcp::McpLogLevel::Info,
                format!("mcp_self: script rewritten unchanged ({rel_script})"),
            );
        }
    } else {
        write_script(&script_path, &code)?;
    }

    let id = existing
        .as_ref()
        .map(|server| server.id.clone())
        .unwrap_or_else(|| format!("mcp-self-{}", slugify(&name)));
    // Never take over a panel-managed entry: the id is derived from the name,
    // so a user-created server with the same slug must win.
    if mcp::load_servers(&state.mcp_servers_path)
        .iter()
        .any(|entry| entry.id == id && entry.source != SOURCE_SELF_EVOLUTION)
    {
        return Err(format!(
            "the id '{id}' already belongs to a panel-managed server; pick another name"
        ));
    }
    let server = mcp::McpServer {
        id,
        name: name.clone(),
        transport: mcp::McpTransport::Stdio,
        command: spec.command.clone(),
        args: spec
            .leading_args
            .iter()
            .cloned()
            .chain([rel_script.clone()])
            .collect(),
        env,
        url: String::new(),
        auth_token: String::new(),
        enabled: false,
        source: SOURCE_SELF_EVOLUTION.to_string(),
        definition_path: rel_script.clone(),
    };

    // Register disabled first: an entry that cannot start must never linger in
    // the enabled state (every later request would retry tools/list).
    mcp::upsert_workspace_server(&state, server.clone())?;
    mcp::append_mcp_log(
        &state,
        &server.id,
        mcp::McpLogLevel::Info,
        format!(
            "mcp_self: {} {}{} ({})",
            if existing.is_some() {
                "updated"
            } else {
                "created"
            },
            spec.command,
            spec.leading_args.join(" "),
            if description.is_empty() {
                rel_script.clone()
            } else {
                format!("{rel_script} — {description}")
            }
        ),
    );
    emit_changed(
        app,
        &server,
        if existing.is_some() {
            "updated"
        } else {
            "created"
        },
    );

    if !start_enabled {
        // An explicit `enabled: false` must also retire a process that the
        // previous version of the script left running.
        if existing
            .as_ref()
            .map(|server| server.enabled)
            .unwrap_or(false)
        {
            mcp::stop_stdio_process(&state, &server.id).await;
            emit_changed(app, &server, "disabled");
        }
        return Ok(server_report(&state, &server, &[]));
    }

    let confirmed = ask_confirmation(app, &server, &code, cancel).await;
    if !confirmed {
        return Err(format!(
            "the user declined to run '{}'; the script is saved and registered but stays disabled",
            server.name
        ));
    }

    start_server(app, &state, server).await
}

/// Enable or disable one server.
async fn set_enabled(
    app: &AppHandle,
    args: &Value,
    enabled: bool,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let mut server = require_self_built(&state, args)?;

    if !enabled {
        server.enabled = false;
        mcp::upsert_workspace_server(&state, server.clone())?;
        mcp::stop_stdio_process(&state, &server.id).await;
        emit_changed(app, &server, "disabled");
        return Ok(server_report(&state, &server, &[]));
    }

    if !ask_confirmation(app, &server, "", cancel).await {
        return Err(format!("the user declined to start '{}'", server.name));
    }
    start_server(app, &state, server).await
}

/// Run `tools/list` on an already enabled server.
async fn test(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let server = require_self_built(&state, args)?;
    if !server.enabled {
        return Err(format!(
            "'{}' is disabled — enable it first (enable), then test",
            server.name
        ));
    }
    match mcp::get_server_tools(&state, &server).await {
        Ok(tools) => Ok(server_report(&state, &server, &tools)),
        Err(err) => Err(with_log_tail(
            &state,
            &server.id,
            format!("'{}' failed tools/list: {err}", server.name),
        )),
    }
}

/// Kill the process and re-run the handshake, so an edited script takes effect.
async fn restart(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let server = require_self_built(&state, args)?;
    if !server.enabled {
        return Err(format!(
            "'{}' is disabled — enable it first (enable), then restart",
            server.name
        ));
    }
    mcp::stop_stdio_process(&state, &server.id).await;
    match mcp::get_server_tools(&state, &server).await {
        Ok(tools) => Ok(server_report(&state, &server, &tools)),
        Err(err) => Err(with_log_tail(
            &state,
            &server.id,
            format!("'{}' failed tools/list after restart: {err}", server.name),
        )),
    }
}

/// Recent diagnostic lines for one server (spawn, stderr, tool calls).
fn read_logs(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let server = require_self_built(&state, args)?;
    Ok(json!({
        "id": server.id,
        "name": server.name,
        "enabled": server.enabled,
        "lines": log_tail(&state, &server.id),
    }))
}

/// Stop the server, delete its folder and drop the registry entry.
async fn remove(
    app: &AppHandle,
    args: &Value,
    workspace_dir: &Path,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let server = require_self_built(&state, args)?;

    if !ask_confirmation(app, &server, "", cancel).await {
        return Err(format!("the user declined to remove '{}'", server.name));
    }

    mcp::stop_stdio_process(&state, &server.id).await;
    mcp::remove_workspace_server(&state, &server.id)?;

    // Only ever delete inside the tool-owned directory — a hand-edited
    // `definition_path` must not turn into an arbitrary workspace delete.
    let mut removed_dir = false;
    // `existing_rel_dir` is the guard: it normalizes the stored spelling and
    // returns `None` unless it still points inside the tool-owned directory.
    if let Some(dir) = existing_rel_dir(workspace_dir, &server.definition_path) {
        if let Ok((path, _)) = resolve_input_path(workspace_dir, &dir) {
            if path.is_dir() {
                removed_dir = std::fs::remove_dir_all(&path).is_ok();
            }
        }
    }
    state.mcp_logs.lock().unwrap().remove(&server.id);
    emit_changed(app, &server, "removed");

    Ok(json!({
        "id": server.id,
        "name": server.name,
        "removed_entry": true,
        "removed_directory": removed_dir,
        "definition_path": server.definition_path,
    }))
}

// ─── Startup / report helpers ────────────────────────────────────────────────

/// Upsert + start + `tools/list`. On failure the entry is disabled again and
/// the error carries the log tail, so the model can fix the script instead of
/// guessing.
async fn start_server(
    app: &AppHandle,
    state: &AppState,
    mut server: mcp::McpServer,
) -> Result<Value, String> {
    // Always start from a fresh process: `write` may have replaced the script,
    // and a live child keeps serving the code it was started with (`tools/list`
    // would report the old tools). No-op when nothing is running.
    mcp::stop_stdio_process(state, &server.id).await;
    server.enabled = true;
    mcp::upsert_workspace_server(state, server.clone())?;

    match mcp::get_server_tools(state, &server).await {
        Ok(tools) => {
            emit_changed(app, &server, "enabled");
            Ok(server_report(state, &server, &tools))
        }
        Err(err) => {
            server.enabled = false;
            let _ = mcp::upsert_workspace_server(state, server.clone());
            mcp::stop_stdio_process(state, &server.id).await;
            emit_changed(app, &server, "start-failed");
            Err(with_log_tail(
                state,
                &server.id,
                format!(
                    "'{}' did not answer tools/list, so it was left disabled: {err}\n\
                     common causes: non-JSON output on stdout (log to stderr instead), a missing \
                     flush after a response, or no reply to `initialize` carrying the same id",
                    server.name
                ),
            ))
        }
    }
}

/// The JSON description of one server returned to the model.
fn server_report(state: &AppState, server: &mcp::McpServer, tools: &[Value]) -> Value {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let script = script_of(&workspace_dir, &server);

    let advertised: Vec<Value> = tools
        .iter()
        .filter_map(|tool| {
            let (fn_name, actual_name, _) = mcp::adapt_tool_for_llm(server, tool, false)?;
            Some(json!({
                "advertised_name": fn_name,
                "name": actual_name,
                "description": clip(
                    tool.pointer("/function/description")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    600
                ),
                "parameters": tool
                    .pointer("/function/parameters")
                    .map(|value| clip(&value.to_string(), 2_000))
                    .unwrap_or_else(|| "{}".to_string()),
            }))
        })
        .collect();

    json!({
        "id": server.id,
        "name": server.name,
        "enabled": server.enabled,
        "runtime_command": server.command,
        "args": server.args,
        "definition_path": server.definition_path,
        "script_exists": script.is_some(),
        "tool_count": advertised.len(),
        "tools": advertised,
        "note": if server.enabled && !advertised.is_empty() {
            "these tools are advertised to you from your next request on — call them by advertised_name"
        } else if !server.enabled {
            "the server is registered but disabled; enable it to use its tools"
        } else {
            "the server answered tools/list with no tools — add at least one tool to its handler"
        },
    })
}

// ─── Registry / path helpers ────────────────────────────────────────────────

fn require_self_built(state: &AppState, args: &Value) -> Result<mcp::McpServer, String> {
    find_self_built(state, args["id"].as_str(), args["name"].as_str()).ok_or_else(|| {
        "no self-built server matches 'id'/'name' — call list for the ids".to_string()
    })
}

/// Look a server up by `id`, else by `name` (case-insensitive), restricted to
/// the entries this tool owns. Matching by name is what makes a second `write`
/// of the same server an update instead of `<slug>-2`.
fn find_self_built(
    state: &AppState,
    id: Option<&str>,
    name: Option<&str>,
) -> Option<mcp::McpServer> {
    // The workspace roster *is* this tool's set: `.data/mcp.conf` only ever
    // holds servers the assistant built (the panel owns `mcp_servers.json`).
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let servers = mcp::load_workspace_servers(&workspace_dir);
    if let Some(id) = id.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(found) = servers.iter().find(|server| server.id == id) {
            return Some(found.clone());
        }
    }
    let name = name.map(str::trim).filter(|value| !value.is_empty())?;
    servers
        .into_iter()
        .find(|server| server.name.eq_ignore_ascii_case(name))
}

/// Choose the concrete interpreter for a runtime. The model never supplies a
/// command — this whitelist is the whole containment story.
struct RuntimeSpec {
    command: String,
    leading_args: Vec<String>,
    ext: &'static str,
}

fn resolve_runtime(runtime: &str) -> Result<RuntimeSpec, String> {
    #[cfg(windows)]
    let (command, leading_args, ext): (String, Vec<String>, &'static str) = match runtime {
        "python" => (pick(&["python", "python3", "py"]), Vec::new(), "py"),
        "node" => (pick(&["node", "nodejs"]), Vec::new(), "js"),
        "shell" => (
            pick(&["powershell", "pwsh"]),
            [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]
            .iter()
            .map(|arg| arg.to_string())
            .collect(),
            "ps1",
        ),
        other => {
            return Err(format!(
                "unknown runtime '{other}'; use python | node | shell"
            ))
        }
    };
    #[cfg(not(windows))]
    let (command, leading_args, ext): (String, Vec<String>, &'static str) = match runtime {
        "python" => (pick(&["python3", "python"]), Vec::new(), "py"),
        "node" => (pick(&["node", "nodejs"]), Vec::new(), "js"),
        "shell" => (pick(&["bash", "sh"]), Vec::new(), "sh"),
        other => {
            return Err(format!(
                "unknown runtime '{other}'; use python | node | shell"
            ))
        }
    };

    Ok(RuntimeSpec {
        command,
        leading_args,
        ext,
    })
}

/// First interpreter of `candidates` that exists on PATH.
///
/// The app process PATH can be narrower than a login shell's (that is exactly
/// why `mcp::build_stdio_cmd` launches stdio servers through `$SHELL -l -c`),
/// so an inconclusive probe returns the first candidate instead of failing:
/// `tools/list` then reports the real error with the spawn output.
fn pick(candidates: &[&str]) -> String {
    candidates
        .iter()
        .find(|candidate| mcp::command_exists(candidate))
        .or_else(|| candidates.first())
        .map(|candidate| (*candidate).to_string())
        .unwrap_or_default()
}

/// `name` → folder slug (`Weather Tools!` → `weather-tools`), never empty.
fn slugify(name: &str) -> String {
    let slug = crate::util::slugify(name, 40);
    if slug.is_empty() {
        "server".to_string()
    } else {
        slug
    }
}

/// `<workspace>/.data/mcp/<slug>`, suffixed until the folder is unused.
fn unique_rel_dir(workspace_dir: &Path, name: &str) -> String {
    let root = self_mcp_rel_dir();
    let slug = slugify(name);
    let mut candidate = format!("{root}/{slug}");
    let mut suffix = 2;
    // A candidate that cannot be resolved at all ends the loop as-is: the
    // caller resolves it again and reports the real error, rather than this
    // loop inventing a `-99` folder for a structural problem.
    while let Ok(path) = resolve_safe_path(workspace_dir, &candidate) {
        if !path.exists() {
            break;
        }
        candidate = format!("{root}/{slug}-{suffix}");
        suffix += 1;
        if suffix > 99 {
            break;
        }
    }
    candidate
}

/// `.data/mcp/x/server.py` → `.data/mcp/x`, whatever the caller spelled.
///
/// Both separators are accepted: a legacy roster entry may hold
/// `C:\\ws\\.data\\mcp\\x\\server.py`, which used to yield `None` here — a
/// second `write` then created a second folder and `remove` left the first one
/// behind.
fn parent_dir(definition_path: &str) -> Option<String> {
    let normalized = super::paths::normalize_input_path(definition_path);
    let slash = normalized.rfind('/')?;
    let dir = normalized[..slash].trim_end_matches('/');
    if dir.is_empty() {
        None
    } else {
        Some(dir.to_string())
    }
}

/// True for the tool-owned root itself and any folder inside it.
fn is_inside_self_root(dir: &str) -> bool {
    let root = self_mcp_rel_dir();
    dir == root || dir.starts_with(&format!("{root}/"))
}

/// Workspace-relative folder of an existing server, but only while it still
/// points inside the tool-owned directory.
///
/// Absolute and `\`-separated legacy spellings are mapped back to
/// `.data/mcp/<slug>` (the resolver canonicalizes them first), so an in-place
/// update keeps the folder that already exists instead of forking a new one.
fn existing_rel_dir(workspace_dir: &Path, definition_path: &str) -> Option<String> {
    let (_, relative) = resolve_input_path(workspace_dir, definition_path).ok()?;
    let dir = parent_dir(&relative)?;
    is_inside_self_root(&dir).then_some(dir)
}

/// The script of a server, when it is a file that resolves inside the
/// workspace (`list` / `server_report`).
fn script_of(workspace_dir: &Path, server: &mcp::McpServer) -> Option<PathBuf> {
    resolve_input_path(workspace_dir, &server.definition_path)
        .ok()
        .map(|(path, _)| path)
        .filter(|path| path.is_file())
}

/// Write the script, keeping one `.bak.<n>` copy of the previous version.
/// Returns `true` when the bytes were already identical (no write happened).
fn write_script(path: &Path, code: &str) -> Result<bool, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create '{}': {e}", parent.display()))?;
    }
    let payload = if code.ends_with('\n') {
        code.to_string()
    } else {
        format!("{code}\n")
    };
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == payload {
            return Ok(true);
        }
        backup_script(path)?;
    }
    std::fs::write(path, payload).map_err(|e| format!("cannot write '{}': {e}", path.display()))?;
    Ok(false)
}

/// Copy `path` to the next free `<file>.bak.<n>`.
fn backup_script(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let base = path.to_string_lossy().to_string();
    let mut n = 1;
    loop {
        let candidate = PathBuf::from(format!("{base}.bak.{n}"));
        if !candidate.exists() {
            std::fs::copy(path, &candidate).map_err(|e| {
                format!(
                    "cannot back up '{}' to '{}': {e}",
                    path.display(),
                    candidate.display()
                )
            })?;
            return Ok(Some(candidate));
        }
        n += 1;
        if n > 999 {
            return Err(format!(
                "cannot back up '{}': too many .bak files",
                path.display()
            ));
        }
    }
}

// ─── Confirmation / logging / events ────────────────────────────────────────

/// Ask the user once before the server runs on their machine.
///
/// `user_software` is the risk kind that means "installs / modifies a user
/// application": exactly what registering a program that the app will execute
/// on every request is. `auto_accept_confirm_kinds` can pre-approve it.
async fn ask_confirmation(
    app: &AppHandle,
    server: &mcp::McpServer,
    code: &str,
    cancel: Option<&Arc<AtomicBool>>,
) -> bool {
    let command = if server.args.is_empty() {
        server.command.clone()
    } else {
        format!("{} {}", server.command, server.args.join(" "))
    };
    let reason = format!(
        "Self-built MCP server '{}' runs `{}` from {} and exposes its tools to the assistant.",
        server.name, command, server.definition_path
    );
    let preview = if code.trim().is_empty() {
        format!("command: {command}\nscript: {}\n", server.definition_path)
    } else {
        clip(code, PREVIEW_CHARS)
    };
    let outcome = request_tool_confirmation(
        app,
        reason,
        "mcp_self".to_string(),
        preview,
        "user_software",
        "none",
        cancel,
    )
    .await;
    outcome.confirmed
}

fn emit_changed(app: &AppHandle, server: &mcp::McpServer, action: &str) {
    let _ = app.emit(
        CHANGED_EVENT,
        json!({
            "id": server.id,
            "name": server.name,
            "action": action,
            "enabled": server.enabled,
            "source": server.source,
            "definition_path": server.definition_path,
        }),
    );
}

fn log_tail(state: &AppState, id: &str) -> Vec<String> {
    let Ok(map) = state.mcp_logs.lock() else {
        return Vec::new();
    };
    let Some(buffer) = map.get(id) else {
        return Vec::new();
    };
    buffer
        .iter()
        .rev()
        .take(LOG_TAIL_LINES)
        .rev()
        .map(|entry| format!("[{:?}] {}", entry.level, entry.message))
        .collect()
}

/// Append the last diagnostic lines to a failure message, so one tool result is
/// enough to fix the script.
fn with_log_tail(state: &AppState, id: &str, message: String) -> String {
    let tail = log_tail(state, id);
    if tail.is_empty() {
        message
    } else {
        format!("{message}\nrecent diagnostics:\n{}", tail.join("\n"))
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_a_function_named_mcp_self() {
        let schema = schema();
        assert_eq!(schema["function"]["name"], TOOL_NAME);
        let actions = schema["function"]["parameters"]["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        for expected in [
            "list", "write", "enable", "disable", "test", "restart", "logs", "remove",
        ] {
            assert!(
                actions.iter().any(|value| value.as_str() == Some(expected)),
                "missing action {expected}"
            );
        }
    }

    #[test]
    fn slugs_are_folder_safe_and_never_empty() {
        assert_eq!(slugify("Weather Tools!"), "weather-tools");
        assert_eq!(slugify("  a//b  "), "a-b");
        assert_eq!(slugify("天气 tool"), "天气-tool");
        assert_eq!(slugify("!!!"), "server");
        assert_eq!(slugify(""), "server");
        // 40 characters, no trailing separator after the cut.
        assert_eq!(slugify(&"x".repeat(60)).chars().count(), 40);
    }

    #[test]
    fn parent_dir_keeps_the_self_built_root_only() {
        assert_eq!(
            parent_dir(".data/mcp/weather/server.py").as_deref(),
            Some(".data/mcp/weather")
        );
        assert_eq!(parent_dir("server.py"), None);
        assert_eq!(parent_dir(""), None);
    }

    /// A first-cut or hand-edited roster entry used `\` separators. Splitting
    /// on `/` alone returned `None`, so the next `write` forked a second folder
    /// and `remove` left the original one behind.
    #[test]
    fn parent_dir_normalizes_legacy_separators() {
        assert_eq!(
            parent_dir(".data\\mcp\\weather\\server.py").as_deref(),
            Some(".data/mcp/weather")
        );
        assert_eq!(
            parent_dir("  .data/mcp/weather/server.py  ").as_deref(),
            Some(".data/mcp/weather")
        );
    }

    /// An absolute legacy `definition_path` maps back to the workspace-relative
    /// folder that already exists, so an in-place update never forks a copy.
    #[test]
    fn existing_rel_dir_maps_an_absolute_legacy_path_back_into_the_root() {
        let dir = std::env::temp_dir().join(format!("mcp-self-rel-{}", std::process::id()));
        let folder = dir.join(".data/mcp/weather");
        std::fs::create_dir_all(&folder).expect("create folder");
        let script = folder.join("server.py");
        std::fs::write(&script, "print('hi')\n").expect("write script");

        let absolute = script.display().to_string();
        assert_eq!(
            existing_rel_dir(&dir, &absolute).as_deref(),
            Some(".data/mcp/weather")
        );
        // Relative spellings work the same way.
        assert_eq!(
            existing_rel_dir(&dir, ".data/mcp/weather/server.py").as_deref(),
            Some(".data/mcp/weather")
        );
        // Anything outside the tool-owned directory is never a reuse target.
        assert_eq!(existing_rel_dir(&dir, "src/main.py"), None);
        assert_eq!(existing_rel_dir(&dir, ""), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_whitelist_never_returns_an_empty_command() {
        for runtime in ["python", "node", "shell"] {
            let spec = resolve_runtime(runtime).expect("known runtime");
            assert!(!spec.command.is_empty(), "{runtime} resolved to nothing");
            assert!(!spec.ext.is_empty());
        }
        assert!(resolve_runtime("ruby").is_err());
        assert!(resolve_runtime("").is_err());
    }

    #[test]
    fn clip_keeps_the_head_and_marks_the_cut() {
        assert_eq!(clip("abc", 10), "abc");
        assert_eq!(clip("abcdef", 3), "abc…");
    }

    #[test]
    fn a_fresh_directory_never_collides_with_an_existing_one() {
        let dir = std::env::temp_dir().join(format!("mcp-self-test-{}", std::process::id()));
        let first = unique_rel_dir(&dir, "Weather Tools");
        assert_eq!(first, ".data/mcp/weather-tools");
        std::fs::create_dir_all(dir.join(&first)).expect("create first dir");
        let second = unique_rel_dir(&dir, "Weather Tools");
        assert_ne!(first, second);
        assert_eq!(second, ".data/mcp/weather-tools-2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
