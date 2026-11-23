//! Streaming transport: chunk processing, retries and the unified
//! request helper.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::db;

pub(crate) use crate::http::{build_http_client, build_stream_client};

/// Streaming retry policy: attempts and the two backoff steps.
const STREAM_MAX_RETRIES: usize = 3;
const STREAM_RETRY_BACKOFF_MS: [u64; 2] = [300, 900];

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

/// Validate and sanitize the messages array before sending to the API.
///
/// Ensures that every `role: "tool"` message has a preceding `role: "assistant"`
/// message whose `tool_calls` array contains a matching `tool_call_id`.  Any
/// orphaned tool message (no matching assistant), duplicate tool response for
/// an already-satisfied id, and incomplete assistant round (one or more of its
/// tool_call_ids never received a response) are **all** removed — including the
/// tool messages tied to an incomplete assistant.
pub fn sanitize_tool_pairs(all_messages: &mut Vec<Value>) {
    if all_messages.is_empty() {
        return;
    }

    let mut keep = vec![true; all_messages.len()];

    // Stack entries: (assistant_index, unconsumed_ids, consumed_tool_indices)
    let mut assistant_stack: Vec<(usize, std::collections::HashSet<String>, Vec<usize>)> =
        Vec::new();

    for (i, msg) in all_messages.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");

        if role == "assistant" {
            if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                if !tcs.is_empty() {
                    let ids: std::collections::HashSet<String> = tcs
                        .iter()
                        .filter_map(|tc| tc.get("id").and_then(|v| v.as_str()))
                        .map(|s| s.to_string())
                        .collect();
                    assistant_stack.push((i, ids, Vec::new()));
                }
            }
        } else if role == "tool" {
            let call_id = msg
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Search from newest assistant backwards so a tool response is
            // paired with the most recent round that declared its id.
            let mut found = false;
            for (_, ref mut ids, ref mut consumed) in assistant_stack.iter_mut().rev() {
                if ids.remove(&call_id) {
                    consumed.push(i);
                    found = true;
                    break;
                }
            }
            if !found {
                keep[i] = false; // orphaned (or duplicate) — no matching assistant tool_call
            }
        }
    }

    // Remove incomplete assistants (unconsumed ids remain) AND their tool
    // messages, so we never send a partial round to the API.
    for &(idx, ref remaining_ids, ref tool_indices) in &assistant_stack {
        if !remaining_ids.is_empty() {
            keep[idx] = false; // incomplete round
            for &ti in tool_indices {
                keep[ti] = false;
            }
        }
    }

    // ── Apply removals ───────────────────────────────────────────────────────
    let mut cleaned: Vec<Value> = Vec::with_capacity(all_messages.len());
    for (i, msg) in all_messages.drain(..).enumerate() {
        if keep[i] {
            cleaned.push(msg);
        }
    }
    *all_messages = cleaned;
}

/// DashScope-strict variant of [`sanitize_tool_pairs`].
///
/// The lenient version pairs a `tool` response with the newest assistant that
/// declared its id, wherever that assistant sits in the array. DashScope (and
/// strict OpenAI-compatible gateways) validate a tighter shape: every `tool`
/// message must sit in the block that directly follows its assistant, and that
/// block must answer every declared id exactly once — this is exactly what the
/// `400 … The tool_call_id in the response message does not match the
/// tool_call_id in the tool_call` rejection enforces. Where the two rules
/// diverge, the lenient version blesses a sequence the provider then rejects,
/// so what actually goes out is validated with the provider's own rule here.
///
/// On the sequences the chat loop builds this is a no-op: each round is pushed
/// as one adjacent block (assistant, then its tool responses) and nothing is
/// inserted between them.
pub fn sanitize_tool_pairs_strict(all_messages: &mut Vec<Value>) {
    if all_messages.is_empty() {
        return;
    }

    let mut keep = vec![true; all_messages.len()];
    let mut i = 0usize;
    while i < all_messages.len() {
        let role = all_messages[i]
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if role == "assistant" {
            // Declared ids, with multiplicities: a duplicated id needs one
            // response per occurrence, so counts, not a set.
            let mut pending: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            if let Some(tcs) = all_messages[i]
                .get("tool_calls")
                .and_then(|v| v.as_array())
            {
                for tc in tcs {
                    if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                        *pending.entry(id.to_string()).or_insert(0) += 1;
                    }
                }
            }
            if !pending.is_empty() {
                // The response block: the run of `tool` messages that directly
                // follows the assistant.
                let mut j = i + 1;
                while j < all_messages.len()
                    && all_messages[j]
                        .get("role")
                        .and_then(|v| v.as_str())
                        == Some("tool")
                {
                    let call_id = all_messages[j]
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    match pending.get_mut(&call_id) {
                        Some(count) if *count > 0 => {
                            *count -= 1;
                        }
                        _ => keep[j] = false, // orphan or duplicate response
                    }
                    j += 1;
                }
                if pending.values().any(|count| *count > 0) {
                    // Incomplete round: the provider would reject it wholesale,
                    // so the assistant AND its response block both go.
                    keep[i] = false;
                    for k in i + 1..j {
                        keep[k] = false;
                    }
                }
                i = j;
                continue;
            }
            // A plain assistant without tool_calls falls through: `tool`
            // messages after it have no adjacent declarer and are orphans.
        }
        if role == "tool" {
            keep[i] = false;
        }
        i += 1;
    }

    let mut cleaned: Vec<Value> = Vec::with_capacity(all_messages.len());
    for (i, msg) in all_messages.drain(..).enumerate() {
        if keep[i] {
            cleaned.push(msg);
        }
    }
    *all_messages = cleaned;
}

