//! Executor for the built-in tools: argument parsing, the `run_cmd`
//! path and the dispatch table itself.

use super::*;

mod run_cmd;
mod run_shell;
mod skill_read;

/// Everything one tool call needs, so a handler takes one context instead of a
/// dozen positional parameters. Borrowed for the duration of the call.
#[derive(Clone, Copy)]
pub(crate) struct ToolContext<'a> {
    pub app: &'a AppHandle,
    pub workspace_dir: &'a Path,
    pub allowed_commands: &'a [String],
    pub mission_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub call_id: Option<&'a str>,
    pub active_skill_dirs: &'a [(String, PathBuf)],
    pub cancel: Option<&'a std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub working_dir: Option<&'a Path>,
}

/// Simple command-line splitter that handles single/double quotes and
/// backslash escapes before whitespace/quotes (so `rm\ -rf\ ~` parses as
/// `rm -rf ~` and is caught by the risk assessment). Backslashes before other
/// characters (e.g. `..\..` on Windows) are kept literal.
pub(crate) fn split_command_line(code: &str) -> Vec<String> {
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
    // `tool_call.id` of the call that is asking, so the inline allow/deny row
    // is created with the same id its progress events carry — otherwise the
    // parallel-round rework would leave a second, never settled row behind.
    call_id: Option<&str>,
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
    let _confirm_guard = state.confirm.tool_lock.lock().await;

    // Unique id ties the frontend's confirm/deny response back to THIS pending
    // request, so a stale response cannot approve a different command.
    let request_id = format!("confirm-{}", uuid::Uuid::new_v4());

    // Register the reply slot *before* emitting, so an immediate click cannot
    // find an empty slot and be dropped.
    let (tx, rx) = tokio::sync::oneshot::channel::<crate::ToolConfirmation>();
    {
        *state.confirm.sender.lock().unwrap() = Some((request_id.clone(), tx));
    }

    let _ = app.emit(
        crate::events::CONFIRM_REQUIRED,
        crate::events::ConfirmRequired {
            request_id: request_id.clone(),
            call_id: call_id.map(str::to_string),
            reason: reason.clone(),
            cmd_type: cmd_type.clone(),
            code: code.clone(),
            confirm_kind: confirm_kind.to_string(),
            requires_auth: requires_auth.to_string(),
        },
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
        let mut slot = state.confirm.sender.lock().unwrap();
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
pub(crate) fn machine_label() -> &'static str {
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
pub(crate) const RUN_SHELL_LANGUAGE: &str = "PowerShell (pwsh)";
#[cfg(not(windows))]
pub(crate) const RUN_SHELL_LANGUAGE: &str = "bash";

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
    // `tool_call.id` of the call being executed. Progress events carry it so
    // the UI can apply them to that call's row when several run in parallel;
    // `None` keeps the legacy "newest running row" behaviour.
    call_id: Option<&str>,
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
    let ctx = ToolContext {
        app,
        workspace_dir: &workspace_dir,
        allowed_commands,
        mission_id,
        session_id,
        call_id,
        active_skill_dirs,
        cancel,
        working_dir,
    };
    let tool_output = match name {
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
                        session_id,
                        call_id,
                        format!(
                            "*Timer set: {}*\n\n```\n{}\n```\n",
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
                        session_id,
                        call_id,
                        format!("*Timer cancelled: {}*\n", entry.label),
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
                call_id,
            )
            .await
        }
        // The `todos` tool is a single action-based entry point (like
        // `file_actions`). Legacy per-action names are routed to the same
        // handler so chats continued from older sessions keep working. The
        // `ask` action blocks on the user's answers, so the turn's cancel
        // flag is threaded through (Stop must end the wait).
        "todos"
        | "todo_add"
        | "todo_update_status"
        | "todo_list"
        | "todo_clear_completed"
        | "todo_archive" => {
            todos::handle_todos_tool(app, name, &args, session_id, mission_id, call_id, cancel)
                .await
        }
        "calculator" => calculator::handle(&args),
        "skill_read" => skill_read::handle(&ctx, &args).await,
        "run_cmd" => return run_cmd::handle(&ctx, &args).await,
        "run_shell" => return run_shell::handle(&ctx, &args).await,
        "file_actions" => {
            file_actions::handle_file_actions(
                app,
                &args,
                &workspace_dir,
                protected_skill_root,
                mission_id,
                session_id,
                call_id,
                config.self_evolution_mode,
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
                skill_admin::handle_skill_merge(
                    app,
                    &args,
                    &workspace_dir,
                    config,
                    mission_id,
                    session_id,
                    call_id,
                )
            }
        }
        "experience" => {
            // Global experience packages: only while self-evolution mode is
            // on, never mixed with skills / scripts / MCP servers.
            experience::handle(
                app,
                &args,
                &workspace_dir,
                config,
                mission_id,
                session_id,
                cancel,
            )
            .await
        }
        _ => format!("Unknown tool: {}", name),
    };

    truncate_tool_output(tool_output)
}
