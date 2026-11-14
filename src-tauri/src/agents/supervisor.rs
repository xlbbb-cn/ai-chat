//! Event-driven planner supervisor.
//!
//! The one-shot selector/planner (`select_agents` + `plan_tasks`) ends before
//! the tasks run. This module keeps the same role alive for the duration of the
//! mission: it observes task state on the mission bus, can query peer status and
//! send short coordination messages, and stops when the executor reports that
//! every task has settled (or when its own budget runs out).
//!
//! It is deliberately *not* an autonomous task runner: it has only the `mbus`
//! tool and cannot edit files, cancel tasks or re-plan the mission.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::mbus::{self, ActivityEvent, BusMessage, MissionBus, PLANNER_ID};
use super::{log_agent_event, preview_text, Task};
use crate::llm_complete::{
    apply_completion_token_limit, content_to_text, sanitize_tool_pairs, stream_llm_request,
    upstream_request_headers, ChatMessage, StreamOptions,
};
use crate::util::now_ms;
use crate::AppConfig;

/// Hard cap on planner LLM activations for one mission. The supervisor is a
/// cost-bearing observer; tasks keep running after it stops.
const MAX_ACTIVATIONS: u32 = 10;
/// Tool rounds inside one activation.
const MAX_TOOL_ROUNDS: usize = 4;
/// Completion budget for one activation (the tool calls themselves are small).
const MAX_ACTIVATION_TOKENS: u32 = 1024;
/// Timeout for one supervisor LLM request. A slow provider must not pin the
/// supervisor forever; the mission itself is unaffected.
const ACTIVATION_TIMEOUT_SECS: u64 = 60;
/// Heartbeat used to detect a task that has stopped making progress.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// A running task with no status update for this long triggers a heartbeat
/// activation (only if the previous activation is old enough).
const STALL_MS: i64 = 90_000;
/// Minimum distance between heartbeat-driven activations.
const MIN_ACTIVATION_GAP_MS: i64 = 20_000;
/// Planner notes carried into the next activation.
const MAX_NOTES: usize = 12;
const NOTE_MAX_CHARS: usize = 240;
const STATUS_PROMPT_MAX_CHARS: usize = 12_000;

struct ActivationOutcome {
    notes: Vec<String>,
    finished: bool,
    finish_note: String,
    done: bool,
    cancelled: bool,
    error: Option<String>,
}

