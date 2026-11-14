//! Planning: provider helpers, JSON repair, task plan parsing and the entry
//! points that pick agents and build the plan.

use super::*;

// ─── LLM helpers ──────────────────────────────────────────────────────────────

/// Non-streaming LLM call; returns the raw text content.
pub(crate) async fn call_llm_once(
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    mut messages: Vec<Value>,
    model: &str,
    max_complete_tokens: u32,
    temperature: Option<f32>,
) -> Result<String, String> {
    sanitize_tool_pairs(&mut messages);
    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    if let Some(temperature) = temperature {
        // Planner/selector calls want the same answer every time. Some
        // OpenAI-compatible gateways ignore this field; harmless when they do.
        body["temperature"] = json!(temperature);
    }
    // No DS-Format normalization needed here: these prompts only ever carry
    // system/user messages plus (for a repair retry) the model's own previous
    // text, never a streamed assistant turn with tool calls.
    apply_completion_token_limit(
        &mut body,
        Some(max_complete_tokens),
        Some(LLM_DEFAULT_MAX_TOKENS),
    );
    let mut request = client.post(url);
    if !has_authorization_header(headers) {
        request = request.bearer_auth(api_key);
    }
    let res = apply_request_headers(request, headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(extract_upstream_error_message(&body));
    }
    let parsed: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(parsed["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

// ─── Planning helpers (pure, unit-tested) ─────────────────────────────────────

/// App-log writer for orchestration diagnostics. `plan_tasks` and the selector
/// run outside the chat turn, so they cannot use `llm_complete::log_event`.
pub(crate) fn log_agent_event(app: &AppHandle, level: &str, message: String) {
    let state = app.state::<AppState>();
    let logger = state.logger.lock();
    if let Ok(logger) = logger {
        logger.log(level, &message);
    }
}

/// Announces one tool call on a sub-agent's task row (`agent-task-tool`).
///
/// The row must keep its `running` status while the tool executes: the generic
/// `tool-call` event belongs to the main chat, whose listener settles every
/// running step and appends a row. This event only updates the task's activity
/// line, so the UI shows what the agent is doing during rounds that produce no
/// content at all (`text_chars=0`, tool_calls only).
pub(crate) fn emit_agent_tool_state(
    app: &AppHandle,
    session_id: &str,
    task_id: &str,
    phase: &str,
    tool: &str,
    detail: Option<String>,
    duration_ms: Option<u128>,
    ok: Option<bool>,
) {
    let mut payload = json!({
        "task_id": task_id,
        "session_id": session_id,
        "phase": phase,
        "tool": tool,
    });
    if let Some(detail) = detail {
        payload["detail"] = json!(detail);
    }
    if let Some(duration_ms) = duration_ms {
        payload["duration_ms"] = json!(duration_ms.min(u64::MAX as u128) as u64);
    }
    if let Some(ok) = ok {
        payload["ok"] = json!(ok);
    }
    let _ = app.emit("agent-task-tool", payload);
}

/// Provider balance/quota failures are terminal for this account: retrying or
/// spending more iterations on the same request only burns the turn. The text
/// is provider-specific, so match the common 402/quota phrasings.
pub(crate) fn is_provider_balance_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("insufficient account funds")
        || lower.contains("insufficient_quota")
        || lower.contains("free usage limit")
        || lower.contains("requires more credits")
        || lower.contains("credit balance")
        || lower.contains("billing")
        || lower.contains("http 402")
        || lower.contains(" 402 ")
}

/// UTF-8-safe preview for log lines and prompts (`limit` is bytes).
pub(crate) fn preview_text(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if out.len() + c.len_utf8() > limit {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

/// Joins a capability list for the selector catalogue. Only the first few
/// entries matter for routing; the rest would be pure prompt weight.
pub(crate) fn compact_list(items: &[String]) -> String {
    if items.is_empty() {
        return "-".to_string();
    }
    let mut text = items
        .iter()
        .take(ROSTER_LIST_MAX_ITEMS)
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    if items.len() > ROSTER_LIST_MAX_ITEMS {
        text.push_str(",…");
    }
    text
}

/// Returns the first balanced `{…}` object in `raw`, ignoring braces inside
/// JSON strings. Models routinely wrap the payload in a markdown fence, a
/// preamble or a trailing sentence; a strict `from_str` would throw all of that
/// away and waste the whole mission.
pub(crate) fn extract_json_object(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);

    let start = body.find('{')?;
    let bytes = body.as_bytes();
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, &byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return body.get(start..offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Conservative repair of the near-JSON models actually emit: trailing commas,
/// typographic quotes and `//` line comments. Characters inside string literals
/// are preserved (except a typographic closing quote, which is normalized).
pub(crate) fn repair_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' if !escaped => {
                    escaped = true;
                    out.push(c);
                }
                '"' if !escaped => {
                    in_string = false;
                    out.push(c);
                }
                '”' if !escaped => {
                    in_string = false;
                    out.push('"');
                }
                _ => {
                    escaped = false;
                    out.push(c);
                }
            }
            continue;
        }

        match c {
            '"' | '“' => {
                in_string = true;
                out.push('"');
            }
            '”' => out.push('"'),
            '/' if chars.peek() == Some(&'/') => {
                // Drop the comment but keep its newline so tokens stay apart.
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ',' => {
                // A trailing comma: only whitespace may follow before a closer.
                let mut look = chars.clone();
                while matches!(look.peek(), Some(next) if next.is_whitespace()) {
                    look.next();
                }
                if matches!(look.peek(), Some('}') | Some(']')) {
                    continue;
                }
                out.push(',');
            }
            _ => out.push(c),
        }
    }
    out
}

/// Finds one dependency edge that closes a cycle, if any: `(task, dependency)`.
/// Removing that edge (repeatedly) guarantees the executor can make progress.
pub(crate) fn find_cycle_edge(tasks: &[Task]) -> Option<(String, String)> {
    use std::collections::HashMap;

    let index: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, task)| (task.id.as_str(), i))
        .collect();
    // 0 = unvisited, 1 = on the current DFS stack, 2 = finished.
    let mut state = vec![0u8; tasks.len()];

    fn dfs(
        i: usize,
        tasks: &[Task],
        index: &std::collections::HashMap<&str, usize>,
        state: &mut [u8],
    ) -> Option<(String, String)> {
        state[i] = 1;
        for dep in &tasks[i].dependencies {
            let Some(&j) = index.get(dep.as_str()) else {
                continue;
            };
            if state[j] == 1 {
                return Some((tasks[i].id.clone(), dep.clone()));
            }
            if state[j] == 0 {
                if let Some(edge) = dfs(j, tasks, index, state) {
                    return Some(edge);
                }
            }
        }
        state[i] = 2;
        None
    }

    for i in 0..tasks.len() {
        if state[i] == 0 {
            if let Some(edge) = dfs(i, tasks, &index, &mut state) {
                return Some(edge);
            }
        }
    }
    None
}