/// Does an upstream error mean the provider rejected the message sequence over
/// tool-call pairing? DashScope answers 400 when a `tool` response does not
/// match the `tool_calls` of the assistant it follows, phrased several ways
/// across providers/relays. The turn can repair that (drop the offending round
/// and retry), so the error must be classified instead of staying fatal: a
/// false positive costs one harmless repair retry, a false negative fails the
/// turn.
pub(crate) fn is_tool_pairing_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    const PATTERNS: &[&str] = &[
        "tool_call_id in the response message",
        "must be followed by tool messages",
        "must be a response to a preceding message",
        "tool_call_ids did not have response messages",
    ];
    PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

/// The `message[N]` index a provider's pairing error points at, when it
/// reports one (`…: messages: message[24].`). Best effort: a relay may have
/// rewritten the array, so the index is a hint, not a guarantee.
pub(crate) fn reported_message_index(err: &str) -> Option<usize> {
    let start = err.find("message[")? + "message[".len();
    let rest = &err[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse::<usize>().ok()
}

/// Compact per-message id map for the pairing-repair log: index, role, the ids
/// an assistant declares, and the id a `tool` response answers. No content is
/// included — the full prompt is already in the interaction log, and this map
/// only has to show the pairing structure DashScope complained about.
pub(crate) fn message_id_map(messages: &[Value]) -> String {
    const MAX_ASSISTANT_IDS: usize = 8;
    let mut out = String::new();
    for (i, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("?");
        let mut line = format!("[{i}] {role}");
        if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
            let mut ids: Vec<String> = tcs
                .iter()
                .map(|tc| {
                    tc.get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
            if ids.len() > MAX_ASSISTANT_IDS {
                ids.truncate(MAX_ASSISTANT_IDS);
                ids.push("…".to_string());
            }
            line.push_str(&format!(" calls={}", ids.join(",")));
        }
        if let Some(cid) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
            line.push_str(&format!(" for={cid}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub struct StreamResult {
    pub finish_reason: String,
    pub tool_calls: Vec<(String, String, String)>,
    pub content: String,
    pub reasoning_content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Cached prompt-prefix tokens reported by the provider, or `None` when
    /// the endpoint does not report caching at all. `Some(0)` is real data
    /// (a guaranteed miss, e.g. the first request of a session) and keeps
    /// cache-hit-rate metrics honest, so it must not collapse to `None`.
    pub cached_tokens: Option<u32>,
    /// Streamed phase timings of the round (see [`StreamTiming`]).
    pub timing: StreamTiming,
}

/// Wall-clock phases of one streamed request, measured from the moment the HTTP
/// request is sent. Both are `Option` so "this phase never happened" (a model
/// that answers without reasoning) is not reported as `0 ms` — the session
/// report has to tell a fast thinker from a non-thinking model.
#[derive(Debug, Default, Clone, Copy)]
pub struct StreamTiming {
    /// Time to the first streamed delta of any kind (reasoning, content or tool
    /// call) — the latency the user waits through before output appears.
    pub first_token_ms: Option<u32>,
    /// Time to the first *content* delta when the model streamed reasoning
    /// first: the thinking phase of the round ("推理时长").
    pub reasoning_ms: Option<u32>,
}

/// Accumulates [`StreamTiming`] while the SSE deltas arrive.
pub struct StreamMeter {
    started: std::time::Instant,
    saw_reasoning: bool,
    timing: StreamTiming,
}

impl StreamMeter {
    pub(crate) fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            saw_reasoning: false,
            timing: StreamTiming::default(),
        }
    }

    pub(crate) fn elapsed_ms(&self) -> u32 {
        self.started.elapsed().as_millis().min(u32::MAX as u128) as u32
    }

    /// First streamed delta of any kind.
    pub(crate) fn mark_output(&mut self) {
        if self.timing.first_token_ms.is_none() {
            self.timing.first_token_ms = Some(self.elapsed_ms());
        }
    }

    pub(crate) fn mark_reasoning(&mut self) {
        self.saw_reasoning = true;
        self.mark_output();
    }

    /// First answer token: closes the thinking phase when reasoning was
    /// streamed before it.
    pub(crate) fn mark_content(&mut self) {
        self.mark_output();
        if self.timing.reasoning_ms.is_none() && self.saw_reasoning {
            self.timing.reasoning_ms = Some(self.elapsed_ms());
        }
    }
}

/// Options controlling how a streaming LLM request behaves.
/// Main agent: reasoning + usage, token_event=crate::events::CHAT_TOKEN, no task_id.
/// Sub-agent:  token_event=crate::events::AGENT_TASK_TOKEN, reasoning_event=crate::events::AGENT_TASK_REASONING,
///             task_id=Some(...), no usage.
pub struct StreamOptions<'a> {
    /// Event name for content delta tokens.
    pub token_event: &'a str,
    /// If set, include `{"task_id": ..., "token": ...}` payload instead of a plain string.
    pub task_id: Option<&'a str>,
    /// Chat session the request belongs to. Attached to every structured
    /// payload (`token_event` / `reasoning_event` with a task, `chat-usage`)
    /// so the UI can drop a winding-down turn's events after the user
    /// switched to another conversation. `None` for callers without a chat
    /// session (planner supervisor, evolution maintenance).
    pub session_id: Option<&'a str>,
    /// Id of the chat turn this request belongs to, echoed on every
    /// `chat-token` / `chat-reasoning-token` / `chat-retry` / `chat-error`
    /// payload so the UI can drop what a superseded turn still emits. `None`
    /// for callers that are not a user chat turn (planner, maintenance).
    pub turn_id: Option<&'a str>,
    /// Event name for reasoning deltas, or `None` to keep them off the wire.
    ///
    /// The main chat uses `chat-reasoning-token`; a sub-agent mission uses its
    /// own event so the task row can show a live "thinking" line while the
    /// model reasons — without leaking that text into the chat transcript.
    pub reasoning_event: Option<&'a str>,
    /// Emit `chat-usage` and populate prompt/completion token counts.
    pub emit_usage: bool,
    /// Emit content deltas on `token_event`. The planner supervisor turns this
    /// off so its internal reasoning never reaches the chat transcript.
    pub emit_tokens: bool,
    /// Emit `chat-error` for a terminal upstream error.
    ///
    /// Only the user's own chat turn may emit it: the frontend treats
    /// `chat-error` as the whole turn failing, closes the transcript and drops
    /// later task events. Sub-agent requests and internal helpers therefore turn
    /// this off and report through their own scoped events instead.
    pub emit_errors: bool,
    /// Optional —— token ceiling used for usage ratio reporting.
    pub usage_max_tokens: Option<u32>,
    /// DS-Format (`config.ds_format`): normalize the request so assistant
    /// messages carry `reasoning_content` back, as DeepSeek-family thinking
    /// models require.
    pub ds_format: bool,
    /// Extra HTTP headers attached to every attempt: the conversation-id
    /// header (`AppConfig::conversation_id_header`, default
    /// `x-conversation-id`) plus the user-configured custom headers. Built once
    /// per call with `upstream_request_headers`.
    pub headers: &'a [(String, String)],
    /// Live progress snapshot of the user's own chat turn (see
    /// `turn_registry`). When set, every content/reasoning delta is appended
    /// to it as it streams, so a webview that re-attaches to a background
    /// session rebuilds the identical transcript. `None` for every other
    /// caller (sub-agents, planner, summarizer, maintenance).
    pub progress: Option<&'a Arc<std::sync::Mutex<crate::turn::TurnProgress>>>,
}


pub(crate) fn retry_backoff_ms(attempt: usize) -> u64 {
    STREAM_RETRY_BACKOFF_MS
        .get(attempt.saturating_sub(1))
        .copied()
        .unwrap_or(1500)
}

pub(crate) fn is_retryable_network_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    let patterns = [
        "socket connection was closed unexpectedly",
        "connection reset",
        "connection closed",
        "broken pipe",
        "connection aborted",
        "timed out",
        "timeout",
        "unexpected eof",
        "incomplete message",
        "stream error",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

/// Transient HTTP statuses worth retrying with backoff. Relays answer with
/// 429/5xx far more often than with network-level errors, and matching the
/// error body alone ("timed out", …) misses every one of them — a mid-turn
/// rate limit or overload then killed the whole turn for no reason.
pub(crate) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// `Retry-After` as delta-seconds (the HTTP-date form is ignored). Capped so a
/// hostile header cannot stall the turn for minutes.
pub(crate) fn parse_retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(value.min(30)))
}

pub(crate) fn extract_upstream_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.to_string())
}

