//! Silent evolution-maintenance turn.
//!
//! Clicking the empty-state smiley starts a maintenance pass: one bounded LLM
//! tool loop that tidies the workspace's long-term knowledge (self-evolution
//! skills and the `evolution` memory scope). It is deliberately **not** a chat
//! turn:
//!
//! * it never writes chat history or a transcript row, so the empty state (and
//!   the napping face) stays visible while it runs;
//! * it emits its own `evolution-maintenance-state` events instead of the
//!   `chat-*` stream, so a maintenance `chat-done` can never close a real chat
//!   reply that happens to run at the same time;
//! * it owns a separate cancel flag (`AppState::evolution_maintenance`), so
//!   double-clicking the face stops only the maintenance. Starting a chat turn
//!   cancels it explicitly — the user's conversation always wins.
//!
//! Tools are hard-restricted to `memory`, `file_actions` and the
//! maintenance-only `skill_merge`. `file_actions` writes are limited to
//! auxiliary files inside the skills root (never `skill.md`, never
//! `.data/memory`, never `skills.status`); deletes and moves are refused, so
//! every skill rewrite goes through `skill_merge` and keeps a `.bak.N` copy.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::llm_complete::{
    apply_completion_token_limit, build_stream_client, resolve_reasoning_effort,
    sanitize_tool_pairs, stream_llm_request, upstream_request_headers, StreamOptions,
};
use crate::tools;
use crate::workspace;
use crate::AppConfig;
use crate::AppState;

/// Mission id passed to `execute_tool` while the maintenance loop runs.
///
/// It routes tool progress to `agent-task-tool` (no main-chat transcript row)
/// and is the dispatcher's guard for the maintenance-only `skill_merge` tool.
pub(crate) const MISSION_ID: &str = "evolution-maintenance";

const MAX_ROUNDS: u32 = 20;
const MAX_TOOL_CALLS: u32 = 60;
const MAX_WALL_CLOCK: Duration = Duration::from_secs(15 * 60);
const MAX_COMPLETION_TOKENS: u32 = 8_192;
const MAX_SKILLS_IN_INVENTORY: usize = 40;
const MAX_INVENTORY_DESCRIPTION_CHARS: usize = 140;

/// Injected as a `user` message at the last request when the round / tool-call
/// budget is nearly spent. `tool_choice: "none"` then forces a final report
/// instead of another tool round — the pass ends with whatever is already
/// saved rather than as a failure.
const BUDGET_WRAP_UP_PROMPT: &str = "INTERNAL BUDGET NOTICE — the evolution maintenance pass has reached its round/tool budget. Do not call any more tools. Finish the current work with what is already saved and write the final report now: what was merged, archived or deleted, and what was deferred.";
const BUDGET_FALLBACK: &str = "Evolution maintenance reached its round/tool budget before writing a report. Completed merges are already saved; run it again to continue the rest.";

/// Tools the maintenance loop may actually execute. The model is advertised
/// exactly these; this list is the hard gate if it invents another name.
const ALLOWED_TOOLS: [&str; 3] = ["memory", "file_actions", "skill_merge"];

/// True when the next request must be the wrap-up request: the last round, or
/// fewer than two tool calls left in the budget.
fn budget_is_low(round: u32, tool_calls: u32) -> bool {
    round >= MAX_ROUNDS || tool_calls.saturating_add(2) >= MAX_TOOL_CALLS
}

/// Status phase announced while a tool is running. `None` for a tool the
/// maintenance turn does not allow, so the previous phase stays on screen.
fn phase_for_tool(name: &str) -> Option<&'static str> {
    match name {
        "skill_merge" => Some("skills"),
        "memory" => Some("memory"),
        "file_actions" => Some("files"),
        _ => None,
    }
}

/// Announce live progress on the same event stream the face listens to.
///
/// `running: true` keeps the face napping; `phase`, `round` and `toolCalls`
/// drive the two-line status block that replaces the empty-state greeting
/// while the pass runs. The terminal event (`running: false`) restores it.
fn emit_progress(app: &AppHandle, phase: &str, round: u32, tool_calls: u32) {
    let _ = app.emit(
        "evolution-maintenance-state",
        json!({
            "running": true,
            "phase": phase,
            "round": round,
            "toolCalls": tool_calls,
        }),
    );
}

