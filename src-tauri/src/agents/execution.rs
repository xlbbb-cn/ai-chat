//! Execution: the task runners (parallel, sequential, sub-agent, external
//! cli/cloud backends).

use super::*;

// ─── Execution ────────────────────────────────────────────────────────────────

pub(crate) async fn execute_tasks(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    exchange: Option<exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    use_parallel: bool,
    max_concurrent: usize,
    cancelled: &Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> Vec<TaskResult> {
    if use_parallel {
        execute_parallel(
            app,
            client,
            url,
            config,
            session_id,
            agents,
            tasks,
            workspace_dir,
            exchange,
            skill_access_root,
            max_concurrent,
            cancelled,
            mbus,
        )
        .await
    } else {
        execute_sequential(
            app,
            client,
            url,
            config,
            session_id,
            agents,
            tasks,
            workspace_dir,
            exchange,
            skill_access_root,
            cancelled,
            mbus,
        )
        .await
    }
}

/// Reports a task that never ran. The mission row (if any) is closed so the
/// monitor cannot keep showing it as running, and the UI gets a neutral event
/// instead of a silent gap.
pub(crate) fn finish_skipped(
    app: &AppHandle,
    session_id: &str,
    mbus: Option<&mbus::MissionBus>,
    result: &TaskResult,
) {
    close_mission(app, &result.task_id, "stopped");
    if let Some(bus) = mbus {
        bus.mark_skipped(result);
    }
    let _ = app.emit(
        "agent-task-skipped",
        json!({
            "task_id": &result.task_id,
            "session_id": session_id,
            "agent_id": &result.agent_id,
            "agent_name": &result.agent_name,
            "reason": &result.content,
        }),
    );
}

pub(crate) async fn execute_parallel(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    exchange: Option<exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    max_concurrent: usize,
    cancelled: &Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> Vec<TaskResult> {
    use std::collections::{HashMap, HashSet};
    use tokio::task::JoinSet;

    // `settled` = a result exists (success or error, the latter still lets
    // dependents run, as before). `blocked` = the task never produced a result,
    // so every task depending on it is skipped instead of waiting forever.
    let mut settled: HashSet<String> = HashSet::new();
    let mut blocked: HashSet<String> = HashSet::new();
    let mut pending: Vec<Task> = Vec::new();
    let mut results: Vec<TaskResult> = Vec::new();

    // Whole-batch pre-flight: an unknown agent can never run. Report it here
    // rather than letting its dependents deadlock on a missing result.
    for task in tasks {
        if agents.iter().any(|agent| agent.id == task.agent_id) {
            pending.push(task);
        } else {
            let result = TaskResult::skipped(
                &task,
                &task.agent_id,
                format!("agent '{}' is not available", task.agent_id),
            );
            finish_skipped(app, session_id, mbus.as_deref(), &result);
            blocked.insert(result.task_id.clone());
            results.push(result);
        }
    }

    while !pending.is_empty() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }

        // Cascade skips to a fixpoint: skipping one task can invalidate a whole
        // dependency chain, and those tasks must not be scheduled at all.
        loop {
            let newly: Vec<Task> = pending
                .iter()
                .filter(|task| task.dependencies.iter().any(|dep| blocked.contains(dep)))
                .cloned()
                .collect();
            if newly.is_empty() {
                break;
            }
            for task in newly {
                let missing: Vec<&str> = task
                    .dependencies
                    .iter()
                    .filter(|dep| blocked.contains(*dep))
                    .map(|dep| dep.as_str())
                    .collect();
                let result = TaskResult::skipped(
                    &task,
                    &task.agent_id,
                    format!("skipped because {} did not run", missing.join(", ")),
                );
                finish_skipped(app, session_id, mbus.as_deref(), &result);
                blocked.insert(result.task_id.clone());
                results.push(result);
                pending.retain(|candidate| candidate.id != task.id);
            }
        }

        let ready: Vec<Task> = pending
            .iter()
            .filter(|task| task.dependencies.iter().all(|dep| settled.contains(dep)))
            .cloned()
            .collect();

        if ready.is_empty() {
            // Only reachable with a cycle the planner validation missed: never
            // hang the mission and never drop tasks silently.
            for task in pending.drain(..) {
                let missing: Vec<&str> = task
                    .dependencies
                    .iter()
                    .filter(|dep| !settled.contains(*dep))
                    .map(|dep| dep.as_str())
                    .collect();
                let reason = if missing.is_empty() {
                    "dependencies could not be satisfied".to_string()
                } else {
                    format!(
                        "dependencies could not be satisfied: {}",
                        missing.join(", ")
                    )
                };
                let result = TaskResult::skipped(&task, &task.agent_id, reason);
                finish_skipped(app, session_id, mbus.as_deref(), &result);
                results.push(result);
            }
            break;
        }

        pending.retain(|task| !ready.iter().any(|candidate| candidate.id == task.id));

        // Process ready tasks in batches of max_concurrent. `next` tracks how
        // far the batch window got, so a Stop between batches can put the
        // unscheduled remainder back into `pending` for the tail below.
        let limit = max_concurrent.max(1);
        let mut next = 0;
        while next < ready.len() {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            let end = (next + limit).min(ready.len());
            let batch = ready[next..end].to_vec();
            next = end;

            let mut join_set: JoinSet<TaskResult> = JoinSet::new();
            // Maps each spawned job to its task so an aborted or panicked job can
            // be attributed and reported instead of silently dropping its result.
            let mut spawned_jobs: HashMap<tokio::task::Id, (String, String, String)> =
                HashMap::new();

            for task in batch {
                let Some(agent) = agents.iter().find(|a| a.id == task.agent_id).cloned() else {
                    // Pre-flight already handled this; keep the guard cheap.
                    continue;
                };

                let app_c = app.clone();
                let client_c = client.clone();
                let url_c = url.to_string();
                let config_c = config.clone();
                let session_id_c = session_id.to_string();
                let task_c = task.clone();
                let wd_c = workspace_dir.clone();
                let exchange_c = exchange.clone();
                let skill_roots_c = skill_access_root.clone();
                // Arc snapshot of the app-global cancel flag: the spawned job
                // and every sub-agent inside it observe the same flag, so Stop
                // tears down mid-flight agents instead of only skipping future
                // tasks.
                let cancelled_c = cancelled.clone();
                let is_cancelled = cancelled.load(Ordering::SeqCst);
                let mbus_c = mbus.clone();
                let job_meta = (task.id.clone(), agent.id.clone(), agent.name.clone());

                let handle = join_set.spawn(async move {
                    if is_cancelled {
                        return TaskResult::skipped(&task_c, &agent.name, "cancelled by user");
                    }
                    run_sub_agent(
                        &app_c,
                        &client_c,
                        &url_c,
                        &config_c,
                        &session_id_c,
                        &agent,
                        &task_c,
                        wd_c,
                        exchange_c.as_ref(),
                        skill_roots_c,
                        cancelled_c,
                        mbus_c,
                    )
                    .await
                });
                spawned_jobs.insert(handle.id(), job_meta);
            }

            while let Some(joined) = join_set.join_next().await {
                match joined {
                    Ok(res) => {
                        // Only a real outcome satisfies a dependency; a stopped
                        // or skipped run must cascade instead.
                        if matches!(res.status.as_str(), "success" | "error") {
                            settled.insert(res.task_id.clone());
                        } else {
                            // A pre-cancelled batch slot returns "skipped" without
                            // going through `run_sub_agent`, so it still needs its
                            // UI event.
                            if res.status == "skipped" {
                                finish_skipped(app, session_id, mbus.as_deref(), &res);
                            }
                            blocked.insert(res.task_id.clone());
                        }
                        results.push(res);
                    }
                    Err(err) => {
                        // A panicked or aborted job must not take down the rest of
                        // the batch: report it as an error result and mark it as
                        // finished so dependent tasks can still run.
                        let (task_id, agent_id, agent_name) =
                            spawned_jobs.remove(&err.id()).unwrap_or_else(|| {
                                (
                                    "unknown-task".to_string(),
                                    "unknown-agent".to_string(),
                                    "unknown-agent".to_string(),
                                )
                            });
                        let reason = if err.is_panic() {
                            "panicked"
                        } else {
                            "was aborted"
                        };
                        let result = TaskResult {
                            task_id,
                            agent_id,
                            agent_name,
                            status: "error".to_string(),
                            content: format!("Sub-agent task {reason}: {err}"),
                            tool_calls_count: 0,
                            tokens_used: 0,
                        };
                        // task_id == mission_id: never leave a panicked mission
                        // behind in the "running" state.
                        close_mission(app, &result.task_id, "failed");
                        if let Some(bus) = &mbus {
                            bus.finish_task(&result);
                        }
                        let _ = app.emit(
                            "agent-task-error",
                            json!({
                                "task_id": &result.task_id,
                                "session_id": session_id,
                                "agent_id": &result.agent_id,
                                "error": &result.content,
                            }),
                        );
                        settled.insert(result.task_id.clone());
                        results.push(result);
                    }
                }
            }
        }

        if next < ready.len() {
            pending.extend(ready[next..].iter().cloned());
        }
    }

    // Stop pressed: everything that never started is still reported.
    if cancelled.load(Ordering::SeqCst) {
        for task in pending {
            let result = TaskResult::skipped(&task, &task.agent_id, "cancelled by user");
            finish_skipped(app, session_id, mbus.as_deref(), &result);
            results.push(result);
        }
    }

    results
}