/// Turns raw planner output into an executable plan.
///
/// Models return fences, preambles, trailing commas, hallucinated agent ids,
/// duplicate task ids and cyclic dependencies. None of those should waste the
/// turn: the plan is extracted, repaired and sanitized here, and every fix is
/// reported back as an issue for the log. Only an unusable answer fails.
pub(crate) fn parse_task_plan(raw: &str, roster: &[String]) -> Result<(PlanDecision, Vec<String>), String> {
    use std::collections::HashSet;

    let json = extract_json_object(raw)
        .ok_or_else(|| "no JSON object in the planner output".to_string())?;
    let plan: TaskPlan = serde_json::from_str(&repair_json(json)).map_err(|e| e.to_string())?;

    // The planner is allowed to look at the request and decline delegation
    // before planning anything. A missing `delegate` keeps the old contract:
    // tasks mean delegation.
    let delegate = plan.delegate.unwrap_or(!plan.tasks.is_empty());
    if !delegate {
        let mut issues: Vec<String> = Vec::new();
        if !plan.tasks.is_empty() {
            issues.push(format!(
                "planner returned delegate=false with {} task(s); ignored them",
                plan.tasks.len()
            ));
        }
        return Ok((
            PlanDecision::Direct {
                why: plan.why.trim().to_string(),
            },
            issues,
        ));
    }

    let mut issues: Vec<String> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut used_ids: HashSet<String> = HashSet::new();

    for mut task in plan.tasks {
        if task.description.trim().is_empty() {
            issues.push(format!("dropped task '{}': empty description", task.id));
            continue;
        }
        if !roster.iter().any(|id| id == &task.agent_id) {
            issues.push(format!(
                "dropped task '{}': unknown agent '{}'",
                task.id, task.agent_id
            ));
            continue;
        }

        // `mission_id == task.id`, so duplicates would corrupt mission rows and
        // dependency bookkeeping. Rename instead of dropping the work. `planner`
        // is reserved for the UI/supervisor row.
        let original = task.id.trim().to_string();
        if original == mbus::PLANNER_ID {
            issues.push("renamed reserved task id 'planner' to 'task'".to_string());
        }
        let base = if original.is_empty() || original == mbus::PLANNER_ID {
            "task"
        } else {
            original.as_str()
        }
        .to_string();
        let mut unique = base.clone();
        let mut suffix = 1usize;
        while used_ids.contains(&unique) {
            suffix += 1;
            unique = format!("{base}-{suffix}");
        }
        if unique != base {
            issues.push(format!("renamed duplicate task id '{base}' to '{unique}'"));
        }
        used_ids.insert(unique.clone());
        task.id = unique;
        task.description = task.description.trim().to_string();
        task.context = task.context.trim().to_string();
        // The planner may write "", "." or "./" for "the workspace root".
        task.cwd = normalize_task_cwd(task.cwd.as_deref());
        tasks.push(task);
    }

    if tasks.len() > MAX_PLAN_TASKS {
        issues.push(format!(
            "truncated plan from {} to {} tasks",
            tasks.len(),
            MAX_PLAN_TASKS
        ));
        tasks.truncate(MAX_PLAN_TASKS);
    }

    let known: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();

    // Dependencies: only ids that actually exist, never self-references.
    for task in &mut tasks {
        let before = task.dependencies.len();
        let mut seen = HashSet::new();
        task.dependencies
            .retain(|dep| dep != &task.id && known.contains(dep) && seen.insert(dep.clone()));
        if task.dependencies.len() != before {
            issues.push(format!(
                "dropped {} invalid dependency(ies) from task '{}'",
                before - task.dependencies.len(),
                task.id
            ));
        }
    }

    // Cycles: break one edge at a time so every remaining task stays reachable
    // by the wave scheduler instead of hanging it on an empty ready set.
    for _ in 0..=known.len() {
        let Some((task_id, dep_id)) = find_cycle_edge(&tasks) else {
            break;
        };
        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
            task.dependencies.retain(|dep| dep != &dep_id);
        }
        issues.push(format!(
            "dropped dependency '{task_id}' -> '{dep_id}' to break a cycle"
        ));
    }

    if tasks.is_empty() {
        return Err(if issues.is_empty() {
            "the planner produced no tasks".to_string()
        } else {
            format!(
                "the planner produced no usable tasks: {}",
                issues.join("; ")
            )
        });
    }

    Ok((
        PlanDecision::Mission(TaskPlan {
            tasks,
            execution_mode: plan.execution_mode,
            delegate: Some(true),
            why: plan.why,
        }),
        issues,
    ))
}