/// Runs until the executor finishes, the user stops the turn, the planner calls
/// `finish`, or the activation budget is exhausted.
#[allow(clippy::too_many_arguments)]
pub async fn run_planner_supervisor(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    session_id: &str,
    model: &str,
    conversation: &[ChatMessage],
    tasks: &[Task],
    bus: Arc<MissionBus>,
    cancelled: Arc<AtomicBool>,
    mut done_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let headers = upstream_request_headers(config, session_id);
    let system = build_system_prompt(&bus, tasks, conversation);
    // A single-task plan has no peers to coordinate and the executor's done
    // signal is enough; one initial activation avoids paying for a second
    // planner call just to observe a trivial mission.
    let max_activations: u32 = if tasks.len() <= 1 {
        1
    } else {
        MAX_ACTIVATIONS
    };
    let mut last_seq = 0u64;
    let mut activation: u32 = 0;
    let mut notes: Vec<String> = Vec::new();
    let mut last_activation_ms = 0i64;
    let mut initial = true;
    let mut executor_done = false;
    let mut final_status = "done";
    let mut final_note = String::from("all tasks finished");

    emit_state(
        app,
        session_id,
        bus.bus_id(),
        "monitoring",
        0,
        &format!("monitoring {} task(s)", tasks.len()),
        tasks.len(),
    );

    loop {
        if cancelled.load(Ordering::SeqCst) {
            final_status = "stopped";
            final_note = "stopped by user".to_string();
            break;
        }
        if executor_done {
            break;
        }
        if activation >= max_activations {
            final_status = "done";
            final_note = format!("supervision budget reached ({max_activations} activations)");
            break;
        }

        let now = now_ms();
        let activities = bus.activity_since(last_seq);
        let new_seq = activities
            .iter()
            .map(|event| event.seq)
            .max()
            .unwrap_or(last_seq);
        let unread = bus.inbox_len(PLANNER_ID);
        let heartbeat_due = now.saturating_sub(last_activation_ms) >= MIN_ACTIVATION_GAP_MS
            && now.saturating_sub(last_activation_ms) >= HEARTBEAT.as_millis() as i64;
        let trigger = initial
            || unread > 0
            || activities.iter().any(is_trigger)
            || (heartbeat_due && stalled(&bus));

        if trigger {
            activation = activation.saturating_add(1);
            let inbox = bus.take_inbox(PLANNER_ID, mbus::MAX_DELIVERIES_PER_ROUND);
            // Everything up to this point is represented in the prompt (status
            // + activity + the drained messages). Advancing the cursor now
            // avoids a second activation for a message that arrived just after
            // `activity_since` but was already delivered by `take_inbox`.
            last_seq = bus.activity_seq();
            let status_text = mbus::render_status_text(app, &bus, PLANNER_ID);
            let activity_text = render_activities(&activities);
            let inbox_text = render_inbox(&inbox);
            let prompt = format!(
                "ACTIVATION {activation}/{max_activations}\n\
                 MISSION STATUS:\n{status_text}\n\n\
                 NEW ACTIVITY SINCE YOUR LAST ACTIVATION:\n{activity_text}\n\n\
                 MESSAGES ADDRESSED TO YOU (already delivered to this prompt):\n{inbox_text}\n\n\
                 Decide the next supervision step. Use `mbus status` to drill down, \
                 `mbus send` only when a task needs information it does not already have, \
                 and `mbus finish` when there is nothing left to supervise. \
                 Do not send acknowledgements and do not repeat earlier guidance.",
                status_text = preview_text(&status_text, STATUS_PROMPT_MAX_CHARS),
                activity_text = activity_text,
                inbox_text = inbox_text,
            );

            emit_state(
                app,
                session_id,
                bus.bus_id(),
                "thinking",
                activation,
                &format!("checking {} task(s)", tasks.len()),
                tasks.len(),
            );

            let outcome = run_activation(
                app,
                client,
                url,
                config,
                activation,
                model,
                &headers,
                &system,
                &notes,
                &prompt,
                &bus,
                &cancelled,
                &mut done_rx,
            )
            .await;

            last_activation_ms = now_ms();
            initial = false;

            if outcome.done {
                executor_done = true;
                final_note = "all tasks finished".to_string();
                continue;
            }
            if outcome.cancelled {
                final_status = "stopped";
                final_note = "stopped by user".to_string();
                break;
            }
            if let Some(err) = outcome.error {
                // A supervisor failure must not make the whole mission look
                // failed: the sub-agents and their results are unaffected.
                final_status = "done";
                final_note = format!("supervision stopped: {err}");
                break;
            }

            if !outcome.notes.is_empty() {
                notes.extend(outcome.notes);
                while notes.len() > MAX_NOTES {
                    notes.remove(0);
                }
            }
            if outcome.finished {
                final_status = "done";
                final_note = if outcome.finish_note.is_empty() {
                    "planner finished supervision".to_string()
                } else {
                    outcome.finish_note
                };
                break;
            }

            emit_state(
                app,
                session_id,
                bus.bus_id(),
                "monitoring",
                activation,
                "waiting for task activity",
                tasks.len(),
            );
            continue;
        }

        // No activation is needed: consume the events we just inspected and
        // sleep until the next bus event, heartbeat, or executor completion.
        // The bus sequence is checked before waiting, so a notification that
        // races the await is not lost.
        last_seq = last_seq.max(new_seq);
        let notified = bus.notified();
        tokio::pin!(notified);
        tokio::select! {
            _ = &mut done_rx => {
                executor_done = true;
            }
            _ = &mut notified => {}
            _ = tokio::time::sleep(HEARTBEAT) => {}
        }
    }

    // The bus stays open until `orchestrate` joins the executor: the supervisor
    // may stop early (budget / `finish`) while agents are still running, and
    // they must keep their peer-to-peer channel.
    // The executor only says "settled", not "success": reflect failed/skipped
    // tasks in the planner row instead of claiming everything finished.
    if executor_done && final_status == "done" {
        let statuses = bus.status();
        let completed = statuses.iter().filter(|peer| peer.state == "completed").count();
        let failed = statuses.iter().filter(|peer| peer.state == "error").count();
        let skipped = statuses.iter().filter(|peer| peer.state == "skipped").count();
        let incomplete = statuses
            .iter()
            .filter(|peer| peer.state == "pending" || peer.state == "running")
            .count();
        if completed + failed + skipped + incomplete > 0 {
            final_note = if failed + skipped + incomplete > 0 {
                format!(
                    "settled: {completed} completed, {failed} failed, {skipped} skipped{}",
                    if incomplete > 0 {
                        format!(", {incomplete} not settled")
                    } else {
                        String::new()
                    }
                )
            } else {
                format!("all {completed} task(s) completed")
            };
        }
    }
    emit_state(
        app,
        session_id,
        bus.bus_id(),
        final_status,
        activation,
        &final_note,
        tasks.len(),
    );
    log_agent_event(
        app,
        "INFO",
        format!(
            "planner supervisor finished: session={session_id}, status={final_status}, activations={activation}, note={final_note}"
        ),
    );
}