pub(crate) async fn execute_sequential(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agents: &[SubAgent],
    tasks: Vec<Task>,
    workspace_dir: PathBuf,
    exchange: Option<exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: &Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> Vec<TaskResult> {
    use std::collections::HashSet;

    let mut results = Vec::new();
    // Sequential mode never reorders by dependency, but a task whose dependency
    // was skipped still cannot produce a correct result.
    let mut blocked: HashSet<String> = HashSet::new();

    for task in tasks {
        if cancelled.load(Ordering::SeqCst) {
            let result = TaskResult::skipped(&task, &task.agent_id, "cancelled by user");
            finish_skipped(app, session_id, mbus.as_deref(), &result);
            results.push(result);
            continue;
        }

        let missing: Vec<&str> = task
            .dependencies
            .iter()
            .filter(|dep| blocked.contains(*dep))
            .map(|dep| dep.as_str())
            .collect();
        if !missing.is_empty() {
            let result = TaskResult::skipped(
                &task,
                &task.agent_id,
                format!("skipped because {} did not run", missing.join(", ")),
            );
            finish_skipped(app, session_id, mbus.as_deref(), &result);
            blocked.insert(task.id.clone());
            results.push(result);
            continue;
        }

        let agent = match agents.iter().find(|a| a.id == task.agent_id) {
            Some(agent) => agent,
            None => {
                let result = TaskResult::skipped(
                    &task,
                    &task.agent_id,
                    format!("agent '{}' is not available", task.agent_id),
                );
                finish_skipped(app, session_id, mbus.as_deref(), &result);
                blocked.insert(task.id.clone());
                results.push(result);
                continue;
            }
        };

        let res = run_sub_agent(
            app,
            client,
            url,
            config,
            session_id,
            agent,
            &task,
            workspace_dir.clone(),
            exchange.as_ref(),
            skill_access_root.clone(),
            cancelled.clone(),
            mbus.clone(),
        )
        .await;
        if !matches!(res.status.as_str(), "success" | "error") {
            blocked.insert(res.task_id.clone());
        }
        results.push(res);
    }
    results
}

// ─── Sub-agent loop ───────────────────────────────────────────────────────────

pub async fn run_sub_agent(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    agent: &SubAgent,
    task: &Task,
    workspace_dir: PathBuf,
    exchange: Option<&exchange::ExchangeArea>,
    skill_access_root: Option<PathBuf>,
    cancelled: Arc<AtomicBool>,
    mbus: Option<Arc<mbus::MissionBus>>,
) -> TaskResult {
    let mission_id = mission_id_for_task(task).to_string();
    let agent_type = normalize_agent_type(&agent.agent_type);
    // `llm` tasks are running as soon as their loop starts. External agents are
    // marked running after their prompt absorbed the pre-start inbox, so the
    // planner can still reach a task that was dispatched but not launched yet.
    if agent_type == AGENT_TYPE_LLM {
        if let Some(bus) = &mbus {
            bus.mark_running(&task.id);
        }
    }

    // Resolve the task's working directory once, before anything starts. It is
    // supplied per delegation (planner / `call_subagent`) and never by the
    // agent definition, so one agent can work on different projects. The path
    // is sandboxed to the workspace and created when it does not exist yet; a
    // bad path fails the task with a message the caller can act on instead of
    // silently running somewhere else.
    let task_cwd = match resolve_task_cwd(task.cwd.as_deref(), &workspace_dir) {
        Ok(cwd) => cwd,
        Err(err) => {
            let _ = app.emit(
                "agent-task-start",
                json!({
                    "task_id": task.id,
                    "session_id": session_id,
                    "agent_id": agent.id,
                    "agent_name": agent.name,
                    "description": task.description,
                    "cwd": task.cwd,
                }),
            );
            let _ = app.emit(
                "agent-task-error",
                json!({
                    "task_id": task.id,
                    "session_id": session_id,
                    "agent_id": agent.id,
                    "error": err
                }),
            );
            let result = TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: format!("Error: {err}"),
                tool_calls_count: 0,
                tokens_used: 0,
            };
            if let Some(bus) = &mbus {
                bus.finish_task(&result);
            }
            return result;
        }
    };

    let result = match agent_type.as_str() {
        AGENT_TYPE_CLI => {
            run_external_agent(
                app,
                agent,
                task,
                session_id,
                task_cwd,
                exchange,
                ExternalBackend::Cli,
                cancelled,
                mbus.as_deref(),
            )
            .await
        }
        AGENT_TYPE_CLOUD => {
            run_external_agent(
                app,
                agent,
                task,
                session_id,
                task_cwd,
                exchange,
                ExternalBackend::Cloud,
                cancelled,
                mbus.as_deref(),
            )
            .await
        }
        _ => {
            run_sub_agent_inner(
                app,
                client,
                url,
                config,
                session_id,
                agent,
                task,
                workspace_dir,
                task_cwd,
                exchange,
                skill_access_root,
                &cancelled,
                mbus.as_deref(),
            )
            .await
        }
    };

    if let Some(bus) = &mbus {
        bus.finish_task(&result);
    }

    // Close the mission unless the sub-agent completed it explicitly: without
    // this, a stopped, failed or cancelled mission keeps reporting "running"
    // in the mission monitor forever.
    if result.status != "success" {
        close_mission(
            app,
            &mission_id,
            if result.status == "error" {
                "failed"
            } else {
                "stopped"
            },
        );
    }

    result
}

