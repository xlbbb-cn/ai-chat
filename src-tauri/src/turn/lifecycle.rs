//! Turn-end teardown for one [`super::LlmTurn`] base.
//!
//! This lives with the class it tears down, not in the engine: every exit path
//! of `chat_completion` calls it (final answer, upstream error, Stop). It stops
//! this session's serve processes (verifying the killed trees are gone), shuts
//! down its background tasks and kills its private MCP pool. Serve verification
//! runs in a blocking thread — it polls for up to a few seconds in the
//! pathological case, which must not stall the async runtime.

use tauri::{AppHandle, Emitter, Manager};

use super::LlmTurn;
use crate::{events, AppState};

fn log(state: &AppState, level: &str, message: String) {
    let _ = state.logger.lock().map(|logger| logger.log(level, &message));
}

/// Tear down everything turn-scoped that the [`LlmTurn`] base owns or drives.
/// Silent when nothing existed.
pub(crate) async fn end_turn_cleanup(app: &AppHandle, turn: &LlmTurn) {
    let session_id = turn.session_id.as_str();
    let window_id = turn.window_id;
    // The user may press Stop and send the next message before this turn has
    // finished unwinding. That newer turn owns the session by then, and its
    // background tasks and waiting chip belong to it — tearing them down here
    // killed exactly the work the user had just asked for. The newer turn runs
    // this cleanup of its own when it ends. Turns of OTHER sessions are never
    // a reason to skip: their state is keyed by their own session id.
    let superseded = app.state::<AppState>().turns.superseded(session_id, window_id);
    if superseded {
        log(
            &app.state::<AppState>(),
            "INFO",
            format!("turn ended (session {session_id}): a newer turn owns it now, leaving its state alone"),
        );
        return;
    }

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
        let _ = app.emit(events::BACKGROUND_TASK_STATE, snapshot);
    }
    // The waiting chip is driven by `background-waiting`; zero it on every turn
    // end so a hold that was cut short (Stop, error, budget) cannot leave it
    // counting down a turn that no longer exists.
    let _ = app.emit(
        events::BACKGROUND_WAITING,
        events::BackgroundWaiting {
            session_id: session_id.to_string(),
            pending: 0,
            round: None,
        },
    );
    if removed_count > 0 {
        log(
            &app.state::<AppState>(),
            "INFO",
            format!("turn ended (session {session_id}): shut down {removed_count} background task(s)"),
        );
    }

    let session_id = session_id.to_string();
    // Only THIS session's serve processes: another conversation may still be
    // running and its servers belong to its own turn.
    let summary = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        let session_id = session_id.clone();
        move || {
            app.state::<AppState>()
                .serve_processes
                .shutdown_session(&session_id)
        }
    })
    .await
    .unwrap_or_else(|err| format!("Error: serve cleanup task failed: {err}"));

    if !summary.contains("No background serve processes") {
        log(
            &app.state::<AppState>(),
            "INFO",
            format!("turn ended (session {session_id}): {summary}"),
        );
    }

    // The base's private MCP pool: this turn's server processes die with it.
    // `McpSession::Drop` remains the safety net for unwind paths.
    turn.shutdown_mcp(&app.state::<AppState>()).await;
}

/// Unpublishes a turn's base and announces its end — on EVERY exit path.
///
/// `chat_completion` calls [`TurnEndGuard::finish`] once [`end_turn_cleanup`]
/// has returned. The `Drop` body is the backstop for a path that never reaches
/// it: a panic inside the turn (a tool's `unwrap`, an assertion, an index) or a
/// task aborted while it awaited. Without that backstop the registry kept a
/// dead base forever — the History panel showed a permanent Running badge,
/// Stop set a cancel flag nobody read any more, and the conversation produced
/// no output at all until the app restarted.
pub(crate) struct TurnEndGuard {
    app: AppHandle,
    session_id: String,
    turn_id: String,
    started_at_ms: i64,
    model: String,
    window_id: u64,
    finished: bool,
}

impl TurnEndGuard {
    pub(crate) fn new(
        app: AppHandle,
        session_id: &str,
        turn_id: &str,
        started_at_ms: i64,
        model: &str,
        window_id: u64,
    ) -> Self {
        Self {
            app,
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            started_at_ms,
            model: model.to_string(),
            window_id,
            finished: false,
        }
    }

    /// Unpublish the base, then announce the end. Idempotent, and safe to call
    /// from `Drop` (the emit is best effort).
    pub(crate) fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.app.state::<AppState>().turns.remove_if(self.window_id);
        let _ = self.app.emit(
            events::TURN_STATE,
            events::TurnState {
                session_id: self.session_id.clone(),
                turn_id: self.turn_id.clone(),
                running: false,
                started_at_ms: self.started_at_ms,
                model: self.model.clone(),
            },
        );
    }
}

impl Drop for TurnEndGuard {
    fn drop(&mut self) {
        self.finish();
    }
}