#[allow(clippy::too_many_arguments)]
async fn run_activation(
    app: &AppHandle,
    client: &Client,
    url: &str,
    config: &AppConfig,
    activation: u32,
    model: &str,
    headers: &[(String, String)],
    system: &str,
    notes: &[String],
    prompt: &str,
    bus: &MissionBus,
    cancelled: &AtomicBool,
    done_rx: &mut tokio::sync::oneshot::Receiver<()>,
) -> ActivationOutcome {
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system })];
    if !notes.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": format!(
                "YOUR PREVIOUS SUPERVISION NOTES (newest last):\n{}",
                notes.join("\n")
            ),
        }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));

    let mut collected_notes: Vec<String> = Vec::new();
    let mut finished = false;
    let mut finish_note = String::new();

    for round in 0..MAX_TOOL_ROUNDS {
        sanitize_tool_pairs(&mut messages);
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
            "tools": [mbus::tool_definition(true)],
            "tool_choice": "auto",
            "temperature": 0.0,
        });
        apply_completion_token_limit(
            &mut body,
            Some(MAX_ACTIVATION_TOKENS),
            Some(super::LLM_DEFAULT_MAX_TOKENS),
        );

        let call = stream_llm_request(
            app,
            client,
            url,
            &config.api_key,
            body,
            cancelled,
            StreamOptions {
                // The supervisor's reasoning is not part of the chat transcript;
                // only its status events and bus messages reach the UI.
                token_event: "agent-planner-token",
                task_id: None,
                // The planner itself reports through `agent-planner-*` events,
                // which carry the bus's session id instead.
                session_id: None,
                turn_id: None,
                reasoning_event: None,
                emit_usage: false,
                usage_max_tokens: None,
                ds_format: config.ds_format_for(model),
                headers,
                emit_tokens: false,
                emit_errors: false,
            },
        );

        let result = tokio::select! {
            _ = &mut *done_rx => {
                return ActivationOutcome {
                    notes: collected_notes,
                    finished: false,
                    finish_note: String::new(),
                    done: true,
                    cancelled: false,
                    error: None,
                };
            }
            result = tokio::time::timeout(Duration::from_secs(ACTIVATION_TIMEOUT_SECS), call) => result,
        };

        let stream = match result {
            Err(_) => {
                return ActivationOutcome {
                    notes: collected_notes,
                    finished: false,
                    finish_note: String::new(),
                    done: false,
                    cancelled: false,
                    error: Some(format!(
                        "planner activation timed out after {ACTIVATION_TIMEOUT_SECS}s"
                    )),
                };
            }
            Ok(Err(err)) => {
                return ActivationOutcome {
                    notes: collected_notes,
                    finished: false,
                    finish_note: String::new(),
                    done: false,
                    cancelled: false,
                    error: Some(err),
                };
            }
            Ok(Ok(stream)) => stream,
        };

        if cancelled.load(Ordering::SeqCst) || stream.finish_reason == "cancelled" {
            return ActivationOutcome {
                notes: collected_notes,
                finished: false,
                finish_note: String::new(),
                done: false,
                cancelled: true,
                error: None,
            };
        }

        let content = stream.content.trim().to_string();
        if !content.is_empty() && stream.tool_calls.is_empty() {
            collected_notes.push(format!(
                "activation {}: {}",
                round + 1,
                preview_text(&content, NOTE_MAX_CHARS)
            ));
        }

        let mut assistant = json!({
            "role": "assistant",
            "content": if content.is_empty() { Value::Null } else { Value::String(content.clone()) },
        });
        if !stream.reasoning_content.is_empty() {
            assistant["reasoning_content"] = json!(stream.reasoning_content.as_str());
        }
        if !stream.tool_calls.is_empty() {
            assistant["tool_calls"] = json!(stream
                .tool_calls
                .iter()
                .map(|(id, name, args)| json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args },
                }))
                .collect::<Vec<_>>());
        }
        messages.push(assistant);

        if stream.tool_calls.is_empty() {
            break;
        }

        for (id, name, args) in &stream.tool_calls {
            let parsed: Value = serde_json::from_str(args).unwrap_or_default();
            let action_label = if name == "mbus" {
                let value = parsed["action"].as_str().unwrap_or("mbus").trim();
                if value.is_empty() {
                    "mbus"
                } else {
                    value
                }
            } else {
                name.as_str()
            };
            let target = planner_action_target(&parsed, action_label, name);
            let before_detail = planner_action_before_detail(&parsed, action_label, name);
            emit_planner_action(
                app,
                bus,
                activation,
                round,
                id,
                action_label,
                "running",
                &target,
                &before_detail,
            );

            let (result, is_finish) = if name != "mbus" {
                (
                    format!("Error: unknown planner tool '{name}'. Only 'mbus' is available."),
                    false,
                )
            } else if action_label == "finish" {
                (mbus::handle_tool(app, bus, PLANNER_ID, &parsed, true), true)
            } else {
                (
                    mbus::handle_tool(app, bus, PLANNER_ID, &parsed, true),
                    false,
                )
            };

            let status = if result.trim_start().starts_with("Error") {
                "error"
            } else {
                "done"
            };
            let after_detail = if status == "error" {
                result.clone()
            } else if action_label == "send" {
                parsed["message"].as_str().unwrap_or("").to_string()
            } else {
                summarize_action_result(action_label, &result)
            };
            emit_planner_action(
                app,
                bus,
                activation,
                round,
                id,
                action_label,
                status,
                &target,
                &after_detail,
            );

            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result,
            }));
            if action_label == "send" && status == "done" {
                collected_notes.push(format!(
                    "activation {}: sent to {}: {}",
                    round + 1,
                    target,
                    preview_text(parsed["message"].as_str().unwrap_or(""), 160)
                ));
            }
            if is_finish {
                finished = true;
                finish_note = parsed["note"].as_str().unwrap_or("").trim().to_string();
            }
        }

        if finished {
            break;
        }
    }

    ActivationOutcome {
        notes: collected_notes,
        finished,
        finish_note,
        done: false,
        cancelled: false,
        error: None,
    }
}