/// Save a resume note into the evolution memory before a partial pass ends.
///
/// The note is a normal evolution entry with a stable key, so the next pass
/// reads it from its inventory and continues instead of starting over. It is
/// excluded from consolidation plans and cannot be deleted by the `memory`
/// tool (see `memory::MAINTENANCE_PROGRESS_KEY`).
#[allow(clippy::too_many_arguments)]
fn record_progress(
    state: &AppState,
    workspace_dir: &Path,
    reason: &str,
    round: u32,
    tool_calls_count: u32,
    total_tokens: u32,
    started: Instant,
    working_memory: &[Value],
) {
    let activity = summarize_recent_activity(working_memory);
    let content = format!(
        "Evolution maintenance was interrupted before completion.\n\
         Reason: {reason}\n\
         Stopped at round {round}, tool_calls={tool_calls_count}, tokens={total_tokens}, elapsed={}s.\n\n\
         {activity}\n\n\
         Resume instructions: verify the current skills and evolution memory, then continue the \
         deferred skill merges and memory clusters. Do not redo work that is already saved. Do not \
         archive or delete this entry; it is the resume note.",
        started.elapsed().as_secs()
    );
    let tags = vec![
        "evolution-maintenance".to_string(),
        "progress".to_string(),
        "resume".to_string(),
    ];
    match save_progress_entry(workspace_dir, &content, &tags) {
        Ok(()) => log(
            state,
            "INFO",
            format!("evolution maintenance progress saved: {reason}"),
        ),
        Err(error) => log(
            state,
            "WARN",
            format!("failed to save evolution maintenance progress: {error}"),
        ),
    }
}

/// Write the resume note into the evolution memory. Kept separate from
/// `record_progress` so the persistence path is testable without a Tauri
/// `AppState`.
fn save_progress_entry(workspace_dir: &Path, content: &str, tags: &[String]) -> Result<(), String> {
    crate::tools::memory::upsert_evolution_entry(
        workspace_dir,
        crate::tools::memory::MAINTENANCE_PROGRESS_KEY,
        content,
        "Evolution maintenance progress (resume note)",
        "maintenance",
        Some(5),
        false,
        tags,
        false,
    )
    .map(|_| ())
}

/// Remove the resume note after a completed pass.
fn clear_progress(state: &AppState, workspace_dir: &Path) {
    if crate::tools::memory::clear_evolution_entry(
        workspace_dir,
        crate::tools::memory::MAINTENANCE_PROGRESS_KEY,
    ) {
        log(state, "INFO", "evolution maintenance progress cleared");
    }
}

fn summarize_recent_activity(working_memory: &[Value]) -> String {
    let mut calls: Vec<String> = Vec::new();
    for message in working_memory.iter().rev() {
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for call in tool_calls.iter().rev() {
                let name = call["function"]["name"].as_str().unwrap_or("?");
                let args = call["function"]["arguments"].as_str().unwrap_or("");
                calls.push(format!("- {name}: {}", clip(args, 180)));
                if calls.len() >= 8 {
                    break;
                }
            }
        }
        if calls.len() >= 8 {
            break;
        }
    }
    if calls.is_empty() {
        return "No tool calls had completed before the stop.".to_string();
    }
    calls.reverse();
    format!("Recent tool activity (oldest first):\n{}", calls.join("\n"))
}

/// Record the resume note and return the `stopped` sentinel used by the task's
/// terminal handler.
#[allow(clippy::too_many_arguments)]
fn stopped_with_progress(
    state: &AppState,
    workspace_dir: &Path,
    round: u32,
    tool_calls_count: u32,
    total_tokens: u32,
    started: Instant,
    working_memory: &[Value],
) -> Result<String, String> {
    record_progress(
        state,
        workspace_dir,
        "stopped by user",
        round,
        tool_calls_count,
        total_tokens,
        started,
        working_memory,
    );
    Ok("stopped".to_string())
}

/// RAII slot release: a panic or an early return can never leave the face
/// napping forever with a stale "running" state.
struct SlotGuard<'a>(&'a Mutex<Option<Arc<AtomicBool>>>);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = None;
        }
    }
}

fn log(state: &AppState, level: &str, message: impl Into<String>) {
    if let Ok(logger) = state.logger.lock() {
        logger.log(level, &message.into());
    }
}

