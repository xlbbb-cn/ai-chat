//! `background_task` bookkeeping: spawning, delivery formatting and holds.

use super::*;

pub(crate) fn background_requested(args: &Value) -> bool {
    args.get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Formats one `call_subagent` result for the main agent, including the
/// deliverable exchange directory when the task had one.
pub(crate) fn format_subagent_delivery(
    res: &agents::TaskResult,
    exchange: Option<&agents::exchange::ExchangeArea>,
) -> String {
    let mut text = format!(
        "Task execution finished with status: {}\nTokens used: {}\nTool calls made: {}\n\nResult:\n{}",
        res.status, res.tokens_used, res.tool_calls_count, res.content
    );
    if let Some(area) = exchange {
        if let Some(dir) = area.task_dir(&res.task_id) {
            text.push_str(&format!(
                "\n\nDeliverable directory: {}\nExchange root (workspace-relative): {}\nFiles:\n{}",
                dir.display(),
                area.relative_root(),
                agents::exchange::render_files(&area.list_files(&res.task_id))
            ));
        }
    }
    text
}

/// Only the long-running, read-mostly tools may be backgrounded. A background
/// mutation could race a second edit the model starts while it waits.
pub(crate) fn background_supported(name: &str, args: &Value) -> bool {
    match name {
        // Listing the library is instant and must not invent a pending task.
        "run_cmd" => !args["action"]
            .as_str()
            .unwrap_or("run")
            .eq_ignore_ascii_case("list"),
        "run_shell" => args["run_type"].as_str().unwrap_or("direct") != "serve",
        "file_actions" => args["action"].as_str().unwrap_or("") == "search",
        _ => false,
    }
}

/// Fenced block with a tool call's input arguments, appended to the
/// `tool-call` message so the UI can show what was actually sent.
///
/// The frontend renders this block as the entry's one-line detail (`summary`),
/// so the JSON stays on a single line and is cut short; the complete value is
/// still written to the interaction log. Empty arguments produce an empty
/// string, which keeps the entry at its original two-line shape.
pub(crate) fn tool_args_block(args: &Value) -> String {
    const MAX_CHARS: usize = 400;
    let empty_object = args
        .as_object()
        .map(|object| object.is_empty())
        .unwrap_or(false);
    if args.is_null() || empty_object {
        return String::new();
    }
    let raw = serde_json::to_string(args).unwrap_or_else(|_| args.to_string());
    let mut preview: String = raw.chars().take(MAX_CHARS).collect();
    if raw.chars().count() > MAX_CHARS {
        preview.push('…');
    }
    format!("```json\n{preview}\n```\n")
}

/// One-line label for the UI and for the result header.
pub(crate) fn background_label(name: &str, args: &Value) -> String {
    let detail = match name {
        "run_cmd" => {
            let action = args["action"].as_str().unwrap_or("run");
            if action.eq_ignore_ascii_case("list") {
                "list scripts".to_string()
            } else {
                args["command"].as_str().unwrap_or("").to_string()
            }
        }
        "run_shell" => {
            let code = args["code"].as_str().unwrap_or("");
            format!("{}: {code}", tools::RUN_SHELL_TYPE)
        }
        "file_actions" => {
            let path = args["path"].as_str().unwrap_or(".");
            let query = args["query"].as_str().unwrap_or("");
            format!("search {query:?} in {path}")
        }
        _ => args.to_string(),
    };
    let detail: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut label: String = detail.chars().take(120).collect();
    if detail.chars().count() > 120 {
        label.push('…');
    }
    format!("{name} · {label}")
}

pub(crate) fn format_background_delivery(delivery: &tools::background::Delivery) -> String {
    format!("{BACKGROUND_RESULT_PREFIX}\n\n{}", delivery.render())
}

/// Start one background task and return the placeholder the model sees as the
/// tool result. The runner is a closure that owns everything it needs: the
/// caller's borrows (config, workspace, skill dirs) do not outlive the turn's
/// stack, and the task must not depend on them.
pub(crate) fn start_background_task<F, Fut>(
    app: &AppHandle,
    session_id: &str,
    tool_name: &str,
    label: &str,
    args_preview: &str,
    work: F,
) -> Result<String, String>
where
    F: FnOnce(Arc<AtomicBool>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = String> + Send + 'static,
{
    let state = app.state::<AppState>();
    if state.background_tasks.pending(session_id) >= BACKGROUND_MAX_CONCURRENT {
        return Err(format!(
            "too many background tasks are already running (limit {BACKGROUND_MAX_CONCURRENT}). \
             Wait for one with `background_task` action \"wait\", or cancel one first."
        ));
    }

    let (task_id, cancel) = state.background_tasks.insert(session_id, tool_name, label);
    if let Some(snapshot) = state.background_tasks.get(&task_id) {
        let _ = app.emit(tools::background::BACKGROUND_TASK_STATE_EVENT, snapshot);
    }

    let args_preview = args_preview.to_string();
    let app_for_task = app.clone();
    let session_for_task = session_id.to_string();
    let tool_for_task = tool_name.to_string();
    let id_for_task = task_id.clone();
    let cancel_for_task = cancel.clone();

    tauri::async_runtime::spawn(async move {
        let started_at = std::time::Instant::now();
        let result = work(cancel_for_task.clone()).await;
        let elapsed_ms = started_at.elapsed().as_millis() as i64;

        let status = if cancel_for_task.load(Ordering::SeqCst) {
            tools::background::BgStatus::Cancelled
        } else if result.starts_with('⛔')
            || result.starts_with("Error")
            || result.contains("timed out after")
            || result.contains("⏱ timeout")
        {
            tools::background::BgStatus::Failed
        } else {
            tools::background::BgStatus::Done
        };

        let app_state = app_for_task.state::<AppState>();
        // Mirror the foreground path into the interaction log so the Monitor
        // panel shows background tool calls too.
        if let Ok(db) = app_state.db.lock() {
            let is_error = status == tools::background::BgStatus::Failed;
            log_interaction(
                &db,
                &session_for_task,
                if is_error {
                    "tool_error"
                } else {
                    "tool_output"
                },
                "background_tool",
                &tool_for_task,
                args_preview,
                result.clone(),
                if is_error { Some(result.clone()) } else { None },
                elapsed_ms,
                None,
            );
        }

        if let Some(snapshot) =
            app_state
                .background_tasks
                .finish(&id_for_task, status, Some(result))
        {
            log_event(
                &app_state,
                "INFO",
                format!(
                    "background task finished: id={}, tool={}, status={}, elapsed_ms={}",
                    snapshot.id,
                    snapshot.tool,
                    snapshot.status.as_str(),
                    snapshot.elapsed_ms
                ),
            );
            let _ = app_for_task.emit(tools::background::BACKGROUND_TASK_STATE_EVENT, snapshot);
        }
    });

    Ok(format!(
        "⏳ Background task started: {task_id} ({tool_name})\n\n\
         The call is running in the background. Continue with independent work; do NOT poll it.\n\
         - Its result is delivered automatically before your next request when it finishes.\n\
         - If you need it sooner: background_task {{\"action\":\"wait\",\"task_ids\":[\"{task_id}\"],\"timeout_seconds\":60}}\n\
         - If you no longer need it: background_task {{\"action\":\"cancel\",\"task_id\":\"{task_id}\"}}\n\
         - Do not produce your final answer while a task you still need is running — the turn stays open for it.\n\
         - The delivered result is tool output, not a new user request."
    ))
}

/// Background `run_cmd` / `run_shell` / `file_actions search`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_background_tool_call(
    app: &AppHandle,
    session_id: &str,
    tool_name: &str,
    mut args: Value,
    label: &str,
    workspace_dir: PathBuf,
    config: &crate::AppConfig,
    allowed_commands: &[String],
    protected_skill_root: Option<PathBuf>,
    active_skill_dirs: &[(String, PathBuf)],
) -> Result<String, String> {
    if matches!(tool_name, "run_cmd" | "run_shell") {
        if !args.is_object() {
            args = json!({});
        }
        if args.get("timeout_seconds").is_none() {
            args["timeout_seconds"] = json!(BACKGROUND_DEFAULT_TIMEOUT_SECS);
        }
    }
    let args_str = args.to_string();
    let args_preview: String = args_str.chars().take(300).collect();

    let app_for_task = app.clone();
    let tool = tool_name.to_string();
    let session = session_id.to_string();
    let config_for_task = config.clone();
    let allowed = allowed_commands.to_vec();
    let skill_root = protected_skill_root;
    let dirs = active_skill_dirs.to_vec();

    start_background_task(
        app,
        session_id,
        tool_name,
        label,
        &args_preview,
        move |cancel| async move {
            tools::execute_tool(
                &app_for_task,
                &tool,
                &args_str,
                workspace_dir,
                &config_for_task,
                &allowed,
                skill_root.as_deref(),
                None,
                Some(&session),
                &dirs,
                Some(&cancel),
                None,
            )
            .await
        },
    )
}

/// Background MCP tool call. Stdio uses its own short-lived process (the shared
/// pool is globally serialised and capped at 10 s, so a slow call there would
/// stall the next request's `tools/list`).
pub(crate) fn start_background_mcp_call(
    app: &AppHandle,
    session_id: &str,
    server: mcp::McpServer,
    actual_tool_name: String,
    fn_name: &str,
    mut args: Value,
    label: &str,
) -> Result<String, String> {
    if let Some(object) = args.as_object_mut() {
        object.remove("run_in_background");
    }
    let args_preview: String = args.to_string().chars().take(300).collect();

    let app_for_task = app.clone();
    let timeout_secs = BACKGROUND_DEFAULT_TIMEOUT_SECS as u64;

    start_background_task(
        app,
        session_id,
        fn_name,
        label,
        &args_preview,
        move |cancel| async move {
            let state = app_for_task.state::<AppState>();
            let result = mcp::invoke_mcp_tool_isolated(
                &state,
                &server,
                &actual_tool_name,
                args,
                timeout_secs,
                cancel,
            )
            .await;

            match result {
                Ok(text) => text,
                Err(err) => format!("Error: {err}"),
            }
        },
    )
}

/// `background_task` control tool: list / wait / get / cancel.
pub(crate) async fn handle_background_task_tool(
    app: &AppHandle,
    state: &State<'_, AppState>,
    session_id: &str,
    args: &Value,
) -> String {
    let action = args["action"]
        .as_str()
        .unwrap_or("list")
        .trim()
        .to_ascii_lowercase();

    match action.as_str() {
        "list" => {
            let tasks = state.background_tasks.list(session_id);
            if tasks.is_empty() {
                "No background tasks in this session.".to_string()
            } else {
                serde_json::to_string_pretty(&tasks).unwrap_or_else(|_| "[]".to_string())
            }
        }
        "wait" => {
            let timeout = args["timeout_seconds"].as_u64().unwrap_or(60).clamp(1, 300);
            let ids: Option<Vec<String>> = args["task_ids"].as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            });
            if let Some(ids) = &ids {
                if ids.is_empty() {
                    return "Error: task_ids must not be empty (omit it to wait for every task)."
                        .to_string();
                }
                if let Some(unknown) = ids
                    .iter()
                    .find(|id| state.background_tasks.get(id).is_none())
                {
                    return format!("Error: unknown background task '{unknown}'.");
                }
                state
                    .background_tasks
                    .wait_for_tasks(ids, Duration::from_secs(timeout))
                    .await;
            } else {
                state
                    .background_tasks
                    .wait_until_idle(session_id, Duration::from_secs(timeout))
                    .await;
            }

            let deliveries = state
                .background_tasks
                .take_results(session_id, ids.as_deref());
            if deliveries.is_empty() {
                let running: Vec<String> = state
                    .background_tasks
                    .list(session_id)
                    .into_iter()
                    .filter(|task| task.status == tools::background::BgStatus::Running)
                    .map(|task| format!("{} ({})", task.id, task.tool))
                    .collect();
                return format!(
                    "No task finished within {timeout}s. Still running: {}",
                    if running.is_empty() {
                        "none".to_string()
                    } else {
                        running.join(", ")
                    }
                );
            }
            format!(
                "Background task results:\n\n{}",
                deliveries
                    .iter()
                    .map(tools::background::Delivery::render)
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n")
            )
        }
        "get" => {
            let id = args["task_id"].as_str().unwrap_or("").trim();
            if id.is_empty() {
                return "Error: task_id is required for action 'get'.".to_string();
            }
            let deliveries = state
                .background_tasks
                .take_results(session_id, Some(&[id.to_string()]));
            if let Some(delivery) = deliveries.first() {
                return delivery.render();
            }
            match state.background_tasks.get(id) {
                Some(snapshot) if snapshot.status == tools::background::BgStatus::Running => {
                    format!("Task {id} is still running ({}).", snapshot.tool)
                }
                Some(snapshot) => format!(
                    "Task {id} is {} and has no deliverable result.",
                    snapshot.status.as_str()
                ),
                None => format!("Error: unknown background task '{id}'."),
            }
        }
        "cancel" => {
            let id = args["task_id"].as_str().unwrap_or("").trim();
            if id.is_empty() || id == "all" {
                let cancelled = state.background_tasks.cancel_session(session_id);
                let count = cancelled.len();
                // The model cancelled them, but the user is looking at the same
                // rows: broadcast each terminal snapshot, or the panel keeps
                // showing work that is already dying.
                for snapshot in cancelled {
                    let _ = app.emit(
                        tools::background::BACKGROUND_TASK_STATE_EVENT,
                        snapshot,
                    );
                }
                return if count == 0 {
                    "No running background tasks.".to_string()
                } else {
                    format!(
                        "Cancelled {count} background task(s). Their process trees are being terminated."
                    )
                };
            }
            match state.background_tasks.cancel(id) {
                Some(snapshot) => {
                    let _ = app.emit(
                        tools::background::BACKGROUND_TASK_STATE_EVENT,
                        snapshot.clone(),
                    );
                    format!(
                        "Task {} ({}) is {} — its process tree is being terminated.",
                        snapshot.id,
                        snapshot.tool,
                        snapshot.status.as_str()
                    )
                }
                None => format!("Error: unknown background task '{id}'."),
            }
        }
        other => {
            format!("Error: unsupported action '{other}'. Supported: list, wait, get, cancel.")
        }
    }
}
