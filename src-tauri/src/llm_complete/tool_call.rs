//! One tool call of the main agent's tool loop.
//!
//! Extracted from `run_chat_turn` so a round can execute several independent
//! calls at once: the loop builds one future per call with these arguments and
//! joins them, and the results are written back in `tool_calls` order.
//!
//! `use_skill` is deliberately NOT handled here. It mutates the round's own
//! state — the merged `allowed_commands` list, the active skill directories
//! and the queued skill-context messages — so the loop runs it as a barrier
//! and only starts a concurrent segment after it returned.

use super::*;

use crate::AppConfig;

/// One entry of the assistant response's `tool_calls`.
pub(crate) struct ToolCall<'a> {
    /// The model's `tool_call.id`. Unique inside the round and used by the UI
    /// to apply progress events to this call's row.
    pub id: &'a str,
    pub name: &'a str,
    /// Raw JSON arguments. Read by `tools::execute_tool`; routing decisions use
    /// `parsed` so they never depend on a second parse of a huge string.
    pub args: &'a str,
    pub parsed: &'a Value,
}

/// Executes one call and returns the `tool` message content.
///
/// The caller has already settled the failure guard and the turn budget; this
/// function only routes the call. It never mutates loop state, which is what
/// makes it safe to run several instances concurrently.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_tool_call(
    app: &AppHandle,
    state: &AppState,
    config: &AppConfig,
    session_id: &str,
    workspace_dir: &Path,
    call: ToolCall<'_>,
    skill_allowed_commands: &[String],
    skill_evolution_root: Option<&Path>,
    active_skill_dirs: &[(String, PathBuf)],
    mcp_tool_map: &std::collections::HashMap<String, (mcp::McpServer, String)>,
    enabled_agents: &[agents::SubAgent],
    subagents_enabled: bool,
    turn_cancelled: &Arc<AtomicBool>,
) -> String {
    let ToolCall {
        id,
        name,
        args,
        parsed,
    } = call;

    // ── Background task control (`list` / `wait` / `get` / `cancel`) ────────
    if name == BACKGROUND_TOOL_NAME {
        return handle_background_task_tool(app, state, session_id, parsed).await;
    }

    // ── Background routing ──────────────────────────────────────────────────
    // The call returns a placeholder immediately; its real output is injected
    // at the next request boundary.
    if config.background_tools && background_requested(parsed) && name != "call_subagent" {
        if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
            let mut mcp_args = parsed.clone();
            if let Some(object) = mcp_args.as_object_mut() {
                object.remove("run_in_background");
            }
            let label = format!(
                "MCP [{}] {} · {}",
                mcp_server.name,
                actual_tool_name,
                serde_json::to_string(&mcp_args)
                    .unwrap_or_default()
                    .chars()
                    .take(100)
                    .collect::<String>()
            );
            return match start_background_mcp_call(
                app,
                session_id,
                mcp_server.clone(),
                actual_tool_name.clone(),
                name,
                mcp_args,
                &label,
            ) {
                Ok(placeholder) => placeholder,
                Err(err) => format!("Error: {err}"),
            };
        }
        if background_supported(name, parsed) {
            let mut bg_args = parsed.clone();
            if let Some(object) = bg_args.as_object_mut() {
                // The real tool must never see the routing flag.
                object.remove("run_in_background");
            }
            let label = background_label(name, &bg_args);
            return match start_background_tool_call(
                app,
                session_id,
                name,
                bg_args,
                &label,
                workspace_dir.to_path_buf(),
                config,
                skill_allowed_commands,
                skill_evolution_root.map(|path| path.to_path_buf()),
                active_skill_dirs,
            ) {
                Ok(placeholder) => placeholder,
                Err(err) => format!("Error: {err}"),
            };
        }
        return format!(
            "Error: `run_in_background` is not supported by the tool '{name}'. \
             Use it only with `run_cmd`, `run_shell` (run_type \"direct\"), \
             `file_actions` action \"search\", MCP tools and `call_subagent`."
        );
    }

    // ── Sub-agent delegation ────────────────────────────────────────────────
    if name == "call_subagent" {
        return call_subagent(app, config, session_id, workspace_dir, parsed, enabled_agents, subagents_enabled, skill_evolution_root, turn_cancelled).await;
    }

    // ── MCP tool ────────────────────────────────────────────────────────────
    if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
        let mut args_json = parsed.clone();
        // The background routing flag is ours, never the server's.
        if let Some(object) = args_json.as_object_mut() {
            object.remove("run_in_background");
        }
        // MCP tools are NOT sandboxed — path arguments are forwarded verbatim
        // to the MCP server. The server itself decides what paths it can
        // access.
        log_event(
            &app.state::<AppState>(),
            "INFO",
            format!("Args for MCP tool '{}': {}", actual_tool_name, args_json),
        );
        let _ = tools::emit_tool_progress(
            app,
            None,
            Some(session_id),
            Some(id),
            format!(
                "🔌 *MCP [{}]: {}*\n\n{}",
                mcp_server.name,
                actual_tool_name,
                tool_args_block(&args_json)
            ),
        );
        let start_time = std::time::Instant::now();
        let mcp_result = mcp::invoke_mcp_tool(state, mcp_server, actual_tool_name, args_json.clone())
            .await;
        let duration_ms = start_time.elapsed().as_millis() as i64;
        return match &mcp_result {
            Ok(result) => {
                if let Ok(db) = state.db.lock() {
                    log_interaction(
                        &db,
                        session_id,
                        "mcp_response",
                        &mcp_server.name,
                        actual_tool_name,
                        serde_json::to_string(&args_json).unwrap_or_default(),
                        result.clone(),
                        None,
                        duration_ms,
                        None,
                    );
                }
                result.clone()
            }
            Err(e) => {
                if let Ok(db) = state.db.lock() {
                    log_interaction(
                        &db,
                        session_id,
                        "mcp_call",
                        &mcp_server.name,
                        actual_tool_name,
                        serde_json::to_string(&args_json).unwrap_or_default(),
                        String::new(),
                        Some(e.clone()),
                        duration_ms,
                        None,
                    );
                }
                format!("MCP tool error: {e}")
            }
        };
    }

    // ── Built-in tool ───────────────────────────────────────────────────────
    let start_time = std::time::Instant::now();
    // Mission-ledger tools live with the agent system now. The main chat passes
    // `mission_id: None`, so the handler answers with its "only inside a
    // sub-agent mission" error instead of the tool being unknown.
    let tool_result = if let Some(result) =
        crate::agents::mission_tools::handle(app, name, None, Some(session_id), parsed)
    {
        result
    } else {
        tools::execute_tool(
            app,
            name,
            args,
            workspace_dir.to_path_buf(),
            config,
            skill_allowed_commands,
            skill_evolution_root,
            None,
            Some(session_id),
            Some(id),
            active_skill_dirs,
            None,
            None,
        )
        .await
    };
    let duration_ms = start_time.elapsed().as_millis() as i64;

    // Log tool execution
    if let Ok(db) = state.db.lock() {
        let is_error = turn_guard::tool_output_is_error(&tool_result);
        log_interaction(
            &db,
            session_id,
            if is_error { "tool_error" } else { "tool_output" },
            "tool_executor",
            name,
            serde_json::to_string(&serde_json::from_str::<Value>(args).unwrap_or_default())
                .unwrap_or_default(),
            tool_result.clone(),
            if is_error {
                Some(tool_result.clone())
            } else {
                None
            },
            duration_ms,
            None,
        );
    }
    tool_result
}