/// Cancel a running maintenance pass (used by a chat turn, a workspace switch
/// and turning self-evolution mode off).
pub(crate) fn cancel(state: &AppState) {
    if let Ok(slot) = state.evolution_maintenance.lock() {
        if let Some(flag) = slot.as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

/// Start a maintenance pass. Returns immediately; progress and the outcome
/// arrive through `evolution-maintenance-state`.
#[tauri::command]
pub fn start_evolution_maintenance(
    app: AppHandle,
    state: State<'_, AppState>,
    model_override: Option<String>,
) -> Result<(), String> {
    let config = state.config.lock().unwrap().clone();
    if !config.self_evolution_mode {
        return Err(
            "Self-evolution mode is off. Enable it in Settings → Workspace first.".to_string(),
        );
    }
    if !config.effective_tools().iter().any(|tool| tool == "memory") {
        return Err(
            "The memory tool is disabled. Enable self-evolution mode (it forces memory on) and try again."
                .to_string(),
        );
    }
    if state
        .active_turn
        .lock()
        .map(|turn| turn.is_some())
        .unwrap_or(false)
    {
        return Err(
            "A chat turn is running. Finish or stop it before consolidating memory.".to_string(),
        );
    }

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state.evolution_maintenance.lock().unwrap();
        if slot.is_some() {
            return Err("Evolution maintenance is already running.".to_string());
        }
        *slot = Some(cancel.clone());
    }

    let _ = app.emit(
        "evolution-maintenance-state",
        json!({
            "running": true,
            "phase": "inventory",
            "round": 1,
            "toolCalls": 0,
        }),
    );

    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app_for_task.state::<AppState>();
        let slot_guard = SlotGuard(&state.evolution_maintenance);
        let sleep_guard = app_for_task
            .state::<crate::tools::timer::SleepLock>()
            .acquire();

        let result = run_inner(&app_for_task, &state, config, model_override, cancel).await;

        drop(sleep_guard);
        // Release the slot before announcing the outcome: a reloaded webview
        // that asks for the state must not see a finished run as "running".
        drop(slot_guard);

        let payload = match result {
            Ok(summary) if summary == "stopped" => {
                log(&state, "INFO", "evolution maintenance stopped by user");
                json!({ "running": false, "outcome": "stopped" })
            }
            Ok(summary) => {
                log(
                    &state,
                    "INFO",
                    format!(
                        "evolution maintenance finished: {}",
                        clip(&summary, 500)
                    ),
                );
                json!({ "running": false, "outcome": "done", "summary": summary })
            }
            Err(error) => {
                log(
                    &state,
                    "WARN",
                    format!("evolution maintenance failed: {error}"),
                );
                json!({ "running": false, "outcome": "error", "error": error })
            }
        };
        let _ = app_for_task.emit("evolution-maintenance-state", payload);
    });

    Ok(())
}

/// Stop the maintenance pass that is currently running (double-click on the
/// napping face).
#[tauri::command]
pub fn stop_evolution_maintenance(state: State<'_, AppState>) {
    cancel(&state);
}

/// Whether a maintenance pass is currently running. A webview reload loses the
/// in-flight invoke, so the UI asks this on mount and restores the napping
/// state instead of showing an idle face over a running pass.
#[tauri::command]
pub fn get_evolution_maintenance_state(state: State<'_, AppState>) -> bool {
    state
        .evolution_maintenance
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or(false)
}