pub(crate) fn is_retryable_http_error_body(body: &str) -> bool {
    is_retryable_network_error(&extract_upstream_error_message(body))
}


/// Default header correlating every upstream request with the chat session it belongs to.
pub(crate) const CONVERSATION_ID_HEADER: &str = "x-conversation-id";

/// Resolve the conversation-id header name configured in Settings.
///
/// A missing or blank value means "use the default", so clearing the field can
/// never silently stop sending the correlation header.
pub(crate) fn conversation_id_header_name(config: &crate::AppConfig) -> &str {
    config
        .conversation_id_header
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(CONVERSATION_ID_HEADER)
}

/// Extra headers configured in Settings (Settings → API → Custom headers).
///
/// Entries with a blank name are dropped; when the same name appears twice the
/// first one wins (HTTP header names are case-insensitive). Invalid names and
/// values are rejected later by `apply_request_headers`, so one bad row cannot
/// break a request.
pub(crate) fn custom_request_headers(config: &crate::AppConfig) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::with_capacity(config.custom_headers.len());
    for header in &config.custom_headers {
        let name = header.name.trim();
        if name.is_empty()
            || headers
                .iter()
                .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        headers.push((name.to_string(), header.value.clone()));
    }
    headers
}

/// Every header attached to an upstream chat request for `session_id`: the
/// conversation id first, then the user-configured custom headers (which may
/// not shadow it).
pub(crate) fn upstream_request_headers(
    config: &crate::AppConfig,
    session_id: &str,
) -> Vec<(String, String)> {
    let conversation_header = conversation_id_header_name(config).to_string();
    let mut headers = Vec::with_capacity(config.custom_headers.len() + 1);
    headers.push((
        conversation_header.clone(),
        conversation_id_header_value(session_id),
    ));
    for header in custom_request_headers(config) {
        if header.0.eq_ignore_ascii_case(&conversation_header) {
            continue;
        }
        headers.push(header);
    }
    headers
}

/// `true` when the resolved headers already carry `Authorization` (for
/// example a custom row in Settings). Callers then skip `bearer_auth` so the
/// configured value wins instead of being sent twice.
pub(crate) fn has_authorization_header(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
}

/// Apply pre-resolved headers to a request. Names or values `reqwest` rejects
/// are skipped rather than failing the whole turn.
pub(crate) fn apply_request_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    use reqwest::header::{HeaderName, HeaderValue};
    for (name, value) in headers {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) else {
            continue;
        };
        request = request.header(name, value);
    }
    request
}

/// Value for the conversation-id header, derived from the chat session id.
///
/// Session ids are UUID v4 strings — the UI creates them with
/// `crypto.randomUUID()`. A valid UUID is forwarded in canonical form, so the
/// header always matches the session id verbatim; a malformed value is
/// replaced by a fresh v4 id, because upstreams that key conversations on this
/// header expect UUID v4 and must never receive a non-UUID value.
pub(crate) fn conversation_id_header_value(session_id: &str) -> String {
    uuid::Uuid::parse_str(session_id.trim())
        .map(|id| id.to_string())
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
}

/// Apply output token limits to the request body.
///
/// OpenAI-compatible APIs accept two related but distinct parameters:
/// - `max_completion_tokens` (used by newer models such as the o1 family) — default 128K.
/// - `max_tokens` (legacy field) — default 512K.
///
/// Each is set independently. Passing `None` for either leaves it unset, so
/// the upstream API falls back to its own default for that field.
pub(crate) fn apply_completion_token_limit(
    req_body: &mut Value,
    max_completion_tokens: Option<u32>,
    max_tokens: Option<u32>,
) {
    if let Some(v) = max_completion_tokens {
        req_body["max_completion_tokens"] = json!(v);
    }
    if let Some(v) = max_tokens {
        req_body["max_tokens"] = json!(v);
    }
}