/// `call_subagent`: one delegation, foreground or background.
#[allow(clippy::too_many_arguments)]
async fn call_subagent(
    app: &AppHandle,
    config: &AppConfig,
    session_id: &str,
    workspace_dir: &Path,
    args_json: &Value,
    enabled_agents: &[agents::SubAgent],
    subagents_enabled: bool,
    skill_evolution_root: Option<&Path>,
    turn_cancelled: &Arc<AtomicBool>,
) -> String {
    let agent_id = args_json["agent_id"].as_str().unwrap_or("");
    let task_desc = args_json["task_description"].as_str().unwrap_or("");
    let task_context = args_json["context"].as_str().unwrap_or("");

    if !subagents_enabled {
        return "Error: sub-agent delegation is disabled. Enable the sub-agent system (`use_agents`) in the Agents panel to use `call_subagent`.".to_string();
    }
    let Some(agent) = enabled_agents.iter().find(|a| a.id == agent_id) else {
        return format!("Error: Agent with ID '{}' not found or not enabled.", agent_id);
    };

    let task = agents::Task {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent_id.to_string(),
        description: task_desc.to_string(),
        context: task_context.to_string(),
        cwd: agents::normalize_task_cwd(args_json["cwd"].as_str()),
        images: agents::sanitize_task_images(
            &args_json["images"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|value| value.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        ),
        params: args_json["params"].as_object().cloned().unwrap_or_default(),
        dependencies: vec![],
    };

    // Deliverables are exchanged through
    // `<workspace>/.data/deliver/<session-id>`. A cloud agent cannot write the
    // workspace itself, but the app stores the media it generates (image/video)
    // in its task directory and reports the saved paths, so it gets a
    // directory like `llm` / `cli` agents do.
    let exchange = match agents::exchange::ExchangeArea::create(
        workspace_dir,
        session_id,
        std::slice::from_ref(&task.id),
    ) {
        Ok(area) => Some(area),
        Err(err) => {
            log_event(
                &app.state::<AppState>(),
                "WARN",
                format!("deliverable exchange area unavailable: {err}"),
            );
            None
        }
    };
    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
    let client = match build_stream_client() {
        Ok(client) => client,
        Err(err) => return format!("Error: {err}"),
    };

    if config.background_tools && background_requested(args_json) {
        let agent = agent.clone();
        let app_for_task = app.clone();
        let session_for_task = session_id.to_string();
        let config_for_task = config.clone();
        let skill_root_for_task = skill_evolution_root.map(|path| path.to_path_buf());
        let exchange_for_task = exchange.clone();
        let task_for_run = task.clone();
        let workspace_for_task = workspace_dir.to_path_buf();
        let label = format!(
            "{} · {}",
            agent.name,
            task_desc.chars().take(100).collect::<String>()
        );
        let preview: String = args_json.to_string().chars().take(300).collect();
        return start_background_task(
            app,
            session_id,
            "call_subagent",
            &label,
            &preview,
            move |cancel| async move {
                let res = agents::run_sub_agent(
                    &app_for_task,
                    &client,
                    &url,
                    &config_for_task,
                    &session_for_task,
                    &agent,
                    &task_for_run,
                    workspace_for_task,
                    exchange_for_task.as_ref(),
                    skill_root_for_task,
                    cancel,
                    None,
                )
                .await;
                format_subagent_delivery(&res, exchange_for_task.as_ref())
            },
        )
        .unwrap_or_else(|err| format!("Error: {err}"));
    }

    let res = agents::run_sub_agent(
        app,
        &client,
        &url,
        config,
        session_id,
        agent,
        &task,
        workspace_dir.to_path_buf(),
        exchange.as_ref(),
        skill_evolution_root.map(|path| path.to_path_buf()),
        turn_cancelled.clone(),
        None,
    )
    .await;

    format_subagent_delivery(&res, exchange.as_ref())
}