async fn run_inner(
    app: &AppHandle,
    state: &AppState,
    config: AppConfig,
    model_override: Option<String>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
    let skills_root = workspace::skills_root(&workspace_dir);
    let model = model_override
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| config.model.clone());

    let inventory = build_inventory(&workspace_dir);
    let system_prompt = build_system_prompt(&workspace_dir);
    let initial_user = format!(
        "Run the maintenance pass now.\n\nInitial inventory:\n{inventory}"
    );

    let mut working_memory: Vec<Value> = vec![json!({
        "role": "user",
        "content": initial_user,
    })];

    let mut tools_list = tools::get_all_tools(
        &["memory".to_string(), "file_actions".to_string()],
        true,
        false,
    );
    tools_list.push(tools::skill_admin::merge_tool_definition());

    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
    let client = build_stream_client()?;
    let headers = upstream_request_headers(&config, MISSION_ID);
    let started = Instant::now();
    let mut total_tokens: u32 = 0;
    let mut tool_calls_count: u32 = 0;
    let mut budget_wrap_up_sent = false;
    let mut phase = "inventory";

    log(
        state,
        "INFO",
        format!(
            "evolution maintenance started: workspace={}, model={}",
            workspace_dir.display(),
            model
        ),
    );

    for round in 1..=MAX_ROUNDS {
        if cancel.load(Ordering::SeqCst) {
            return stopped_with_progress(
                state,
                &workspace_dir,
                round,
                tool_calls_count,
                total_tokens,
                started,
                &working_memory,
            );
        }
        if started.elapsed() > MAX_WALL_CLOCK {
            record_progress(
                state,
                &workspace_dir,
                "timed out",
                round,
                tool_calls_count,
                total_tokens,
                started,
                &working_memory,
            );
            return Err(format!(
                "Evolution maintenance timed out after {} minutes without finishing.",
                MAX_WALL_CLOCK.as_secs() / 60
            ));
        }

        // Reserve the last request for a report: once the round or tool-call
        // budget is nearly spent, append the budget notice and force
        // `tool_choice: "none"`. The pass then ends with a summary (and any
        // work already saved) instead of a "failed after N rounds" error.
        let budget_low = budget_is_low(round, tool_calls_count);
        if budget_low {
            phase = "wrap_up";
        }
        emit_progress(app, phase, round, tool_calls_count);
        if budget_low && !budget_wrap_up_sent {
            working_memory.push(json!({
                "role": "user",
                "content": BUDGET_WRAP_UP_PROMPT,
            }));
            budget_wrap_up_sent = true;
            log(
                state,
                "INFO",
                format!(
                    "evolution maintenance budget low: wrap-up request at round {round}, tool_calls={tool_calls_count}"
                ),
            );
        }

        let mut messages = vec![json!({
            "role": "system",
            "content": system_prompt,
        })];
        messages.extend(working_memory.iter().cloned());
        sanitize_tool_pairs(&mut messages);

        let mut req_body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        apply_completion_token_limit(
            &mut req_body,
            Some(MAX_COMPLETION_TOKENS),
            Some(MAX_COMPLETION_TOKENS),
        );
        if let Some(temp) = config.model_settings.temperature {
            req_body["temperature"] = json!(temp);
        }
        if let Some(effort) = resolve_reasoning_effort(&config, &model) {
            req_body["reasoning_effort"] = json!(effort);
        }
        req_body["tools"] = json!(tools_list);
        req_body["tool_choice"] = json!(if budget_low { "none" } else { "auto" });

        log(
            state,
            "INFO",
            format!(
                "evolution maintenance round {round}: messages={}, tools={}, tool_calls_so_far={}",
                working_memory.len() + 1,
                tools_list.len(),
                tool_calls_count
            ),
        );

        let stream_result = stream_llm_request(
            app,
            &client,
            &url,
            &config.api_key,
            req_body,
            &cancel,
            StreamOptions {
                // Never emitted (`emit_tokens: false`), but the field is
                // mandatory for every request built by this helper.
                token_event: "evolution-maintenance-token",
                task_id: None,
                // The maintenance pass is silent: it emits its own
                // `evolution-maintenance-*` events, never chat events.
                session_id: None,
                reasoning_event: None,
                emit_usage: false,
                emit_tokens: false,
                emit_errors: false,
                usage_max_tokens: None,
                ds_format: config.ds_format_for(&model),
                headers: &headers,
            },
        )
        .await;

        let result = match stream_result {
            Ok(result) => result,
            Err(error) => {
                if cancel.load(Ordering::SeqCst) {
                    return stopped_with_progress(
                        state,
                        &workspace_dir,
                        round,
                        tool_calls_count,
                        total_tokens,
                        started,
                        &working_memory,
                    );
                }
                if budget_low {
                    record_progress(
                        state,
                        &workspace_dir,
                        "budget exhausted",
                        round,
                        tool_calls_count,
                        total_tokens,
                        started,
                        &working_memory,
                    );
                    log(
                        state,
                        "WARN",
                        format!("evolution maintenance wrap-up request failed: {error}"),
                    );
                    return Ok(BUDGET_FALLBACK.to_string());
                }
                record_progress(
                    state,
                    &workspace_dir,
                    "upstream error",
                    round,
                    tool_calls_count,
                    total_tokens,
                    started,
                    &working_memory,
                );
                return Err(error);
            }
        };
        total_tokens = total_tokens
            .saturating_add(result.prompt_tokens)
            .saturating_add(result.completion_tokens);

        if result.finish_reason == "cancelled" || cancel.load(Ordering::SeqCst) {
            return stopped_with_progress(
                state,
                &workspace_dir,
                round,
                tool_calls_count,
                total_tokens,
                started,
                &working_memory,
            );
        }

        if result.tool_calls.is_empty() {
            let answer = result.content.trim().to_string();
            if !answer.is_empty() {
                working_memory.push(json!({ "role": "assistant", "content": answer }));
            }
            log(
                state,
                "INFO",
                format!(
                    "evolution maintenance finished: rounds={round}, tool_calls={tool_calls_count}, tokens={total_tokens}"
                ),
            );
            if budget_low {
                record_progress(
                    state,
                    &workspace_dir,
                    "budget exhausted",
                    round,
                    tool_calls_count,
                    total_tokens,
                    started,
                    &working_memory,
                );
            } else {
                clear_progress(state, &workspace_dir);
            }
            return Ok(if answer.is_empty() {
                if budget_low {
                    BUDGET_FALLBACK.to_string()
                } else {
                    "Evolution maintenance finished without a report.".to_string()
                }
            } else {
                answer
            });
        }

        if budget_low {
            // A gateway that ignored `tool_choice: "none"` must not start new
            // work at the very end of the pass.
            log(
                state,
                "WARN",
                format!(
                    "evolution maintenance wrap-up returned {} tool call(s); ignoring them",
                    result.tool_calls.len()
                ),
            );
            record_progress(
                state,
                &workspace_dir,
                "budget exhausted",
                round,
                tool_calls_count,
                total_tokens,
                started,
                &working_memory,
            );
            return Ok(BUDGET_FALLBACK.to_string());
        }

        let assistant_tool_calls: Vec<Value> = result
            .tool_calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args },
                })
            })
            .collect();
        let assistant_content = if result.content.trim().is_empty() {
            Value::Null
        } else {
            Value::String(result.content.clone())
        };
        let mut assistant_message = json!({
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": assistant_tool_calls,
        });
        if !result.reasoning_content.is_empty() {
            assistant_message["reasoning_content"] = json!(result.reasoning_content.as_str());
        }
        working_memory.push(assistant_message);

        let mut budget_hit = false;
        for (id, name, args) in &result.tool_calls {
            if cancel.load(Ordering::SeqCst) {
                return stopped_with_progress(
                    state,
                    &workspace_dir,
                    round,
                    tool_calls_count,
                    total_tokens,
                    started,
                    &working_memory,
                );
            }
            if tool_calls_count >= MAX_TOOL_CALLS {
                budget_hit = true;
                working_memory.push(json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": "Error: tool-call budget exhausted; do not call more tools. Write the final report now.",
                }));
                continue;
            }
            tool_calls_count = tool_calls_count.saturating_add(1);
            if let Some(next_phase) = phase_for_tool(name) {
                phase = next_phase;
                emit_progress(app, phase, round, tool_calls_count);
            }

            let tool_result = if !ALLOWED_TOOLS.contains(&name.as_str()) {
                format!(
                    "Error: tool '{name}' is not available in the evolution maintenance turn. Use memory, file_actions or skill_merge."
                )
            } else if name == "file_actions" {
                match file_action_guard(&workspace_dir, &skills_root, args) {
                    Ok(()) => {
                        tools::execute_tool(
                            app,
                            name,
                            args,
                            workspace_dir.clone(),
                            &config,
                            &[],
                            Some(skills_root.as_path()),
                            Some(MISSION_ID),
                            Some(MISSION_ID),
                            &[],
                            Some(&cancel),
                            None,
                        )
                        .await
                    }
                    Err(error) => error,
                }
            } else {
                tools::execute_tool(
                    app,
                    name,
                    args,
                    workspace_dir.clone(),
                    &config,
                    &[],
                    Some(skills_root.as_path()),
                    Some(MISSION_ID),
                    Some(MISSION_ID),
                    &[],
                    Some(&cancel),
                    None,
                )
                .await
            };

            log(
                state,
                "INFO",
                format!(
                    "evolution maintenance tool {tool_calls_count}: {name} → {}",
                    clip(&tool_result, 300)
                ),
            );
            working_memory.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": tool_result,
            }));
        }

        if budget_hit {
            // The next iteration sees `budget_low` and sends the wrap-up
            // request; the tool-call round is already answered, so the message
            // sequence stays valid.
            continue;
        }
    }

    record_progress(
        state,
        &workspace_dir,
        "budget exhausted",
        MAX_ROUNDS,
        tool_calls_count,
        total_tokens,
        started,
        &working_memory,
    );
    Ok(BUDGET_FALLBACK.to_string())
}