/// Reasoning effort values forwarded to the provider as `reasoning_effort`.
/// Anything else (typo, provider-specific string) is dropped instead of being
/// sent upstream, where strict OpenAI-compatible endpoints reject it with 400.
pub(crate) const REASONING_EFFORTS: [&str; 4] = ["minimal", "low", "medium", "high"];

/// Resolve the reasoning effort ("thinking depth") for one model.
///
/// Resolution order:
/// 1. the per-model value chosen in the chat toolbar (`model_reasoning_effort`,
///    stored keyed by model id);
/// 2. the legacy global `model_settings.reasoning_effort`;
/// 3. `None` — the field is omitted so the provider applies its own default.
pub(crate) fn resolve_reasoning_effort(
    config: &crate::AppConfig,
    model: &str,
) -> Option<&'static str> {
    let candidate = config
        .model_reasoning_effort
        .get(model)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let global = config.model_settings.reasoning_effort.trim();
            (!global.is_empty()).then_some(global)
        })?;

    REASONING_EFFORTS
        .iter()
        .find(|effort| effort.eq_ignore_ascii_case(candidate))
        .copied()
}

/// Effective context window for one model.
///
/// Resolution order:
/// 1. the per-model value in `config.model_context_lengths` (auto-detected
///    from the provider's `/models` response, matched from the metadata
///    catalogue, or entered manually);
/// 2. `default` — a conservative value chosen by the caller.
///
/// Output-token caps (`max_tokens` / `max_complete_tokens`) are deliberately
/// not consulted: they describe a different quantity and using one as the
/// context budget silently shrinks the window to a few thousand tokens.
pub(crate) fn resolve_context_window(config: &crate::AppConfig, model: &str, default: u32) -> u32 {
    config
        .model_context_lengths
        .get(model.trim())
        .copied()
        .filter(|limit| *limit > 0)
        .unwrap_or(default)
}

/// Cached prompt-prefix token count reported in one of the OpenAI-compatible
/// usage dialects. The three shapes are mutually exclusive per provider, so
/// the first present wins:
/// - OpenAI: `prompt_tokens_details.cached_tokens`
/// - DeepSeek: `prompt_cache_hit_tokens`
/// - Anthropic-style pass-throughs of OpenAI-compatible relays:
///   `cache_read_input_tokens`
///
/// `None` when none is present — the endpoint does not report caching, which
/// is different from reporting a zero-hit request.
pub(crate) fn extract_cached_tokens(usage: &Value) -> Option<u32> {
    let pick = |value: Option<&Value>| -> Option<u32> {
        let number = value?;
        Some(
            number
                .as_u64()
                .or_else(|| number.as_f64().map(|f| f.round() as u64))?
                .min(u32::MAX as u64) as u32,
        )
    };
    pick(usage.pointer("/prompt_tokens_details/cached_tokens"))
        .or_else(|| pick(usage.get("prompt_cache_hit_tokens")))
        .or_else(|| pick(usage.get("cache_read_input_tokens")))
}

