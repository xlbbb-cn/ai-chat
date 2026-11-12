//! `ask_main_agent` — the helper loop where a sub-agent asks the main agent a
//! question, plus the mission hand-off it produces.

use super::*;

// ─── Main-agent help (`ask_main_agent`) ──────────────────────────────────────
//
// A sub-agent cannot ask the *live* main chat turn for help: `call_subagent`
// awaits this whole function, so the main agent's loop is blocked. Instead the
// request is answered by a self-contained helper run that has the main model,
// the session history, the enabled skills, the main tool set and the enabled
// MCP servers. From the sub-agent's point of view it is still an ordinary tool
// call: the material comes back and the autonomous loop continues.

pub(crate) struct MainHelperOutcome {
    content: String,
    tokens: u32,
    tool_calls: u32,
}

/// Main-agent tools for the helper: the configured main selection minus
/// `timer` (a chat-session tool that would resume a conversation later).
pub(crate) fn main_helper_tool_ids(config: &AppConfig) -> Vec<String> {
    config
        .effective_tools()
        .into_iter()
        .filter(|tool| tool.as_str() != "timer")
        .collect()
}

/// Drop the self-evolution `mcp_self` tool from a sub-agent / helper list.
///
/// Building an MCP server is a main-agent capability: parallel tasks would race
/// on `mcp_servers.json`, an `AgentMcpSession` only loads servers at task start
/// (so a task's own creation would never reach its tool list), and registering
/// one asks the user for confirmation — a blocking dialog inside a mission.
/// The main agent owns the registry; sub-agents consume what it enabled.
pub(crate) fn drop_mcp_self(tools_list: &mut Vec<Value>) {
    let name = tools::mcp_self::TOOL_NAME;
    tools_list.retain(|tool| tool.pointer("/function/name").and_then(Value::as_str) != Some(name));
}

/// Compact transcript of the real conversation for the helper. Newest entries
/// win when the budget is exhausted; tool-call rounds are skipped because only
/// the user/assistant text is useful as context.
pub(crate) fn build_main_helper_history(records: &[crate::db::HistoryRecord]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;

    for record in records.iter().rev() {
        if lines.len() >= MAIN_HELPER_HISTORY_MESSAGES || used >= MAIN_HELPER_HISTORY_BUDGET_BYTES {
            break;
        }
        let text = record.content.trim();
        if text.is_empty() {
            continue;
        }
        let text = preview_text(text, MAIN_HELPER_HISTORY_MESSAGE_BYTES);
        let line = format!("[{}]: {}", record.role, text);
        used += line.len();
        lines.push(line);
    }

    lines.reverse();
    lines.join("\n")
}

/// Keeps a single tool result within the helper's context budget.
pub(crate) fn cap_helper_tool_result(result: String) -> String {
    if result.len() <= MAIN_HELPER_TOOL_RESULT_MAX_BYTES {
        return result;
    }
    let mut clipped = preview_text(&result, MAIN_HELPER_TOOL_RESULT_MAX_BYTES);
    clipped.push_str("\n\n[truncated: tool output exceeded the helper limit]");
    clipped
}

/// Appends one exchange to the mission's visible help log. Best effort: the
/// helper must still answer when the database write fails.
pub(crate) fn append_mission_help_note(app: &AppHandle, mission_id: &str, note: &str) {
    let state = app.state::<AppState>();
    let Ok(db) = state.db.lock() else {
        return;
    };
    let stamp = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{stamp}] {note}");
    let _ = db.execute(
        "UPDATE agent_missions SET help_notes = \
         CASE WHEN help_notes = '' THEN ?2 ELSE help_notes || char(10) || ?2 END, \
         updated_at = CURRENT_TIMESTAMP WHERE mission_id = ?1",
        rusqlite::params![mission_id, line],
    );
}

/// Loads every enabled MCP server into `session` and returns the OpenAI tool
/// definitions plus the name → (server, actual tool) map. Shared by a task's own
/// `enable_mcp` tools and its main-agent helper, so both reuse one process pool.
pub(crate) async fn load_enabled_mcp_tools(
    app: &AppHandle,
    session: &mut mcp::AgentMcpSession,
    owner: &str,
) -> (
    Vec<Value>,
    std::collections::HashMap<String, (mcp::McpServer, String)>,
) {
    let state = app.state::<AppState>();
    let mut tools_out: Vec<Value> = Vec::new();
    let mut tool_map: std::collections::HashMap<String, (mcp::McpServer, String)> =
        std::collections::HashMap::new();

    // Merged roster: panel servers plus the assistant's own (`.data/mcp.conf`).
    let enabled_servers: Vec<mcp::McpServer> = mcp::load_effective_servers(&state)
        .into_iter()
        .filter(|server| server.enabled)
        .collect();

    for server in enabled_servers {
        match mcp::get_server_tools_for_agent(&state, session, &server, owner).await {
            Ok(server_tools) => {
                for tool in server_tools {
                    if let Some((fn_name, actual_name, openai_tool)) =
                        mcp::adapt_tool_for_llm(&server, &tool, false)
                    {
                        tools_out.push(openai_tool);
                        tool_map.insert(fn_name, (server.clone(), actual_name));
                    }
                }
            }
            Err(err) => {
                if let Ok(logger) = state.logger.lock() {
                    logger.log(
                        "ERROR",
                        &format!(
                            "MCP server '{}' tools/list failed for {owner}: {err}",
                            server.name
                        ),
                    );
                }
            }
        }
    }

    // Prompt-cache stability: the tools payload opens the request prefix, so
    // it must not depend on the `tools/list` enumeration order.
    mcp::sort_adapted_tools(&mut tools_out);

    (tools_out, tool_map)
}