/// Hard restrictions on `file_actions` inside the maintenance turn:
///
/// * read-only actions are free;
/// * `write` / `patch` / `mkdir` may only touch auxiliary files inside the
///   skills root — `skill.md` itself is reserved for `skill_merge`, and the
///   memory store / status file belong to the `memory` tool;
/// * `delete` / `rename` / `move` are refused outright (skill folders are
///   retired by `skill_merge`, which validates the source and keeps a backup).
fn file_action_guard(
    workspace_dir: &Path,
    skills_root: &Path,
    args_str: &str,
) -> Result<(), String> {
    let args: Value = serde_json::from_str(args_str).unwrap_or_default();
    let action = args["action"].as_str().unwrap_or("");
    match action {
        "read" | "list" | "search" | "diff" => Ok(()),
        "write" | "patch" | "mkdir" => {
            let path = args["path"].as_str().unwrap_or("").trim();
            if path.is_empty() {
                return Err("Error: 'path' is required.".to_string());
            }
            let resolved = crate::tools::resolve_safe_path(workspace_dir, path)?;
            let canonical_skills = skills_root
                .canonicalize()
                .unwrap_or_else(|_| skills_root.to_path_buf());
            if !resolved.starts_with(&canonical_skills) && !resolved.starts_with(skills_root) {
                return Err(format!(
                    "Error: evolution maintenance may only write inside the skills root ({}). \
                     Use the memory tool for memory and skill_merge for skills.",
                    skills_root.display()
                ));
            }
            if action != "mkdir"
                && resolved
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.eq_ignore_ascii_case("skill.md"))
                    .unwrap_or(false)
            {
                return Err(
                    "Error: write/patch of a skill.md is reserved for skill_merge, which validates \
                     the source and keeps a backup."
                        .to_string(),
                );
            }
            Ok(())
        }
        "delete" | "rename" | "move" => Err(format!(
            "Error: '{action}' is disabled during evolution maintenance. Retire a skill folder \
             with skill_merge, which validates the source and keeps a .bak.N copy."
        )),
        other => Err(format!(
            "Error: file_actions action '{other}' is not available during evolution maintenance."
        )),
    }
}

