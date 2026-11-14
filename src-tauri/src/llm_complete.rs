use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{agents, db, mcp, skills, todos, tools, turn_guard, AppState};

fn log_event(state: &State<'_, AppState>, level: &str, message: String) {
    if let Ok(logger) = state.logger.lock() {
        logger.log(level, &message);
    }
}

fn merge_allowed_commands(allowed_commands: &mut Vec<String>, incoming: &[String]) {
    for command in incoming {
        if !allowed_commands
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(command))
        {
            allowed_commands.push(command.clone());
        }
    }
}

/// Union of the `loaded_skills` recorded across the conversation history, in
/// first-seen order (deterministic, so the injected prompt prefix stays stable
/// across turns). The UI records these names explicitly per message; nothing
/// is inferred from message text.
fn collect_loaded_skills(messages: &[ChatMessage]) -> Vec<String> {
    let mut loaded: Vec<String> = Vec::new();
    for m in messages {
        let Some(names) = &m.loaded_skills else {
            continue;
        };
        for name in names {
            let name = name.trim();
            if !name.is_empty() && !loaded.iter().any(|existing| existing == name) {
                loaded.push(name.to_string());
            }
        }
    }
    loaded
}

/// Append the re-injected full-instructions block for a skill that is active
/// in the session. The `--- Skill: <name> ---` framing is kept stable to avoid
/// invalidating cached prompt prefixes.
fn push_loaded_skill_block(
    out: &mut String,
    skill: &skills::Skill,
    skill_dir: &Path,
    skill_file: &Path,
) {
    let cmd_constraint = if skill.allowed_commands.is_empty() {
        String::new()
    } else {
        format!(
            "\n[Allowed commands for this skill: {}]\n",
            skill.allowed_commands.join(", ")
        )
    };
    out.push_str(&format!(
        "\n\n--- Skill: {} ---\n[Skill root: {}]\n[Skill file: {}]\n{}{}",
        skill.name,
        skill_dir.display(),
        skill_file.display(),
        cmd_constraint,
        skill.system_prompt
    ));
}

// ─── Chat ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    /// `content` is either a plain string or a multimodal array of parts
    /// (`text` / `image_url` / `file`) following the OpenAI Chat Completions
    /// `messages[].content` schema. We keep it as a generic `Value` so the
    /// struct can be deserialized from either form without losing fidelity.
    pub content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Names of the skills that were loaded (via `use_skill`) during this
    /// message's turn, recorded explicitly by the UI. This is the durable
    /// "already loaded" state across turns — the backend must not infer it
    /// from message text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded_skills: Option<Vec<String>>,
}

/// Extract the textual portion of a `messages[].content` value.
/// - `String` content is returned verbatim.
/// - Array content concatenates all `{"type":"text","text":...}` parts.
/// - Anything else (e.g. malformed payloads) returns an empty string.
pub(crate) fn content_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        out.push_str(text);
                    }
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// The most recent user message as plain text (empty when there is none).
fn last_user_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| content_to_text(&message.content))
        .unwrap_or_default()
}

/// The reason carried by a `[WAIT_EVT: …]` marker, when the model declared
/// that the unfinished goals are waiting on an external event instead of being
/// blocked. An empty body degrades to a generic phrase, so the suspension
/// notice never prints a dangling colon.
fn wait_evt_reason(content: &str) -> Option<String> {
    let start = content.find(WAIT_EVT_PREFIX)?;
    let body_start = start + WAIT_EVT_PREFIX.len();
    let close = content[body_start..].find(']')?;
    let reason = content[body_start..body_start + close].trim();
    Some(if reason.is_empty() {
        "an external event".to_string()
    } else {
        reason.to_string()
    })
}

