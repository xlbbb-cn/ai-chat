//! The LLM turn base: everything one executing chat turn owns.
//!
//! A turn is the unit of concurrency in this app. Several conversations can
//! run a turn at once in the same workspace (new chat while a reply streams),
//! so nothing turn-scoped may live in a global slot: each turn is an
//! [`LlmTurn`] base that owns its identity, its **frozen configuration
//! snapshot**, its cancel flag, its live progress snapshot, its injection
//! inbox, its request/tool budget, its turn-scoped event emission and its
//! **private MCP stdio process pool**. Configuration is just the base's
//! configuration — taken once at construction, never re-read mid-turn.
//!
//! Class family:
//! * [`LlmTurn`] — the base itself (this file).
//! * [`progress`] — [`TurnProgress`] / [`TurnUsage`] / the snapshot DTOs.
//! * [`registry`] — [`TurnRegistry`], the session → base map with concurrency
//!   admission and same-session supersede.
//! * [`guard`] — [`guard::TurnBudget`] / [`guard::ToolFailureGuard`].
//! * `lifecycle` — turn-end teardown (background tasks, serve processes, the
//!   private MCP pool), keyed on the registry's supersede check.
//! * [`commands`] — the IPC-facing turn operations (stop / inject / list /
//!   snapshot).
//!
//! The engine that RUNS on the base lives in `crate::llm_complete` (prompt
//! assembly, the ReAct loop, compression, background holds). What stays
//! outside the base is what outlives a turn: the workspace database, session
//! memories, todos, timers, the session-scoped tool-failure guard, and the
//! shared MCP pool the *panel* tests against. Background tool tasks and serve
//! processes are registered per session in `AppState` (a session has at most
//! one active turn, so session scope == turn scope for teardown) and torn
//! down by the base's turn-end cleanup.

pub mod commands;
pub mod guard;
pub(crate) mod lifecycle;
pub mod progress;
pub mod registry;

pub(crate) use lifecycle::end_turn_cleanup;
pub(crate) use lifecycle::TurnEndGuard;
pub use progress::{
    now_ms, TurnInboxItem, TurnProgress, TurnProgressSink, TurnSnapshot, TurnUsageSnapshot,
};
pub use registry::{
    default_max_concurrent_turns, ActiveTurnInfo, TurnRegistry, DEFAULT_MAX_CONCURRENT_TURNS,
    MAX_CONCURRENT_TURNS,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};

use crate::{events, AppConfig};
use guard::{BudgetStop, TurnBudget};

/// The base every chat turn runs on. See the module docs.
///
/// Cheap to share (`Arc<LlmTurn>`): the loop, the tool dispatcher and the
/// streaming tap all hold handles to the same cancel flag and progress.
pub struct LlmTurn {
    // ── Identity ────────────────────────────────────────────────────────────
    pub session_id: String,
    /// Monotonic window id used by the registry (`AppState::turn_seq`) to
    /// supersede a winding-down turn without removing a newer one.
    pub window_id: u64,
    /// Id the frontend minted for this turn; echoed on every `chat-*` event.
    pub turn_id: String,
    pub model: String,
    pub started_at_ms: i64,
    // ── The base's configuration: frozen at construction ───────────────────
    pub config: AppConfig,
    // ── Turn-owned state ────────────────────────────────────────────────────
    app: AppHandle,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<TurnProgress>>,
    inbox: Mutex<Inbox>,
    budget: Mutex<TurnBudget>,
    /// Private MCP stdio pool: this turn's server processes are never shared
    /// with another turn (or with the panel's warmup pool). Spawned lazily on
    /// the first `tools/list` / `tools/call`, killed at turn-end cleanup;
    /// `McpSession::Drop` is the safety net for unwind paths.
    pub mcp: tokio::sync::Mutex<crate::mcp::McpSession>,
}

#[derive(Default)]
struct Inbox {
    open: bool,
    items: Vec<TurnInboxItem>,
}

impl Inbox {
    fn inject(&mut self, id: String, text: String) -> bool {
        let text = text.trim().to_string();
        if text.is_empty() || !self.open {
            return false;
        }
        self.items.push(TurnInboxItem {
            id,
            text,
            created_at_ms: now_ms(),
        });
        true
    }

    fn drain(&mut self) -> Vec<TurnInboxItem> {
        std::mem::take(&mut self.items)
    }

    fn close(&mut self) {
        self.open = false;
        self.items.clear();
    }
}