fn build_system_prompt(workspace_dir: &Path) -> String {
    let skills_root = workspace::skills_root(workspace_dir);
    let status_path = workspace::skills_status_path(workspace_dir);
    let memory_root = workspace::memory_root(workspace_dir);
    format!(
        "You are the workspace evolution maintainer of Rabbit Chat Studio. This is a background maintenance \
         pass, not a conversation: the user is not present, nobody will answer a question, and \
         your work is applied directly to the workspace. Work autonomously and finish with a \
         short report.\n\n\
         Workspace: {workspace}\n\
         Skills root: {skills}\n\
         Skill status: {status}\n\
         Evolution memory: {memory}\n\n\
         You have exactly three tools: `memory`, `file_actions` and `skill_merge`.\n\n\
         GOAL: keep the workspace's long-term knowledge compact, non-duplicated and reusable.\n\n\
         PROCEDURE\n\
         1. Inventory. Read the skill status file and list the skills root. For each skill note \
         its folder, frontmatter name, description, enabled state and source (`user` or \
         `self-evolution`).\n\
         2. Merge overlapping self-evolution skills. Two self-evolution skills that cover the \
         same subject (same tool, same workflow, same project area) should become one focused \
         skill. Handle at most 3 overlapping groups per pass; report any remaining overlaps as \
         deferred instead of starting them:\n\
            - Read the source SKILL.md files.\n\
            - Choose an existing self-evolution folder as the target (prefer the broader or \
              enabled one). Keep that folder as `target` even when its frontmatter name differs.\n\
            - Copy any auxiliary files the merged skill needs into the target folder with \
              `file_actions` (read + write); never write `skill.md` with `file_actions`.\n\
            - Write the complete merged SKILL.md: valid YAML frontmatter (`name`, `description`) \
              plus the full body. Preserve every concrete command, path and gotcha from the \
              sources. Omit `allowed-commands` / `allowed-tools` unless every source agrees. \
              Pass the raw document as `skill_md` — do not wrap it in a Markdown code fence.\n\
            - Call `skill_merge` with `target`, `sources` and `skill_md`. It validates that every \
              skill is self-evolution, writes and enables the target, and archives each source \
              folder as `<folder>.bak.N` before deleting it. Never delete a skill with \
              `file_actions`.\n\
            - Never merge unrelated skills just to reduce the count.\n\
         3. Consolidate evolution memory. Call `memory` with action `consolidate`, scope \
         `evolution`, phase `plan`. Handle at most 3 clusters per pass; if more are eligible, do \
         the three largest and report the rest as deferred. For every cluster worth keeping:\n\
            - Write one skill whose frontmatter `source-memory` lists every key in the cluster.\n\
            - Call `memory` action `consolidate`, scope `evolution`, phase `apply` with the same \
              `keys` and the full `skill_md`.\n\
            - One apply per cluster. Do not archive keys that are not in the plan. If the plan \
              says no cluster is large enough, skip this step.\n\
         4. Merge duplicate evolution memory entries. Use `memory list` / `search` on the \
         evolution scope. If several entries state the same fact, write one dense entry with \
         `memory add` (upsert) and then delete the redundant keys with `memory delete`. Keep the \
         highest-confidence wording; never invent facts. Never edit memory files with \
         `file_actions`.\n\
         5. Verify and report. List skills and evolution memory again. If you are running low on \
         rounds or tool calls, stop starting new work and report what is already saved. Report:\n\
            - skills merged (old → new) or \"none\";\n\
            - clusters archived (keys → skill) or \"none\";\n\
            - memory entries merged/deleted or \"none\";\n\
            - anything skipped and why.\n\n\
         RULES\n\
         - Only `memory`, `file_actions` and `skill_merge`. Never run commands, never touch \
           `.data/chat.db`, never edit files outside the workspace.\n\
         - If the inventory contains a \"Previous maintenance progress\" note, treat it as \
           authoritative: verify what is already saved, then continue the deferred items instead \
           of starting over.\n\
         - Never archive, delete or merge the `evolution-maintenance-progress` memory entry. It is \
           the resume note and is excluded from consolidation plans on purpose.\n\
         - Do not modify a skill whose status source is `user` (the Skills panel owns it) or whose \
           source is unknown. Refuse rather than guess.\n\
         - Do not change `skills.status` by hand; `skill_merge` and the memory tool manage it.\n\
         - Do not delete a skill until its replacement is written and the merged content covers \
           the original's important details.\n\
         - Do not ask questions. If there is nothing to do, say \"Nothing to consolidate.\" and stop.\n\
         - Keep the final report under 200 words.",
        workspace = workspace_dir.display(),
        skills = skills_root.display(),
        status = status_path.display(),
        memory = memory_root.display(),
    )
}