// ─── External agents (cli / cloud) ───────────────────────────────────────────

/// What a `cli` run needs beyond the agent settings: where it starts.
pub(crate) struct CliRunContext {
    cwd: PathBuf,
}

/// Upper bound for any external agent run's timeout.
pub(crate) const MAX_AGENT_TIMEOUT_SECS: u64 = 7_200;

/// Resolves a configured agent timeout: `0` disables it, while positive values
/// are capped at [`MAX_AGENT_TIMEOUT_SECS`].
pub(crate) fn resolve_agent_timeout(configured: u32) -> Option<Duration> {
    let seconds = (configured as u64).min(MAX_AGENT_TIMEOUT_SECS);
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// Maps a finished run to the task/mission status. Only an exit-0 run is a
/// success: a user stop closes the mission as "stopped", while a non-zero exit
/// or a timeout is an "error" (it must not mark the mission accomplished).
pub(crate) fn task_status_for_run(run_status: &str) -> &'static str {
    match run_status {
        "success" => "success",
        "cancelled" => "stopped",
        _ => "error",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalBackend {
    Cli,
    Cloud,
}

impl ExternalBackend {
    fn label(self) -> &'static str {
        match self {
            ExternalBackend::Cli => "CLI",
            ExternalBackend::Cloud => "Cloud",
        }
    }
}

/// Runs a `cli` / `cloud` agent. The agent has no LLM loop of its own inside the
/// app: it receives one self-contained prompt, runs to completion (or the
/// timeout) and its output becomes the mission report.
pub(crate) async fn run_external_agent(
    app: &AppHandle,
    agent: &SubAgent,
    task: &Task,
    session_id: &str,
    task_cwd: PathBuf,
    exchange: Option<&exchange::ExchangeArea>,
    backend: ExternalBackend,
    cancelled: Arc<AtomicBool>,
    mbus: Option<&mbus::MissionBus>,
) -> TaskResult {
    let mission_id = mission_id_for_task(task).to_string();
    let _ = app.emit(
        "agent-task-start",
        json!({
            "task_id": task.id,
            "session_id": session_id,
            "agent_id": agent.id,
            "agent_name": agent.name,
            "description": task.description,
            "cwd": task.cwd,
        }),
    );

    {
        let state = app.state::<AppState>();
        let db = match state.db.lock() {
            Ok(db) => db,
            Err(err) => {
                let error = format!("Failed to lock database for mission initialization: {err}");
                let _ = app.emit(
                    "agent-task-error",
                    json!({
                        "task_id": task.id,
                        "session_id": session_id,
                        "agent_id": agent.id,
                        "error": error
                    }),
                );
                return TaskResult {
                    task_id: task.id.clone(),
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    status: "error".to_string(),
                    content: error,
                    tool_calls_count: 0,
                    tokens_used: 0,
                };
            }
        };

        if let Err(err) =
            initialize_mission_state(&db, session_id, agent, task, mbus.map(|bus| bus.bus_id()))
        {
            let _ = app.emit(
                "agent-task-error",
                json!({
                    "task_id": task.id,
                    "session_id": session_id,
                    "agent_id": agent.id,
                    "error": err
                }),
            );
            return TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: format!("Error: {}", err),
                tool_calls_count: 0,
                tokens_used: 0,
            };
        }
    }
    let _ = app.emit(
        "agent-task-state",
        json!({ "mission_id": mission_id, "session_id": session_id, "status": "running", "event": "initialized" }),
    );

    let mut prompt = build_external_prompt(
        agent,
        task,
        exchange,
        matches!(backend, ExternalBackend::Cli),
    );
    // External agents cannot call app tools, but a message queued before the
    // process starts can still be absorbed into its one-shot prompt.
    if let Some(bus) = mbus {
        let mut queued = bus.take_inbox(&task.id, mbus::MAX_DELIVERIES_PER_ROUND);
        // Close the race between reading the inbox and publishing `running`:
        // a sender that saw `pending` just before this point may have queued
        // one more message, so absorb it before the prompt is fixed.
        bus.mark_running(&task.id);
        queued.extend(bus.take_inbox(&task.id, mbus::MAX_DELIVERIES_PER_ROUND));
        if !queued.is_empty() {
            prompt.push_str("\n\nMISSION BUS MESSAGES (queued before you started):\n");
            for message in queued {
                prompt.push_str(&format!("- from {}: {}\n", message.from_name, message.body));
            }
        }
    }

    // CLI agents run in the task's working directory. Each run starts a
    // fresh CLI session; no session state is carried across agent calls.
    let cli_run = match backend {
        ExternalBackend::Cli => Some(CliRunContext {
            cwd: task_cwd.clone(),
        }),
        ExternalBackend::Cloud => None,
    };

    if agent.require_approval {
        if let Some(rejection) =
            request_external_agent_approval(app, agent, &prompt, backend, cli_run.as_ref()).await
        {
            let _ = app.emit(
                "agent-task-error",
                json!({
                    "task_id": task.id,
                    "session_id": session_id,
                    "agent_id": agent.id,
                    "error": rejection.clone()
                }),
            );
            return TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: "error".to_string(),
                content: rejection,
                tool_calls_count: 0,
                tokens_used: 0,
            };
        }
    }

    let outcome: Result<(&'static str, String, String), String> = match backend {
        ExternalBackend::Cli => {
            let cli_run = cli_run.as_ref().expect("CLI run context");
            cli::run(
                app,
                agent,
                &task.id,
                &prompt,
                cli_run.cwd.clone(),
                session_id,
                cancelled,
            )
            .await
            // The run status distinguishes "user stopped it" from a real
            // success so the mission is not marked accomplished on cancel.
            .map(|run| (run.status, run.output, run.summary))
            .map_err(|err| format!("CLI agent \"{}\" failed: {err}", agent.name))
        }
        ExternalBackend::Cloud => {
            cloud::run(app, agent, &task.id, &prompt, &task_cwd, session_id, cancelled)
                .await
                .map(|run| ("success", run.result, run.summary))
                .map_err(|err| format!("Cloud agent \"{}\" failed: {err}", agent.name))
        }
    };

    match outcome {
        Ok((run_status, content, summary)) => {
            let status = task_status_for_run(run_status);
            // A run the user stopped is not a success: the mission closes as
            // "stopped" (via run_sub_agent) instead of "accomplished". The same
            // holds for a non-zero exit or a timeout, which close it as
            // "failed" — only a real success records an accomplishment.
            if status == "success" {
                let state = app.state::<AppState>();
                let db = state.db.lock();
                if let Ok(db) = db {
                    let _ = mark_mission_accomplished(&db, &mission_id, Some(&content));
                }
            }
            // A failed/timeout run must show as an error in the agent panel and
            // the chat's tool-call group; the captured output still travels back
            // to the caller in `content`.
            if status == "error" {
                let _ = app.emit(
                    "agent-task-error",
                    json!({
                        "task_id": task.id,
                        "session_id": session_id,
                        "agent_id": agent.id,
                        "error": summary.chars().take(200).collect::<String>(),
                    }),
                );
            } else {
                let _ = app.emit(
                    "agent-task-done",
                    json!({
                        "task_id": task.id,
                        "session_id": session_id,
                        "agent_id": agent.id,
                        "agent_name": agent.name,
                        "summary": summary.chars().take(200).collect::<String>(),
                        // "success" | "stopped": the row must never show a
                        // completed checkmark for a run the user cancelled.
                        "status": status,
                    }),
                );
            }
            TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: status.to_string(),
                content,
                tool_calls_count: 0,
                tokens_used: 0,
            }
        }
        Err(err) => {
            // Cloud agents report a stop as an error message; surface the
            // mission as "stopped" rather than "failed" for those.
            let status = if err.contains("cancel") {
                "stopped"
            } else {
                "error"
            };
            let _ = app.emit(
                "agent-task-error",
                json!({
                    "task_id": task.id,
                    "session_id": session_id,
                    "agent_id": agent.id,
                    "error": err
                }),
            );
            TaskResult {
                task_id: task.id.clone(),
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                status: status.to_string(),
                content: format!("Error: {err}"),
                tool_calls_count: 0,
                tokens_used: 0,
            }
        }
    }
}