/// Remove every answer-level control marker (goal loop + WAIT_EVT) from a
/// finished reply. `[WAIT_EVT: …]` is variable length, hence the scan for its
/// closing bracket; an unterminated occurrence is left alone (a marker that
/// never closed is not a marker).
fn strip_control_markers(text: &str) -> String {
    let mut next = text.replace(GOAL_DONE_MARKER, "");
    while let Some(start) = next.find(WAIT_EVT_PREFIX) {
        let body_start = start + WAIT_EVT_PREFIX.len();
        let Some(close) = next[body_start..].find(']') else {
            break;
        };
        let end = body_start + close + 1;
        next.replace_range(start..end, "");
    }
    next.trim_end().to_string()
}

#[cfg(test)]
mod tests;

mod background;
mod conclusion;
mod context;
mod stream;
mod turn;

pub(crate) use self::background::*;
pub(crate) use self::conclusion::*;
pub(crate) use self::context::*;
pub(crate) use self::stream::*;
pub(crate) use self::turn::*;


const CONTEXT_COMPRESSION_THRESHOLD: f32 = 0.8;
const CONTEXT_KEEP_RECENT_MESSAGES: usize = 12;
/// Keep-window for the reactive compression — a provider that hard-rejected
/// the request as over-window needs a much smaller tail than the predictive
/// path, because the retry must fit whatever the cap actually is.
const CONTEXT_REACTIVE_KEEP_RECENT_MESSAGES: usize = 6;
const CONTEXT_SUMMARY_MARKER: &str = "INTERNAL CONTEXT - SESSION SUMMARY";
/// Framing line under the marker, mirroring Claude Code's compact user
/// message ("This session is being continued from a previous conversation…").
/// The marker stays the FIRST line so `extract_summary_body` keeps parsing
/// both new and legacy rows.
const CONTEXT_SUMMARY_CONTINUATION_PREFIX: &str = "[This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.]";
/// Marks the backend continuation nudge pushed after a mid-turn compression.
/// A user-role message (strict gateways reject mid-conversation `system`
/// turns), but never part of the UI history — `is_frontend_user_message`
/// excludes it from coverage counting.
const CONTEXT_CONTINUE_MARKER: &str = "INTERNAL CONTEXT - CONTEXT COMPRESSED";
const CONTEXT_TODO_MARKER: &str = "INTERNAL CONTEXT - ACTIVE TODO LIST";
const CONTEXT_MISSION_MARKER: &str = "INTERNAL CONTEXT - MULTI-AGENT MISSION RESULTS";
/// Marks the system message that turns the last budgeted request into a
/// wrap-up request (no tools) once the turn's round / tool-call budget is
/// spent. See `turn_guard`.
const CONTEXT_BUDGET_MARKER: &str = "INTERNAL CONTEXT - TURN BUDGET";
/// Answer used when the wrap-up request produced nothing at all, so the turn
/// never ends silently.
const BUDGET_FALLBACK_ANSWER: &str =
    "The turn stopped after reaching its tool budget without a written answer. Narrow the request, or raise the limit in Settings → Advanced.";