pub(crate) fn process_stream_chunk(
    app: &AppHandle,
    parsed: &Value,
    opts: &StreamOptions<'_>,
    finish_reason: &mut String,
    tool_calls: &mut Vec<(String, String, String)>,
    content: &mut String,
    reasoning_content: &mut String,
    prompt_tokens: &mut u32,
    completion_tokens: &mut u32,
    cached_tokens: &mut Option<u32>,
    got_tool_calls: &mut bool,
    meter: &mut StreamMeter,
) {
    // Always parse usage so every caller (sub-agents, the main-agent helper and
    // the planner supervisor) gets accurate token counts. Only the main chat
    // paints the usage gauge, so `emit_usage` now controls just the event.
    if let Some(usage) = parsed.get("usage").filter(|v| !v.is_null()) {
        *prompt_tokens = usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        *completion_tokens = usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let cached_report = extract_cached_tokens(usage);
        *cached_tokens = cached_report;
        if opts.emit_usage {
            let total_tokens = usage
                .get("total_tokens")
                .and_then(|v| v.as_u64())
                .map(|total| total as u32)
                .unwrap_or_else(|| prompt_tokens.saturating_add(*completion_tokens));
            let usage_ratio = opts
                .usage_max_tokens
                .filter(|max| *max > 0)
                .map(|max| total_tokens as f64 / max as f64)
                .unwrap_or(0.0);
            // Only reported when the provider exposes a cache dialect at all:
            // without it a hard-coded `0%` would look like real data.
            let (cached_tokens, hit_ratio) = match cached_report {
                Some(cached) => {
                    let hit_ratio = if *prompt_tokens > 0 {
                        ((cached as f64) / (*prompt_tokens as f64)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    (Some(cached), Some(hit_ratio))
                }
                None => (None, None),
            };
            let _ = app.emit(
                crate::events::CHAT_USAGE,
                crate::events::ChatUsage {
                    prompt_tokens: *prompt_tokens,
                    completion_tokens: *completion_tokens,
                    total_tokens,
                    max_tokens: opts.usage_max_tokens,
                    usage_ratio,
                    session_id: opts.session_id.map(str::to_string),
                    cached_tokens,
                    hit_ratio,
                },
            );
        }
    }

    let Some(choice) = parsed["choices"].get(0) else {
        return;
    };
    let delta = &choice["delta"];

    if let Some(fr) = choice["finish_reason"].as_str() {
        if !fr.is_empty() {
            *finish_reason = fr.to_string();
            if fr == "tool_calls" {
                *got_tool_calls = true;
            }
        }
    }

    // Always accumulate the reasoning trace (both field spellings used by
    // providers/relays) — thinking-mode APIs require the original
    // `reasoning_content` to be passed back on assistant messages later in the
    // turn. Only the UI event emission is optional.
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(|v| v.as_str())
    {
        if !reasoning.is_empty() {
            meter.mark_reasoning();
        }
        reasoning_content.push_str(reasoning);
        if let Some(progress) = opts.progress {
            if let Ok(mut progress) = progress.lock() {
                progress.reasoning.push_str(reasoning);
            }
        }
        if let Some(event) = opts.reasoning_event {
            if let Some(task_id) = opts.task_id {
                let _ = app.emit(
                    event,
                    crate::events::AgentTaskToken {
                        task_id: task_id.to_string(),
                        session_id: opts.session_id.map(str::to_string),
                        token: reasoning.to_string(),
                    },
                );
            } else {
                let _ = app.emit(
                    event,
                    crate::events::ChatToken {
                        turn_id: opts.turn_id.map(str::to_string),
                        text: reasoning.to_string(),
                    },
                );
            }
        }
    }

    if let Some(tcs) = delta["tool_calls"].as_array() {
        if !tcs.is_empty() {
            meter.mark_output();
        }
        for tc in tcs {
            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
            while tool_calls.len() <= idx {
                tool_calls.push((String::new(), String::new(), String::new()));
            }
            if let Some(id) = tc["id"].as_str() {
                // Keep the first non-empty id: some relays repeat `id: ""` on
                // the deltas after the first one, which would otherwise erase a
                // valid id and break the tool round on the next request.
                if !id.trim().is_empty() && tool_calls[idx].0.trim().is_empty() {
                    tool_calls[idx].0 = id.to_string();
                }
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                tool_calls[idx].1 = name.to_string();
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                tool_calls[idx].2.push_str(args);
            }
        }
    }

    if let Some(token) = delta["content"].as_str() {
        if !token.is_empty() {
            meter.mark_content();
        }
        content.push_str(token);
        if let Some(progress) = opts.progress {
            if let Ok(mut progress) = progress.lock() {
                progress.content.push_str(token);
            }
        }
        if opts.emit_tokens {
            if let Some(task_id) = opts.task_id {
                let _ = app.emit(
                    opts.token_event,
                    crate::events::AgentTaskToken {
                        task_id: task_id.to_string(),
                        session_id: opts.session_id.map(str::to_string),
                        token: token.to_string(),
                    },
                );
            } else {
                let _ = app.emit(
                    opts.token_event,
                    crate::events::ChatToken {
                        turn_id: opts.turn_id.map(str::to_string),
                        text: token.to_string(),
                    },
                );
            }
        }
    }
}

/// Guarantee every accumulated tool call has a non-empty, unique id.
///
/// Some OpenAI-compatible relays omit `id` in the streamed `tool_calls` deltas
/// (or repeat one). Echoing an empty/duplicate id back in the assistant message
/// and the matching `tool` message makes strict providers (e.g. DashScope)
/// reject the next request with "The tool_call_id in the response message does
/// not match the tool_call_id in the tool_call". Generating a stable id here
/// keeps both sides in sync.
pub(crate) fn ensure_tool_call_ids(tool_calls: &mut [(String, String, String)]) {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (id, _, _) in tool_calls.iter_mut() {
        if id.trim().is_empty() || used.contains(id.as_str()) {
            let generated = format!("call_{}", uuid::Uuid::new_v4().simple());
            *id = generated.clone();
            used.insert(generated);
        } else {
            used.insert(id.clone());
        }
    }
}

/// A finished stream that carries neither text nor a tool call produced
/// nothing usable. Relays close an aborted generation politely — a usage
/// chunk then `[DONE]`, no `finish_reason`, no content — so the completeness
/// check above cannot tell it apart from a real answer. Accepting it ended the
/// turn on an empty reply: the transcript stopped on the previous partial text
/// and the task died mid-way.
pub(crate) fn is_empty_completion(content: &str, tool_calls: &[(String, String, String)]) -> bool {
    content.trim().is_empty() && tool_calls.is_empty()
}

/// Build the assistant message that carries a finished tool-call round back
/// into the request history.
///
/// `content` keeps the narration the model streamed next to its calls: the UI
/// already shows that text, so dropping it only here made the model forget what
/// it had just told the user — it re-read files and repeated itself. A round
/// that streamed no text keeps `null`, never an empty string.
///
/// `reasoning_content` rides along unmodified because thinking-mode APIs
/// require it on every tool-call turn.
pub(crate) fn assistant_tool_message(
    content: &str,
    reasoning_content: &str,
    tool_calls: Vec<Value>,
) -> Value {
    let mut msg = json!({
        "role": "assistant",
        "content": Value::Null,
        "tool_calls": tool_calls,
    });
    if !content.trim().is_empty() {
        msg["content"] = json!(content);
    }
    if !reasoning_content.is_empty() {
        msg["reasoning_content"] = json!(reasoning_content);
    }
    msg
}

pub(crate) fn truncate_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect::<String>()
}

/// Collapse every whitespace run to a single space — summary lines, ledger
/// rows and boundary texts must all stay single-line.
pub(crate) fn flatten_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}


// ─── Thinking-mode `reasoning_content` pass-through ──────────────────────────