/// The single prompt handed to a `cli` / `cloud` agent: the task plus whatever
/// context the caller attached. External agents never see the chat history.
pub(crate) fn build_external_prompt(
    agent: &SubAgent,
    task: &Task,
    exchange: Option<&exchange::ExchangeArea>,
    local_files: bool,
) -> String {
    let mut prompt = String::new();
    if !agent.system_prompt.trim().is_empty() {
        prompt.push_str(agent.system_prompt.trim());
        prompt.push_str("\n\n");
    }
    prompt.push_str(task.description.trim());
    if !task.context.trim().is_empty() {
        prompt.push_str("\n\nContext:\n");
        prompt.push_str(task.context.trim());
    }
    if let Some(area) = exchange {
        if local_files {
            prompt.push_str(&area.prompt_block(&task.id, true));
        } else {
            prompt.push_str(
                "\n\nDELIVERABLE EXCHANGE AREA: not available — a cloud agent cannot access the \
                 local workspace. Return the deliverable inline in your answer.",
            );
        }
    }
    prompt
}

/// Trims a task-level `cwd` and drops the "workspace root" spellings the
/// planner may emit (`""`, `"."`, `"./"`, `".\\"`).
pub(crate) fn normalize_task_cwd(cwd: Option<&str>) -> Option<String> {
    let value = cwd?.trim();
    let value = value.trim_end_matches(['/', '\\']);
    if value.is_empty() || value == "." {
        return None;
    }
    Some(value.to_string())
}

