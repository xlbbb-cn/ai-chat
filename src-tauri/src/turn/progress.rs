//! Live state of one [`super::LlmTurn`] and the DTOs derived from it.
//!
//! The progress snapshot is what a re-attaching webview rebuilds the live
//! assistant row from: `content` / `reasoning` accumulate EVERY delta the turn
//! emits (narration of tool rounds included), exactly like the transcript's
//! assistant row does; `tool_calls` holds one settled row per executed call;
//! `injections` holds the `chat-injected` items delivered at request
//! boundaries, which the frontend stores on the assistant row when the turn
//! settles.

use serde::Serialize;
use serde_json::Value;

/// Token accounting accumulated across every request of one turn. Mirrors what
/// the frontend's turn-stats footer sums from `chat-usage`, so a re-attached
/// transcript can seed the same figures instead of restarting from zero.
#[derive(Debug, Default, Clone)]
pub struct TurnUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub requests: u32,
    /// Whether any request reported a cache dialect (`cached_tokens` present).
    /// The footer only shows a hit rate when this is true.
    pub cache_reported: bool,
}

/// Live state of one running turn. See the module docs.
#[derive(Debug, Default)]
pub struct TurnProgress {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<Value>,
    pub injections: Vec<Value>,
    /// Coarse activity label for the History badge: `planning`, `tools`,
    /// `answering`. Informational only.
    pub phase: String,
    pub usage: TurnUsage,
}

/// One message the user pushed into a running turn. The inbox is owned by the
/// base, so an item can only ever be drained by the turn that accepted it.
#[derive(Clone, Debug, Serialize)]
pub struct TurnInboxItem {
    pub id: String,
    pub text: String,
    /// Unix milliseconds, rendered as the "added at" clock label.
    pub created_at_ms: i64,
}

/// Serializable form of [`TurnUsage`].
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct TurnUsageSnapshot {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub requests: u32,
    pub cache_reported: bool,
}

/// Everything a re-attaching webview needs to rebuild the live assistant row
/// of a turn that kept running while another conversation was in front.
#[derive(Debug, Clone, Serialize)]
pub struct TurnSnapshot {
    pub session_id: String,
    pub turn_id: String,
    pub model: String,
    /// Thinking depth the turn runs with; absent = the provider default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub started_at_ms: i64,
    pub phase: String,
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<Value>,
    pub injections: Vec<Value>,
    pub usage: TurnUsageSnapshot,
}

/// Write side of a turn's live progress snapshot.
///
/// The streaming transport holds a `&dyn TurnProgressSink` instead of the raw
/// `Arc<Mutex<TurnProgress>>`, so every mutation of the snapshot goes through
/// the turn that owns it. `LlmTurn` is the only implementation; callers without
/// a chat turn (sub-agents, planner, summarizer, maintenance) pass `None`.
pub trait TurnProgressSink: Send + Sync {
    fn append_content(&self, text: &str);
    fn append_reasoning(&self, text: &str);
    /// Drop the failed attempt's tail so a retry re-streams cleanly. Same
    /// suffix match the frontend's `onRetry` uses; never truncates a prefix.
    fn rewind_attempt(&self, content: &str, reasoning: &str);
}

/// Unix milliseconds, the turn family's timestamp unit.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}