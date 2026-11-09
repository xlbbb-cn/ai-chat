//! Turn budget and tool-failure guard for the ReAct loops.
//!
//! A ReAct turn is driven by the model: `run_chat_turn` keeps requesting for as
//! long as the reply carries tool calls. Two independent guards bound that:
//!
//! * [`TurnBudget`] caps how many requests and tool calls one turn may use, so
//!   a model that never stops asking for tools cannot burn the context window
//!   (or the user's quota) forever.
//! * [`ToolFailureGuard`] disables one tool for the rest of the *session* once
//!   it failed `tool_failure_limit` times in a row. Repeating an identical
//!   failing call is the most common way a loop wastes a turn; telling the
//!   model the tool is disabled is what makes it switch approach.
//!
//! Both limits come from Settings → Advanced; `0` means "no limit". Nothing in
//! here touches Tauri types, so the behaviour is unit tested in isolation.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Hard ceiling for `main_agent_max_rounds`; configured values are clamped.
pub const MAX_MAIN_AGENT_ROUNDS: u32 = 1_000;
/// Hard ceiling for `main_agent_max_tool_calls`.
pub const MAX_MAIN_AGENT_TOOL_CALLS: u32 = 5_000;
/// Hard ceiling for `tool_failure_limit`.
pub const MAX_TOOL_FAILURE_LIMIT: u32 = 50;

/// Defaults written into a fresh `config.json`.
pub const DEFAULT_MAIN_AGENT_MAX_ROUNDS: u32 = 40;
pub const DEFAULT_MAIN_AGENT_MAX_TOOL_CALLS: u32 = 120;
pub const DEFAULT_TOOL_FAILURE_LIMIT: u32 = 3;

// `serde(default = "…")` needs a function path, not a constant.
pub fn default_main_agent_max_rounds() -> u32 {
    DEFAULT_MAIN_AGENT_MAX_ROUNDS
}

pub fn default_main_agent_max_tool_calls() -> u32 {
    DEFAULT_MAIN_AGENT_MAX_TOOL_CALLS
}

pub fn default_tool_failure_limit() -> u32 {
    DEFAULT_TOOL_FAILURE_LIMIT
}

/// Which limit stopped the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetStop {
    Rounds,
    ToolCalls,
}

impl BudgetStop {
    /// Stable machine-readable reason for the `chat-notice` payload.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rounds => "rounds",
            Self::ToolCalls => "tool_calls",
        }
    }
}

/// Request / tool-call budget of a single chat turn.
///
/// Counting happens at the request boundary (`begin_round`) and immediately
/// before a tool actually runs (`record_tool_call`), so a turn never exceeds
/// the configured numbers by more than the round it is already inside.
#[derive(Clone, Debug)]
pub struct TurnBudget {
    max_rounds: u32,
    max_tool_calls: u32,
    rounds: u32,
    tool_calls: u32,
}

impl TurnBudget {
    pub fn new(max_rounds: u32, max_tool_calls: u32) -> Self {
        Self {
            max_rounds: max_rounds.min(MAX_MAIN_AGENT_ROUNDS),
            max_tool_calls: max_tool_calls.min(MAX_MAIN_AGENT_TOOL_CALLS),
            rounds: 0,
            tool_calls: 0,
        }
    }

    /// Count the request that is about to be sent.
    ///
    /// `None` means "still within budget"; `Some(Rounds)` means the limit was
    /// already spent and the caller must send a wrap-up request instead.
    pub fn begin_round(&mut self) -> Option<BudgetStop> {
        if self.max_rounds > 0 && self.rounds >= self.max_rounds {
            return Some(BudgetStop::Rounds);
        }
        self.rounds = self.rounds.saturating_add(1);
        None
    }

    /// Count one tool call that is about to run.
    ///
    /// `Some(ToolCalls)` means the call must not run: the caller answers it with
    /// [`budget_notice`] instead, so the assistant still receives a `tool`
    /// message for every `tool_call_id` it produced.
    pub fn record_tool_call(&mut self) -> Option<BudgetStop> {
        if self.max_tool_calls > 0 && self.tool_calls >= self.max_tool_calls {
            return Some(BudgetStop::ToolCalls);
        }
        self.tool_calls = self.tool_calls.saturating_add(1);
        None
    }

    /// `(rounds, tool_calls)` used so far — reported to the UI and the model.
    pub fn snapshot(&self) -> (u32, u32) {
        (self.rounds, self.tool_calls)
    }
}

/// Consecutive-failure state of one tool within one session.
#[derive(Clone, Copy, Debug, Default)]
struct ToolState {
    consecutive_failures: u32,
    disabled: bool,
}

/// What the failure guard learned about a call that already ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolTrip {
    pub failures: u32,
}

