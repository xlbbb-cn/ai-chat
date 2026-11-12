//! Executor for the built-in tools: argument parsing, the `run_cmd`
//! path and the dispatch table itself.

use super::*;

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
