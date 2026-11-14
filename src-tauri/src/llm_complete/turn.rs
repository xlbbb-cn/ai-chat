//! One chat turn: tool assembly, the agent loop and the `chat_completion`
//! command.

use super::*;

// ─── Chat command ─────────────────────────────────────────────────────────────

/// Closes the mid-turn injection window when a turn ends — on EVERY path,
/// including the `?` early return after an upstream error (that path emits
/// `chat-error` and never reaches `chat-done`).
///
/// The window is keyed by turn id, so a turn that is still winding down after
/// `chat-done` can neither close a newer turn's window nor drop its items.
pub(crate) struct InjectionWindow {
    pub(crate) app: AppHandle,
    pub(crate) turn_id: u64,
}

impl InjectionWindow {
    pub(crate) fn open(app: &AppHandle, state: &AppState, session_id: &str) -> Self {
        Self {
            app: app.clone(),
            turn_id: state.open_turn_window(session_id),
        }
    }
}

impl Drop for InjectionWindow {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<AppState>() {
            state.close_turn_window(self.turn_id);
        }
    }
}

/// Assemble the complete tool payload for one request of a chat turn.
///
/// Re-entrant on purpose: the `mcp_self` tool can create and enable an MCP
/// server from inside the turn (which bumps `AppState::mcp_revision`), and the
/// tool loop rebuilds this payload at the next request boundary so the new
/// tools are callable in the *same* turn instead of the next one. Built-ins,
/// `skill_read`, `use_skill` and `call_subagent` are re-derived from the same
/// inputs, so a rebuild is idempotent.
pub(crate) async fn build_turn_tools(
    state: &AppState,
    config: &crate::AppConfig,
    active_skill_dirs: &[(String, PathBuf)],
    has_skills: bool,
    evolution_root: Option<&Path>,
    enabled_agents: &[agents::SubAgent],
) -> (
    Vec<Value>,
    std::collections::HashMap<String, (mcp::McpServer, String)>,
    u64,
) {
    let mut tools_list = tools::get_all_tools(
        &config.effective_tools(),
        config.self_evolution_mode,
        config.background_tools,
    );

    // ── Skill-read tool ─────────────────────────────────────────────────────
    // Auto-inject skill_read when one or more skills are active.
    if !active_skill_dirs.is_empty() {
        tools_list.push(tools::get_skill_read_tool());
    }

    // ── MCP tools ───────────────────────────────────────────────────────────
    let mut mcp_tool_map: std::collections::HashMap<String, (mcp::McpServer, String)> =
        std::collections::HashMap::new();
    {
        // Merged roster: the user's panel servers (`mcp_servers.json`) plus the
        // ones the assistant built (`.data/mcp.conf`).
        let enabled_servers: Vec<mcp::McpServer> = mcp::load_effective_servers(state)
            .into_iter()
            .filter(|s| s.enabled)
            .collect();

        // Sorting boundary: only the appended MCP segment is re-ordered — the
        // built-in tools and the skill tools keep their layout.
        let mcp_tools_start = tools_list.len();

        for server in enabled_servers.iter() {
            match mcp::get_server_tools(&state, server).await {
                Ok(server_tools) => {
                    for tool in server_tools {
                        // Stable `mcp_<server-hash>_<name>` function name and
                        // a description that names the server.
                        if let Some((fn_name, actual_name, openai_tool)) =
                            mcp::adapt_tool_for_llm(server, &tool, config.background_tools)
                        {
                            tools_list.push(openai_tool);
                            mcp_tool_map.insert(fn_name, (server.clone(), actual_name));
                        }
                    }
                }
                Err(e) => {
                    // Same shape as the sub-agent loader: a server that cannot
                    // list its tools is skipped, never fatal for the turn.
                    if let Ok(logger) = state.logger.lock() {
                        logger.log(
                            "ERROR",
                            &format!("MCP server '{}' tools/list failed: {e}", server.name),
                        );
                    }
                }
            }
        }

        // The tools payload sits at the very top of the cached request prefix,
        // so its order must not depend on enumeration order (see
        // `mcp::sort_adapted_tools`).
        mcp::sort_adapted_tools(&mut tools_list[mcp_tools_start..]);
    }

    // `use_skill` is advertised when the user has at least one enabled skill —
    // and always in self-evolution mode, where the assistant may have just written
    // a new skill that is not enabled yet but must still be loadable.
    if has_skills || evolution_root.is_some() {
        tools_list.push(tools::get_use_skill_tool());
    }

    if !enabled_agents.is_empty() {
        let mut subagent_tool = json!({
            "type": "function",
            "function": {
                "name": "call_subagent",
                "description": "Delegate a task to a configured agent (llm / cli / cloud). Use an exact `agent_id` from the available-agents list; the agent never sees this conversation, so `task_description` must be self-contained. Waits and returns the final result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "Exact id of the agent to invoke."
                        },
                        "task_description": {
                            "type": "string",
                            "description": "Detailed, self-contained task prompt for the agent."
                        },
                        "context": {
                            "type": "string",
                            "description": "Optional extra context (file paths, findings, constraints) appended to the task description."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Working directory for the task: workspace-relative (e.g. \"./apps/web\") or absolute inside the workspace. The agent starts there — pass it whenever the work targets a specific project or subfolder; omit for the workspace root."
                        }
                    },
                    "required": ["agent_id", "task_description"]
                }
            }
        });
        if config.background_tools {
            subagent_tool["function"]["parameters"]["properties"]["run_in_background"] = json!({
                "type": "boolean",
                "description": "Delegate in the background: return a task id immediately instead of waiting. The result is delivered automatically before your next request and the turn stays open until it arrives — for long work you can do something else meanwhile."
            });
        }
        tools_list.push(subagent_tool);
    }

    (tools_list, mcp_tool_map, state.mcp_revision())
}

/// Render the session's embedded sub-agent roles (`+agent` in the composer) as
/// a system-prompt block.
///
/// The block is appended to the main system prompt: a role defines *how* the
/// assistant answers, while tools, the model and approval rules stay the main
/// agent's. Empty/blank prompts are skipped, and an empty list renders nothing
/// so the caller leaves the prompt byte-identical to a normal session.
pub(crate) fn render_embedded_roles(roles: &[db::EmbeddedRole]) -> String {
    let mut block = String::new();
    for role in roles {
        let prompt = role.prompt.trim();
        if prompt.is_empty() {
            continue;
        }
        if block.is_empty() {
            block.push_str(
                "EMBEDDED ROLES: the user attached the following specialist role(s) to this \
                 conversation with `+agent`. Adopt each role as your own operating persona and \
                 follow it alongside the instructions above. Safety, tool and approval rules win \
                 over a role; tool access, the model and permissions are unchanged by it. Do not \
                 delegate to an embedded role with `call_subagent` — you act as it directly.",
            );
        }
        let name = role.name.trim();
        let name = if name.is_empty() {
            role.id.as_str()
        } else {
            name
        };
        block.push_str(&format!("\n\n## Embedded role: {name}"));
        let description = role.description.trim();
        if !description.is_empty() {
            block.push('\n');
            block.push_str(description);
        }
        block.push('\n');
        block.push_str(prompt);
    }
    block
}