/// Answers one `ask_main_agent` request, bounded by [`MAIN_HELPER_TIMEOUT_SECS`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_main_agent_helper(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agent: &SubAgent,
    task: &Task,
    session_id: &str,
    workspace_dir: &Path,
    task_cwd: &Path,
    skill_access_root: Option<&Path>,
    request: &str,
    context: &str,
    cancelled: &AtomicBool,
    mcp_session: &mut mcp::AgentMcpSession,
) -> Result<MainHelperOutcome, String> {
    match tokio::time::timeout(
        Duration::from_secs(MAIN_HELPER_TIMEOUT_SECS),
        run_main_agent_helper_inner(
            app,
            client,
            url,
            config,
            agent,
            task,
            session_id,
            workspace_dir,
            task_cwd,
            skill_access_root,
            request,
            context,
            cancelled,
            mcp_session,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "main-agent help timed out after {} seconds",
            MAIN_HELPER_TIMEOUT_SECS
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_main_agent_helper_inner(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    agent: &SubAgent,
    task: &Task,
    session_id: &str,
    workspace_dir: &Path,
    task_cwd: &Path,
    skill_access_root: Option<&Path>,
    request: &str,
    context: &str,
    cancelled: &AtomicBool,
    mcp_session: &mut mcp::AgentMcpSession,
) -> Result<MainHelperOutcome, String> {
    // Tool progress raised by the helper belongs to the sub-agent's task row,
    // not to the main chat: pass the mission id down to the tool executor.
    let mission_id = mission_id_for_task(task);
    // ── Conversation history (locks the DB, so drop it before any await) ─────
    let history = {
        let state = app.state::<AppState>();
        let db = state
            .db
            .lock()
            .map_err(|_| "database lock poisoned".to_string())?;
        crate::db::query_recent_session_messages(&db, session_id, MAIN_HELPER_HISTORY_MESSAGES * 2)
            .map_err(|e| e.to_string())?
    };
    let transcript = build_main_helper_history(&history);

    // ── Enabled skills (main capability) ─────────────────────────────────────
    let skills = crate::skills::enabled_skills(workspace_dir);
    let mut skills_info = String::new();
    for skill in &skills {
        skills_info.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }

    // ── Tools: main selection + skills + enabled MCP ─────────────────────────
    let tool_ids = main_helper_tool_ids(config);
    let mut tools_list = tools::get_all_tools(&tool_ids, config.self_evolution_mode, false);
    drop_mcp_self(&mut tools_list);
    let has_skills = !skills.is_empty() || config.self_evolution_mode;
    if has_skills {
        tools_list.push(tools::get_use_skill_tool());
        tools_list.push(tools::get_skill_read_tool());
    }
    let (mcp_tools, mcp_tool_map) = load_enabled_mcp_tools(
        app,
        mcp_session,
        &format!("agent:{}:main-helper", agent.name),
    )
    .await;
    tools_list.extend(mcp_tools);

    // ── Prompt ───────────────────────────────────────────────────────────────
    let os_info = os_info::get();
    let base_prompt = config.system_message.trim();
    let base_block = if base_prompt.is_empty() {
        String::new()
    } else {
        format!("{base_prompt}\n\n")
    };
    let skills_block = if skills_info.is_empty() {
        String::new()
    } else {
        format!("Enabled skills (call `use_skill` to load one):\n{skills_info}\n")
    };
    let system = format!(
        "{base}You are the MAIN assistant of this app, temporarily helping the sub-agent \"{agent_name}\" \
         (id {agent_id}) with its autonomous mission. The sub-agent cannot see this conversation.\n\
         Your only job is to collect and return material for its request: concrete findings, exact file \
         paths, excerpts, command output or links. Do not write the final user-facing answer and do not \
         ask the user questions. If the material cannot be obtained, state exactly what is missing.\n\
         User's machine: {os} {os_version}\n\
         Sub-agent working directory: {cwd} (`run_cmd` / `run_shell` start here)\n\
         Workspace root: {workspace_root} (`file_actions` paths are relative to it)\n\n\
         {skills}\
         Main conversation (for context):\n{transcript}",
        base = base_block,
        agent_name = agent.name,
        agent_id = agent.id,
        os = os_info.os_type(),
        os_version = os_info.version(),
        cwd = task_cwd.display(),
        workspace_root = workspace_dir.display(),
        skills = skills_block,
        transcript = if transcript.is_empty() {
            "(no prior conversation)"
        } else {
            transcript.as_str()
        },
    );

    let mut messages: Vec<Value> = vec![
        json!({ "role": "system", "content": system }),
        json!({
            "role": "user",
            "content": format!(
                "Sub-agent \"{}\" is executing this task:\n{}\n\nIts request:\n{}\n\nWhy it needs it:\n{}",
                agent.name,
                task.description.trim(),
                request.trim(),
                if context.trim().is_empty() { "(not specified)" } else { context.trim() },
            ),
        }),
    ];

    let model = config.model.trim();
    if model.is_empty() {
        return Err("no main model is configured".to_string());
    }
    let headers = upstream_request_headers(config, session_id);
    let max_complete_tokens = config
        .model_settings
        .max_complete_tokens
        .filter(|value| *value > 0)
        .unwrap_or(LLM_DEFAULT_MAX_COMLETE_TOKENS);

    let mut loaded_skills: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut active_skill_dirs: Vec<(String, PathBuf)> = Vec::new();
    let mut allowed_commands: Vec<String> = Vec::new();
    let mut tokens_used: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mut last_content = String::new();
    // Same session-scoped failure guard as the chat turn and the sub-agents.
    let failure_limit = config
        .tool_failure_limit
        .min(crate::turn_guard::MAX_TOOL_FAILURE_LIMIT);
    // Tools the session gave up on in an earlier turn are dropped once, right
    // here: `tools` is part of the prompt prefix, so it must stay identical
    // across the rounds of one run (`turn_guard`).
    let disabled_tools = app
        .state::<AppState>()
        .tool_guard
        .disabled_tools(session_id);
    let req_tools: Vec<Value> = tools_list
        .iter()
        .filter(|tool| {
            tool["function"]["name"]
                .as_str()
                .map(|name| !disabled_tools.contains(name))
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    for _ in 0..MAIN_HELPER_MAX_ITERATIONS {
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled by the user".to_string());
        }

        sanitize_tool_pairs(&mut messages);
        let mut req_body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        apply_completion_token_limit(
            &mut req_body,
            Some(max_complete_tokens),
            Some(LLM_DEFAULT_MAX_TOKENS),
        );
        if let Some(effort) = resolve_reasoning_effort(config, model) {
            req_body["reasoning_effort"] = json!(effort);
        }
        if !req_tools.is_empty() {
            req_body["tools"] = json!(req_tools);
            req_body["tool_choice"] = json!("auto");
        }

        let stream = stream_llm_request(
            app,
            client,
            url,
            &config.api_key,
            req_body,
            cancelled,
            StreamOptions {
                token_event: "agent-help-token",
                task_id: Some(&task.id),
                reasoning_event: None,
                emit_usage: false,
                emit_tokens: true,
                // The helper runs inside the sub-agent's turn: an upstream
                // failure must surface on the task row, never as a terminal
                // `chat-error` that closes the user's chat turn.
                emit_errors: false,
                usage_max_tokens: None,
                ds_format: config.ds_format_for(model),
                headers: &headers,
            },
        )
        .await?;

        tokens_used = tokens_used.saturating_add(stream.prompt_tokens + stream.completion_tokens);
        let content = stream.content;
        let reasoning_content = stream.reasoning_content;
        let tool_calls = stream.tool_calls;

        if tool_calls.is_empty() {
            let content = content.trim().to_string();
            let content = if content.is_empty() {
                "(the main agent returned no material)".to_string()
            } else if content.chars().count() > MAIN_HELPER_ANSWER_MAX_CHARS {
                let clipped: String = content.chars().take(MAIN_HELPER_ANSWER_MAX_CHARS).collect();
                format!("{clipped}\n\n[truncated: the material exceeded the size limit]")
            } else {
                content
            };
            return Ok(MainHelperOutcome {
                content,
                tokens: tokens_used,
                tool_calls: tool_calls_count,
            });
        }

        if !content.trim().is_empty() {
            last_content = content.clone();
        }
        tool_calls_count = tool_calls_count.saturating_add(tool_calls.len() as u32);

        let assistant_tcs: Vec<Value> = tool_calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args },
                })
            })
            .collect();
        let assistant_content = if content.trim().is_empty() {
            Value::Null
        } else {
            Value::String(content)
        };
        let mut assistant_msg = json!({
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": assistant_tcs,
        });
        if !reasoning_content.is_empty() {
            assistant_msg["reasoning_content"] = json!(reasoning_content);
        }
        messages.push(assistant_msg);

        // Skill bodies are queued and appended AFTER all tool results of this
        // round: an `assistant` message carrying tool_calls must stay adjacent
        // to its `tool` results, or strict providers reject the next request.
        let mut pending_skill_context: Vec<Value> = Vec::new();
        for (id, name, args) in &tool_calls {
            let tool_started = std::time::Instant::now();
            emit_agent_tool_state(
                app,
                &task.id,
                "start",
                name,
                Some(preview_text(args, 200)),
                None,
                None,
            );
            // A tool disabled for this session never runs again; the call is
            // still answered so the request stays protocol-valid.
            let blocked = app
                .state::<AppState>()
                .tool_guard
                .blocked_notice(session_id, name, failure_limit);
            let ran = blocked.is_none();
            let mut result = if let Some(notice) = blocked {
                notice
            } else if name == "use_skill" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let skill_name = args_json["skill_name"].as_str().unwrap_or("");
                let skills_dir = crate::workspace::skills_root(workspace_dir);
                match crate::skills::resolve_skill_by_name(&skills_dir, skill_name) {
                    Ok(resolved) => {
                        let skill = resolved.skill;
                        if loaded_skills.insert(skill.name.clone()) {
                            for command in &skill.allowed_commands {
                                if !allowed_commands.iter().any(|existing| existing == command) {
                                    allowed_commands.push(command.clone());
                                }
                            }
                            if !active_skill_dirs
                                .iter()
                                .any(|(name, _)| name == &skill.name)
                            {
                                active_skill_dirs
                                    .push((skill.name.clone(), resolved.skill_dir.clone()));
                            }
                            let cmd_constraint = if skill.allowed_commands.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    "\n[Allowed commands for this skill: {}]\n",
                                    skill.allowed_commands.join(", ")
                                )
                            };
                            pending_skill_context.push(json!({
                                "role": "user",
                                "content": format!(
                                    "INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL (not a user request):\n\
                                     --- Skill: {} ---\n[Skill root: {}]\n{}{}",
                                    skill.name,
                                    resolved.skill_dir.display(),
                                    cmd_constraint,
                                    skill.system_prompt,
                                ),
                            }));
                            format!("Skill '{}' loaded. Follow its instructions.", skill.name)
                        } else {
                            format!("Skill '{}' already loaded.", skill.name)
                        }
                    }
                    Err(err) => format!("Error: {err}"),
                }
            } else if let Some((server, actual_name)) = mcp_tool_map.get(name) {
                let state = app.state::<AppState>();
                let arguments: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                let owner = format!("agent:{}:main-helper", agent.name);
                match mcp::invoke_mcp_tool_for_agent(
                    &state,
                    mcp_session,
                    server,
                    actual_name,
                    arguments,
                    &owner,
                )
                .await
                {
                    Ok(text) => format!("MCP [{}] {} result:\n{}", server.name, actual_name, text),
                    Err(err) => format!("MCP tool error: {err}"),
                }
            } else {
                tools::execute_tool(
                    app,
                    name,
                    args,
                    workspace_dir.to_path_buf(),
                    config,
                    &allowed_commands,
                    skill_access_root,
                    Some(mission_id),
                    Some(session_id),
                    &active_skill_dirs,
                    None,
                    Some(task_cwd),
                )
                .await
            };
            let failed = crate::turn_guard::tool_output_is_error(&result);
            if ran {
                if let Some(trip) = app.state::<AppState>().tool_guard.record(
                    session_id,
                    name,
                    failed,
                    failure_limit,
                ) {
                    result.push_str("\n\n");
                    result.push_str(&crate::turn_guard::disabled_notice(
                        name,
                        trip.failures,
                        failure_limit,
                        true,
                    ));
                    let _ = app.emit(
                        "chat-notice",
                        json!({
                            "kind": "tool_disabled",
                            "tool": name,
                            "failures": trip.failures,
                            "limit": failure_limit,
                        }),
                    );
                }
            }
            emit_agent_tool_state(
                app,
                &task.id,
                "done",
                name,
                Some(preview_text(&result, 160)),
                Some(tool_started.elapsed().as_millis()),
                Some(!failed),
            );

            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": cap_helper_tool_result(result),
            }));
        }
        messages.extend(pending_skill_context);
    }

    let fallback = if last_content.trim().is_empty() {
        format!(
            "Main-agent helper reached its {}-round tool limit without a final answer. Tighten the request or collect the material directly.",
            MAIN_HELPER_MAX_ITERATIONS
        )
    } else {
        format!(
            "Main-agent helper reached its {}-round tool limit. Last partial material:\n{}",
            MAIN_HELPER_MAX_ITERATIONS,
            preview_text(&last_content, 4_000)
        )
    };
    Ok(MainHelperOutcome {
        content: fallback,
        tokens: tokens_used,
        tool_calls: tool_calls_count,
    })
}