pub(crate) fn message_has_reasoning(msg: &Value) -> bool {
    msg.get("reasoning_content")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// Is this a DeepSeek-family model id?
///
/// DS-Format applies to them automatically (see `AppConfig::ds_format_for`),
/// because their thinking mode rejects replayed assistant messages that omit
/// `reasoning_content`. Matching is deliberately loose: providers prefix the
/// vendor (`deepseek/deepseek-chat`, `deepseek-ai/DeepSeek-V3.1`) and append
/// date stamps, so a case-insensitive substring check is the stable signal.
pub(crate) fn is_deepseek_model(model: &str) -> bool {
    model.trim().to_ascii_lowercase().contains("deepseek")
}

/// DeepSeek-format ("DS-Format") request normalization. Thinking-mode
/// DeepSeek-family APIs reject assistant messages that come back without
/// their `reasoning_content`:
/// `400 invalid_request_error: The "reasoning_content" in the thinking mode
/// must be passed back to the API` — typically on tool-call continuation
/// rounds where the model produced no new reasoning, or when history is
/// replayed without the trace. When enabled, every assistant message is
/// guaranteed to carry the field (empty string when the original trace is
/// unavailable); existing traces are always preserved unmodified. When
/// disabled, request messages are sent untouched.
///
/// The flag is the *effective* DS-Format state for the request's model:
/// `AppConfig::ds_format_for` turns it on automatically for DeepSeek-family
/// models, so this guard is not tied to the manual Settings switch alone.
pub(crate) fn ensure_reasoning_passthrough(req_body: &mut Value, ds_format: bool) {
    if !ds_format {
        return;
    }
    let Some(messages) = req_body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };
    for msg in messages {
        if msg.get("role").and_then(|v| v.as_str()) == Some("assistant")
            && !message_has_reasoning(msg)
        {
            msg["reasoning_content"] = json!("");
        }
    }
}

/// Does a `data:` URL carry an image? Compared on bytes so a non-ASCII payload
/// can never panic on a slice boundary.
fn is_image_data_url(url: &str) -> bool {
    let bytes = url.as_bytes();
    bytes.len() > 11 && bytes[..11].eq_ignore_ascii_case(b"data:image/")
}

/// Rewrite content parts the provider cannot take.
///
/// A `file` part is never forwarded. It is a dead end for this app: text is
/// inlined by the frontend, PDFs arrive as Markdown, images travel as
/// `image_url` — and a provider whose catalogue entry *advertised* file input
/// still rejected the part:
/// `400 invalid_request_error … .messages[2]: file content parts are not
/// supported here; send the image as an image_url part`. Worse, the part is
/// persisted with its message, so a single one failed that session on every
/// later turn — including after the frontend stopped producing them.
///
/// Three rewrites, in order:
/// 1. `data:image/…` → an `image_url` part: the payload is an image that was
///    classified as a binary further upstream.
/// 2. a base64 payload that decodes to UTF-8 → the text itself, inlined in the
///    same `<details>` shape the frontend uses. Rows saved by older builds come
///    back with their content intact: `@scripts/user_admin` was stored as
///    `application/octet-stream` and is a plain bash script.
/// 3. anything else → a text part naming the file, so the turn survives.
///
/// Runs at the single streaming choke point, so main-agent and sub-agent
/// requests are covered alike.
pub(crate) fn normalize_content_parts(req_body: &mut Value) {
    let Some(messages) = req_body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return;
    };
    for msg in messages.iter_mut() {
        let Some(parts) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            if part.get("type").and_then(|t| t.as_str()) != Some("file") {
                continue;
            }
            let file_data = part
                .get("file")
                .and_then(|f| f.get("file_data"))
                .and_then(|d| d.as_str())
                .unwrap_or("");
            if is_image_data_url(file_data) {
                *part = json!({ "type": "image_url", "image_url": { "url": file_data } });
                continue;
            }
            let name = part
                .get("file")
                .and_then(|f| f.get("filename"))
                .and_then(|n| n.as_str())
                .unwrap_or("attachment");
            *part = match decode_text_payload(file_data) {
                Some(text) => json!({
                    "type": "text",
                    "text": format!("\n<details><summary>Attached File: {name} — already provided below</summary>\n\n{text}\n\n</details>"),
                }),
                None => json!({
                    "type": "text",
                    "text": format!("\n[Attached file: {name} — not sent: this provider accepts text and images only]"),
                }),
            };
        }
    }
}

/// Largest `file` payload that is decoded back into the prompt, in bytes.
///
/// A legacy `file` part carrying a script or a config file is small; past this
/// size the file is better named than half-inlined.
const MAX_INLINE_FILE_BYTES: usize = 512 * 1024;

/// Decode a `data:…;base64,…` payload that is really text.
///
/// `None` when the payload is not base64, is too large, or does not decode as
/// UTF-8 — the caller then names the file instead of inlining it.
fn decode_text_payload(file_data: &str) -> Option<String> {
    let rest = file_data.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.contains(";base64") || payload.is_empty() {
        return None;
    }
    // Checked before decoding: base64 inflates by 4/3.
    if payload.len() / 4 * 3 > MAX_INLINE_FILE_BYTES {
        return None;
    }
    let bytes = BASE64.decode(payload.trim()).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_INLINE_FILE_BYTES {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.contains('\u{0}') {
        return None;
    }
    Some(text)
}

/// Which message carries which non-text content parts, e.g.
/// `parts=m1=image_url,m2=file`.
///
/// Logged beside a failed request. The provider names a message index and a
/// part type (`.messages[2]: file content parts are not supported here`), and
/// without this line there is no way to tell which attachment produced it —
/// exactly the dead end this one cost.
pub(crate) fn content_parts_signature(req_body: &Value) -> String {
    let Some(messages) = req_body.get("messages").and_then(|m| m.as_array()) else {
        return "parts=n/a".to_string();
    };
    let mut out: Vec<String> = Vec::new();
    for (idx, msg) in messages.iter().enumerate() {
        let Some(parts) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        let kinds: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.get("type").and_then(|t| t.as_str()))
            .filter(|k| *k != "text")
            .collect();
        if !kinds.is_empty() {
            out.push(format!("m{idx}={}", kinds.join("+")));
        }
    }
    if out.is_empty() {
        "parts=text-only".to_string()
    } else {
        format!("parts={}", out.join(","))
    }
}

// ─── Unified streaming helper ─────────────────────────────────────────────────