impl LlmTurn {
    pub(crate) fn new(
        app: AppHandle,
        session_id: &str,
        window_id: u64,
        turn_id: &str,
        model: &str,
        config: AppConfig,
    ) -> Self {
        let budget = TurnBudget::new(
            config.main_agent_max_rounds,
            config.main_agent_max_tool_calls,
        );
        Self {
            session_id: session_id.to_string(),
            window_id,
            turn_id: turn_id.to_string(),
            model: model.to_string(),
            started_at_ms: now_ms(),
            config,
            app,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(TurnProgress::default())),
            inbox: Mutex::new(Inbox {
                open: true,
                items: Vec::new(),
            }),
            budget: Mutex::new(budget),
            mcp: tokio::sync::Mutex::new(crate::mcp::McpSession::new()),
        }
    }

    // ── Cancellation ────────────────────────────────────────────────────────

    /// The flag the turn loop, its tools and its streams poll.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Stop this turn (the Stop button, addressed by session id).
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    // ── Injection inbox ─────────────────────────────────────────────────────

    /// Queue a user message for the next request boundary. `false` once the
    /// inbox is closed (the turn is winding down) — the UI then sends the text
    /// as a normal next-turn message instead of stranding it here.
    pub fn inject(&self, id: String, text: String) -> bool {
        self.inbox
            .lock()
            .map(|mut inbox| inbox.inject(id, text))
            .unwrap_or(false)
    }

    /// Take the queued injections — called at a request boundary, right
    /// before the next request is built.
    pub fn drain_injections(&self) -> Vec<TurnInboxItem> {
        self.inbox
            .lock()
            .map(|mut inbox| inbox.drain())
            .unwrap_or_default()
    }

    /// Close the inbox before announcing the end of the turn: nothing
    /// accepted after this point could ever reach a request. Undelivered
    /// items are dropped; the UI re-sends its own pending chips.
    pub fn close_inbox(&self) {
        if let Ok(mut inbox) = self.inbox.lock() {
            inbox.close();
        }
    }

    // ── Live progress ───────────────────────────────────────────────────────

    pub fn set_phase(&self, phase: &str) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.phase = phase.to_string();
        }
    }

    /// One settled tool row for a re-attaching webview. The row shape matches
    /// the frontend's `ToolCallEntry`; it is written here, not by the loop
    /// reaching into the snapshot.
    pub fn record_tool_row(&self, row: serde_json::Value) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.tool_calls.push(row);
        }
    }

    /// One delivered injection (`chat-injected`) for the snapshot.
    pub fn record_injection(&self, item: serde_json::Value) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.injections.push(item);
        }
    }

    /// Accumulate one request's usage totals into the snapshot.
    pub fn record_usage(&self, prompt: u64, completion: u64, cached: Option<u64>) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.usage.requests = progress.usage.requests.saturating_add(1);
            progress.usage.prompt_tokens += prompt;
            progress.usage.completion_tokens += completion;
            if let Some(cached) = cached {
                progress.usage.cached_tokens += cached;
                progress.usage.cache_reported = true;
            }
        }
    }

    // ── Turn-scoped emission ────────────────────────────────────────────────

    /// Emit a `chat-token` delta, accumulating it into the live snapshot on
    /// the way out (system notices take this path; streamed model tokens go
    /// through [`TurnProgressSink`] so there is one writer).
    pub fn emit_token(&self, text: String) {
        self.append_content(&text);
        let _ = self.app.emit(
            events::CHAT_TOKEN,
            events::ChatToken {
                turn_id: Some(self.turn_id.clone()),
                text,
            },
        );
    }

    /// Emit `chat-done` for this turn. The inbox closes first: nothing
    /// accepted after this point could reach another request.
    pub fn emit_done(&self) {
        self.close_inbox();
        let _ = self.app.emit(
            events::CHAT_DONE,
            events::ChatTurn {
                turn_id: self.turn_id.clone(),
            },
        );
    }

    /// Emit `chat-error` for this turn. Like `emit_done`, this is an end-of-turn
    /// announcement, so the inbox closes before the UI drops the row.
    pub fn emit_error(&self, error: &str) {
        self.close_inbox();
        let _ = self.app.emit(
            events::CHAT_ERROR,
            events::ChatError {
                turn_id: Some(self.turn_id.clone()),
                error: error.to_string(),
            },
        );
    }

    // ── Budget ──────────────────────────────────────────────────────────────

    /// Count the request that is about to be sent (`None` = within budget).
    pub fn begin_round(&self) -> Option<BudgetStop> {
        self.budget.lock().ok()?.begin_round()
    }

    /// Count one tool call that is about to run.
    pub fn record_tool_call(&self) -> Option<BudgetStop> {
        self.budget.lock().ok()?.record_tool_call()
    }

    /// `(rounds, tool_calls)` used so far.
    pub fn budget_snapshot(&self) -> (u32, u32) {
        self.budget
            .lock()
            .map(|budget| budget.snapshot())
            .unwrap_or((0, 0))
    }

    // ── Lifecycle ───────────────────────────────────────────────────────────

    /// Kill this turn's private MCP server processes. Called by turn-end
    /// cleanup; `McpSession::Drop` covers the unwind paths.
    pub async fn shutdown_mcp(&self, state: &crate::AppState) {
        let mut pool = self.mcp.lock().await;
        pool.shutdown(state);
    }

    pub fn info(&self) -> ActiveTurnInfo {
        ActiveTurnInfo {
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            model: self.model.clone(),
            started_at_ms: self.started_at_ms,
            phase: self
                .progress
                .lock()
                .map(|progress| progress.phase.clone())
                .unwrap_or_default(),
        }
    }

    pub fn snapshot(&self) -> TurnSnapshot {
        let progress = self.progress.lock();
        let (phase, content, reasoning, tool_calls, injections, usage) = match progress {
            Ok(progress) => (
                progress.phase.clone(),
                progress.content.clone(),
                progress.reasoning.clone(),
                progress.tool_calls.clone(),
                progress.injections.clone(),
                TurnUsageSnapshot {
                    prompt_tokens: progress.usage.prompt_tokens,
                    completion_tokens: progress.usage.completion_tokens,
                    cached_tokens: progress.usage.cached_tokens,
                    requests: progress.usage.requests,
                    cache_reported: progress.usage.cache_reported,
                },
            ),
            Err(_) => (
                String::new(),
                String::new(),
                String::new(),
                Vec::new(),
                Vec::new(),
                TurnUsageSnapshot::default(),
            ),
        };
        TurnSnapshot {
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            model: self.model.clone(),
            started_at_ms: self.started_at_ms,
            phase,
            content,
            reasoning,
            tool_calls,
            injections,
            usage,
        }
    }
}