/// The whole chat turn: orchestration, the main-agent tool loop, and the
/// final answer. Extracted from `chat_completion` so the command wrapper can
/// run turn-end cleanup on every exit path (final answer, upstream error,
/// or Stop pressed) while holding the shared sleep lock.
pub(crate) async fn run_chat_turn(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_ids: Vec<String>,
    session_id: String,
    model_override: Option<String>,
    use_agents: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // A fresh cancel flag belongs to this turn. Any previous turn that is still
    // winding down keeps polling its own flag — which just became permanently
    // `true` — so it exits instead of resurrecting when this turn runs. Stop
    // pressed while idle can also never leak into the next turn.
    let turn_cancelled = {
        let turn_flag = Arc::new(AtomicBool::new(false));
        let previous = {
            let mut slot = state.chat_cancelled.lock().unwrap();
            std::mem::replace(&mut *slot, turn_flag.clone())
        };
        previous.store(true, Ordering::SeqCst);
        turn_flag
    };

    // From here on the user may push new information into this turn; the
    // window is closed (and all undelivered items dropped for the UI to
    // re-send) by the guard's `Drop`, whatever path the turn takes.
    let injection_window = InjectionWindow::open(&app, &state, &session_id);

    let config = state.config.lock().unwrap().clone();
    let multi_agent_enabled = use_agents.unwrap_or(false);
    log_event(
        &state,
        "INFO",
        format!(
            "chat_completion started: session_id={}, model={}",
            session_id, config.model
        ),
    );

    // ── Agent orchestration path ──────────────────────────────────────────────
    // Sub-agent missions run first. Their reports are injected into the main
    // agent's context below and the NORMAL completion flow answers the user:
    // the main agent defines the final result, sub-agents never write it.
    // The outcome also carries the selector's choice, so the main session
    // advertises the agents picked for this turn (soft narrowing: every enabled
    // agent stays callable through `call_subagent`).
    let mut orchestration: Option<agents::OrchestrationOutcome> = None;
    if multi_agent_enabled {
        let agents_cfg = agents::load_configured_agents(&state.agents_config_path);
        let has_enabled = agents_cfg.agents.iter().any(|a| a.enabled);
        if has_enabled {
            match agents::orchestrate(
                &app,
                &state,
                &config,
                &messages,
                &session_id,
                turn_cancelled.clone(),
            )
            .await
            {
                Ok(outcome) => orchestration = Some(outcome),
                Err(err) => log_event(
                    &state,
                    "WARN",
                    format!("agent orchestration skipped, answering directly: {err}"),
                ),
            }
            // "Stop" pressed during planning/execution ends the whole turn: the
            // in-flight sub-agents tore themselves down (cli/cloud kill their
            // process trees, the executor skipped the remaining tasks), so the
            // final answer must not run — and the turn flag must NOT be reset
            // here, or the rest of the turn would keep working after the user
            // asked it to stop.
            if turn_cancelled.load(Ordering::SeqCst) {
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "agent orchestration stopped by user: session_id={}",
                        session_id
                    ),
                );
                // Close the injection window before announcing the end of the
                // turn, so nothing accepted after this point can be stranded.
                drop(injection_window);
                let _ = app.emit("chat-done", ());
                return Ok(());
            }
            log_event(
                &state,
                "INFO",
                format!("agent orchestration finished: session_id={}", session_id),
            );
        }
    }

    let mut all_messages: Vec<Value> = vec![];
    let mut skill_allowed_commands: Vec<String> = Vec::new();
    let workspace_dir_for_roots = state.workspace_dir.lock().unwrap().clone();

    // 1. Skills already loaded earlier in this session. The UI records the
    // names explicitly on each message (`loaded_skills`) — explicit state
    // instead of scanning message text, which broke silently whenever the UI
    // event plumbing changed.
    let activated_skills = collect_loaded_skills(&messages);

    let mut system_content = config.system_message.clone(); //top-level system instructions from config
    let mut loaded_skills_content = String::new();
    let mut available_skills_info = String::new();
    // Track active skill name→directory pairs for skill_read tool injection.
    let mut active_skill_dirs: Vec<(String, PathBuf)> = Vec::new();

    // The machine's OS name and version are advertised by the `run_shell` tool
    // schema itself (tools::get_all_tools), next to the fixed script language
    // they explain, instead of sitting in this system prompt.
    if !system_content.is_empty() {
        system_content.push_str("\n\n");
    }
    system_content.push_str(
        "Explicit invocation: {tool:<name>} / {mcp:<server>.<tool>} / {skill:<name>} are direct \
        requests — call the tool or load the skill before answering, no confirmation needed.",
    ); //Add explicit invocation syntax to system prompt

    let ws_skills_dir = crate::workspace::skills_root(&workspace_dir_for_roots);

    // Self-evolution mode: the assistant owns `<workspace>/.data/skills` and may
    // install/edit/delete skills through `file_actions`. The one root is handed
    // to the tool layer below — without it every skill write is rejected.
    let skill_evolution_root = skills::self_evolution_skill_root(
        Some(&workspace_dir_for_roots),
        config.self_evolution_mode,
    );

    for skill_name in &skill_ids {
        if let Ok(resolved_skill) = skills::resolve_skill_by_name(&ws_skills_dir, skill_name) {
            let skill = resolved_skill.skill;
            merge_allowed_commands(&mut skill_allowed_commands, &skill.allowed_commands);

            // Track skill dir for skill_read tool injection
            if !active_skill_dirs.iter().any(|(n, _)| n == &skill.name) {
                active_skill_dirs.push((skill.name.clone(), resolved_skill.skill_dir.clone()));
            }

            // Keep the available-skills listing stable across turns (prompt-cache
            // friendly): activated skills stay listed here AND additionally get
            // their full instructions appended to the loaded-skills block below.
            available_skills_info.push_str(&format!(
                "- {}: {}\n",
                skill.name, skill.description
            ));
            if activated_skills.contains(&skill.name) {
                push_loaded_skill_block(
                    &mut loaded_skills_content,
                    &skill,
                    &resolved_skill.skill_dir,
                    &resolved_skill.skill_file,
                );
            }
        }
    }

    // Skills loaded earlier in this session but no longer among the selected
    // `skill_ids` (e.g. loaded dynamically, or deselected afterwards) keep
    // their instructions pinned, so `use_skill` can report them as loaded
    // without their content silently disappearing from the context.
    for skill_name in &activated_skills {
        if skill_ids.iter().any(|id| id == skill_name) {
            continue;
        }
        if let Ok(resolved_skill) = skills::resolve_skill_by_name(&ws_skills_dir, skill_name) {
            let skill = resolved_skill.skill;
            merge_allowed_commands(&mut skill_allowed_commands, &skill.allowed_commands);
            if !active_skill_dirs.iter().any(|(n, _)| n == &skill.name) {
                active_skill_dirs.push((skill.name.clone(), resolved_skill.skill_dir.clone()));
            }
            push_loaded_skill_block(
                &mut loaded_skills_content,
                &skill,
                &resolved_skill.skill_dir,
                &resolved_skill.skill_file,
            );
        }
    }

    if !available_skills_info.is_empty() {
        let skills_sys_msg = format!(
            "Available skills — descriptions only; `use_skill` loads the full instructions when \
            the request clearly needs them:\n{}",
            available_skills_info
        );
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&skills_sys_msg);
    }

    // ── Self-evolution guidance ─────────────────────────────────────────────
    // The tool description carries the mechanics; this states the *policy* so
    // the model actually creates/updates/deletes skills instead of only
    // describing how it would do it.
    let self_evolution_context = skills::self_evolution_prompt(skill_evolution_root.as_deref());
    if !self_evolution_context.is_empty() {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&self_evolution_context);

        // Earlier sessions' verified facts are deliberately NOT preloaded: a
        // "VERIFIED FACTS" block dominated the first request's prefill and was
        // mostly irrelevant to the current task. The self-evolution text only
        // points at the `evolution` memory scope, and the model searches it
        // (`memory search` / `get`) when it actually hits a problem.

        // The user may explicitly ask for a memory tidy-up. Point the model at
        // the consolidation flow instead of letting it delete entries by hand.
        if crate::tools::memory::wants_memory_consolidation(&last_user_text(&messages)) {
            system_content.push_str(
                "\n\nThe user explicitly asked to consolidate memory: call `memory` with \
                 action `consolidate` and phase `plan` first, review the clusters, then call \
                 `apply` with the full `skill_md` and the same `keys`. Never delete entries by hand.",
            );
        }
    }

    // ── Workspace script library ────────────────────────────────────────────
    // Self-evolution mode adds the save policy; every mode lists the scripts
    // that already exist so the model reuses them instead of retyping the same
    // command from scratch.
    if config.effective_tools().iter().any(|t| t == "run_cmd") {
        let scripts_context = crate::tools::scripts::prompt_block(
            &workspace_dir_for_roots,
            config.self_evolution_mode,
        );
        if !scripts_context.is_empty() {
            if !system_content.is_empty() {
                system_content.push_str("\n\n");
            }
            system_content.push_str(&scripts_context);
        }
    }

    // ── Figures ─────────────────────────────────────────────────────────────
    // The transcript renders `svg` / `carve` blocks and `mermaid` blocks; the
    // model only uses them if it is told they exist. `enhanced_figures`
    // (Settings → Advanced, on by default) picks the hand-drawn SVG path; with
    // it off the model writes Mermaid instead. Static text either way, so it
    // stays a stable prefix.
    let figures_context = crate::figures::prompt_block_for(config.enhanced_figures);
    if !figures_context.is_empty() {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&figures_context);
    }

    // Sub-agents (`llm`, `cli`, `cloud`) are only reachable when automatic
    // orchestration is enabled (`use_agents`). With orchestration off the main
    // agent answers directly: `call_subagent` is neither advertised in the
    // system prompt nor exposed in the tool list.
    let enabled_agents: Vec<agents::SubAgent> = if multi_agent_enabled {
        agents::load_configured_agents(&state.agents_config_path)
            .agents
            .into_iter()
            .filter(|a| a.enabled)
            .collect()
    } else {
        Vec::new()
    };

    // Soft narrowing: the system prompt lists only the agents the selector
    // picked for this turn. The `call_subagent` executor still accepts any
    // enabled agent id below, so one bad pick cannot lock the right agent out.
    let advertised_agents: Vec<agents::SubAgent> = {
        let selected_ids: Vec<&str> = orchestration
            .as_ref()
            .and_then(|outcome| outcome.selection.as_ref())
            .filter(|selection| selection.delegate)
            .map(|selection| {
                selection
                    .choices
                    .iter()
                    .map(|choice| choice.id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let narrowed: Vec<agents::SubAgent> = enabled_agents
            .iter()
            .filter(|agent| selected_ids.iter().any(|id| *id == agent.id))
            .cloned()
            .collect();
        if narrowed.is_empty() {
            enabled_agents.clone()
        } else {
            narrowed
        }
    };

    if !advertised_agents.is_empty() {
        let mut available_agents_info = String::new();
        for agent in &advertised_agents {
            let kind = agents::normalize_agent_type(&agent.agent_type);
            let detail = match kind.as_str() {
                agents::AGENT_TYPE_CLI => format!(" (cli: {})", agent.cli.command.trim()),
                agents::AGENT_TYPE_CLOUD => format!(" (cloud: {})", agent.cloud.endpoint.trim()),
                _ => {
                    let mcp = if agent.enable_mcp { " + MCP" } else { "" };
                    format!(
                        " (model {}{})",
                        agent
                            .model
                            .clone()
                            .unwrap_or_else(|| "main".to_string()),
                        mcp
                    )
                }
            };
            available_agents_info.push_str(&format!(
                "- {} [{}] {} — {}{}\n",
                agent.id, kind, agent.name, agent.description, detail
            ));
        }
        let mut agents_sys_msg = format!(
        "Agents you can reach with `call_subagent` (exact `agent_id`, self-contained `task_description` — they never see this conversation). Types: `llm` runs an autonomous tool loop inside the app; `cli` is a headless coding CLI on this machine that edits workspace files directly; `cloud` runs the task on a remote endpoint. Prefer `llm` for reasoning/reporting, `cli`/`cloud` for large end-to-end coding; a first `cli`/`cloud` run may need the user's approval.\n\
        Workspace root: {} — pass a workspace-relative `cwd` (e.g. \"./apps/web\") when the task targets a subfolder, omit it for the root.\n\n\
        Available agents:\n{}",
        workspace_dir_for_roots.display(),
        available_agents_info
    );

        // Explain the selection so the main agent writes task descriptions that
        // match the picked agents' purpose instead of guessing.
        if let Some(selection) = orchestration
            .as_ref()
            .and_then(|outcome| outcome.selection.as_ref())
            .filter(|selection| selection.delegate && !selection.choices.is_empty())
        {
            agents_sys_msg.push_str(
                "\n<agent-selection>\nPicked for this turn (the others stay callable if the work really needs them):\n",
            );
            for choice in &selection.choices {
                if choice.why.is_empty() {
                    agents_sys_msg.push_str(&format!("- {} ({})\n", choice.name, choice.role));
                } else {
                    agents_sys_msg.push_str(&format!(
                        "- {} ({}): {}\n",
                        choice.name, choice.role, choice.why
                    ));
                }
            }
            agents_sys_msg.push_str("</agent-selection>");
        }

        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&agents_sys_msg);
    }

    // A declined selection is not a ban — just a hint that the cheap path is to
    // answer directly. Keep it short so it does not become a second policy.
    if matches!(
        orchestration
            .as_ref()
            .and_then(|outcome| outcome.selection.as_ref()),
        Some(selection) if !selection.delegate
    ) {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(
            "The orchestrator decided this request needs no sub-agents. Answer directly; delegate only if the request clearly requires it.",
        );
    }

    // ── Memory guidance ─────────────────────────────────────────────────────
    // Explicit instructions so conclusions are reliably persisted via the
    // `memory` tool (the tool description carries the full scope/usage details).
    if config.effective_tools().iter().any(|t| t == "memory") {
        let evolution_hint = if config.self_evolution_mode {
            ", `evolution` for verified facts that must survive across sessions (runtime \
             environment, correct tool usage, file locations)"
        } else {
            ""
        };
        let memory_guidance = format!(
            "\
        MEMORY: You have a `memory` tool (scopes: `session`, `user`, `repo`{}). \
        Nothing is preloaded — look memory up on demand: search it when you hit a problem \
        you may have solved before, or need a verified environment/tool fact. \
        When you reach a definitive, accurate conclusion (a resolved question, confirmed \
        root cause, decided approach, verified fact), `memory add` it before ending your \
        turn: `repo` for codebase facts and decisions, `user` for preferences and general \
        insights, `session` for in-progress notes. Short descriptive `key`, conclusion in \
        `content`.",
            evolution_hint
        );
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&memory_guidance);
    }

    // ── Timer guidance ──────────────────────────────────────────────────────
    // The tool description carries the mechanics; this states the *policy* so
    // the model waits asynchronously instead of burning a turn on polling.
    if config.selected_tools.iter().any(|t| t == "timer") {
        let timer_guidance = "\
        TIMER: `timer_set` / `timer_list` / `timer_cancel` cover work that cannot finish within this \
        turn. When the next step depends on something slow (log collection, build, download, \
        deployment, external job, rate-limit window), never busy-wait, `sleep` or poll in a loop: \
        start the slow work, arm a timer whose `message` restates what to do when it fires (that turn \
        sees only the history plus that message), tell the user when it will fire and END YOUR TURN. \
        Check `timer_list` first to avoid duplicates, `timer_cancel` when the wait is no longer needed.";
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(timer_guidance);
    }

    // ── Todo guidance ───────────────────────────────────────────────────────
    // The tool description carries the mechanics; this states the *policy* —
    // check the session's list before planning so work continues from the
    // existing plan instead of silently starting a new one.
    if config
        .selected_tools
        .iter()
        .any(|t| t == "todo_list" || t == "todo")
    {
        let todo_guidance = "\
        TODOS: the `todos` tool holds this session's plan. Before planning or starting \
        non-trivial work, `list` it — the active list is also injected as internal context \
        while it has unfinished items. Continue from an existing list and move items \
        `pending` → `in_progress` → `completed` as you work; `add` items when a new \
        multi-step task begins. The UI mirrors every change live, so never leave stale \
        items behind. \
        GOAL LOOP: while the list has unfinished items, a final answer does NOT end the \
        turn — it is continued automatically so you keep working; do not write a final \
        summary with goals open. End the turn explicitly by writing [goal:DONE] in your \
        final answer, either when the goal is fully reached or when you are blocked and \
        need the user (state plainly what is blocking you).";
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(todo_guidance);
    }

    // ── Embedded sub-agent roles (`+agent` in the composer) ─────────────────
    // A snapshot written when the session's first message was sent. It is
    // appended at the END of the stable system prefix, so it stays
    // byte-identical on every turn (prompt-cache friendly) and never reorders
    // the blocks above. A missing table/row simply means "no roles".
    let embedded_roles = state
        .db
        .lock()
        .map(|db| db::get_embedded_roles(&db, &session_id))
        .unwrap_or_default();
    let embedded_block = render_embedded_roles(&embedded_roles);
    if !embedded_block.is_empty() {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&embedded_block);
    }

    // ── Message layout (prompt-cache conscious) ─────────────────────────────
    // Static prefix first (system prompt, skill context), then the persisted
    // summary (stable across turns — only a new compression rewrites it), then
    // the append-only conversation history. Dynamic per-turn blocks (todo
    // list, mission report) come last, right before the latest user message,
    // so turn-to-turn changes invalidate as little cached prefix as possible.
    if !system_content.is_empty() {
        all_messages.push(json!({ "role": "system", "content": system_content }));
    }

    if !loaded_skills_content.is_empty() {
        let workspace_dir = state.workspace_dir.lock().unwrap().clone();
        let workspace_root_display = workspace_dir.display().to_string();
        // Generic sandbox rules live in the tool descriptions (always sent);
        // only skill-specific notes are repeated here.
        let active_skills_context = format!(
            "INTERNAL CONTEXT - ACTIVE SKILLS (root: {workspace_root_display}). \
             Local file tools are sandboxed to the workspace root; `skill_read` is locked to each \
             skill's own directory (`..` rejected); skill edits are limited to `./.data/skills/...` \
             and create `.bak.<n>` backups automatically; MCP tool paths are NOT sandboxed — treat \
             them as opaque. Reuse exact paths returned by tools; never reference app-data or \
             external absolute paths.\n\n\
             Active skill instructions:{}",
            loaded_skills_content
        );
        all_messages.push(json!({ "role": "user", "content": active_skills_context }));
    }

    // Conversation history, keeping the latest user message for the tail.
    let (mut history, last_user_msg) = match messages.last() {
        Some(m) if m.role == "user" => (&messages[..messages.len() - 1], Some(m)),
        _ => (&messages[..], None),
    };

    // ── Persisted session summary: replace the history it covers ────────────
    // A summary written by an earlier compression records how many frontend
    // messages it covers plus the flattened prefix of the last one (the
    // boundary). When the incoming history still matches that boundary, the
    // covered prefix is dropped and the summary stands in for it — Claude
    // Code's compact semantics, where the summary REPLACES the old transcript
    // instead of being replayed on top of it. On any mismatch (history edited,
    // truncated, restored differently) the stale summary is discarded and the
    // full history is replayed — the old, always-safe additive behavior.
    let mut summary_block: Option<db::SessionSummaryRow> = None;
    let mut summary_coverage = CoverageState::default();
    if let Ok(db_guard) = state.db.lock() {
        match db::get_session_summary(&db_guard, &session_id) {
            Ok(Some(saved)) if !saved.body.trim().is_empty() => {
                let covered = usize::try_from(saved.covered_messages).unwrap_or(0);
                let mut valid = true;
                if covered > 0 {
                    let boundary_message_text = covered
                        .checked_sub(1)
                        .and_then(|idx| history.get(idx))
                        .map(|m| content_to_text(&m.content));
                    match summary_trim_count(
                        covered,
                        &saved.boundary_text,
                        boundary_message_text.as_deref(),
                    ) {
                        Some(count) => {
                            history = &history[count..];
                            summary_coverage.offset = count;
                            summary_coverage.boundary = saved.boundary_text.clone();
                        }
                        None => valid = false,
                    }
                }
                if valid {
                    summary_block = Some(saved);
                } else {
                    let _ = db::delete_session_summary(&db_guard, &session_id);
                    log_event(
                        &state,
                        "INFO",
                        format!(
                            "stale session summary discarded (history boundary mismatch): session_id={session_id}"
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    // The summary goes BEFORE the remaining history: it is stable across
    // turns (only a new compression rewrites it), so everything after it is
    // an append-only region — the best layout for the prompt cache. Claude
    // Code places the compact summary at the start of the continuation for
    // the same reason.
    if let Some(saved) = &summary_block {
        all_messages.push(json!({
            "role": "user",
            "content": render_summary_message_body(&saved.body)
        }));
    }

    for m in history {
        let mut msg = json!({ "role": m.role, "content": m.content });
        if let Some(rc) = &m.reasoning_content {
            if !rc.is_empty() {
                msg["reasoning_content"] = json!(rc);
            }
        }
        all_messages.push(msg);
    }

    // ── Dynamic blocks (change between turns → placed near the tail) ────────

    // Inject the active todo list (if any) so the assistant can always
    // re-orient itself to the current plan, even mid-turn.
    // The same snapshot is pushed to the UI: the panel is remounted per
    // assistant message and only the newest one mirrors the session list, so
    // without it a list created in an earlier turn would stay invisible until
    // the model happened to touch it again.
    let todo_snapshot = match state.workspace_dir.lock() {
        Ok(workspace_guard) => todos::active_list_snapshot(&workspace_guard, &session_id)
            .ok()
            .flatten(),
        Err(_) => None,
    };
    if let Some(summary) = &todo_snapshot {
        if !summary.todos.is_empty() {
            // Mirror the very same snapshot to the UI. Emitted together with
            // the prompt block on purpose, and also for a finished list: the
            // panel must be able to disappear (or never appear) instead of
            // lingering at 100% from an earlier turn.
            let _ = app.emit(
                "todo-state",
                json!({
                    "type": "todo_state",
                    "list_id": summary.list_id,
                    "session_id": summary.session_id,
                    "summary": summary,
                }),
            );
        }
        // Nothing left to track → no prompt block either.
        if let Some(todo_block) = todos::render_list_block(summary) {
            all_messages.push(json!({
                "role": "system",
                "content": format!("{CONTEXT_TODO_MARKER}\n{}", todo_block)
            }));
        }
    }

    // Inject the finished multi-agent mission report so the main agent can
    // define the final answer with its full context and tools.
    if let Some(report) = orchestration
        .as_ref()
        .and_then(|outcome| outcome.report.as_ref())
    {
        all_messages.push(json!({
            "role": "system",
            "content": format!("{CONTEXT_MISSION_MARKER}\n{report}")
        }));
    }

    if let Some(m) = last_user_msg {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
    // Streaming client: no total timeout (a long thinking turn must not die at
    // 300 s), liveness via read_timeout instead. See `build_stream_client`.
    let client = build_stream_client()?;
    // Resolved once per turn so every round and retry of this session carries
    // the same conversation id (and the same custom headers).
    let request_headers = upstream_request_headers(&config, &session_id);

    let active_model = model_override
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| config.model.clone());

    // Everything the context compressor needs; created once per turn so the
    // summary call uses the same model, headers and cancel flag as the chat.
    let compressor = ContextCompressor {
        app: &app,
        client: &client,
        url: &url,
        api_key: &config.api_key,
        model: &active_model,
        ds_format: config.ds_format_for(&active_model),
        headers: &request_headers,
        cancelled: turn_cancelled.as_ref(),
    };
    // The reactive (context-overflow) compression may retry the request once
    // per turn; the flag keeps that retry from looping.
    let mut reactive_compress_used = false;
    // Set when a predictive compression ran but the estimate stayed above the
    // threshold — the static payload (system prompt + tool definitions) is
    // itself near the cap, so further pre-checks would only burn summary
    // calls. Reactive / post-response paths still cover the rest of the turn.
    let mut predictive_compress_saturated = false;

    let (mut tools_list, mut mcp_tool_map, mut tools_revision) = build_turn_tools(
        &state,
        &config,
        &active_skill_dirs,
        !skill_ids.is_empty(),
        skill_evolution_root.as_deref(),
        &enabled_agents,
    )
    .await;

    // Track the final answer and whether the LLM already persisted memory via
    // the `memory` tool, so the automatic fallback doesn't duplicate it.
    let mut final_answer: Option<String> = None;
    let mut memory_tool_called = false;

    // Skills loaded dynamically during this turn (the model can request the
    // same skill again in a later tool-call round of the same turn).
    let mut loaded_this_turn: std::collections::HashSet<String> = std::collections::HashSet::new();
    // How many times this turn has been held open for background results.
    let mut background_hold_rounds: u32 = 0;
    // Goal-loop continuations this turn (todo list still unfinished).
    let mut goal_nudges: u32 = 0;
    // Continuations after a `finish_reason: "length"` truncation.
    let mut length_continues: u32 = 0;
    // The goal loop only applies while the todos tool is part of the payload
    // (same condition as the TODOS guidance block above).
    let todos_tool_enabled = config
        .selected_tools
        .iter()
        .any(|t| t == "todo_list" || t == "todo");

    // ── Turn budget & tool failure guard ────────────────────────────────────
    // The loop below is model-driven: it repeats for as long as the reply
    // carries tool calls. `TurnBudget` bounds that (rounds + tool calls) and
    // `ToolFailureGuard` disables a tool for the rest of the session after
    // repeated failures. Both limits come from Settings → Advanced; `0` means
    // "no limit". See `turn_guard`.
    let mut budget = turn_guard::TurnBudget::new(
        config.main_agent_max_rounds,
        config.main_agent_max_tool_calls,
    );
    let failure_limit = config
        .tool_failure_limit
        .min(turn_guard::MAX_TOOL_FAILURE_LIMIT);
    let guard = &state.tool_guard;
    // `budget_stop` is set once a limit is spent; `wrap_up` marks the single
    // final request that goes out with `tool_choice: "none"`.
    let mut budget_stop: Option<turn_guard::BudgetStop> = None;
    let mut wrap_up = false;

    // ── Tools disabled by earlier turns ─────────────────────────────────────
    // The failure guard remembers its verdict per session, so a tool that hit
    // the limit in an earlier turn is not offered again. The snapshot is taken
    // ONCE per turn on purpose: `tools` sits in front of the messages in the
    // rendered prompt, so adding or removing a definition mid-turn would move
    // every following token and throw away the KV / prefix cache the previous
    // rounds built. Tools that get disabled *during* this turn therefore stay
    // advertised until the next turn; their calls are refused in the tool loop
    // below (`blocked_notice`) instead of being hidden.
    let disabled_at_turn_start = guard.disabled_tools(&session_id);
    let drop_disabled_tools = |list: &mut Vec<Value>| {
        if disabled_at_turn_start.is_empty() {
            return;
        }
        list.retain(|tool| {
            tool["function"]["name"]
                .as_str()
                .map(|name| !disabled_at_turn_start.contains(name))
                .unwrap_or(true)
        });
    };
    drop_disabled_tools(&mut tools_list);

    loop {
        // ── Stop pressed while the last round was winding down? ──────────────
        // The cancel flag is only polled inside the tool loop and by the
        // stream, so a turn that was executing a foreground tool (or sat in a
        // confirmation wait) can reach this boundary seconds after the user
        // pressed Stop. Ending here, BEFORE the request-boundary drains below,
        // is what keeps a dead turn from announcing deliveries the model will
        // never see: the drains emit `chat-injected`, which is heard by
        // whatever conversation the user has opened in the meantime. The
        // queued injections are dropped with the window (`InjectionWindow`)
        // and the UI re-sends them as a normal next-turn message.
        if turn_cancelled.load(Ordering::SeqCst) {
            break;
        }

        // ── MCP registry changed during this turn? ───────────────────────────
        // `mcp_self` can create and enable a server while the turn is running.
        // Rebuilding the payload before the next request is what makes the new
        // tools callable *now*; without it the model would have to wait for the
        // next conversation turn, which reads like "the server was created but
        // its tools do not exist".
        let registry_revision = state.mcp_revision();
        if registry_revision != tools_revision {
            let rebuilt = build_turn_tools(
                &state,
                &config,
                &active_skill_dirs,
                !skill_ids.is_empty(),
                skill_evolution_root.as_deref(),
                &enabled_agents,
            )
            .await;
            tools_list = rebuilt.0;
            mcp_tool_map = rebuilt.1;
            tools_revision = rebuilt.2;
            // A rebuild resurrects every tool the session disabled in an
            // earlier turn; the snapshot taken at turn start still applies.
            drop_disabled_tools(&mut tools_list);
            log_event(
                &state,
                "INFO",
                format!(
                    "MCP tool payload rebuilt mid-turn: session_id={}, revision={tools_revision}, tools={}",
                    session_id,
                    tools_list.len()
                ),
            );
        }

        // Sanitize tool call pairs before each request to prevent the API error:
        // "Messages with role 'tool' must be a response to a preceding message with 'tool_calls'"
        sanitize_tool_pairs(&mut all_messages);

        // ── Mid-turn injections ──────────────────────────────────────────────
        // The user can append new information while the assistant is working
        // (external state changed, a wrong path, a new constraint). A request
        // boundary is the only safe place: an `assistant` message carrying
        // `tool_calls` must stay adjacent to its `tool` results, so the injected
        // text goes AFTER the whole round, right before the next request.
        //
        // A turn whose last request was the final answer never reaches another
        // boundary — those items stay in the inbox, the window closes with the
        // turn, and the UI sends the text as a normal next-turn message.
        let injections = state.drain_turn_injections(injection_window.turn_id);
        if !injections.is_empty() {
            for item in &injections {
                all_messages.push(json!({
                    "role": "user",
                    "content": format!("{}\n\n{}", MID_TURN_INJECTION_PREFIX, item.text),
                }));
            }
            log_event(
                &state,
                "INFO",
                format!(
                    "mid-turn injection delivered: session_id={}, count={}",
                    session_id,
                    injections.len()
                ),
            );
            // Only now is the text actually part of a request. The UI keeps
            // showing an item as "queued" until this event lands, so anything
            // that never reached a request is re-sent as a normal turn instead
            // of vanishing.
            //
            // `session_id` lets the UI route the event: a turn that outlives
            // the conversation it started in (a foreground tool still running
            // after Stop, a confirmation timeout) must not announce its
            // deliveries inside another chat.
            let _ = app.emit(
                "chat-injected",
                json!({
                    "session_id": session_id,
                    "items": injections
                        .iter()
                        .map(|item| json!({
                            "id": item.id,
                            "text": item.text,
                            "created_at_ms": item.created_at_ms,
                        }))
                        .collect::<Vec<_>>(),
                }),
            );
        }

        // ── Background task results ──────────────────────────────────────────
        // A background tool call returned a placeholder immediately (the
        // protocol has no slot for a late `tool` message). Its real output is
        // delivered here, at a request boundary, as a `user` message — and
        // announced with `chat-injected` so the UI stores it on the assistant
        // row and replays it in later turns exactly like a mid-turn injection.
        let background_results = state.background_tasks.take_deliverable(&session_id);
        if !background_results.is_empty() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or(0);
            let mut items = Vec::with_capacity(background_results.len());
            for delivery in &background_results {
                let text = format_background_delivery(delivery);
                all_messages.push(json!({
                    "role": "user",
                    "content": text.clone(),
                }));
                items.push(json!({
                    "id": delivery.id,
                    "text": text,
                    "created_at_ms": now_ms,
                    "kind": "background_result",
                }));
            }
            log_event(
                &state,
                "INFO",
                format!(
                    "background results delivered: session_id={}, count={}",
                    session_id,
                    background_results.len()
                ),
            );
            let _ = app.emit(
                "chat-injected",
                json!({ "session_id": session_id, "items": items }),
            );
        }

        // ── Turn budget ──────────────────────────────────────────────────────
        // Count the request that is about to be sent. When the round or
        // tool-call budget is spent the turn does not die mid-flight: one last
        // request goes out with `tool_choice: "none"`, so the model has to
        // answer with what it already collected instead of asking for more.
        if budget_stop.is_none() {
            budget_stop = budget.begin_round();
        }
        if let Some(stop) = budget_stop {
            if !wrap_up {
                wrap_up = true;
                let (rounds, tool_calls) = budget.snapshot();
                let reason = if stop == turn_guard::BudgetStop::ToolCalls {
                    "the tool-call limit was reached"
                } else {
                    "the round limit was reached"
                };
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "turn budget exhausted: session_id={}, reason={}, rounds={}, tool_calls={}",
                        session_id,
                        stop.as_str(),
                        rounds,
                        tool_calls
                    ),
                );
                let _ = app.emit(
                    "chat-notice",
                    json!({
                        "kind": "budget_exhausted",
                        "session_id": session_id,
                        "reason": stop.as_str(),
                        "rounds": rounds,
                        "tool_calls": tool_calls,
                    }),
                );
                // Appended at the tail as a `user` message: the prompt prefix
                // stays byte-identical (only new tokens are uncached), and no
                // strict chat template has to accept a `system` turn appearing
                // after `tool` results.
                all_messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "{CONTEXT_BUDGET_MARKER}\nThe tool budget for this turn is spent \
({rounds} request(s), {tool_calls} tool call(s); {reason}). Do not call any tool. Answer now with \
the best final answer you can give from the material already collected, and state plainly what is \
still missing."
                    ),
                }));
            }
        }

        // Context window used for the usage gauge (`Tokens: X / Y`) and the
        // compression threshold. Same resolver as sub-agents: only the
        // per-model window counts, never an output-token cap.
        let effective_max_tokens =
            resolve_context_window(&config, &active_model, DEFAULT_MODEL_MAX_TOKENS);

        // ── Predictive compression pre-check ────────────────────────────────
        // The post-response ratio check can only fire once a request came
        // back with usage figures. A single request carrying large tool
        // results can still jump past the window and be rejected outright.
        // Estimate the prompt size (≈4 chars/token, tools payload included)
        // and compress BEFORE sending when the estimate approaches the cap.
        if !predictive_compress_saturated {
            let estimated_prompt_tokens = estimate_prompt_tokens(&all_messages, &tools_list);
            let precheck_threshold =
                (effective_max_tokens as f32 * CONTEXT_PRECHECK_RATIO) as usize;
            if estimated_prompt_tokens >= precheck_threshold {
                if let Some(outcome) = compressor
                    .compress(
                        &mut all_messages,
                        CONTEXT_KEEP_RECENT_MESSAGES,
                        &summary_coverage,
                    )
                    .await
                {
                    persist_session_summary(&state, &session_id, &outcome);
                    summary_coverage.offset = outcome.covered_messages;
                    summary_coverage.boundary = outcome.boundary_text;
                    log_event(
                        &state,
                        "INFO",
                        format!(
                            "predictive context compression: session_id={}, estimated_prompt_tokens={}, max_tokens={}",
                            session_id, estimated_prompt_tokens, effective_max_tokens
                        ),
                    );
                    // If the estimate is still above the threshold after
                    // summarizing everything summarizable, the remaining bulk
                    // is static — stop pre-checking this turn.
                    if estimate_prompt_tokens(&all_messages, &tools_list) >= precheck_threshold {
                        predictive_compress_saturated = true;
                        log_event(
                            &state,
                            "WARN",
                            format!(
                                "predictive compression saturated (static payload dominates the window): session_id={session_id}"
                            ),
                        );
                    }
                }
            }
        }

        let mut req_body = json!({
            "model": active_model,
            "messages": all_messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        // Never strip `tools` from a request. The array is rendered in front of
        // the messages, so removing it would move every following token and
        // throw away the whole prompt cache for this request. The wrap-up
        // request keeps the identical list and only flips `tool_choice` to
        // "none", which forbids calling them while the prefix stays cached.
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
            req_body["tool_choice"] = if wrap_up {
                json!("none")
            } else {
                json!("auto")
            };
        }
        if let Some(temp) = config.model_settings.temperature {
            req_body["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.model_settings.top_p {
            req_body["top_p"] = json!(top_p);
        }
        if let Some(effort) = resolve_reasoning_effort(&config, &active_model) {
            req_body["reasoning_effort"] = json!(effort);
        }

        apply_completion_token_limit(
            &mut req_body,
            config.model_settings.max_complete_tokens,
            config.model_settings.max_tokens,
        );

        let started_at = std::time::Instant::now();
        let request_snapshot = req_body.clone();

        let result = stream_request(
            &app,
            &client,
            &url,
            &config.api_key,
            req_body,
            &turn_cancelled,
            Some(effective_max_tokens),
            config.ds_format_for(&active_model),
            &request_headers,
            Some(&session_id),
        )
        .await;

        let duration_ms = started_at.elapsed().as_millis() as i64;

        match &result {
            Ok(sr) => {
                // Log successful request
                let tool_calls_json = if sr.tool_calls.is_empty() {
                    String::new()
                } else {
                    serde_json::to_string(
                        &sr.tool_calls
                            .iter()
                            .map(|(id, name, args)| {
                                json!({
                                    "id": id, "name": name, "arguments": args
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap_or_default()
                };
                let db = state.db.lock().unwrap();
                // `request_body` is deliberately not written: the full prompt is
                // already recorded once in `interaction_log.input_data`, and
                // storing a second copy doubled the database size
                // (~92 KiB per request). This table keeps the metrics.
                let _ = db.execute(
                    "INSERT INTO api_requests (session_id, model, response_content, reasoning_content, tool_calls, finish_reason, prompt_tokens, completion_tokens, cached_tokens, reasoning_ms, first_token_ms, duration_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        session_id,
                        active_model.as_str(),
                        sr.content,
                        sr.reasoning_content,
                        tool_calls_json,
                        sr.finish_reason,
                        sr.prompt_tokens,
                        sr.completion_tokens,
                        sr.cached_tokens.map(i64::from),
                        sr.timing.reasoning_ms.map(i64::from),
                        sr.timing.first_token_ms.map(i64::from),
                        duration_ms,
                    ],
                );
                // Log to interaction_log table
                let request_body_display = request_snapshot.to_string();
                let response_display = sr.content.clone();
                // Cache-hit metrics travel in the interaction metadata so the
                // Monitor detail view (and the list chip) can show them without
                // touching the interaction schema. The round's streamed phase
                // timings ride along so the detail pane can explain the wait.
                let usage_metadata = {
                    let mut meta = serde_json::Map::new();
                    if let Some(cached) = sr.cached_tokens {
                        meta.insert("prompt_tokens".into(), json!(sr.prompt_tokens));
                        meta.insert("cached_tokens".into(), json!(cached));
                        meta.insert(
                            "hit_ratio".into(),
                            json!(if sr.prompt_tokens > 0 {
                                ((cached as f64) / (sr.prompt_tokens as f64)).clamp(0.0, 1.0)
                            } else {
                                0.0
                            }),
                        );
                    }
                    if let Some(ms) = sr.timing.reasoning_ms {
                        meta.insert("reasoning_ms".into(), json!(ms));
                    }
                    if let Some(ms) = sr.timing.first_token_ms {
                        meta.insert("first_token_ms".into(), json!(ms));
                    }
                    (!meta.is_empty()).then(|| Value::Object(meta).to_string())
                };
                log_interaction(
                    &db,
                    &session_id,
                    "llm_api",
                    &active_model,
                    "chat_completion",
                    request_body_display,
                    response_display,
                    None,
                    duration_ms,
                    usage_metadata,
                );
                drop(db);
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "chat_completion request succeeded: session_id={}, finish_reason={}, duration_ms={}",
                        session_id, sr.finish_reason, duration_ms
                    ),
                );
            }
            Err(err) => {
                // Log failed request (metrics only — see the success branch for
                // why the prompt body is not duplicated here).
                let db = state.db.lock().unwrap();
                let _ = db.execute(
                    "INSERT INTO api_requests (session_id, model, finish_reason, duration_ms, error) \
                     VALUES (?1, ?2, 'error', ?3, ?4)",
                    rusqlite::params![
                        session_id,
                        active_model.as_str(),
                        duration_ms,
                        err,
                    ],
                );
                // Log to interaction_log table
                let request_body_display = request_snapshot.to_string();
                log_interaction(
                    &db,
                    &session_id,
                    "llm_error",
                    &active_model,
                    "chat_completion",
                    request_body_display,
                    String::new(),
                    Some(err.clone()),
                    duration_ms,
                    None,
                );
                drop(db);
                log_event(
                    &state,
                    "ERROR",
                    format!(
                        "chat_completion request failed: session_id={}, duration_ms={}, error={}",
                        session_id, duration_ms, err
                    ),
                );
            }
        }

        let sr = match result {
            Ok(sr) => sr,
            // A rejected wrap-up request (some gateways refuse
            // `tool_choice: "none"`) must not fail the whole turn: close it with
            // the answer the model already produced instead of an error.
            Err(err) if wrap_up => {
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "wrap-up request failed, ending the turn: session_id={}, error={err}",
                        session_id
                    ),
                );
                final_answer = Some(BUDGET_FALLBACK_ANSWER.to_string());
                break;
            }
            Err(err) => {
                // ── Reactive compression (context-overflow recovery) ────────
                // Last line of defense, mirroring Claude Code's 413 recovery:
                // a provider that hard-rejects an over-window request instead
                // of streaming an answer. Compress with a smaller keep-window
                // and retry once — the flag keeps the retry from looping.
                if !reactive_compress_used && is_context_overflow_error(&err) {
                    reactive_compress_used = true;
                    log_event(
                        &state,
                        "WARN",
                        format!(
                            "context overflow from provider, compressing and retrying once: session_id={}, error={err}",
                            session_id
                        ),
                    );
                    if let Some(outcome) = compressor
                        .compress(
                            &mut all_messages,
                            CONTEXT_REACTIVE_KEEP_RECENT_MESSAGES,
                            &summary_coverage,
                        )
                        .await
                    {
                        persist_session_summary(&state, &session_id, &outcome);
                        summary_coverage.offset = outcome.covered_messages;
                        summary_coverage.boundary = outcome.boundary_text;
                        continue;
                    }
                }
                return Err(err);
            }
        };

        let total_tokens = sr.prompt_tokens.saturating_add(sr.completion_tokens);
        let ratio = total_tokens as f32 / effective_max_tokens as f32;
        let mut compressed_this_turn = false;
        if ratio >= CONTEXT_COMPRESSION_THRESHOLD {
            if let Some(outcome) = compressor
                .compress(
                    &mut all_messages,
                    CONTEXT_KEEP_RECENT_MESSAGES,
                    &summary_coverage,
                )
                .await
            {
                persist_session_summary(&state, &session_id, &outcome);
                summary_coverage.offset = outcome.covered_messages;
                summary_coverage.boundary = outcome.boundary_text;
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "session context compressed: session_id={}, total_tokens={}, max_tokens={}, ratio={:.3}, summary_covers={} message(s)",
                        session_id, total_tokens, effective_max_tokens, ratio, outcome.covered_messages
                    ),
                );
                let _ = app.emit(
                    "chat-token",
                    "\n\n[System] Context compressed (older messages summarized) and persisted for next turns.\n\n",
                );
                compressed_this_turn = true;
            }
        }

        if sr.finish_reason == "cancelled" {
            break;
        }

        // ── Tool-round determination ─────────────────────────────────────────
        // A round is authoritative in two dialects: the standard
        // `finish_reason: "tool_calls"`, and relays that stream the calls but
        // close the chunk stream with "stop" (or no finish_reason at all).
        // Dropping the accumulated calls on the non-standard dialects ended
        // the turn mid-task without a tool ever running — execute the round
        // instead, and log when the dialect was non-standard so the relay can
        // be identified in app.log. "length" is excluded: cut-off arguments
        // are regenerated by the continuation below, never executed.
        let tool_round = !sr.tool_calls.is_empty()
            && (sr.finish_reason == "tool_calls"
                || sr.finish_reason == "stop"
                || sr.finish_reason.is_empty());
        if tool_round && sr.finish_reason != "tool_calls" {
            log_event(
                &state,
                "WARN",
                format!(
                    "tool calls arrived with finish_reason '{}', executing the round anyway: session_id={}, calls={}",
                    if sr.finish_reason.is_empty() { "(none)" } else { &sr.finish_reason },
                    session_id,
                    sr.tool_calls.len()
                ),
            );
        }

        // ── Wrap-up answer ───────────────────────────────────────────────────
        // The budget was spent before this request, so it went out with
        // `tool_choice: "none"`: whatever comes back is the answer for this
        // turn. Never hold the turn open again for background results here —
        // that is what `background_hold_rounds` would otherwise do.
        if wrap_up {
            final_answer = Some(if sr.content.trim().is_empty() {
                BUDGET_FALLBACK_ANSWER.to_string()
            } else {
                sr.content.clone()
            });
            break;
        }

        // ── Output-truncation continuation ──────────────────────────────────
        // `finish_reason: "length"` means the provider cut the response at the
        // output token cap — with thinking models that cap includes the
        // reasoning, so the answer can be empty or cut mid-sentence, and
        // streamed tool-call arguments can be truncated. Ending the turn here
        // silently delivered half an answer; a bounded continuation lets the
        // model pick up where it stopped. Truncated tool calls are never
        // executed (see `tool_round`); the model regenerates them.
        if sr.finish_reason == "length" {
            if length_continues < MAX_LENGTH_CONTINUES {
                length_continues += 1;
                if !sr.content.is_empty() {
                    let mut assistant_msg = json!({
                        "role": "assistant",
                        "content": sr.content,
                    });
                    if !sr.reasoning_content.is_empty() {
                        assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
                    }
                    all_messages.push(assistant_msg);
                }
                all_messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "{CONTEXT_LENGTH_MARKER}\nThe previous response was cut off by the output token limit \
(finish_reason \"length\"{}). Continue from exactly where it stopped — do not repeat what is already written.",
                        if sr.tool_calls.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "; {} truncated tool call(s) were discarded and must be re-issued completely",
                                sr.tool_calls.len()
                            )
                        }
                    ),
                }));
                let _ = app.emit(
                    "chat-token",
                    format!(
                        "\n\n[System] Output hit the token limit (continuation {length_continues}/{MAX_LENGTH_CONTINUES}) — continuing.\n\n"
                    ),
                );
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "output truncated by max_tokens, continuing: session_id={}, continuation={length_continues}, content_chars={}",
                        session_id,
                        sr.content.chars().count()
                    ),
                );
                continue;
            }
            let _ = app.emit(
                "chat-token",
                "\n\n[System] Output hit the token limit again — ending the turn with the truncated answer.\n\n",
            );
            log_event(
                &state,
                "WARN",
                format!(
                    "output truncated by max_tokens, continuation budget spent: session_id={session_id}"
                ),
            );
            // Fall through: the final-answer branch ends the turn with what
            // the model managed to produce.
        }

        // After compression, the previous response was generated against the
        // full (uncompressed) context. If that response was a final answer
        // (no tool calls), the task is *not* yet finished — re-enter the loop
        // so the LLM can re-evaluate against the compressed context and
        // continue. When the response carries tool calls, fall through to
        // execute them; the next iteration will naturally use the compressed
        // context.
        if compressed_this_turn && !tool_round {
            if !sr.content.is_empty() {
                let mut assistant_msg = json!({
                    "role": "assistant",
                    "content": sr.content,
                });
                if !sr.reasoning_content.is_empty() {
                    assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
                }
                all_messages.push(assistant_msg);
            }
            all_messages.push(json!({
                "role": "user",
                "content": format!(
                    "{CONTEXT_CONTINUE_MARKER}\nContext was just compressed to free up space. Review the session summary and your previous response, then continue the task if it is not yet complete; otherwise provide your final answer."
                ),
            }));
            log_event(
                &state,
                "INFO",
                format!(
                    "context compressed — re-calling LLM to continue task: session_id={}",
                    session_id
                ),
            );
            continue;
        }

        if !tool_round {
            // A background task started earlier in this turn may produce its
            // result only after this answer was generated. Delivering a final
            // answer that ignores that output would be wrong, so the turn is
            // held open: the text is kept as an interim assistant message, the
            // results are drained at the top of the next iteration and the
            // model gets one more request to incorporate them.
            if background_hold_rounds < BACKGROUND_HOLD_MAX_ROUNDS
                && !turn_cancelled.load(Ordering::SeqCst)
                && state.background_tasks.unsettled(&session_id) > 0
            {
                let pending = state.background_tasks.unsettled(&session_id);
                if !sr.content.is_empty() {
                    let mut assistant_msg = json!({
                        "role": "assistant",
                        "content": sr.content,
                    });
                    if !sr.reasoning_content.is_empty() {
                        assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
                    }
                    all_messages.push(assistant_msg);
                }
                background_hold_rounds += 1;
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "holding turn for background tasks: session_id={}, pending={}, round={}",
                        session_id, pending, background_hold_rounds
                    ),
                );
                let _ = app.emit(
                    tools::background::BACKGROUND_WAITING_EVENT,
                    json!({
                        "session_id": session_id,
                        "pending": pending,
                        "round": background_hold_rounds,
                    }),
                );
                state
                    .background_tasks
                    .wait_until_idle(&session_id, Duration::from_secs(BACKGROUND_HOLD_WAIT_SECS))
                    .await;
                continue;
            }

            if state.background_tasks.unsettled(&session_id) > 0 {
                // Held as long as allowed: drop the rest instead of hanging
                // the turn forever.
                let cancelled = state.background_tasks.cancel_session(&session_id);
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "background hold budget exhausted: session_id={}, cancelled={}",
                        session_id,
                        cancelled.len()
                    ),
                );
                // Tell the UI now: these tasks never get a runner report the
                // user could see (the turn is ending right after).
                for snapshot in cancelled {
                    let _ = app.emit(
                        tools::background::BACKGROUND_TASK_STATE_EVENT,
                        snapshot,
                    );
                }
            }

            // ── Goal loop ────────────────────────────────────────────────────
            // With the todos tool enabled, a final answer does not end the
            // turn while the session's list still has unfinished items: the
            // model is nudged back to work (bounded), and it ends the turn
            // explicitly by writing [goal:DONE] into its final answer —
            // either because the goal is fully reached, or because it is
            // blocked and needs the user.
            if todos_tool_enabled
                && !sr.content.contains(GOAL_DONE_MARKER)
                && goal_nudges < GOAL_LOOP_MAX_NUDGES
            {
                let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                let fresh_list = todos::active_list_snapshot(&workspace_dir, &session_id)
                    .ok()
                    .flatten();
                let unfinished = fresh_list
                    .as_ref()
                    .map(|list| list.pending + list.in_progress > 0)
                    .unwrap_or(false);
                if unfinished {
                    goal_nudges += 1;
                    if !sr.content.is_empty() {
                        let mut assistant_msg = json!({
                            "role": "assistant",
                            "content": sr.content,
                        });
                        if !sr.reasoning_content.is_empty() {
                            assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
                        }
                        all_messages.push(assistant_msg);
                    }
                    // Re-render from the FRESH snapshot: the model may have
                    // just completed items through the todos tool, so the
                    // block injected at turn start is stale.
                    let list_block = fresh_list
                        .as_ref()
                        .and_then(|list| todos::render_list_block(list))
                        .unwrap_or_else(|| {
                            "The active todo list still has unfinished items.".to_string()
                        });
                    all_messages.push(json!({
                        "role": "user",
                        "content": format!(
                            "{CONTEXT_GOAL_MARKER}\nYour final answer did not end the turn: the todo \
list still has unfinished items.\n\n{list_block}\n\nContinue working on them now — use your tools, \
and update the list (pending → in_progress → completed) as you go. End your reply with [goal:DONE] \
only when every item is done, or when you are blocked and need the user."
                        ),
                    }));
                    let _ = app.emit(
                        "chat-token",
                        format!(
                            "\n\n[System] Todo list still has unfinished items (continuation {goal_nudges}/{GOAL_LOOP_MAX_NUDGES}) — continuing.\n\n"
                        ),
                    );
                    log_event(
                        &state,
                        "INFO",
                        format!(
                            "goal loop: todo list unfinished, continuing: session_id={}, nudge={goal_nudges}",
                            session_id
                        ),
                    );
                    continue;
                }
            }

            // Defensive: `stream_chat_request` already rejects an empty
            // completion, so this is only reachable if that check is ever lost
            // again. Never close the transcript on nothing — the user would be
            // left with the previous partial text and a task that stopped
            // mid-way. Surface it as a failure the retry button can act on.
            if sr.content.trim().is_empty() {
                let err =
                    "the model returned an empty reply, so the turn stopped before producing an answer"
                        .to_string();
                log_event(
                    &state,
                    "WARN",
                    format!("empty final answer, ending the turn: session_id={session_id}"),
                );
                let _ = app.emit("chat-error", err.clone());
                return Err(err);
            }

            final_answer = Some(sr.content.clone());
            break;
        }

        let assistant_tcs: Vec<Value> = sr
            .tool_calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect();
        // The round's narration and its thinking ride along with the calls (see
        // `assistant_tool_message`): dropping them lost the line the model had
        // just told the user.
        all_messages.push(assistant_tool_message(
            &sr.content,
            &sr.reasoning_content,
            assistant_tcs,
        ));

        let mut pending_skill_context_messages: Vec<Value> = vec![];

        for (id, name, args) in &sr.tool_calls {
            if turn_cancelled.load(Ordering::SeqCst) {
                break;
            }
            if name == "memory" {
                memory_tool_called = true;
            }
            let background_args: Value = serde_json::from_str(args).unwrap_or_default();

            // ── Failure guard / tool budget ──────────────────────────────────
            // Two independent gates can keep this call from running:
            //   1. the tool was disabled earlier in this session (its
            //      consecutive-failure budget is spent), or
            //   2. the turn's tool-call budget is spent.
            // Either way the call is still *answered*: the protocol requires a
            // `tool` message for every `tool_call_id` the assistant produced.
            let blocked = match guard.blocked_notice(&session_id, name, failure_limit) {
                Some(notice) => Some(notice),
                None => match budget.record_tool_call() {
                    Some(stop) => {
                        budget_stop = Some(stop);
                        Some(turn_guard::budget_notice(name))
                    }
                    None => None,
                },
            };
            let ran = blocked.is_none();
            if let Some(notice) = &blocked {
                let reason = if notice.starts_with("⛔ TOOL DISABLED") {
                    "disabled for this session"
                } else {
                    "turn tool budget spent"
                };
                let _ = app.emit(
                    "tool-call",
                    json!({
                        "session_id": session_id,
                        "text": format!("🚫 *{name}* — not executed ({reason})"),
                    }),
                );
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "tool call not executed: session_id={}, tool={}, reason={}",
                        session_id, name, reason
                    ),
                );
            }

            let mut result = if let Some(notice) = blocked {
                notice
            } else if name == BACKGROUND_TOOL_NAME {
                handle_background_task_tool(&app, &state, &session_id, &background_args).await
            } else if config.background_tools
                && background_requested(&background_args)
                && name != "call_subagent"
            {
                if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                    let mut mcp_args = background_args.clone();
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
                    match start_background_mcp_call(
                        &app,
                        &session_id,
                        mcp_server.clone(),
                        actual_tool_name.clone(),
                        name,
                        mcp_args,
                        &label,
                    ) {
                        Ok(placeholder) => placeholder,
                        Err(err) => format!("Error: {err}"),
                    }
                } else if background_supported(name, &background_args) {
                    let mut bg_args = background_args.clone();
                    if let Some(object) = bg_args.as_object_mut() {
                        // The real tool must never see the routing flag.
                        object.remove("run_in_background");
                    }
                    let label = background_label(name, &bg_args);
                    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                    match start_background_tool_call(
                        &app,
                        &session_id,
                        name,
                        bg_args,
                        &label,
                        workspace_dir,
                        &config,
                        &skill_allowed_commands,
                        skill_evolution_root.clone(),
                        &active_skill_dirs,
                    ) {
                        Ok(placeholder) => placeholder,
                        Err(err) => format!("Error: {err}"),
                    }
                } else {
                    format!(
                        "Error: `run_in_background` is not supported by the tool '{name}'. \
                         Use it only with `run_cmd`, `run_shell` (run_type \"direct\"), \
                         `file_actions` action \"search\", MCP tools and `call_subagent`."
                    )
                }
            } else if name == "use_skill" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let skill_name = args_json["skill_name"].as_str().unwrap_or("");
                let ws_skills_dir = crate::workspace::skills_root(&workspace_dir_for_roots);

                if let Ok(resolved_skill) =
                    skills::resolve_skill_by_name(&ws_skills_dir, skill_name)
                {
                    let skill = resolved_skill.skill;
                    let dyn_marker = format!("--- Skill (Dynamically Loaded): {} ---", skill.name);

                    // Explicit "already loaded" state: recorded by earlier turns
                    // (`loaded_skills` on the message history) or loaded earlier
                    // in this same turn.
                    let already_loaded = activated_skills.contains(&skill.name)
                        || loaded_this_turn.contains(&skill.name);

                    if !already_loaded {
                        loaded_this_turn.insert(skill.name.clone());
                        merge_allowed_commands(
                            &mut skill_allowed_commands,
                            &skill.allowed_commands,
                        );
                        // Track skill dir for skill_read tool injection
                        if !active_skill_dirs.iter().any(|(n, _)| n == &skill.name) {
                            active_skill_dirs
                                .push((skill.name.clone(), resolved_skill.skill_dir.clone()));
                        }
                        let cmd_constraint = if skill.allowed_commands.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\n[Allowed commands for this skill: {}]\n",
                                skill.allowed_commands.join(", ")
                            )
                        };
                        let skill_context = format!(
                            "INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL (not a user request):\n\
{}\n[Skill root: {}]\n[Skill file: {}]\n{}{}",
                            dyn_marker,
                            resolved_skill.skill_dir.display(),
                            resolved_skill.skill_file.display(),
                            cmd_constraint,
                            skill.system_prompt
                        );
                        pending_skill_context_messages.push(json!({
                            "role": "user",
                            "content": skill_context
                        }));
                        // Structured state event: the UI records this on the
                        // assistant message and sends it back as
                        // `loaded_skills` on the next turn, so a skill is
                        // loaded at most once per session. `session_id` routes
                        // the event: a winding-down turn must not record its
                        // skill inside another chat.
                        let _ = app.emit(
                            "skill-loaded",
                            json!({ "session_id": session_id, "skill_name": skill.name }),
                        );
                        format!(
                            "Skill '{}' loaded. Follow its instructions to fulfill the request.",
                            skill_name
                        )
                    } else {
                        format!("Skill '{}' already loaded.", skill_name)
                    }
                } else {
                    format!("Error: Skill '{}' not found.", skill_name)
                }
            } else if name == "call_subagent" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let agent_id = args_json["agent_id"].as_str().unwrap_or("");
                let task_desc = args_json["task_description"].as_str().unwrap_or("");
                let task_context = args_json["context"].as_str().unwrap_or("");

                if !multi_agent_enabled {
                    "Error: sub-agent delegation is disabled. Enable automatic orchestration (`use_agents`) in the Agents panel to use `call_subagent`.".to_string()
                } else if let Some(agent) = enabled_agents.iter().find(|a| a.id == agent_id) {
                    let task = agents::Task {
                        id: uuid::Uuid::new_v4().to_string(),
                        agent_id: agent_id.to_string(),
                        description: task_desc.to_string(),
                        context: task_context.to_string(),
                        cwd: agents::normalize_task_cwd(args_json["cwd"].as_str()),
                        dependencies: vec![],
                    };
                    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                    // Deliverables are exchanged through `<workspace>/.data/tmp`.
                    // A cloud agent cannot reach the local workspace, so it gets
                    // no exchange directory.
                    let agent_type = agents::normalize_agent_type(&agent.agent_type);
                    let exchange = if matches!(
                        agent_type.as_str(),
                        agents::AGENT_TYPE_LLM | agents::AGENT_TYPE_CLI
                    ) {
                        match agents::exchange::ExchangeArea::create(
                            &workspace_dir,
                            std::slice::from_ref(&task.id),
                        ) {
                            Ok(area) => Some(area),
                            Err(err) => {
                                log_event(
                                    &state,
                                    "WARN",
                                    format!("deliverable exchange area unavailable: {err}"),
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let url = format!(
                        "{}/chat/completions",
                        config.api_base_url.trim_end_matches('/')
                    );
                    match build_stream_client() {
                        Ok(client) => {
                            if config.background_tools && background_requested(&background_args) {
                                let agent = agent.clone();
                                let app_for_task = app.clone();
                                let session_for_task = session_id.clone();
                                let config_for_task = config.clone();
                                let skill_root_for_task = skill_evolution_root.clone();
                                let exchange_for_task = exchange.clone();
                                let label = format!(
                                    "{} · {}",
                                    agent.name,
                                    task_desc.chars().take(100).collect::<String>()
                                );
                                let preview: String =
                                    args_json.to_string().chars().take(300).collect();
                                start_background_task(
                                    &app,
                                    &session_id,
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
                                            &task,
                                            workspace_dir,
                                            exchange_for_task.as_ref(),
                                            skill_root_for_task,
                                            cancel,
                                            None,
                                        )
                                        .await;
                                        format_subagent_delivery(&res, exchange_for_task.as_ref())
                                    },
                                )
                                .unwrap_or_else(|err| format!("Error: {err}"))
                            } else {
                                let res = agents::run_sub_agent(
                                    &app,
                                    &client,
                                    &url,
                                    &config,
                                    &session_id,
                                    agent,
                                    &task,
                                    workspace_dir,
                                    exchange.as_ref(),
                                    skill_evolution_root.clone(),
                                    turn_cancelled.clone(),
                                    None,
                                )
                                .await;

                                format_subagent_delivery(&res, exchange.as_ref())
                            }
                        }
                        Err(err) => format!("Error: {err}"),
                    }
                } else {
                    format!(
                        "Error: Agent with ID '{}' not found or not enabled.",
                        agent_id
                    )
                }
            } else if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                let mut args_json: Value = serde_json::from_str(args).unwrap_or_default();
                // The background routing flag is ours, never the server's.
                if let Some(object) = args_json.as_object_mut() {
                    object.remove("run_in_background");
                }
                // MCP tools are NOT sandboxed — path arguments are forwarded
                // verbatim to the MCP server. The server itself decides what
                // paths it can access.
                log_event(
                    &state,
                    "INFO",
                    format!("Args for MCP tool '{}': {}", actual_tool_name, args_json),
                );
                let _ = app.emit(
                    "tool-call",
                    format!(
                        "🔌 *MCP [{}]: {}*\n\n{}",
                        mcp_server.name,
                        actual_tool_name,
                        tool_args_block(&args_json)
                    ),
                );
                let start_time = std::time::Instant::now();
                let mcp_result =
                    mcp::invoke_mcp_tool(&state, mcp_server, actual_tool_name, args_json.clone())
                        .await;
                let duration_ms = start_time.elapsed().as_millis() as i64;
                let result = match &mcp_result {
                    Ok(result) => {
                        // Log successful MCP call
                        let db = state.db.lock().unwrap();
                        log_interaction(
                            &db,
                            &session_id,
                            "mcp_response",
                            &mcp_server.name,
                            actual_tool_name,
                            serde_json::to_string(&args_json).unwrap_or_default(),
                            result.clone(),
                            None,
                            duration_ms,
                            None,
                        );
                        result.clone()
                    }
                    Err(e) => {
                        // Log MCP error
                        let db = state.db.lock().unwrap();
                        log_interaction(
                            &db,
                            &session_id,
                            "mcp_call",
                            &mcp_server.name,
                            actual_tool_name,
                            serde_json::to_string(&args_json).unwrap_or_default(),
                            String::new(),
                            Some(e.clone()),
                            duration_ms,
                            None,
                        );
                        format!("MCP tool error: {e}")
                    }
                };
                result
            } else {
                let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                let start_time = std::time::Instant::now();
                let tool_result = tools::execute_tool(
                    &app,
                    name,
                    args,
                    workspace_dir,
                    &config,
                    &skill_allowed_commands,
                    skill_evolution_root.as_deref(),
                    None,
                    Some(&session_id),
                    &active_skill_dirs,
                    None,
                    None,
                )
                .await;
                let duration_ms = start_time.elapsed().as_millis() as i64;

                // Log tool execution
                let db = state.db.lock().unwrap();
                let is_error = turn_guard::tool_output_is_error(&tool_result);
                log_interaction(
                    &db,
                    &session_id,
                    if is_error {
                        "tool_error"
                    } else {
                        "tool_output"
                    },
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
                drop(db);
                tool_result
            };

            // The call ran: a failed result counts towards the tool's failure
            // budget, and the failure that crosses the limit is announced to
            // the model (appended to the very same result) and to the user.
            if ran {
                if let Some(trip) = guard.record(
                    &session_id,
                    name,
                    turn_guard::tool_output_is_error(&result),
                    failure_limit,
                ) {
                    result.push_str("\n\n");
                    result.push_str(&turn_guard::disabled_notice(
                        name,
                        trip.failures,
                        failure_limit,
                        true,
                    ));
                    log_event(
                        &state,
                        "WARN",
                        format!(
                            "tool disabled for this session: session_id={}, tool={}, failures={}, limit={}",
                            session_id, name, trip.failures, failure_limit
                        ),
                    );
                    let _ = app.emit(
                        "chat-notice",
                        json!({
                            "kind": "tool_disabled",
                            "session_id": session_id,
                            "tool": name,
                            "failures": trip.failures,
                            "limit": failure_limit,
                        }),
                    );
                }
            }

            all_messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }

        all_messages.extend(pending_skill_context_messages);
    }

    // ── Automatic conclusion recording ──────────────────────────────────────
    // If the turn ended with a substantive final answer that reads like a
    // conclusion, and the LLM did not already persist it via the `memory`
    // tool, record it so it survives across sessions. Self-evolution mode
    // routes it into the dedicated evolution area instead of `repo`.
    if let Some(answer) = final_answer {
        // The goal-loop marker is a control signal, not answer text — it must
        // not leak into the memory entry the answer produces.
        let answer = answer.split(GOAL_DONE_MARKER).collect::<Vec<_>>().join("");
        let answer = answer.trim_end();
        let memory_enabled = config.effective_tools().iter().any(|t| t == "memory");
        if memory_enabled
            && !memory_tool_called
            && should_record_conclusion(&answer)
            && looks_like_conclusion(&answer)
        {
            let workspace_dir = state.workspace_dir.lock().unwrap().clone();
            let scope = if config.self_evolution_mode {
                crate::tools::memory::EVOLUTION_SCOPE
            } else {
                "repo"
            };
            record_conclusion_to_memory(&state, &app, &workspace_dir, &session_id, scope, &answer);
        }
    }

    log_event(
        &state,
        "INFO",
        format!("chat_completion finished: session_id={}", session_id),
    );
    // Close the injection window before announcing the end of the turn. The
    // guard also runs on the error path, where `chat-done` is never emitted.
    drop(injection_window);
    let _ = app.emit("chat-done", ());
    Ok(())
}