/// Resolves the task's workspace-relative working directory. An unset `cwd`
/// means the workspace root; anything else is sandbox-checked with
/// `resolve_safe_path` and created when it does not exist yet (a task may
/// legitimately target a folder it is about to scaffold). This is the single
/// source of truth for where a sub-agent starts — the agent definition never
/// carries a directory of its own.
pub(crate) fn resolve_task_cwd(cwd: Option<&str>, workspace_dir: &Path) -> Result<PathBuf, String> {
    let Some(configured) = normalize_task_cwd(cwd) else {
        return Ok(workspace_dir.to_path_buf());
    };
    let path = crate::tools::resolve_safe_path(workspace_dir, &configured)?;
    if !path.is_dir() {
        std::fs::create_dir_all(&path).map_err(|err| {
            format!(
                "could not create the task's working directory '{}': {err}",
                path.display()
            )
        })?;
    }
    Ok(path)
}

/// Approval gate for `cli` / `cloud` agents: asked once per agent per app run
/// (unless `require_approval` is off, in which case the panel switch is the
/// standing authorization).
pub(crate) async fn request_external_agent_approval(
    app: &AppHandle,
    agent: &SubAgent,
    prompt: &str,
    backend: ExternalBackend,
    cli_run: Option<&CliRunContext>,
) -> Option<String> {
    let state = app.state::<AppState>();
    if state
        .external_agent_confirmed
        .lock()
        .map(|set| set.contains(&agent.id))
        .unwrap_or(false)
    {
        return None;
    }

    // Only one confirmation dialog can be pending at a time (single
    // `confirm_sender`), so parallel agents must queue here instead of
    // overwriting each other's request.
    let _guard = state.external_agent_confirm_lock.lock().await;

    if state
        .external_agent_confirmed
        .lock()
        .map(|set| set.contains(&agent.id))
        .unwrap_or(false)
    {
        return None;
    }

    let target = match backend {
        ExternalBackend::Cli => match cli_run {
            // The approval text must show the directory the agent will actually
            // start in.
            Some(cli_run) => cli::describe_run(agent, prompt, &cli_run.cwd)
                .map(|command| format!("It runs with your user permissions and can read and write files in the workspace.\n\n{}", command))
                .unwrap_or_else(|err| format!("Its command could not be resolved yet: {err}")),
            None => "Its working directory could not be resolved yet.".to_string(),
        },
        ExternalBackend::Cloud => format!(
            "It sends the task to a remote endpoint and can read and write files there.\n\nPOST {}",
            agent.cloud.endpoint.trim()
        ),
    };

    let confirm = crate::tools::request_tool_confirmation(
        app,
        format!(
            "The {} agent '{}' is about to run.\n\n{}",
            backend.label(),
            agent.name,
            target
        ),
        "external_agent".to_string(),
        target.clone(),
        "external_agent",
        "none",
        None,
    )
    .await;

    if !confirm.confirmed {
        return Some(format!(
            "⛔ The user rejected running the {} agent '{}'. Do not retry it; ask the user how to proceed or do the work with the available tools.",
            backend.label(),
            agent.name
        ));
    }

    if let Ok(mut set) = state.external_agent_confirmed.lock() {
        set.insert(agent.id.clone());
    }
    None
}

pub(crate) async fn run_sub_agent_inner(
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
    mbus: Option<&mbus::MissionBus>,
) -> TaskResult {
    // Every task gets a private MCP process pool. The pool is shut down before
    // the task result leaves this function, so a finished/stopped/failed task
    // never leaves an MCP server behind. `Drop` covers unwind paths.
    let state = app.state::<AppState>();
    let mut mcp_session = mcp::AgentMcpSession::new();
    let result = run_llm_agent_body(
        app,
        client,
        url,
        config,
        session_id,
        agent,
        task,
        workspace_dir,
        task_cwd,
        exchange,
        skill_access_root,
        cancelled,
        &mut mcp_session,
        mbus,
    )
    .await;
    mcp_session.shutdown(&state);
    result
}