/// Session-scoped tool circuit breaker.
///
/// State is keyed by `(session_id, tool_name)` and lives in memory only: a
/// disabled tool is disabled for the rest of that chat session, and the next
/// app launch starts clean. The main agent, its helper and every sub-agent of
/// the session share the same state, which is what makes "disabled for this
/// session" true no matter which loop tries the call again.
#[derive(Debug, Default)]
pub struct ToolFailureGuard {
    sessions: Mutex<HashMap<String, HashMap<String, ToolState>>>,
}

impl ToolFailureGuard {
    /// Pre-call gate.
    ///
    /// `Some(notice)` means the tool is already disabled in this session: it
    /// must not run, and the notice is the `tool` answer for the call.
    pub fn blocked_notice(&self, session: &str, tool: &str, limit: u32) -> Option<String> {
        if limit == 0 {
            return None;
        }
        let sessions = self.sessions.lock().ok()?;
        let state = sessions.get(session)?.get(tool)?;
        state
            .disabled
            .then(|| disabled_notice(tool, state.consecutive_failures, limit, false))
    }

    /// Post-call bookkeeping.
    ///
    /// A success resets the tool's counter. A failure increments it, and
    /// `Some(ToolTrip)` is returned exactly once — on the failure that crossed
    /// the limit — so the caller appends the disable notice a single time.
    pub fn record(&self, session: &str, tool: &str, failed: bool, limit: u32) -> Option<ToolTrip> {
        let mut sessions = self.sessions.lock().ok()?;
        let state = sessions
            .entry(session.to_string())
            .or_default()
            .entry(tool.to_string())
            .or_default();

        if !failed {
            state.consecutive_failures = 0;
            return None;
        }

        if state.disabled {
            // Already reported; keep the count for the notice text only.
            return None;
        }

        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if limit > 0 && state.consecutive_failures >= limit {
            state.disabled = true;
            return Some(ToolTrip {
                failures: state.consecutive_failures,
            });
        }
        None
    }