/// Announced before a streaming attempt is retried: the payload is the exact
/// text the failed attempt had streamed, so the frontend can truncate its
/// accumulator by that tail and the retry re-streams the response cleanly
/// (without it, regenerated tokens would be appended after the partial text).
pub(crate) fn announce_stream_retry(
    app: &AppHandle,
    opts: &StreamOptions<'_>,
    content: &str,
    reasoning: &str,
) {
    // Main chat only: sub-agent tokens land in task rows that have no clear
    // event, and UI-less callers (summaries, planner) have nothing to clear.
    if opts.task_id.is_none() && opts.emit_tokens {
        // The live progress snapshot rewinds with the transcript: the failed
        // attempt's deltas are dropped so a re-attaching webview rebuilds the
        // same content the retry re-streams. The snapshot accumulates across
        // rounds, so only the failed attempt's TAIL is stripped (the same
        // suffix match the frontend's onRetry does), never a truncation.
        if let Some(progress) = opts.progress {
            if let Ok(mut progress) = progress.lock() {
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
        let _ = app.emit(
            crate::events::CHAT_RETRY,
            crate::events::ChatRetry {
                turn_id: opts.turn_id.map(str::to_string),
                content: content.to_string(),
                reasoning: reasoning.to_string(),
            },
        );
    }
}

/// Stream one completion request according to `opts`.
/// Returns StreamResult with finish_reason, accumulated tool_calls, content, and token counts.
pub async fn stream_llm_request(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    mut req_body: Value,
    cancelled: &AtomicBool,
    opts: StreamOptions<'_>,
) -> Result<StreamResult, String> {
    // Single choke point for every streaming request (main agent + sub-agents):
    // DS-Format guard, see `ensure_reasoning_passthrough`.
    ensure_reasoning_passthrough(&mut req_body, opts.ds_format);
    // Same choke point, attachment side: an image that arrived inside a `file`
    // part becomes `image_url`, and a non-image `file` part the model did not
    // advertise is degraded to text — see `normalize_content_parts`.
    normalize_content_parts(&mut req_body);
    let mut last_err: Option<String> = None;

    for attempt in 1..=STREAM_MAX_RETRIES {
        // Started before the request goes out, so the first delta's latency
        // includes the upstream round trip (that wait is what "thinking time"
        // feels like to the user).
        let mut meter = StreamMeter::new();
        let mut request = client
            .post(url)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if !has_authorization_header(opts.headers) {
            request = request.bearer_auth(api_key);
        }
        request = apply_request_headers(request, opts.headers);
        let res = match request.json(&req_body).send().await {
            Ok(res) => res,
            Err(err) => {
                let err = err.to_string();
                let retryable = is_retryable_network_error(&err) && attempt < STREAM_MAX_RETRIES;
                last_err = Some(err.clone());
                if retryable {
                    tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
                    continue;
                }
                return Err(err);
            }
        };

        if !res.status().is_success() {
            let status = res.status();
            let retry_after = parse_retry_after_header(res.headers());
            let err_body = res.text().await.unwrap_or_default();
            let err = extract_upstream_error_message(&err_body);
            let retryable = (is_retryable_http_error_body(&err_body) || is_retryable_status(status))
                && attempt < STREAM_MAX_RETRIES;
            last_err = Some(err.clone());
            if retryable {
                tokio::time::sleep(
                    retry_after.unwrap_or_else(|| Duration::from_millis(retry_backoff_ms(attempt))),
                )
                .await;
                continue;
            }
            if opts.emit_errors {
                let _ = app.emit(
                    crate::events::CHAT_ERROR,
                    crate::events::ChatError {
                        turn_id: opts.turn_id.map(str::to_string),
                        error: err.clone(),
                    },
                );
            }
            return Err(err);
        }

        let mut finish_reason = String::new();
        let mut tool_calls: Vec<(String, String, String)> = Vec::new();
        let mut content = String::new();
        let mut reasoning_content = String::new();
        let mut prompt_tokens: u32 = 0;
        let mut completion_tokens: u32 = 0;
        let mut cached_tokens: Option<u32> = None;
        let mut got_tool_calls = false;
        let mut saw_done = false;
        let mut stream_error: Option<String> = None;

        let mut stream = res.bytes_stream();
        let mut buffer = String::new();

        'stream_loop: while let Some(chunk) = stream.next().await {
            if cancelled.load(Ordering::SeqCst) {
                return Ok(StreamResult {
                    finish_reason: "cancelled".into(),
                    tool_calls: vec![],
                    content,
                    reasoning_content,
                    prompt_tokens,
                    completion_tokens,
                    cached_tokens,
                    timing: meter.timing,
                });
            }
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(err) => {
                    let err = err.to_string();
                    // A connection that drops mid-stream is retried even when
                    // tokens already arrived: the `chat-retry` event carries
                    // exactly what this attempt streamed, so a UI appending
                    // tokens live can drop that tail before the retry
                    // re-streams the response. Sub-agent task rows have no
                    // clear event yet, so they keep the fail-fast path when
                    // something was already streamed.
                    let can_retry = is_retryable_network_error(&err)
                        && attempt < STREAM_MAX_RETRIES
                        && (opts.task_id.is_none()
                            || (content.is_empty() && tool_calls.is_empty()));
                    if can_retry {
                        stream_error = Some(err);
                        break 'stream_loop;
                    }
                    return Err(err);
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(idx) = buffer.find('\n') {
                let line = buffer[..idx].to_string();
                buffer.drain(..=idx);

                if cancelled.load(Ordering::SeqCst) {
                    return Ok(StreamResult {
                        finish_reason: "cancelled".into(),
                        tool_calls: vec![],
                        content,
                        reasoning_content,
                        prompt_tokens,
                        completion_tokens,
                        cached_tokens,
                        timing: meter.timing,
                    });
                }
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                // SSE payloads tolerate a missing space after the colon; some
                // relays send `data:[DONE]` / `data:{...}` without one, and the
                // old exact `data: ` match silently dropped those lines.
                let Some(payload) = line.strip_prefix("data:").map(str::trim_start) else {
                    continue;
                };
                if payload == "[DONE]" {
                    saw_done = true;
                    break 'stream_loop;
                }
                if payload.is_empty() {
                    continue;
                }
                let Ok(parsed) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                process_stream_chunk(
                    app,
                    &parsed,
                    &opts,
                    &mut finish_reason,
                    &mut tool_calls,
                    &mut content,
                    &mut reasoning_content,
                    &mut prompt_tokens,
                    &mut completion_tokens,
                    &mut cached_tokens,
                    &mut got_tool_calls,
                    &mut meter,
                );
            }
        }

        let trailing_line = buffer.trim();
        if !trailing_line.is_empty() {
            if let Some(payload) = trailing_line.strip_prefix("data:").map(str::trim_start) {
                if payload == "[DONE]" {
                    saw_done = true;
                } else if !payload.is_empty() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(payload) {
                        process_stream_chunk(
                            app,
                            &parsed,
                            &opts,
                            &mut finish_reason,
                            &mut tool_calls,
                            &mut content,
                            &mut reasoning_content,
                            &mut prompt_tokens,
                            &mut completion_tokens,
                            &mut cached_tokens,
                            &mut got_tool_calls,
                            &mut meter,
                        );
                    }
                }
            }
        }

        if let Some(err) = stream_error {
            // Announce AFTER the trailing-line processing above, so the
            // payload covers everything this attempt streamed (a chunk
            // boundary can split the error from a complete final line).
            announce_stream_retry(app, &opts, &content, &reasoning_content);
            last_err = Some(err);
            tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
            continue;
        }

        // ── Stream completeness evidence ────────────────────────────────────
        // A stream that ends without an explicit `finish_reason` or the
        // `[DONE]` sentinel was cut off (a proxy closed the connection early;
        // a clean EOF is indistinguishable from a complete body). Ending the
        // turn there silently truncated the reply, so treat it as an error:
        // retryable when nothing usable arrived, a visible failure otherwise.
        if finish_reason.is_empty() && !saw_done {
            let err = format!(
                "stream closed without finish_reason/[DONE] (received {} chars, {} tool call(s))",
                content.chars().count(),
                tool_calls.len()
            );
            let can_retry = attempt < STREAM_MAX_RETRIES
                && (opts.task_id.is_none()
                    || (content.is_empty() && tool_calls.is_empty()));
            last_err = Some(err.clone());
            if can_retry {
                announce_stream_retry(app, &opts, &content, &reasoning_content);
                tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
                continue;
            }
            if opts.emit_errors {
                let _ = app.emit(
                    crate::events::CHAT_ERROR,
                    crate::events::ChatError {
                        turn_id: opts.turn_id.map(str::to_string),
                        error: err.clone(),
                    },
                );
            }
            return Err(err);
        }

        // ── Empty completion ────────────────────────────────────────────────
        // `[DONE]` + usage with no content and no tool call: a relay closed an
        // aborted generation mid-answer (observed after a large prompt, right
        // after a mission report is injected). Nothing usable arrived, so this
        // is not an answer — retry while attempts remain, then fail loudly
        // instead of letting the turn end on an empty reply.
        if is_empty_completion(&content, &tool_calls) {
            let err = format!(
                "empty completion: no content and no tool call (finish_reason: {}, {} reasoning char(s))",
                if finish_reason.is_empty() {
                    "(none)"
                } else {
                    finish_reason.as_str()
                },
                reasoning_content.chars().count()
            );
            last_err = Some(err.clone());
            if attempt < STREAM_MAX_RETRIES {
                announce_stream_retry(app, &opts, &content, &reasoning_content);
                tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
                continue;
            }
            if opts.emit_errors {
                let _ = app.emit(
                    crate::events::CHAT_ERROR,
                    crate::events::ChatError {
                        turn_id: opts.turn_id.map(str::to_string),
                        error: err.clone(),
                    },
                );
            }
            return Err(err);
        }

        // Sub-agent mode: clear tool_calls if the LLM never signalled tool use
        if !got_tool_calls && opts.task_id.is_some() {
            tool_calls.clear();
        }

        // Providers that stream tool calls without ids must still get a valid
        // round back on the next request; see `ensure_tool_call_ids`.
        ensure_tool_call_ids(&mut tool_calls);

        return Ok(StreamResult {
            finish_reason,
            tool_calls,
            content,
            reasoning_content,
            prompt_tokens,
            completion_tokens,
            cached_tokens,
            timing: meter.timing,
        });
    }

    Err(last_err.unwrap_or_else(|| "stream request failed after retries".to_string()))
}

/// Thin wrapper used by the main agent (preserves existing call sites).
pub(crate) async fn stream_request(
    app: &AppHandle,
    client: &Client,
    url: &str,
    api_key: &str,
    req_body: Value,
    cancelled: &AtomicBool,
    usage_max_tokens: Option<u32>,
    ds_format: bool,
    headers: &[(String, String)],
    session_id: Option<&str>,
    turn_id: Option<&str>,
    progress: Option<&Arc<std::sync::Mutex<crate::turn::TurnProgress>>>,
) -> Result<StreamResult, String> {
    stream_llm_request(
        app,
        client,
        url,
        api_key,
        req_body,
        cancelled,
        StreamOptions {
            token_event: crate::events::CHAT_TOKEN,
            task_id: None,
            session_id,
            turn_id,
            reasoning_event: Some(crate::events::CHAT_REASONING_TOKEN),
            emit_usage: true,
            emit_tokens: true,
            emit_errors: true,
            usage_max_tokens,
            ds_format,
            headers,
            progress,
        },
    )
    .await
}

/// Log interaction to database
pub(crate) fn log_interaction(
    db: &rusqlite::Connection,
    session_id: &str,
    interaction_type: &str,
    actor: &str,
    action_name: &str,
    input_data: String,
    output_data: String,
    error_message: Option<String>,
    duration_ms: i64,
    metadata: Option<String>,
) {
    let _ = db::save_interaction_log(
        db,
        session_id,
        interaction_type,
        actor,
        action_name,
        &input_data,
        &output_data,
        error_message.as_deref(),
        duration_ms,
        metadata.as_deref(),
    );
}