fn build_inventory(workspace_dir: &Path) -> String {
    let skills_root = workspace::skills_root(workspace_dir);
    let status = fs::read_to_string(workspace::skills_status_path(workspace_dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<crate::skills::SkillsStatus>(&raw).ok());

    let mut skills: Vec<String> = Vec::new();
    let mut total_skills = 0usize;
    if let Ok(entries) = fs::read_dir(&skills_root) {
        let mut folders: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            let Some(skill_file) = find_skill_file(&folder) else {
                continue;
            };
            total_skills += 1;
            if skills.len() >= MAX_SKILLS_IN_INVENTORY {
                continue;
            }
            let folder_name = folder
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_string();
            let parsed = fs::read_to_string(&skill_file)
                .ok()
                .and_then(|raw| crate::skills::parse_skill_md(&raw).ok());
            let (name, description) = match parsed {
                Some(skill) => (skill.name, skill.description),
                None => (folder_name.clone(), "(invalid or missing frontmatter)".to_string()),
            };
            let entry = status.as_ref().and_then(|status| status.skills.get(&name));
            let source = entry
                .map(|entry| entry.source.trim())
                .filter(|source| !source.is_empty())
                .unwrap_or("unknown");
            let enabled = entry.map(|entry| entry.enabled).unwrap_or(false);
            skills.push(format!(
                "- folder: {folder_name} | name: {name} | source: {source} | enabled: {enabled} | description: {}",
                clip(&description, MAX_INVENTORY_DESCRIPTION_CHARS)
            ));
        }
    }

    let mut out = String::from("Skills:\n");
    if skills.is_empty() {
        out.push_str("- (none)\n");
    } else {
        for line in &skills {
            out.push_str(line);
            out.push('\n');
        }
        if total_skills > skills.len() {
            out.push_str(&format!(
                "- (+{} more skill(s) not listed)\n",
                total_skills - skills.len()
            ));
        }
    }

    let ranked = crate::tools::memory::ranked_evolution_entries(workspace_dir);
    let clusters = crate::tools::memory::plan_clusters(
        &ranked,
        crate::tools::memory::EVOLUTION_CONTEXT_BUDGET_CHARS,
    );
    out.push_str(&format!(
        "\nEvolution memory: {} entries, {} chars. Eligible clusters: {}.\n",
        ranked.len(),
        ranked.iter().map(|scored| scored.entry.body.chars().count()).sum::<usize>(),
        clusters.len()
    ));
    for cluster in clusters.iter().take(8) {
        out.push_str(&format!(
            "- cluster {}: {} entries, suggested skill `{}`, keys: {}\n",
            cluster.id,
            cluster.keys.len(),
            cluster.suggested_skill,
            cluster.keys.join(", ")
        ));
    }
    if clusters.is_empty() {
        out.push_str(
            "- no cluster meets the consolidation threshold yet (>= 4 entries and >= 20% of the prompt budget)\n",
        );
    }

    // The previous pass' resume note is the first thing the next pass should
    // read: it names what was already saved and what was deferred.
    let progress = crate::tools::memory::load_evolution_entries(workspace_dir)
        .into_iter()
        .find(|entry| entry.meta.key == crate::tools::memory::MAINTENANCE_PROGRESS_KEY)
        .map(|entry| entry.body)
        .unwrap_or_default();
    if !progress.trim().is_empty() {
        out.push_str("\nPrevious maintenance progress (resume from here):\n");
        out.push_str(&clip(&progress, 3_000));
        out.push('\n');
    }
    out
}