/// Output-token cap for one `llm` sub-agent request.
///
/// `max_complete_tokens` is the current field; `max_tokens` is the pre-2.8 name
/// and is only read as a fallback for configurations that were not migrated
/// yet. `0` means "unset" for either field.
pub(crate) fn resolve_completion_limit(agent: &SubAgent) -> u32 {
    agent
        .max_complete_tokens
        .filter(|limit| *limit > 0)
        .or_else(|| agent.max_tokens.filter(|limit| *limit > 0))
        .unwrap_or(LLM_DEFAULT_MAX_COMLETE_TOKENS)
}

pub(crate) async fn run_llm_agent_body(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agent: &SubAgent,
    task: &Task,
    workspace_dir: PathBuf,
    task_cwd: PathBuf,
    exchange: Option<&exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: &AtomicBool,
    mcp_session: &mut mcp::AgentMcpSession,
    mbus: Option<&mbus::MissionBus>,
) -> TaskResult {
    let _ = app.emit(
        "agent-task-start",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "description": task.description,
            "cwd": task.cwd,
        }),
    );

    let model = agent.model.as_deref().unwrap_or(&config.model);
    // Same configured conversation-id header and custom headers as the main
    // chat: a sub-agent is still part of the same gateway conversation.
    let headers = upstream_request_headers(config, session_id);
    let max_complete_tokens = resolve_completion_limit(agent);
    // `timer` is a chat-session tool (it resumes the conversation later), so it
    // is never handed to a sub-agent even if an agent config lists it.
    let mut agent_tool_ids: Vec<String> = agent
        .allowed_tools
        .iter()
        .filter(|tool| tool.as_str() != "timer")
        .cloned()
        .collect();
    // Self-evolution mode owns `file_actions` and `memory`: a sub-agent that
    // may evolve skills must also be able to read and write them, and to
    // remember what it learned.
    if config.self_evolution_mode {
        for required in crate::SELF_EVOLUTION_REQUIRED_TOOLS {
            if !agent_tool_ids.iter().any(|tool| tool == required) {
                agent_tool_ids.push(required.to_string());
            }
        }
    }
    let mut tools_list = tools::get_all_tools(&agent_tool_ids, config.self_evolution_mode, false);
    drop_mcp_self(&mut tools_list);
    tools_list.extend(tools::get_agent_task_tools());
    // The mission bus is available only inside a planner-dispatched batch.
    // `call_subagent` (a direct main-agent delegation) has no bus, so the tool
    // is neither advertised nor callable there.
    if mbus.is_some() {
        tools_list.push(mbus::tool_definition(false));
    }
    // Main-agent assistance: only `llm` sub-agents run an in-app loop, so only
    // they can ask for it. The switch is global (Settings → Runtime & Debug).
    if config.sub_agent_main_help {
        tools_list.push(tools::get_main_agent_help_tool());
    }

    // ── MCP tools (`enable_mcp`) ─────────────────────────────────────────────
    // The agent gets the same enabled MCP servers as the main agent, but its
    // stdio processes live in this task's private session — never the shared
    // `AppState` pool — and the wrapper kills them when the task ends.
    let (mcp_tools, mcp_tool_map) = if agent.enable_mcp {
        load_enabled_mcp_tools(app, mcp_session, &format!("agent:{}", agent.name)).await
    } else {
        (Vec::new(), std::collections::HashMap::new())
    };
    tools_list.extend(mcp_tools);
    // `cancelled` is the caller's flag — the turn's own cancel flag when the
    // agent runs inside a chat turn, so pressing Stop in the chat tears the
    // autonomous loop down as well. It is never a fresh local flag: a detached
    // flag can never become true, and Stop would be ignored.
    let mut total_tokens: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mut main_help_calls: u32 = 0;
    let mission_id = mission_id_for_task(task).to_string();
    // Context budget: the per-model window from config (auto-detected from the
    // provider's `/models` response, the metadata catalogue, or entered for the
    // model in the Agents panel), otherwise a conservative default. Output-token
    // caps are a different quantity and must never shrink the context budget.
    let context_budget = crate::llm_complete::resolve_context_window(
        config,
        model,
        AGENT_DEFAULT_CONTEXT_WINDOW as u32,
    ) as usize;

    let mut self_evolution_context =
        crate::skills::self_evolution_prompt(skill_access_root.as_deref());

    // Verified facts are not preloaded here either: the sub-agent searches the
    // `evolution` memory scope on demand (see the self-evolution text), which
    // keeps every task's first request small.

    // The workspace script library is shared with the main chat. Only a
    // sub-agent that can actually run commands sees the listing.
    if agent_tool_ids.iter().any(|tool| tool == "run_cmd") {
        let scripts_context =
            crate::tools::scripts::prompt_block(&workspace_dir, config.self_evolution_mode);
        if !scripts_context.is_empty() {
            if !self_evolution_context.is_empty() {
                self_evolution_context.push_str("\n\n");
            }
            self_evolution_context.push_str(&scripts_context);
        }
    }

    let task_system = format!(
        "Execute this mission.\nPrimary task: {}\nContext: {}\nTreat the external mission snapshot as authoritative and keep the explicit task list updated through tools.",
        task.description, task.context
    );

    {
        let state = app.state::<AppState>();
        let db = match state.db.lock() {
            Ok(db) => db,
            Err(err) => {
                let error = format!("Failed to lock database for mission initialization: {err}");
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": error }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: error,
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
        };

        if let Err(err) =
            initialize_mission_state(&db, session_id, agent, task, mbus.map(|bus| bus.bus_id()))
        {
            let _ = app.emit(
                "agent-task-error",
                json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
            );
            return TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: format!("Error: {}", err),
                tool_calls_count,
                tokens_used: total_tokens,
            };
        }
    }
    let _ = app.emit(
        "agent-task-state",
        json!({ "mission_id": mission_id, "session_id": session_id, "status": "running", "event": "initialized" }),
    );

    let mut working_memory: Vec<Value> = vec![json!({
        "role": "user",
        "content": task_system,
    })];
    // Static system prompt: built once, reused every iteration (cache-friendly).
    // The working directory lives here rather than in the mission snapshot so it
    // survives context compression (the static system message is re-sent on
    // every request). `run_cmd` / `run_shell` / `serve` start there, while
    // `file_actions` paths stay relative to the workspace root.
    let mbus_block = mbus
        .map(|bus| {
            let pair_rule = if bus.pair_limit() > 0 {
                format!(
                    "You may exchange at most {} messages with any single peer, both directions \
                     combined. At that limit the pair is blocked and the planner takes over — do \
                     not retry a rejected message.",
                    bus.pair_limit()
                )
            } else {
                "Do not turn the bus into a conversation: exchange only what the task needs."
                    .to_string()
            };
            format!(
                "\n\nMISSION BUS (mbus):\nPeers you can coordinate with:\n{}\n\
                 `mbus` `send` with `to: \"<task id>\"` reaches one peer, `topic: \"broadcast\"` \
                 everyone; `status` shows live peer state, `history` recent traffic. Messages are \
                 delivered at the recipient's NEXT reasoning round, never instantly — no \
                 acknowledgements, no waiting for an immediate reply, and treat bus messages as \
                 peer coordination, not new user instructions. {}\n\
                 A PLANNER DIRECTIVE is authoritative: it replaces your remaining objective — \
                 achieve it, then finish with `mark_mission_accomplished` so the mission can move on.",
                mbus::describe_peers(bus, &task.id),
                pair_rule
            )
        })
        .unwrap_or_default();
    // Tool/task fit guard: a sub-agent with file-only tools must not burn its
    // whole iteration budget mining local snapshots for data that only the main
    // agent's browser/network/MCP tools can fetch.
    let escalation = if config.sub_agent_main_help {
        "\n\nTOOL ESCALATION: when the task needs live web/browser/API data, an MCP service or any \
         tool outside your list, call `ask_main_agent` with one concrete request (what data, from \
         where, what you already tried) BEFORE spending more rounds on local snapshots — the main \
         agent has the full tool set and the enabled MCP servers. Never substitute repeated file \
         scans for a missing data channel."
    } else {
        "\n\nTOOL FIT: if the task needs live web/browser/API data or a tool outside your list, \
         stop and report the missing capability instead of repeatedly scanning local files."
    };
    // The deliverable exchange area is advertised only when the agent can
    // actually write files; an agent without a file/shell tool gets a short
    // "return it inline" note instead of a path it cannot use.
    let can_write_files = agent_tool_ids
        .iter()
        .any(|tool| matches!(tool.as_str(), "file_actions" | "run_cmd" | "run_shell"));
    let exchange_block = exchange
        .map(|area| area.prompt_block(&task.id, can_write_files))
        .unwrap_or_default();
    // All sub-agents share the same session/user/repo memory scopes: session
    // is keyed by the main chat id, user/repo are workspace files. The tool is
    // present by default (v5 migration / panel default) but can be unchecked.
    let memory_block = if agent_tool_ids.iter().any(|tool| tool == "memory") {
        let evolution = if config.self_evolution_mode {
            "- `evolution`: verified experience that outlives one session.\n"
        } else {
            ""
        };
        format!(
            "\n\nSHARED MEMORY: the `memory` tool is shared with the main agent and every sibling task:\n\
             - `session`: this chat's shared notes.\n\
             - `repo`: durable project facts and decisions in the workspace.\n\
             - `user`: user preferences.\n\
             {}\
             Search before starting (`memory search`) and add one concise conclusion before finishing \
             (`memory add`); prefix mission notes with `{}-...`, never store credentials, and do not \
             run `consolidate` — skill consolidation belongs to the main agent.",
            evolution, task.id,
        )
    } else {
        String::new()
    };
    let static_system_prompt = format!(
        "{}\n\nWorking directory: {} — `run_cmd`, `run_shell` and serve processes start here, while \
         `file_actions` paths stay relative to the workspace root ({}).{}{}{}{}",
        build_agent_static_system_prompt(agent, &self_evolution_context),
        task_cwd.display(),
        workspace_dir.display(),
        mbus_block,
        exchange_block,
        memory_block,
        escalation,
    );
    let max_iterations = (agent.max_iterations > 0).then_some(agent.max_iterations);
    let mut iterations: u32 = 0;
    // Session-scoped tool failure guard, shared with the main agent: a tool the
    // chat already gave up on must not be retried by a sub-agent either.
    let failure_limit = config
        .tool_failure_limit
        .min(crate::turn_guard::MAX_TOOL_FAILURE_LIMIT);
    // Tools the session gave up on in an earlier turn are dropped once, before
    // the first request: the tool list is part of the prompt prefix, so it must
    // stay identical across this agent's rounds (`turn_guard`).
    let disabled_tools = app
        .state::<AppState>()
        .tool_guard
        .disabled_tools(session_id);
    let req_tools: Vec<Value> = tools_list
        .iter()
        .filter(|tool| {
            tool["function"]["name"]
                .as_str()
                .map(|name| !disabled_tools.contains(name))
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    loop {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        // The planner can end a task at any reasoning boundary (`mbus`
        // `end_task`). Stop before spending another request: report what the
        // task has produced so far and let the planner collect the result.
        if let Some(bus) = mbus {
            if let Some(note) = bus.conclusion(&task.id) {
                let content = last_assistant_content(&working_memory)
                    .unwrap_or_else(|| format!("Ended by the planner: {}", note));
                return finalize_mission_task(
                    app,
                    &mission_id,
                    task,
                    agent,
                    content,
                    tool_calls_count,
                    total_tokens,
                );
            }
        }

        // `mark_mission_accomplished` set the flag in the previous round's tool
        // call. Finish now instead of asking the model for a wrap-up round.
        let accomplished = {
            let state = app.state::<AppState>();
            state
                .db
                .lock()
                .ok()
                .and_then(|db| load_mission_state(&db, &mission_id).ok())
                .filter(|snapshot| snapshot.mission_accomplished)
        };
        if let Some(snapshot) = accomplished {
            let content = if !snapshot.final_report.trim().is_empty() {
                snapshot.final_report.clone()
            } else {
                last_assistant_content(&working_memory)
                    .unwrap_or_else(|| "Mission accomplished.".to_string())
            };
            return finalize_mission_task(
                app,
                &mission_id,
                task,
                agent,
                content,
                tool_calls_count,
                total_tokens,
            );
        }

        if let Some(limit) = max_iterations {
            if iterations >= limit {
                break;
            }
        }
        iterations = iterations.saturating_add(1);
        if let Some(bus) = mbus {
            bus.mark_iteration(&task.id, iterations, total_tokens);
        }

        let mut mission_snapshot = {
            let state = app.state::<AppState>();
            let db = match state.db.lock() {
                Ok(db) => db,
                Err(err) => {
                    let error =
                        format!("Failed to lock database while loading mission state: {err}");
                    let _ = app.emit(
                        "agent-task-error",
                        json!({ "task_id": task.id, "agent_id": agent.id, "error": error }),
                    );
                    return TaskResult {
                        task_id: task.id.clone(),
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        status: "error".to_string(),
                        content: error,
                        tool_calls_count,
                        tokens_used: total_tokens,
                    };
                }
            };

            match load_mission_state(&db, &mission_id) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    let _ = app.emit(
                        "agent-task-error",
                        json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
                    );
                    return TaskResult {
                        task_id: task.id.clone(),
                        agent_id: agent.id.clone(),
                        agent_name: agent.name.clone(),
                        status: "error".to_string(),
                        content: format!("Error: {}", err),
                        tool_calls_count,
                        tokens_used: total_tokens,
                    };
                }
            }
        };

        let snapshot_prompt = build_mission_snapshot_prompt(&mission_snapshot);

        match compress_working_memory_if_needed(
            client,
            url,
            &config.api_key,
            &headers,
            model,
            &static_system_prompt,
            &snapshot_prompt,
            &mut working_memory,
            &mut mission_snapshot.episodic_summary,
            context_budget,
        )
        .await
        {
            Ok(true) => {
                let state = app.state::<AppState>();
                if let Ok(db) = state.db.lock() {
                    let _ =
                        save_mission_summary(&db, &mission_id, &mission_snapshot.episodic_summary);
                };
                let _ = app.emit(
                    "agent-task-state",
                    json!({ "mission_id": mission_id, "session_id": session_id, "summary_updated": true }),
                );
            }
            Ok(false) => {}
            Err(err) => {
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: format!("Error: {}", err),
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
        }

        // ── Mission-bus messages ─────────────────────────────────────────────
        // A request boundary is the only safe insertion point: an assistant
        // message carrying tool_calls must stay adjacent to its tool results.
        // This mirrors the main agent's `drain_turn_injections`; the injected
        // text uses the `user` role (some gateways reject mid-conversation
        // system messages) and is clearly marked as peer coordination.
        if let Some(bus) = mbus {
            let queued = bus.take_inbox(&task.id, mbus::MAX_DELIVERIES_PER_ROUND);
            if !queued.is_empty() {
                for message in &queued {
                    working_memory.push(json!({
                        "role": "user",
                        "content": format!(
                            "{}\nfrom: {} ({})\nvia: {}\n\n{}",
                            mbus::MBUS_INJECTION_PREFIX,
                            message.from_name,
                            message.from_id,
                            if message.target_kind == "topic" {
                                format!("#{}", message.topic)
                            } else {
                                "direct".to_string()
                            },
                            message.body,
                        ),
                    }));
                }
                let _ = app.emit(
                    "agent-mbus-delivered",
                    json!({
                        "bus_id": bus.bus_id(),
                        "session_id": session_id,
                        "mission_id": mission_id,
                        "task_id": task.id,
                        "count": queued.len(),
                        "messages": queued
                            .iter()
                            .map(|message| json!({
                                "from_id": message.from_id,
                                "from_name": message.from_name,
                                "topic": message.topic,
                                "body": message.body,
                                "created_at_ms": message.created_at_ms,
                            }))
                            .collect::<Vec<_>>(),
                    }),
                );
            }
        }

        // Rebuild the snapshot after a possible compression (episodic summary
        // may have changed), then append it as the final message.
        let mut snapshot_prompt = build_mission_snapshot_prompt(&mission_snapshot);
        if let Some(bus) = mbus {
            if let Some((objective, done_when)) = bus.directive(&task.id) {
                snapshot_prompt.push_str(&format!(
                    "\n\n{}\nObjective: {objective}",
                    mbus::PLANNER_DIRECTIVE_PREFIX
                ));
                if let Some(done) = done_when {
                    snapshot_prompt.push_str(&format!(
                        "\nExit condition (finish the task as soon as it is met): {done}"
                    ));
                }
                snapshot_prompt.push_str(
                    "\nThis objective overrides your earlier plan. Do not exchange further \
                     coordination messages unless strictly required; produce the result and \
                     call mark_mission_accomplished.",
                );
            }
        }
        let mut messages =
            build_agent_messages(&static_system_prompt, &working_memory, &snapshot_prompt);
        sanitize_tool_pairs(&mut messages);

        let mut req_body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        apply_completion_token_limit(
            &mut req_body,
            Some(max_complete_tokens),
            Some(LLM_DEFAULT_MAX_TOKENS),
        );
        if let Some(temp) = agent.temperature {
            req_body["temperature"] = json!(temp);
        }
        // Same thinking depth as the chat toolbar / config, resolved for the
        // model this agent runs on.
        if let Some(effort) = resolve_reasoning_effort(config, model) {
            req_body["reasoning_effort"] = json!(effort);
        }
        if !req_tools.is_empty() {
            req_body["tools"] = json!(req_tools);
            req_body["tool_choice"] = json!("auto");
        }
        log_agent_event(
            app,
            "INFO",
            format!(
                "agent [{}] {} round {} request: model={}, messages={}, tools={}, \
                 context_budget={}, max_completion_tokens={}, max_iterations={} (0 = unlimited)",
                task.id,
                agent.name,
                iterations,
                model,
                working_memory.len(),
                req_tools.len(),
                context_budget,
                max_complete_tokens,
                max_iterations.unwrap_or(0),
            ),
        );

        let stream_result = stream_llm_request(
            app,
            client,
            url,
            &config.api_key,
            req_body,
            cancelled,
            StreamOptions {
                token_event: "agent-task-token",
                task_id: Some(&task.id),
                // Live "thinking" line on the task row. The reasoning text is
                // never persisted in the mission report; only the UI shows it.
                reasoning_event: Some("agent-task-reasoning"),
                emit_usage: false,
                emit_tokens: true,
                // A sub-agent request failure must not emit `chat-error`: the
                // frontend treats that event as the whole turn failing, closes
                // the transcript and drops later task events. The task reports
                // it through `agent-task-error` instead.
                emit_errors: false,
                usage_max_tokens: None,
                ds_format: config.ds_format_for(model),
                headers: &headers,
            },
        )
        .await
        .map(|sr| {
            (
                sr.content,
                sr.reasoning_content,
                sr.tool_calls,
                sr.prompt_tokens + sr.completion_tokens,
            )
        });

        match stream_result {
            Err(e) => {
                let balance_error = is_provider_balance_error(&e);
                let error = if balance_error {
                    format!("Upstream account balance/quota exhausted: {e}")
                } else {
                    e
                };
                log_agent_event(
                    app,
                    "ERROR",
                    format!(
                        "agent [{}] {} round {} request failed{}: {error}",
                        task.id,
                        agent.name,
                        iterations,
                        if balance_error {
                            " (provider balance/quota)"
                        } else {
                            ""
                        },
                    ),
                );
                let _ = app.emit(
                    "agent-task-error",
                    json!({ "task_id": task.id, "agent_id": agent.id, "error": error.clone() }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: format!("Error: {error}"),
                    tool_calls_count,
                    tokens_used: total_tokens,
                };
            }
            Ok((content, reasoning_content, agent_tool_calls, tokens)) => {
                total_tokens += tokens;
                log_agent_event(
                    app,
                    "INFO",
                    format!(
                        "agent [{}] {} round {} response: tokens={}, tool_calls={}, text_chars={}",
                        task.id,
                        agent.name,
                        iterations,
                        tokens,
                        agent_tool_calls.len(),
                        content.chars().count()
                    ),
                );

                if agent_tool_calls.is_empty() {
                    if !content.trim().is_empty() {
                        // Keep the round's reasoning trace: thinking-mode APIs
                        // require it on replayed assistant messages.
                        let mut assistant_msg =
                            json!({ "role": "assistant", "content": content.clone() });
                        if !reasoning_content.is_empty() {
                            assistant_msg["reasoning_content"] = json!(reasoning_content.as_str());
                        }
                        working_memory.push(assistant_msg);
                    }

                    let mission_snapshot = {
                        let state = app.state::<AppState>();
                        let db = match state.db.lock() {
                            Ok(db) => db,
                            Err(err) => {
                                let error =
                                    format!("Failed to lock database after agent turn: {err}");
                                let _ = app.emit(
                                    "agent-task-error",
                                    json!({ "task_id": task.id, "agent_id": agent.id, "error": error }),
                                );
                                return TaskResult {
                                    task_id: task.id.clone(),
                                    agent_id: agent.id.clone(),
                                    agent_name: agent.name.clone(),
                                    status: "error".to_string(),
                                    content: error,
                                    tool_calls_count,
                                    tokens_used: total_tokens,
                                };
                            }
                        };

                        match load_mission_state(&db, &mission_id) {
                            Ok(snapshot) => snapshot,
                            Err(err) => {
                                let _ = app.emit(
                                    "agent-task-error",
                                    json!({ "task_id": task.id, "agent_id": agent.id, "error": err }),
                                );
                                return TaskResult {
                                    task_id: task.id.clone(),
                                    agent_id: agent.id.clone(),
                                    agent_name: agent.name.clone(),
                                    status: "error".to_string(),
                                    content: format!("Error: {}", err),
                                    tool_calls_count,
                                    tokens_used: total_tokens,
                                };
                            }
                        }
                    };

                    if mission_snapshot.mission_accomplished {
                        let final_content = if !content.trim().is_empty() {
                            content.clone()
                        } else if !mission_snapshot.final_report.trim().is_empty() {
                            mission_snapshot.final_report.clone()
                        } else {
                            "Mission accomplished.".to_string()
                        };

                        let state = app.state::<AppState>();
                        if let Ok(db) = state.db.lock() {
                            let _ = save_mission_final_report(&db, &mission_id, &final_content);
                        }

                        let summary: String = final_content.chars().take(200).collect();
                        let _ = app.emit(
                            "agent-task-done",
                            json!({
                                "task_id": task.id,
                                "agent_id": agent.id,
                                "agent_name": agent.name,
                                "summary": summary,
                                "status": "success",
                            }),
                        );
                        return TaskResult {
                            task_id: task.id.clone(),
                            agent_id: agent.id.clone(),
                            agent_name: agent.name.clone(),
                            status: "success".to_string(),
                            content: final_content,
                            tool_calls_count,
                            tokens_used: total_tokens,
                        };
                    }

                    working_memory
                        .push(json!({ "role": "user", "content": AGENT_CONTINUE_PROMPT }));
                    continue;
                }

                // Append assistant turn with tool calls
                let assistant_tcs: Vec<Value> = agent_tool_calls
                    .iter()
                    .map(|(id, name, args)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": args },
                        })
                    })
                    .collect();
                let assistant_content = if content.trim().is_empty() {
                    Value::Null
                } else {
                    Value::String(content.clone())
                };
                // Thinking-mode APIs require the round's `reasoning_content` to
                // ride along with tool-call turns, unmodified.
                let mut assistant_msg = json!({
                    "role": "assistant",
                    "content": assistant_content,
                    "tool_calls": assistant_tcs,
                });
                if !reasoning_content.is_empty() {
                    assistant_msg["reasoning_content"] = json!(reasoning_content.as_str());
                }
                working_memory.push(assistant_msg);

                // Execute each tool call
                for (id, name, args) in &agent_tool_calls {
                    tool_calls_count += 1;
                    if let Some(bus) = mbus {
                        bus.mark_tool(&task.id, name);
                    }
                    // Session-level gate: a tool disabled after repeated
                    // failures never runs again. The call is still answered —
                    // the protocol needs one `tool` message per `tool_call_id`.
                    let blocked = app
                        .state::<AppState>()
                        .tool_guard
                        .blocked_notice(session_id, name, failure_limit);
                    let ran = blocked.is_none();
                    log_agent_event(
                        app,
                        "INFO",
                        format!(
                            "agent [{}] {} tool {}: {}",
                            task.id,
                            agent.name,
                            name,
                            preview_text(args, 240)
                        ),
                    );
                    let tool_started = std::time::Instant::now();
                    emit_agent_tool_state(
                        app,
                        &task.id,
                        "start",
                        name,
                        Some(preview_text(args, 200)),
                        None,
                        None,
                    );
                    let mut result = if let Some(notice) = blocked {
                        notice
                    } else if name == "mbus" {
                        match mbus {
                            Some(bus) => {
                                let parsed: Value = serde_json::from_str(args).unwrap_or_default();
                                mbus::handle_tool(app, bus, &task.id, &parsed, false)
                            }
                            None => {
                                "Error: the mission bus is not available for this task.".to_string()
                            }
                        }
                    } else if name == "ask_main_agent" {
                        if !config.sub_agent_main_help {
                            "Error: asking the main agent is disabled in Settings → Runtime & Debug.".to_string()
                        } else if main_help_calls >= MAIN_HELPER_MAX_CALLS {
                            format!(
                                "Error: this task already used its {} main-agent help requests. Continue with the material you have.",
                                MAIN_HELPER_MAX_CALLS
                            )
                        } else {
                            let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                            let request = args_json["request"]
                                .as_str()
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let context = args_json["context"]
                                .as_str()
                                .unwrap_or("")
                                .trim()
                                .to_string();

                            if request.is_empty() {
                                "Error: ask_main_agent requires a non-empty 'request'.".to_string()
                            } else {
                                main_help_calls += 1;
                                let _ = app.emit(
                                    "agent-help-start",
                                    json!({
                                        "task_id": task.id,
                                        "mission_id": mission_id,
                                        "agent_id": agent.id,
                                        "agent_name": agent.name,
                                        "request": preview_text(&request, 300),
                                        "cwd": task.cwd,
                                    }),
                                );

                                match run_main_agent_helper(
                                    app,
                                    client,
                                    url,
                                    config,
                                    agent,
                                    task,
                                    session_id,
                                    &workspace_dir,
                                    &task_cwd,
                                    skill_access_root.as_deref(),
                                    &request,
                                    &context,
                                    cancelled,
                                    mcp_session,
                                )
                                .await
                                {
                                    Ok(outcome) => {
                                        total_tokens = total_tokens.saturating_add(outcome.tokens);
                                        tool_calls_count =
                                            tool_calls_count.saturating_add(outcome.tool_calls);
                                        let answer_chars = outcome.content.chars().count();
                                        append_mission_help_note(
                                            app,
                                            &mission_id,
                                            &format!(
                                                "asked main agent: {} → {} chars",
                                                preview_text(&request, 160),
                                                answer_chars
                                            ),
                                        );
                                        let _ = app.emit(
                                            "agent-help-done",
                                            json!({
                                                "task_id": task.id,
                                                "mission_id": mission_id,
                                                "agent_id": agent.id,
                                                "answer_chars": answer_chars,
                                                "tokens": outcome.tokens,
                                                "tool_calls": outcome.tool_calls,
                                            }),
                                        );
                                        outcome.content
                                    }
                                    Err(err) => {
                                        append_mission_help_note(
                                            app,
                                            &mission_id,
                                            &format!(
                                                "asked main agent: {} → error: {}",
                                                preview_text(&request, 160),
                                                preview_text(&err, 200)
                                            ),
                                        );
                                        let _ = app.emit(
                                            "agent-help-error",
                                            json!({
                                                "task_id": task.id,
                                                "mission_id": mission_id,
                                                "agent_id": agent.id,
                                                "error": err,
                                            }),
                                        );
                                        format!("Error: main-agent help failed: {err}")
                                    }
                                }
                            }
                        }
                    } else if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                        let state = app.state::<AppState>();
                        let arguments: Value =
                            serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                        let owner = format!("agent:{}", agent.name);
                        match mcp::invoke_mcp_tool_for_agent(
                            &state,
                            mcp_session,
                            mcp_server,
                            actual_tool_name,
                            arguments,
                            &owner,
                        )
                        .await
                        {
                            Ok(text) => format!(
                                "MCP [{}] {} result:\n{}",
                                mcp_server.name, actual_tool_name, text
                            ),
                            Err(err) => format!("MCP tool error: {err}"),
                        }
                    } else {
                        tools::execute_tool(
                            app,
                            name,
                            args,
                            workspace_dir.clone(),
                            config,
                            &[],
                            skill_access_root.as_deref(),
                            Some(&mission_id),
                            Some(session_id),
                            &[],  // sub-agents don't track active skill dirs for skill_read
                            None, // background tasks are never advertised to sub-agents
                            Some(task_cwd.as_path()),
                        )
                        .await
                    };
                    // Count the failure; crossing the limit tells the model and
                    // the UI, and disables the tool for the whole session.
                    let failed = crate::turn_guard::tool_output_is_error(&result);
                    if ran {
                        if let Some(trip) = app.state::<AppState>().tool_guard.record(
                            session_id,
                            name,
                            failed,
                            failure_limit,
                        ) {
                            result.push_str("\n\n");
                            result.push_str(&crate::turn_guard::disabled_notice(
                                name,
                                trip.failures,
                                failure_limit,
                                true,
                            ));
                            let _ = app.emit(
                                "chat-notice",
                                json!({
                                    "kind": "tool_disabled",
                                    "tool": name,
                                    "failures": trip.failures,
                                    "limit": failure_limit,
                                }),
                            );
                        }
                    }
                    emit_agent_tool_state(
                        app,
                        &task.id,
                        "done",
                        name,
                        Some(preview_text(&result, 160)),
                        Some(tool_started.elapsed().as_millis()),
                        Some(!failed),
                    );
                    log_agent_event(
                        app,
                        if failed {
                            "WARN"
                        } else {
                            "INFO"
                        },
                        format!(
                            "agent [{}] {} tool {} -> {} chars: {}",
                            task.id,
                            agent.name,
                            name,
                            result.chars().count(),
                            preview_text(&result, 160)
                        ),
                    );
                    working_memory
                        .push(json!({ "role": "tool", "tool_call_id": id, "content": result }));
                }
            }
        }
    }

    // Max iterations reached — return whatever was last generated
    let last_content = working_memory
        .iter()
        .rev()
        .find_map(|m| {
            m["content"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| {
            if max_iterations.is_some() {
                "Max iterations reached without final answer.".to_string()
            } else {
                "Autonomous loop stopped without a final answer.".to_string()
            }
        });

    let mission_summary = {
        let state = app.state::<AppState>();
        state
            .db
            .lock()
            .ok()
            .and_then(|db| load_mission_state(&db, &mission_id).ok())
    };

    let final_content = mission_summary
        .as_ref()
        .and_then(|mission| {
            if mission.mission_accomplished && !mission.final_report.trim().is_empty() {
                Some(mission.final_report.clone())
            } else {
                None
            }
        })
        .unwrap_or(last_content);

    let accomplished = mission_summary
        .as_ref()
        .map(|mission| mission.mission_accomplished)
        .unwrap_or(false);
    let _ = app.emit(
        "agent-task-done",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "summary": if max_iterations.is_some() { "Max iterations reached" } else { "Loop stopped" },
            // A loop that ended without the completion flag was stopped, not
            // completed: the row must not show a success checkmark.
            "status": if accomplished { "success" } else { "stopped" },
        }),
    );
    TaskResult {
        task_id: task.id.clone(),
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        status: if accomplished {
            "success".to_string()
        } else {
            "stopped".to_string()
        },
        content: final_content,
        tool_calls_count,
        tokens_used: total_tokens,
    }
}