    /// Tools currently disabled in `session`.
    ///
    /// Their definitions are dropped from the next request payload, so the
    /// model cannot pick them again — the short-circuit above only covers a
    /// stale `tool_call` from an earlier assistant message.
    pub fn disabled_tools(&self, session: &str) -> HashSet<String> {
        let Ok(sessions) = self.sessions.lock() else {
            return HashSet::new();
        };
        sessions
            .get(session)
            .map(|tools| {
                tools
                    .iter()
                    .filter(|(_, state)| state.disabled)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drop every session's state.
    ///
    /// Used when the failure limit is reconfigured: raising it is how a user
    /// releases tools that were disabled under the previous value.
    pub fn reset_all(&self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
    }
}

/// Does a tool result represent a failure?
///
/// The three prefixes are the ones the tool layer actually emits: `⛔` for a
/// refused/failed tool, `Error` for the hand-written error strings in
/// `tools/mod.rs` and `agents.rs`, and `MCP tool error` for a failed MCP call.
pub fn tool_output_is_error(output: &str) -> bool {
    let text = output.trim_start();
    text.starts_with('⛔') || text.starts_with("Error") || text.starts_with("MCP tool error")
}

/// Answer for a call the turn's tool-call budget refused to run.
pub fn budget_notice(tool: &str) -> String {
    format!(
        "⛔ TURN BUDGET EXHAUSTED — the tool-call limit for this turn was reached, so `{tool}` was \
not executed.\nNo further tool call will run in this turn. Answer now with the material already \
collected and state plainly what is still missing."
    )
}

/// Notice about a disabled tool.
///
/// `just_now` distinguishes "this failure disabled it" (appended to the real
/// result) from "it was already disabled" (the whole answer of a skipped call).
pub fn disabled_notice(tool: &str, failures: u32, limit: u32, just_now: bool) -> String {
    if just_now {
        format!(
            "⛔ TOOL DISABLED FOR THIS SESSION — `{tool}` failed {failures} time(s) in a row (limit \
{limit}).\nIt will not run again in this session. Do not call it again: use a different tool or \
MCP server, or tell the user what is blocking you."
        )
    } else {
        format!(
            "⛔ TOOL DISABLED FOR THIS SESSION — `{tool}` was disabled after {failures} consecutive \
failure(s) (limit {limit}).\nThis call was not executed. Use a different tool or MCP server, or \
tell the user what is blocking you."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A limited budget stops exactly at the configured counts.
    #[test]
    fn budget_counts_rounds_and_tool_calls() {
        let mut budget = TurnBudget::new(2, 2);
        assert_eq!(budget.begin_round(), None);
        assert_eq!(budget.begin_round(), None);
        assert_eq!(budget.begin_round(), Some(BudgetStop::Rounds));

        let mut budget = TurnBudget::new(0, 1);
        assert_eq!(budget.record_tool_call(), None);
        assert_eq!(budget.record_tool_call(), Some(BudgetStop::ToolCalls));
        assert_eq!(budget.snapshot(), (0, 1));
    }

    /// `0` disables a limit but the counters still advance for the report.
    #[test]
    fn zero_means_unlimited() {
        let mut budget = TurnBudget::new(0, 0);
        for _ in 0..500 {
            assert_eq!(budget.begin_round(), None);
            assert_eq!(budget.record_tool_call(), None);
        }
        assert_eq!(budget.snapshot(), (500, 500));
    }

    /// Configured values above the hard ceiling are clamped, never trusted.
    #[test]
    fn budget_clamps_to_the_hard_ceiling() {
        let mut budget = TurnBudget::new(u32::MAX, u32::MAX);
        let mut rounds = 0;
        while budget.begin_round().is_none() {
            rounds += 1;
            assert!(rounds <= MAX_MAIN_AGENT_ROUNDS, "round cap not applied");
        }
        assert_eq!(rounds, MAX_MAIN_AGENT_ROUNDS);
    }

    /// Consecutive failures disable a tool once, then short-circuit every call.
    #[test]
    fn failures_disable_the_tool_for_the_session() {
        let guard = ToolFailureGuard::default();
        assert!(guard.blocked_notice("s1", "run_shell", 2).is_none());

        assert_eq!(guard.record("s1", "run_shell", true, 2), None);
        assert!(guard.blocked_notice("s1", "run_shell", 2).is_none());

        let trip = guard
            .record("s1", "run_shell", true, 2)
            .expect("the second consecutive failure trips the limit");
        assert_eq!(trip.failures, 2);

        // Already disabled: reported once, never counted again.
        assert_eq!(guard.record("s1", "run_shell", true, 2), None);
        assert!(guard
            .blocked_notice("s1", "run_shell", 2)
            .expect("a disabled tool is short-circuited")
            .contains("TOOL DISABLED FOR THIS SESSION"));
        assert!(guard.disabled_tools("s1").contains("run_shell"));
    }

    /// A successful call resets the counter: only *consecutive* failures count.
    #[test]
    fn success_resets_the_failure_count() {
        let guard = ToolFailureGuard::default();
        assert_eq!(guard.record("s1", "file_actions", true, 2), None);
        assert_eq!(guard.record("s1", "file_actions", false, 2), None);
        assert_eq!(guard.record("s1", "file_actions", true, 2), None);
        assert!(guard.disabled_tools("s1").is_empty());
        // The next failure is the second one in a row and trips.
        assert!(guard.record("s1", "file_actions", true, 2).is_some());
    }

    /// Sessions are independent, and `limit = 0` never disables anything.
    #[test]
    fn sessions_are_isolated_and_zero_disables_the_guard() {
        let guard = ToolFailureGuard::default();
        for _ in 0..5 {
            assert_eq!(guard.record("s1", "run_cmd", true, 0), None);
            assert!(guard.blocked_notice("s1", "run_cmd", 0).is_none());
        }
        assert!(guard.disabled_tools("s1").is_empty());

        for _ in 0..2 {
            guard.record("s1", "run_cmd", true, 2);
        }
        assert!(guard.disabled_tools("s1").contains("run_cmd"));
        assert!(guard.disabled_tools("s2").is_empty());
        assert!(guard.blocked_notice("s2", "run_cmd", 2).is_none());
    }

    /// `reset_all` releases everything (limit reconfigured in Settings).
    #[test]
    fn reset_all_releases_disabled_tools() {
        let guard = ToolFailureGuard::default();
        guard.record("s1", "run_cmd", true, 1).expect("trips at 1");
        assert!(!guard.disabled_tools("s1").is_empty());
        guard.reset_all();
        assert!(guard.disabled_tools("s1").is_empty());
        assert!(guard.blocked_notice("s1", "run_cmd", 1).is_none());
    }

    /// The three error prefixes the tool layer emits are recognised.
    #[test]
    fn error_prefixes_are_recognised() {
        assert!(tool_output_is_error("⛔ Command blocked"));
        assert!(tool_output_is_error("Error: file not found"));
        assert!(tool_output_is_error("MCP tool error: connection reset"));
        assert!(tool_output_is_error("  Error: indented"));
        assert!(!tool_output_is_error("OK: 12 files"));
        assert!(!tool_output_is_error(""));
        // Ordinary prose that merely mentions an error is not a failure.
        assert!(!tool_output_is_error("Found 2 errors in the log"));
    }
}
