//! IPC-facing operations on the turn family: stop, inject, list, snapshot.
//!
//! `ipc/chat.rs` forwards here; the bodies live with the class they operate
//! on instead of in `lib.rs`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::progress::TurnSnapshot;
use super::registry::ActiveTurnInfo;
use crate::{events, tools, AppState};

#[derive(Serialize)]
pub struct InjectUserMessageResult {
    /// `false` → no turn is running for this session (or its inbox already
    /// closed); the UI sends the text as a normal next-turn message instead
    /// of leaving it in the inbox.
    pub accepted: bool,
}

/// Stop one session's turn (the Stop button). Several sessions can run at
/// once, so Stop is addressed by session id; a session without a running turn
/// is a no-op (the next turn installs its own flag anyway).
pub(crate) fn stop_chat_completion(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
) {
    state.turns.cancel(&session_id);
    // Background tasks must die immediately, not at the next loop iteration:
    // a foreground tool may still be blocking the turn. Only this session's
    // tasks are touched.
    let cancelled = state.background_tasks.cancel_session(&session_id);
    // Broadcast before the turn unwinds. Stop is the one path where the
    // turn-end teardown may drop the entry before the runner can report it,
    // so the UI would otherwise keep the row (and its cancel button) alive
    // for a task that is already dying.
    for snapshot in &cancelled {
        let _ = app.emit(events::BACKGROUND_TASK_STATE, snapshot.clone());
    }
    let _ = app.emit(
        events::BACKGROUND_WAITING,
        events::BackgroundWaiting {
            session_id: session_id.clone(),
            pending: 0,
            round: None,
        },
    );
    if !cancelled.is_empty() {
        let count = cancelled.len();
        let _ = state.logger.lock().map(|logger| {
            logger.log(
                "INFO",
                &format!(
                    "Stop pressed: cancelled {count} background task(s) of session {session_id}"
                ),
            );
        });
    }
    // The plan is not running any more: whatever the turn had marked
    // `in_progress` goes back to `pending`, so the next turn (and the goal
    // loop that resumes an unfinished list) does not believe work is still
    // underway.
    match tools::todos::settle_interrupted(&app, &session_id) {
        Ok(Some(summary)) => {
            let _ = app.emit(
                events::TODO_STATE,
                serde_json::json!({
                    "type": "todo_state",
                    "list_id": summary.list_id,
                    "session_id": summary.session_id,
                    "summary": &summary,
                }),
            );
        }
        Ok(None) => {}
        Err(err) => {
            let _ = state.logger.lock().map(|logger| {
                logger.log(
                    "WARN",
                    &format!(
                        "Stop pressed: could not settle the todo list of session {session_id}: {err}"
                    ),
                );
            });
        }
    }
}

/// Push a message into the turn that is currently running for this session,
/// so it reaches the model at the next request boundary (i.e. before the next
/// tool round or the final answer) instead of waiting for the turn to end.
///
/// Returns `accepted: false` when no turn is running for this session — the
/// UI then sends the text as a normal next-turn message, which is the only
/// other safe option: a turn that already finished will never make another
/// request, so an item left here would be drained by an unrelated later turn.
pub(crate) fn inject_user_message(
    session_id: String,
    id: String,
    text: String,
    state: State<'_, AppState>,
) -> InjectUserMessageResult {
    let accepted = state
        .turns
        .get(&session_id)
        .map(|turn| turn.inject(id, text))
        .unwrap_or(false);
    InjectUserMessageResult { accepted }
}

/// The turns currently executing in this window, oldest first — the History
/// panel's "running" badge source. A session switch never stops a turn, so the
/// panel needs this to mark the conversations that are still working.
pub(crate) fn list_active_turns(state: State<'_, AppState>) -> Vec<ActiveTurnInfo> {
    state.turns.list()
}

/// Live snapshot of one session's running turn (content, reasoning, settled
/// tool rows, delivered injections, usage totals), or `None` when the session
/// has no turn executing. The frontend re-attaches to a background turn with
/// this when the user switches back to it.
pub(crate) fn get_turn_snapshot(
    session_id: String,
    state: State<'_, AppState>,
) -> Option<TurnSnapshot> {
    state.turns.snapshot(&session_id)
}