/// Marks the backend nudge pushed when the model tried to end the turn while
/// the session's todo list still has unfinished items (goal loop). A user-role
/// message excluded from frontend-history counting like the other notices.
const CONTEXT_GOAL_MARKER: &str = "INTERNAL CONTEXT - GOAL LOOP";
/// Marks the backend nudge pushed when the response hit the provider's output
/// token cap (`finish_reason: "length"`) mid-answer.
const CONTEXT_LENGTH_MARKER: &str = "INTERNAL CONTEXT - OUTPUT TRUNCATED";
/// The explicit end-of-turn marker the model writes into its final answer to
/// conclude a goal loop (or stop while items are still open, e.g. blocked).
/// Keep in sync with the goal-loop prompt wording below.
const GOAL_DONE_MARKER: &str = "[goal:DONE]";
/// Inline marker through which the model declares that the unfinished todo
/// items are waiting on an external event the app cannot schedule. The turn
/// ends and the goal loop stays suspended until the next turn (timer fire or
/// user message). Variable length: `[WAIT_EVT: what is being waited on]` —
/// stripping scans for the closing bracket. Keep in sync with
/// `WAIT_EVT_MARKER` in `src/App.tsx`.
const WAIT_EVT_PREFIX: &str = "[WAIT_EVT:";
/// Upper bound of goal-loop continuations per turn. Each one spends a round
/// anyway, but a model that keeps answering without working must not loop
/// forever even with an unlimited round budget.
const GOAL_LOOP_MAX_NUDGES: u32 = 4;
/// Upper bound of continuation requests after a `length` truncation.
const MAX_LENGTH_CONTINUES: u32 = 2;
const CONTEXT_SUMMARY_MAX_LINES: usize = 120;
/// Hard cap on a generated (LLM) summary. Dense output; cutting further is
/// the sign the summarizer ignored the prompt — better a truncated summary
/// than an unbounded one.
const CONTEXT_SUMMARY_MAX_CHARS: usize = 8_000;
/// Degenerate-output guard for the LLM summary: shorter answers are treated
/// as a failure and the mechanical fallback runs instead.
const CONTEXT_SUMMARY_MIN_CHARS: usize = 80;
/// Input budget (characters) for the transcript fed to the LLM summarizer,
/// ≈ 24k tokens at ~4 chars/token — roughly half of a 200K-class window.
/// The mechanical fallback is bounded by `CONTEXT_SUMMARY_MAX_LINES` instead.
const CONTEXT_SUMMARY_INPUT_CHARS: usize = 96_000;
/// Per-message budget in the summary transcript (user/assistant text).
const CONTEXT_LEDGER_MSG_CHARS: usize = 2_000;
/// Tool-result preview kept in the tool ledger / mechanical summary.
const CONTEXT_LEDGER_RESULT_CHARS: usize = 200;
/// Tool-argument preview kept in the tool ledger.
const CONTEXT_LEDGER_ARG_CHARS: usize = 120;
/// Rough prompt-token estimate: ≈4 characters per token for English + code.
/// CJK text runs denser (~1.5–2 chars/token), so the estimate UNDER-counts
/// CJK-heavy prompts — this pre-check is best-effort and the post-response
/// usage ratio remains the precise backstop.
const TOKEN_ESTIMATE_CHARS_PER_TOKEN: usize = 4;
/// Predictive pre-check trigger, as a fraction of the context window. Fires
/// before a request whose estimated prompt already approaches the cap.
const CONTEXT_PRECHECK_RATIO: f32 = 0.9;
const DEFAULT_MODEL_MAX_TOKENS: u32 = 524_288;
const STREAM_MAX_RETRIES: usize = 3;
const STREAM_RETRY_BACKOFF_MS: [u64; 2] = [300, 900];

/// Wrapper around text the user pushed into a running turn. It has to read as
/// a NEW user instruction, not as an internal note: some OpenAI-compatible
/// gateways reject `system` messages that appear mid-conversation, and a
/// `user` message is what this actually is.
const MID_TURN_INJECTION_PREFIX: &str =
    "[User update received while you were working — treat this as a new user message and adjust the current work if it changes anything]";

// ── Background tool tasks ───────────────────────────────────────────────────

/// Control tool the model uses to list / wait for / fetch / cancel tasks.
const BACKGROUND_TOOL_NAME: &str = "background_task";
/// Upper bound on simultaneously running background tasks per session. Beyond
/// this the model is told to wait or cancel instead of piling up processes.
const BACKGROUND_MAX_CONCURRENT: usize = 4;
/// `run_cmd` / `run_shell` default to 30 s in the foreground; a background task
/// is, by definition, the slow case, so it gets a much larger default.
const BACKGROUND_DEFAULT_TIMEOUT_SECS: i64 = 600;
/// One hold round waits at most this long for results.
const BACKGROUND_HOLD_WAIT_SECS: u64 = 120;
/// How many times a turn may be held open for background results before the
/// remaining tasks are cancelled and the answer is delivered as-is.
const BACKGROUND_HOLD_MAX_ROUNDS: u32 = 3;
/// Prefix of an injected result. It must read as tool output, not as a new
/// user request, or the model starts answering a message nobody wrote.
const BACKGROUND_RESULT_PREFIX: &str =
    "[BACKGROUND TASK RESULT — delivered automatically. This is the output of a tool you started earlier, NOT a new user request.]";