/// Turn-end cleanup: stop every serve process and verify that the killed
/// trees are actually gone. Runs in a blocking thread — verification polls
/// for up to a few seconds in the pathological case, which must not stall the
/// async runtime. Silent when no serve process ever existed.
pub(crate) async fn end_turn_cleanup(app: &AppHandle, session_id: &str) {
    // Background tool tasks are turn-scoped, exactly like serve processes: a
    // turn that ends (final answer, error or Stop) must not leave a runner
    // alive. `shutdown_session` sets every cancel flag — each runner then kills
    // its process tree — and drops the entries so nothing is injected into the
    // next turn.
    let removed = app
        .state::<AppState>()
        .background_tasks
        .shutdown_session(session_id);
    let removed_count = removed.len();
    // Broadcast a terminal snapshot for every dropped task *before*
    // `chat_completion` returns. The runner that dies now finds no entry to
    // report into, so this is the only thing that can flip the UI: without it
    // the panel keeps rendering the last `running` state forever and the
    // transcript row keeps offering a cancel button for a task that is gone.
    for snapshot in removed {
        let _ = app.emit(
            tools::background::BACKGROUND_TASK_STATE_EVENT,
            snapshot,
        );
    }
    // The waiting chip is driven by `background-waiting`; zero it on every turn
    // end so a hold that was cut short (Stop, error, budget) cannot leave it
    // counting down a turn that no longer exists.
    let _ = app.emit(
        tools::background::BACKGROUND_WAITING_EVENT,
        json!({ "session_id": session_id, "pending": 0 }),
    );
    if removed_count > 0 {
        log_event(
            &app.state::<AppState>(),
            "INFO",
            format!(
                "turn ended (session {session_id}): shut down {removed_count} background task(s)"
            ),
        );
    }

    let session_id = session_id.to_string();
    let summary = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || app.state::<AppState>().serve_processes.shutdown_all()
    })
    .await
    .unwrap_or_else(|err| format!("Error: serve cleanup task failed: {err}"));

    if !summary.contains("No background serve processes") {
        log_event(
            &app.state::<AppState>(),
            "INFO",
            format!("turn ended (session {session_id}): {summary}"),
        );
    }
}