impl TurnProgressSink for LlmTurn {
    fn append_content(&self, text: &str) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.content.push_str(text);
        }
    }

    fn append_reasoning(&self, text: &str) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.reasoning.push_str(text);
        }
    }

    fn rewind_attempt(&self, content: &str, reasoning: &str) {
        if let Ok(mut progress) = self.progress.lock() {
            if !content.is_empty() && progress.content.ends_with(content) {
                let keep = progress.content.len() - content.len();
                progress.content.truncate(keep);
            }
            if !reasoning.is_empty() && progress.reasoning.ends_with(reasoning) {
                let keep = progress.reasoning.len() - reasoning.len();
                progress.reasoning.truncate(keep);
            }
        }
    }
}

/// Closes a turn's injection inbox on drop, on EVERY exit path: final answer,
/// upstream error, Stop, or an unwind. The explicit `emit_done` / `emit_error`
/// close still runs first so the window shuts before the end is announced;
/// this guard is the backstop for paths that announce nothing.
pub(crate) struct InboxGuard<'a>(&'a LlmTurn);

impl<'a> InboxGuard<'a> {
    pub(crate) fn new(turn: &'a LlmTurn) -> Self {
        InboxGuard(turn)
    }
}

impl Drop for InboxGuard<'_> {
    fn drop(&mut self) {
        self.0.close_inbox();
    }
}

#[cfg(test)]
mod tests {
    use super::progress::TurnUsage;
    use super::*;

    #[test]
    fn inbox_accepts_until_closed() {
        let mut inbox = Inbox {
            open: true,
            items: Vec::new(),
        };
        assert!(inbox.inject("1".into(), "  hello  ".into()));
        assert!(!inbox.inject("2".into(), "   ".into()));
        let drained = inbox.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "hello");
        assert!(inbox.drain().is_empty());
        assert!(inbox.inject("3".into(), "queued".into()));
        inbox.close();
        // A closed inbox accepts nothing and drops what was queued.
        assert!(!inbox.inject("4".into(), "late".into()));
        assert!(inbox.drain().is_empty());
    }

    #[test]
    fn usage_snapshot_defaults_to_no_cache_report() {
        let usage = TurnUsage::default();
        assert!(!usage.cache_reported);
        assert_eq!(usage.requests, 0);
    }
}