/// Last non-empty assistant text in the working memory. Used when the planner
/// ends a task before the model produced a dedicated final answer.
pub(crate) fn last_assistant_content(working_memory: &[Value]) -> Option<String> {
    working_memory.iter().rev().find_map(|message| {
        if message["role"].as_str() != Some("assistant") {
            return None;
        }
        message["content"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
    })
}

/// Close a sub-agent mission as a success without another model round. Used
/// when `mark_mission_accomplished` already ran, or when the planner ended the
/// task through `mbus` `end_task`.
pub(crate) fn finalize_mission_task(
    app: &AppHandle,
    mission_id: &str,
    task: &Task,
    agent: &SubAgent,
    content: String,
    tool_calls_count: u32,
    tokens_used: u32,
) -> TaskResult {
    let state = app.state::<AppState>();
    if let Ok(db) = state.db.lock() {
        let _ = mark_mission_accomplished(&db, mission_id, Some(&content));
    }

    let summary: String = content.chars().take(200).collect();
    let _ = app.emit(
        "agent-task-done",
        json!({
            "task_id": task.id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "summary": summary,
            "status": "success",
        }),
    );
    TaskResult {
        task_id: task.id.clone(),
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        status: "success".to_string(),
        content,
        tool_calls_count,
        tokens_used,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────


// ─── Final result hand-off ──────────────────────────────────────────────────

/// Renders the finished mission for injection into the main agent's context.
/// The main agent — not a separate synthesizer prompt — defines the final
/// user-facing answer from these reports.
pub(crate) fn build_mission_results_block(
    results: &[TaskResult],
    exchange: Option<&exchange::ExchangeArea>,
) -> String {
    let mut body = String::from(
        "All sub-agent tasks have finished. Sub-agents are execution workers and do not \
         write the user-facing answer: define and deliver the final result yourself. \
         Verify the reports below, reconcile conflicting findings, flag anything that \
         failed or looks incomplete, and answer the original request completely. \
         Tasks marked `skipped` produced no work at all: never present them as completed — \
         either do that part yourself or state clearly what was not done.\n\n",
    );

    if let Some(area) = exchange {
        body.push_str(&format!(
            "Deliverable exchange area: {} (workspace-relative: {})\n\
             Read deliverables with `file_actions` before answering; do not paste huge files \
             verbatim into the answer.\n\n",
            area.root().display(),
            area.relative_root(),
        ));
    }

    if results.is_empty() {
        body.push_str("No task results were produced.");
        return body;
    }

    for result in results {
        body.push_str(&format!(
            "### {} (agent: {}, status: {}, tool calls: {}, tokens: {})\n\n{}\n\n",
            result.task_id,
            result.agent_name,
            result.status,
            result.tool_calls_count,
            result.tokens_used,
            result.content
        ));
        if let Some(area) = exchange {
            body.push_str("Deliverables:\n");
            body.push_str(&exchange::render_files(&area.list_files(&result.task_id)));
            body.push_str("\n\n");
        }
    }

    body
}