/// Announces the selector's decision to the UI (chip above the answer).
pub(crate) fn emit_agent_selection(app: &AppHandle, session_id: &str, selection: &AgentSelection) {
    let _ = app.emit(
        "agent-selection",
        json!({
            "session_id": session_id,
            "delegate": selection.delegate,
            "confidence": selection.confidence,
            "agents": &selection.choices,
        }),
    );
}

// ─── Orchestration entry point ────────────────────────────────────────────────

/// Main orchestration entry point called from llm_complete.rs.
///
/// `cancelled` is THIS turn's cancel flag (the per-turn flag swapped in by
/// `chat_completion`), so the mission observes the Stop of its own turn only.
pub async fn orchestrate(
    app: &AppHandle,
    state: &AppState,
    config: &AppConfig,
    messages: &[crate::llm_complete::ChatMessage],
    session_id: &str,
    cancelled: Arc<AtomicBool>,
) -> Result<OrchestrationOutcome, String> {
    let agents_config = load_configured_agents(&state.agents_config_path);
    let enabled: Vec<SubAgent> = agents_config
        .agents
        .iter()
        .filter(|a| a.enabled)
        .cloned()
        .collect();

    if enabled.is_empty() {
        return Err("No enabled sub-agents configured".to_string());
    }

    // Streaming client (planning calls are streamed too): adds the connect +
    // read timeouts `Client::new()` has none of, so a stalled gateway fails
    // and retries instead of hanging the whole turn.
    let client = build_stream_client()?;
    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
    let orchestration = agents_config.orchestration.clone();
    // The planner may run on its own model (usually a smaller, cheaper one). An
    // explicit pick in the Agents panel wins; an empty field inherits the main
    // chat model, which is what every mission used before the field existed.
    // Selection, the task plan and the mbus supervisor all use this value.
    let model = orchestration.planner_model_or(&config.model).to_string();
    // Same headers as the main chat: configured conversation-id header plus
    // the user's custom headers, so planning calls reach the same gateway.
    let headers = upstream_request_headers(config, session_id);

    // Step 1: one selection call decides who takes part in this turn. With the
    // switch off every enabled agent takes part, exactly like before.
    let (working_agents, selection) = if orchestration.auto_configure {
        match select_agents(
            app,
            &client,
            &url,
            &config.api_key,
            &headers,
            &model,
            messages,
            &enabled,
        )
        .await
        {
            Ok(result) => (result.agents, Some(result.selection)),
            Err(err) => {
                // A failed selection must never lose the mission: fall back to
                // the full roster and let the planner decide.
                log_agent_event(
                    app,
                    "WARN",
                    format!("agent selection failed, using every enabled agent: {err}"),
                );
                (enabled.clone(), None)
            }
        }
    } else {
        (enabled.clone(), None)
    };

    // The selector can decide the request needs no sub-agents at all. Skipping
    // planning here saves a full LLM round trip and keeps simple questions on
    // the direct-answer path.
    if matches!(&selection, Some(selection) if !selection.delegate) {
        return Ok(OrchestrationOutcome {
            report: None,
            selection,
        });
    }

    // Step 2: Plan. Stop pressed between steps aborts the mission here, before
    // any sub-agent is launched.
    let _ = app.emit(
        "agent-plan-start",
        json!({ "task_count": 0, "session_id": session_id }),
    );
    if cancelled.load(Ordering::SeqCst) {
        return Err("stopped by the user".to_string());
    }
    // The planner needs the workspace root to name task directories: the main
    // agent decides *where* each delegated task runs, not the agent definition.
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    // The batch root is reserved before planning so the planner can reference
    // the deliverable exchange area in task context. The directory itself is
    // created only when the planner actually delegates.
    let exchange_root = exchange::ExchangeArea::new_batch_root(&workspace_dir, session_id)?;
    let decision = plan_tasks(
        app,
        &client,
        &url,
        &config.api_key,
        &headers,
        &model,
        messages,
        &working_agents,
        &workspace_dir,
        &exchange_root,
    )
    .await?;

    if cancelled.load(Ordering::SeqCst) {
        return Err("stopped by the user".to_string());
    }

    // The planner has the final say: a request that one direct answer can
    // handle exits here, before a mission, bus or supervisor is created.
    let plan = match decision {
        PlanDecision::Direct { why } => {
            let why = if why.trim().is_empty() {
                "the planner decided this request needs no sub-agents".to_string()
            } else {
                why
            };
            log_agent_event(app, "INFO", format!("planner declined delegation: {why}"));
            let declined = AgentSelection {
                delegate: false,
                confidence: selection
                    .as_ref()
                    .map(|selection| selection.confidence)
                    .unwrap_or(1.0),
                choices: Vec::new(),
            };
            if enabled.len() > 1 {
                emit_agent_selection(app, session_id, &declined);
            }
            return Ok(OrchestrationOutcome {
                report: None,
                selection: Some(declined),
            });
        }
        PlanDecision::Mission(plan) => plan,
    };

    // Emit the selector chip only after the planner confirmed the mission, so a
    // declined plan never leaves a stale "agents selected" row in the UI.
    if let Some(selection) = &selection {
        if enabled.len() > 1 {
            emit_agent_selection(app, session_id, selection);
        }
    }

    let exchange_area = match exchange::ExchangeArea::create_at(
        exchange_root,
        &workspace_dir,
        &plan
            .tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>(),
    ) {
        Ok(area) => Some(area),
        Err(err) => {
            log_agent_event(
                app,
                "WARN",
                format!("deliverable exchange area unavailable: {err}"),
            );
            None
        }
    };

    // Step 3: Create the mission bus for this batch. A batch with no in-app LLM
    // agent has no peer that can call mbus tools, so it keeps the old behaviour.
    let mission_bus: Option<Arc<mbus::MissionBus>> = if orchestration.mbus_enabled {
        let bus = Arc::new(
            mbus::MissionBus::new(session_id, &plan.tasks, &working_agents)
                .with_pair_limit(orchestration.mbus_pair_limit),
        );
        if bus.has_llm_peer() {
            Some(bus)
        } else {
            None
        }
    } else {
        None
    };
    let _ = app.emit(
        "agent-plan-start",
        json!({
            "task_count": plan.tasks.len(),
            "session_id": session_id,
            "mbus": mission_bus.is_some(),
            "bus_id": mission_bus.as_ref().map(|bus| bus.bus_id()),
        }),
    );

    // Step 4: Execute. The planner supervisor runs concurrently with the tasks;
    // a oneshot from the executor cancels a slow supervisor call the moment the
    // last task settles, so supervision never delays the final answer.
    let use_parallel = orchestration.mode != "sequential" && plan.execution_mode != "sequential";
    if let Some(bus) = &mission_bus {
        let _ = app.emit(
            "agent-planner-action",
            json!({
                "session_id": session_id,
                "bus_id": bus.bus_id(),
                "task_id": "planner",
                "action_id": "planner-schedule",
                "activation": 0,
                "round": 0,
                "action": "schedule",
                "status": "done",
                "target": "plan",
                "detail": format!(
                    "{} task(s) · {} · max concurrent {}",
                    plan.tasks.len(),
                    if use_parallel { "parallel" } else { "sequential" },
                    orchestration.max_concurrent
                ),
                "created_at_ms": chrono::Local::now().timestamp_millis(),
            }),
        );
    }
    let skill_access_root =
        crate::skills::self_evolution_skill_root(Some(&workspace_dir), config.self_evolution_mode);
    let supervisor_tasks = plan.tasks.clone();
    let bus_for_executor = mission_bus.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let executor_client = client.clone();
    let executor_url = url.clone();
    let executor_cancelled = cancelled.clone();
    let executor_workspace = workspace_dir.clone();
    let executor_exchange = exchange_area.clone();
    let executor = async move {
        let results = execute_tasks(
            app,
            &executor_client,
            &executor_url,
            config,
            session_id,
            &working_agents,
            plan.tasks,
            executor_workspace,
            executor_exchange,
            skill_access_root,
            use_parallel,
            orchestration.max_concurrent,
            &executor_cancelled,
            bus_for_executor,
        )
        .await;
        let _ = done_tx.send(());
        results
    };
    let supervisor_bus = mission_bus.clone();
    let supervisor_client = client.clone();
    let supervisor_url = url.clone();
    let supervisor_model = model.clone();
    let supervisor_cancelled = cancelled.clone();
    let supervisor = async move {
        if let Some(bus) = supervisor_bus {
            supervisor::run_planner_supervisor(
                app,
                &supervisor_client,
                &supervisor_url,
                config,
                session_id,
                &supervisor_model,
                messages,
                &supervisor_tasks,
                bus,
                supervisor_cancelled,
                done_rx,
            )
            .await;
        }
    };
    let (results, _) = tokio::join!(executor, supervisor);
    if let Some(bus) = &mission_bus {
        bus.close();
    }

    // Step 5: Hand the mission record to the caller. Sub-agents are execution
    // workers — they never write the user-facing answer: the main agent (full
    // chat context, skills and tools) defines the final result, so
    // llm_complete injects this report and runs its normal completion flow.
    //
    // The aggregation itself is instantaneous (it only renders the report
    // below), so the event reports a *completed* step: a "start" event here
    // would leave the UI spinning until the whole final answer is streamed.
    let _ = app.emit(
        "agent-aggregate-done",
        json!({ "task_count": results.len(), "session_id": session_id }),
    );
    Ok(OrchestrationOutcome {
        report: Some(build_mission_results_block(
            &results,
            exchange_area.as_ref(),
        )),
        selection,
    })
}