fn find_skill_file(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.eq_ignore_ascii_case("skill.md"))
                .unwrap_or(false)
        {
            return Some(path);
        }
    }
    None
}

fn clip(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-evo-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(workspace: &Path, folder: &str, name: &str, description: &str) {
        let dir = workspace::skills_root(workspace).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("skill.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n"),
        )
        .unwrap();
    }

    #[test]
    fn budget_reserves_the_last_request_for_the_wrap_up() {
        assert!(!budget_is_low(1, 0));
        assert!(!budget_is_low(1, MAX_TOOL_CALLS - 3));
        assert!(budget_is_low(1, MAX_TOOL_CALLS - 2));
        assert!(budget_is_low(MAX_ROUNDS, 0));
    }

    #[test]
    fn tool_names_map_to_progress_phases() {
        assert_eq!(phase_for_tool("skill_merge"), Some("skills"));
        assert_eq!(phase_for_tool("memory"), Some("memory"));
        assert_eq!(phase_for_tool("file_actions"), Some("files"));
        assert_eq!(phase_for_tool("run_cmd"), None);
    }

    #[test]
    fn inventory_lists_skills_and_their_status_source() {
        let workspace = temp_workspace("inventory");
        write_skill(&workspace, "demo", "demo-skill", "does demo things");
        fs::write(
            workspace::skills_status_path(&workspace),
            r#"{"version":1,"skills":{"demo-skill":{"enabled":true,"source":"self-evolution"}}}"#,
        )
        .unwrap();

        let inventory = build_inventory(&workspace);
        assert!(inventory.contains("folder: demo"));
        assert!(inventory.contains("name: demo-skill"));
        assert!(inventory.contains("source: self-evolution"));
        assert!(inventory.contains("enabled: true"));
        assert!(inventory.contains("does demo things"));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn progress_note_round_trips_through_the_inventory() {
        let workspace = temp_workspace("progress");
        let tags = vec!["evolution-maintenance".to_string(), "progress".to_string()];
        save_progress_entry(
            &workspace,
            "Reason: stopped by user\nLast action: skill_merge target=alpha",
            &tags,
        )
        .unwrap();

        let entries = crate::tools::memory::load_evolution_entries(&workspace);
        assert!(entries
            .iter()
            .any(|entry| entry.meta.key == crate::tools::memory::MAINTENANCE_PROGRESS_KEY));

        let inventory = build_inventory(&workspace);
        assert!(inventory.contains("Previous maintenance progress"));
        assert!(inventory.contains("Last action: skill_merge target=alpha"));

        assert!(crate::tools::memory::clear_evolution_entry(
            &workspace,
            crate::tools::memory::MAINTENANCE_PROGRESS_KEY
        ));
        let entries = crate::tools::memory::load_evolution_entries(&workspace);
        assert!(!entries
            .iter()
            .any(|entry| entry.meta.key == crate::tools::memory::MAINTENANCE_PROGRESS_KEY));

        fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn file_actions_guard_blocks_skill_md_and_destructive_actions() {
        let workspace = temp_workspace("guard");
        fs::create_dir_all(workspace::skills_root(&workspace)).unwrap();

        let blocked = |action: &str, path: &str| {
            file_action_guard(
                &workspace,
                &workspace::skills_root(&workspace),
                &json!({ "action": action, "path": path }).to_string(),
            )
        };

        assert!(blocked("write", ".data/skills/demo/skill.md").is_err());
        assert!(blocked("patch", ".data/skills/demo/skill.md").is_err());
        assert!(blocked("write", "notes.txt").is_err());
        assert!(blocked("delete", ".data/skills/demo").is_err());
        assert!(blocked("rename", ".data/skills/demo").is_err());
        assert!(blocked("write", ".data/skills/demo/scripts/run.ps1").is_ok());
        assert!(blocked("read", ".data/skills/demo/skill.md").is_ok());
        assert!(blocked("search", ".").is_ok());

        fs::remove_dir_all(&workspace).ok();
    }
}