/// Thin command wrapper: the turn itself plus turn-end cleanup that must run
/// on EVERY path — serve processes are stopped and their whole process trees
/// are verified dead, so a `run_shell` serve child never outlives the turn
/// that spawned it (see `ServeRegistry::shutdown_all`).
#[tauri::command]
pub async fn chat_completion(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_ids: Vec<String>,
    session_id: String,
    model_override: Option<String>,
    use_agents: Option<bool>,
    embedded_agent_ids: Option<Vec<String>>,
    // Id the UI minted for this turn. Echoed on every `chat-*` event so a turn
    // that is still winding down after Stop cannot drive the turn that
    // replaced it.
    turn_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // The user's own turn always wins over the background evolution
    // maintenance pass: stop it before this turn installs its own cancel flag.
    // The maintenance turn uses a separate slot, so this is an explicit
    // hand-off rather than the shared `chat_cancelled` supersede rule.
    crate::evolution_maintenance::cancel(&state);

    // This turn keeps the system awake (a long streaming answer or a slow
    // sub-agent mission is exactly the kind of idle-looking stretch during
    // which Windows/macOS would suspend the machine). RAII: released on every
    // exit path — final answer, upstream error, Stop — and only after the
    // turn-end cleanup below finished.
    let sleep_guard = app.state::<crate::timer::SleepLock>().acquire();

    // `+agent` roles are frozen on the session's FIRST request. The composer
    // only offers the popup while the transcript is empty, and this guard
    // repeats the rule in the backend, so a retry or a hand-crafted call cannot
    // attach a role halfway through a conversation. The snapshot is what every
    // later turn reads, so editing or deleting the agent definition afterwards
    // leaves this session untouched.
    if let Some(ids) = embedded_agent_ids.as_deref().filter(|ids| !ids.is_empty()) {
        let first_turn = !messages.iter().any(|m| m.role == "assistant");
        if first_turn {
            let roles = agents::snapshot_embedded_roles(&state.agents_config_path, ids);
            if roles.is_empty() {
                log_event(
                    &state,
                    "WARN",
                    format!("no embeddable sub-agent role resolved: ids={ids:?}"),
                );
            } else {
                let names = roles
                    .iter()
                    .map(|role| role.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                match state
                    .db
                    .lock()
                    .map_err(|_| "database lock poisoned".to_string())
                    .and_then(|db| db::set_embedded_roles(&db, &session_id, &roles))
                {
                    Ok(()) => log_event(
                        &state,
                        "INFO",
                        format!(
                            "embedded sub-agent roles: session_id={session_id}, roles=[{names}]"
                        ),
                    ),
                    // A storage failure must not fail the user's turn: the
                    // message is answered without the role rather than erroring.
                    Err(err) => log_event(
                        &state,
                        "WARN",
                        format!("could not store embedded roles: session_id={session_id}, error={err}"),
                    ),
                }
            }
        } else {
            log_event(
                &state,
                "WARN",
                format!("ignored embedded roles on a non-first turn: session_id={session_id}"),
            );
        }
    }

    let turn_id = turn_id.unwrap_or_default();

    // The injection window is opened here, not inside the turn, so turn-end
    // cleanup can tell this turn apart from a newer one the user already
    // started in the same session.
    let injection_window = InjectionWindow::open(&app, &state, &session_id);
    let window_turn_id = injection_window.turn_id;

    let result = run_chat_turn(
        app.clone(),
        messages,
        skill_ids,
        session_id.clone(),
        model_override,
        use_agents,
        turn_id,
        injection_window,
        state,
    )
    .await;

    end_turn_cleanup(&app, &session_id, window_turn_id).await;
    drop(sleep_guard);
    result
}