/// Mirror one planner decision into the chat tool group. The UI keeps a row
/// per `action_id`, so a `running` event creates the row and the following
/// `done`/`error` event settles it.
fn emit_planner_action(
    app: &AppHandle,
    bus: &MissionBus,
    activation: u32,
    round: usize,
    tool_call_id: &str,
    action: &str,
    status: &str,
    target: &str,
    detail: &str,
) {
    let _ = app.emit(
        "agent-planner-action",
        json!({
            "session_id": bus.session_id(),
            "bus_id": bus.bus_id(),
            "task_id": "planner",
            "action_id": format!("planner-{activation}-{round}-{tool_call_id}"),
            "activation": activation,
            "round": round,
            "action": action,
            "status": status,
            "target": preview_text(target, 160),
            "detail": preview_text(detail, 300),
            "created_at_ms": now_ms(),
        }),
    );
}

fn planner_action_target(parsed: &Value, action: &str, tool_name: &str) -> String {
    match action {
        "send" => {
            let to = parsed["to"].as_str().unwrap_or("").trim();
            if !to.is_empty() {
                to.to_string()
            } else {
                let topic = parsed["topic"]
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("broadcast");
                format!("#{topic}")
            }
        }
        "status" => {
            let ids = parsed["task_ids"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if ids.is_empty() {
                "all agents".to_string()
            } else {
                ids
            }
        }
        "history" => {
            let topic = parsed["topic"].as_str().unwrap_or("").trim();
            if topic.is_empty() {
                "all traffic".to_string()
            } else {
                format!("#{topic}")
            }
        }
        "subscribe" | "unsubscribe" => {
            let topics = parsed["topics"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if topics.is_empty() {
                "topics".to_string()
            } else {
                topics
            }
        }
        "finish" => "—".to_string(),
        "direct" => parsed["to"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("task")
            .to_string(),
        "end_task" => parsed["to"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("task")
            .to_string(),
        _ => tool_name.to_string(),
    }
}

fn planner_action_before_detail(parsed: &Value, action: &str, tool_name: &str) -> String {
    match action {
        "send" => preview_text(parsed["message"].as_str().unwrap_or(""), 300),
        "status" => "checking agent status".to_string(),
        "history" => "reading bus history".to_string(),
        "subscribe" => "updating subscriptions".to_string(),
        "unsubscribe" => "updating subscriptions".to_string(),
        "direct" => {
            let objective = parsed["objective"].as_str().unwrap_or("").trim();
            match parsed["done_when"].as_str().map(str::trim) {
                Some(done) if !done.is_empty() => format!(
                    "{} (done when: {})",
                    preview_text(objective, 200),
                    preview_text(done, 100)
                ),
                _ => preview_text(objective, 300),
            }
        }
        "end_task" => parsed["note"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(|value| preview_text(value, 300))
            .unwrap_or_else(|| "ending task".to_string()),
        "finish" => parsed["note"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("finishing supervision")
            .to_string(),
        _ => format!("unsupported tool: {tool_name}"),
    }
}

fn summarize_action_result(action: &str, result: &str) -> String {
    if result.trim_start().starts_with("Error") {
        return preview_text(result.trim(), 300);
    }
    let parsed = serde_json::from_str::<Value>(result).ok();
    match action {
        "status" => {
            if let Some(value) = &parsed {
                if let Some(peers) = value["peers"].as_array() {
                    let parts: Vec<String> = peers
                        .iter()
                        .map(|peer| {
                            format!(
                                "{} {}",
                                peer["id"].as_str().unwrap_or("?"),
                                peer["state"].as_str().unwrap_or("?")
                            )
                        })
                        .collect();
                    if !parts.is_empty() {
                        return format!("{} agent(s): {}", parts.len(), parts.join(", "));
                    }
                }
            }
            preview_text(result.trim(), 300)
        }
        "history" => {
            if let Some(value) = &parsed {
                if let Some(messages) = value["messages"].as_array() {
                    return format!("{} message(s)", messages.len());
                }
            }
            preview_text(result.trim(), 300)
        }
        "subscribe" | "unsubscribe" => {
            if let Some(value) = &parsed {
                if let Some(topics) = value["subscriptions"].as_array() {
                    let topics = topics
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    if !topics.is_empty() {
                        return topics;
                    }
                }
            }
            preview_text(result.trim(), 300)
        }
        "finish" => {
            if let Some(value) = &parsed {
                if let Some(note) = value["note"].as_str().filter(|value| !value.is_empty()) {
                    return note.to_string();
                }
            }
            "supervision finished".to_string()
        }
        "direct" => {
            if let Some(value) = &parsed {
                if let Some(objective) = value["objective"].as_str() {
                    return preview_text(objective, 300);
                }
            }
            preview_text(result.trim(), 300)
        }
        "end_task" => {
            if let Some(value) = &parsed {
                if let Some(note) = value["note"].as_str().filter(|value| !value.is_empty()) {
                    return preview_text(note, 300);
                }
            }
            "task ended".to_string()
        }
        _ => preview_text(result.trim(), 300),
    }
}

fn is_trigger(event: &ActivityEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "task_finished"
            | "task_failed"
            | "task_skipped"
            | "message_to_planner"
            | "message_broadcast"
            | "pair_limit"
    )
}

fn stalled(bus: &MissionBus) -> bool {
    bus.status()
        .iter()
        .any(|peer| peer.state == "running" && peer.last_activity_ms >= STALL_MS)
}

fn emit_state(
    app: &AppHandle,
    session_id: &str,
    bus_id: &str,
    status: &str,
    activation: u32,
    activity: &str,
    task_count: usize,
) {
    let _ = app.emit(
        "agent-planner-state",
        json!({
            "session_id": session_id,
            "bus_id": bus_id,
            "task_id": "planner",
            "status": status,
            "activation": activation,
            "task_count": task_count,
            "activity": preview_text(activity, 300),
        }),
    );
}

fn build_system_prompt(bus: &MissionBus, tasks: &[Task], conversation: &[ChatMessage]) -> String {
    let plan = tasks
        .iter()
        .map(|task| {
            let name = bus
                .peer(&task.id)
                .map(|peer| peer.agent_name)
                .unwrap_or_else(|| task.agent_id.clone());
            let mut line = format!(
                "- {} [{}] {}\n  context: {}",
                task.id,
                name,
                preview_text(task.description.trim(), 300),
                preview_text(task.context.trim(), 600),
            );
            if let Some(cwd) = task.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
                line.push_str(&format!("\n  cwd: {cwd}"));
            }
            if !task.dependencies.is_empty() {
                line.push_str(&format!("\n  depends on: {}", task.dependencies.join(", ")));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");

    let conversation = conversation
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|message| {
            format!(
                "[{}]: {}",
                message.role,
                preview_text(&content_to_text(&message.content), 400)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are the planner/supervisor of a live multi-agent mission. You dispatched the plan below \
         and do NOT execute tasks yourself: watch peer state, unblock agents with short coordination \
         notes, and stop supervising when the work is done.\n\n\
         PLAN:\n{plan}\n\n\
         MISSION BUS PEERS:\n{peers}\n\n\
         RECENT CONVERSATION:\n{conversation}\n\n\
         Rules:\n\
         - `mbus` `status` shows live state: rounds, last tool, queued messages, active subtasks, objectives, per-pair message counts.\n\
         - `mbus` `send` with `to: \"<task id>\"` reaches one peer, `topic: \"broadcast\"` everyone. Messages land at the recipient's NEXT reasoning round, never instantly.\n\
         - A message must add information, correct a misunderstanding or connect two tasks — never acknowledge, never repeat a note from your previous supervision notes.\n\
         - Sub-agents may exchange at most {pair_limit} messages per pair (both directions combined; 0 = unrestricted). At that limit you get a \"Mission bus\" escalation: take the coordination over and give each involved task a `direct` objective plus `done_when`, the authoritative exit condition.\n\
         - `mbus` `end_task` when a task is redundant, already covered, or its remaining work no longer serves the mission — not while it still produces the needed result.\n\
         - Running normally: do nothing. Stuck: ask for a short progress note, give a `direct` objective, or suggest a smaller step.\n\
         - `mbus` `history` reviews recent traffic; do not poll it every activation.\n\
         - `mbus` `finish` when every task is terminal or nothing useful remains. The executor keeps running regardless.",

        plan = plan,
        peers = mbus::describe_peers(bus, PLANNER_ID),
        conversation = conversation,
        pair_limit = bus.pair_limit(),
    )
}

fn render_activities(events: &[ActivityEvent]) -> String {
    if events.is_empty() {
        return "(none)".to_string();
    }
    events
        .iter()
        .rev()
        .take(20)
        .rev()
        .map(|event| {
            format!(
                "- [{}] {}: {}",
                event.seq,
                event.kind,
                preview_text(&event.text, NOTE_MAX_CHARS)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_inbox(messages: &[BusMessage]) -> String {
    if messages.is_empty() {
        return "(none)".to_string();
    }
    messages
        .iter()
        .map(|message| {
            format!(
                "- from {} via {}: {}",
                message.from_name,
                if message.target_kind == "topic" {
                    format!("#{}", message.topic)
                } else {
                    "direct".to_string()
                },
                preview_text(&message.body, 1_000)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