// ─── Selection ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct SelectionResponse {
    #[serde(default)]
    delegate: Option<bool>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    agents: Vec<SelectionChoice>,
    #[serde(default)]
    why: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SelectionChoice {
    #[serde(default)]
    id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    why: String,
}

impl SelectionChoice {
    fn role_or(&self, fallback: &str) -> String {
        let role = self.role.trim();
        if role.is_empty() {
            fallback.chars().take(60).collect()
        } else {
            role.to_string()
        }
    }
}

/// Every enabled agent takes part. Used both by the deterministic single-agent
/// path and by every "the model answer cannot be trusted" fallback.
pub(crate) fn full_roster_selection(existing: &[SubAgent], confidence: f32, why: &str) -> SelectionResult {
    SelectionResult {
        agents: existing.to_vec(),
        selection: AgentSelection {
            delegate: !existing.is_empty(),
            confidence,
            choices: existing
                .iter()
                .map(|agent| AgentChoice {
                    id: agent.id.clone(),
                    name: agent.name.clone(),
                    role: agent.description.chars().take(60).collect(),
                    why: why.to_string(),
                })
                .collect(),
        },
    }
}

/// One non-streaming LLM call that decides which agents take part in this turn.
///
/// The answer is a *proposal*: ids are validated against the live roster, the
/// list is capped, and every failure widens back to the full roster instead of
/// failing the mission. `delegate: false` is a first-class outcome — it skips
/// planning entirely.
pub(crate) async fn select_agents(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    existing: &[SubAgent],
) -> Result<SelectionResult, String> {
    // One agent means there is nothing to choose: skip the request entirely.
    if existing.len() <= 1 {
        return Ok(full_roster_selection(
            existing,
            1.0,
            "the only enabled agent",
        ));
    }

    let catalogue = existing
        .iter()
        .map(|agent| {
            format!(
                "- id={} name={} type={} tools={} skills={} mcp={} desc={}",
                agent.id,
                agent.name,
                normalize_agent_type(&agent.agent_type),
                compact_list(&agent.allowed_tools),
                compact_list(&agent.allowed_skills),
                if agent.enable_mcp { "enabled" } else { "none" },
                preview_text(&agent.description, ROSTER_DESC_MAX_BYTES)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user_query = preview_text(
        &messages
            .last()
            .map(|m| content_to_text(&m.content))
            .unwrap_or_default(),
        SELECTION_QUERY_MAX_BYTES,
    );

    // An attachment-only turn has no text for the selector to judge. Narrowing
    // on an empty query would wrongly decline delegation, so skip the call.
    if user_query.trim().is_empty() {
        return Ok(full_roster_selection(existing, 1.0, "no text to analyse"));
    }

    let system = format!(
        "You are an AI orchestrator. Decide which sub-agents should work on the user's request.\n\
         Existing agents:\n{catalogue}\n\n\
         Rules:\n\
         - Smallest set that can do the job: 0 to {MAX_SELECTED_AGENTS} agents.\n\
         - `llm` for reasoning, analysis and writing; `cli` for large code changes in the workspace; `cloud` for long remote jobs.\n\
         - Match capabilities, not domains: live web/browser/API data needs `mcp=enabled` or browser/network tools. If no agent has that data channel, return delegate=false so the main agent (which has MCP/browser tools) does the fetch.\n\
         - If no agent fits, or the request can be answered directly, return delegate=false.\n\
         - Copy ids exactly from the list above; never invent one.\n\n\
         Return ONLY one JSON object — no markdown, no commentary:\n\
         {{\"delegate\":true,\"confidence\":0.0,\"agents\":[{{\"id\":\"<id>\",\"role\":\"<short role>\",\"why\":\"<one sentence>\"}}]}}\n\
         or {{\"delegate\":false,\"confidence\":0.0,\"why\":\"<one sentence>\"}}",
    );

    let raw = call_llm_once(
        client,
        url,
        api_key,
        headers,
        vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": user_query}),
        ],
        model,
        1024,
        Some(0.0),
    )
    .await?;

    let json = extract_json_object(&raw).ok_or_else(|| {
        format!(
            "no JSON in the selection output: {}",
            preview_text(&raw, 300)
        )
    })?;
    let parsed: SelectionResponse = serde_json::from_str(&repair_json(json))
        .map_err(|e| format!("selection parse error: {e}"))?;
    // A model that lists agents but forgets `delegate` still meant to delegate;
    // an explicit `false` always wins. A missing confidence is treated as
    // "reasonably sure" — otherwise every terse answer would widen the roster.
    let delegate = parsed.delegate.unwrap_or(!parsed.agents.is_empty());
    let confidence = parsed.confidence.unwrap_or(0.7).clamp(0.0, 1.0);

    if !delegate {
        log_agent_event(
            app,
            "INFO",
            format!(
                "agent selection declined delegation (confidence {confidence:.2}): {}",
                preview_text(parsed.why.trim(), 200)
            ),
        );
        return Ok(SelectionResult {
            agents: Vec::new(),
            selection: AgentSelection {
                delegate: false,
                confidence,
                choices: Vec::new(),
            },
        });
    }

    let mut choices: Vec<AgentChoice> = Vec::new();
    let mut agents: Vec<SubAgent> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for choice in parsed.agents {
        let id = choice.id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let Some(agent) = existing.iter().find(|agent| agent.id == id) else {
            // Hallucinated id: never hand it to the planner.
            log_agent_event(
                app,
                "WARN",
                format!("agent selection named unknown agent id '{id}'"),
            );
            continue;
        };
        choices.push(AgentChoice {
            id: agent.id.clone(),
            name: agent.name.clone(),
            role: choice.role_or(&agent.description),
            why: choice.why.trim().to_string(),
        });
        agents.push(agent.clone());
        if agents.len() == MAX_SELECTED_AGENTS {
            break;
        }
    }

    // A selection that matches nothing, or one the model is not confident
    // about, is worse than no narrowing at all: widen to the full roster so the
    // planner still sees every option.
    if agents.is_empty() || confidence < 0.5 {
        let reason = if agents.is_empty() {
            "matched no known agent"
        } else {
            "confidence below 0.5"
        };
        log_agent_event(
            app,
            "WARN",
            format!("agent selection {reason}; falling back to every enabled agent"),
        );
        return Ok(full_roster_selection(
            existing,
            confidence,
            "fallback: full roster",
        ));
    }

    Ok(SelectionResult {
        agents,
        selection: AgentSelection {
            delegate: true,
            confidence,
            choices,
        },
    })
}

// ─── Planning ─────────────────────────────────────────────────────────────────

pub(crate) async fn plan_tasks(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    model: &str,
    messages: &[crate::llm_complete::ChatMessage],
    agents: &[SubAgent],
    workspace_dir: &Path,
    exchange_root: &Path,
) -> Result<PlanDecision, String> {
    let agents_desc = agents
        .iter()
        .map(|a| {
            format!(
                "  - id={} name={} type={} tools={} skills={} mcp={} desc={}",
                a.id,
                a.name,
                normalize_agent_type(&a.agent_type),
                compact_list(&a.allowed_tools),
                compact_list(&a.allowed_skills),
                if a.enable_mcp { "enabled" } else { "none" },
                preview_text(&a.description, ROSTER_DESC_MAX_BYTES)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let roster = agents
        .iter()
        .map(|a| a.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    // Build a short conversation summary (last 3 messages, budgeted per
    // message): the planner only needs the intent and recent corrections.
    let conversation = messages
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|m| {
            let text = content_to_text(&m.content);
            format!(
                "[{}]: {}",
                m.role,
                preview_text(&text, PLANNER_CONVERSATION_MAX_BYTES)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system = format!(
        "You are a task planning expert. FIRST decide whether the request needs sub-agents at all.\n\n\
        Step 1 — delegation decision:\n\
        - {{\"delegate\":false,\"why\":\"<one short sentence>\"}} when one direct answer is enough: a \
        simple question, a single lookup, a small edit, a writing task.\n\
        - {{\"delegate\":true,...}} only when the work clearly benefits: parallel research, large code \
        changes, long remote jobs, independent workstreams.\n\
        - When unsure, delegate — a wrong decline loses capability, a needless mission only costs one \
        task. A single task is a valid plan.\n\n\
        Step 2 — only for delegate:true, plan the tasks.\n\
        Available agents:\n{agents_desc}\n\n\
        Workspace root: {workspace_root}\n\
        Deliverable exchange area: {exchange_root} — every task gets a directory named after its id, \
        shared files go to {exchange_root}/shared. In `context`, name a dependency artifact as \
        `<task-id>/<filename>` relative to the exchange root; never paste large artifacts into a \
        description.\n\n\
        A task may name one working directory with `cwd` (workspace-relative, e.g. \"apps/web\") — use \
        it whenever the request names or targets a specific project/folder, omit it (or \".\") for the \
        workspace root, and never invent a directory the conversation does not mention.\n\n\
        Return ONLY valid JSON — no markdown, no extra text:\n\
        {{\"delegate\":false,\"why\":\"...\"}}\n\
        or\n\
        {{\"delegate\":true,\"tasks\":[{{\"id\":\"t1\",\"agent_id\":\"<id>\",\"description\":\"...\",\
          \"context\":\"...\",\"cwd\":\"<workspace-relative dir, omit for root>\",\"dependencies\":[]}}],\
          \"execution_mode\":\"parallel\"}}\n\
        Rules:\n\
        - execution_mode: \"parallel\" for independent tasks, \"sequential\" when order matters; \
        dependencies lists the task ids that must finish first\n\
        - Every agent_id must be exactly one of: {roster}\n\
        - Match capabilities to the task: live web/browser/API collection needs `mcp=enabled` or \
        browser/network tools — never assign it to a file-only agent (mcp=none), which would burn \
        iterations scanning local snapshots. If no listed agent can fetch the data, leave the fetch to \
        the main agent and plan only analysis.\n\
        - Keep descriptions brief and actionable",
        workspace_root = workspace_dir.display(),
        exchange_root = exchange_root.display(),
    );
    let user_prompt = format!("Conversation:\n{conversation}\n\nCreate a task plan.");
    let roster_ids: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();

    let mut last_error = String::new();
    let mut last_raw = String::new();

    // Two attempts: the second one hands the model its own broken answer plus
    // the parse error. A malformed plan must not waste the whole mission, and
    // one repair round is enough — anything beyond that is a model problem.
    for attempt in 0..2 {
        let mut request = vec![
            json!({"role":"system","content": system}),
            json!({"role":"user","content": user_prompt}),
        ];
        if attempt > 0 {
            request.push(json!({"role":"assistant","content": last_raw}));
            request.push(json!({"role":"user","content": format!(
                "Your previous answer could not be used: {last_error}\nReturn ONLY the JSON object described above — no markdown fence, no commentary."
            )}));
        }

        let raw = call_llm_once(
            client,
            url,
            api_key,
            headers,
            request,
            model,
            2048,
            Some(0.0),
        )
        .await?;

        match parse_task_plan(&raw, &roster_ids) {
            Ok((plan, issues)) => {
                for issue in issues {
                    log_agent_event(app, "WARN", format!("planner: {issue}"));
                }
                return Ok(plan);
            }
            Err(err) => {
                log_agent_event(
                    app,
                    "WARN",
                    format!(
                        "planner attempt {} failed: {err}; output={}",
                        attempt + 1,
                        preview_text(&raw, 500)
                    ),
                );
                last_error = err;
                last_raw = raw;
            }
        }
    }

    Err(format!("Plan parse error after 2 attempts: {last_error}"))
}
