//! Streaming transport: chunk processing, retries and the unified
//! request helper.

use super::*;

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
/// Main agent: reasoning + usage, token_event="chat-token", no task_id.
/// Sub-agent:  token_event="agent-task-token", reasoning_event="agent-task-reasoning",
///             task_id=Some(...), no usage.
pub struct StreamOptions<'a> {
    /// Event name for content delta tokens.
    pub token_event: &'a str,
    /// If set, include `{"task_id": ..., "token": ...}` payload instead of a plain string.
    pub task_id: Option<&'a str>,
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

pub(crate) fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(300))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Client for streaming chat requests.
///
/// Unlike [`build_http_client`] there is NO total request timeout: reqwest's
/// `timeout` covers the whole response body, which for an SSE stream is the
/// entire generation — a thinking model (or a slow relay) that streams longer
/// than the limit died mid-answer no matter how healthy the connection was.
/// Liveness is enforced per read instead: [`ClientBuilder::read_timeout`]
/// fails when no bytes arrive for the interval, so a stalled upstream errors
/// quickly (and retries) while a slow-but-alive stream may run arbitrarily
/// long. Every caller that runs `stream_llm_request` must use this builder.
pub(crate) fn build_stream_client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(12))
        .read_timeout(Duration::from_secs(120))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
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
            let mut event = json!({
                "prompt_tokens": *prompt_tokens,
                "completion_tokens": *completion_tokens,
                "total_tokens": total_tokens,
                "max_tokens": opts.usage_max_tokens,
                "usage_ratio": usage_ratio
            });
            // Only reported when the provider exposes a cache dialect at all:
            // without it a hard-coded `0%` would look like real data.
            if let Some(cached) = cached_report {
                let hit_ratio = if *prompt_tokens > 0 {
                    ((cached as f64) / (*prompt_tokens as f64)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                event["cached_tokens"] = json!(cached);
                event["hit_ratio"] = json!(hit_ratio);
            }
            let _ = app.emit("chat-usage", event);
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
        if let Some(event) = opts.reasoning_event {
            if let Some(task_id) = opts.task_id {
                let _ = app.emit(event, json!({ "task_id": task_id, "token": reasoning }));
            } else {
                let _ = app.emit(event, reasoning.to_string());
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
        if opts.emit_tokens {
            if let Some(task_id) = opts.task_id {
                let _ = app.emit(
                    opts.token_event,
                    json!({ "task_id": task_id, "token": token }),
                );
            } else {
                let _ = app.emit(opts.token_event, token.to_string());
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

// ─── Unified streaming helper ─────────────────────────────────────────────────

/// Announced before a streaming attempt is retried: the payload is the exact
/// text the failed attempt had streamed, so the frontend can truncate its
/// accumulator by that tail and the retry re-streams the response cleanly
/// (without it, regenerated tokens would be appended after the partial text).
pub(crate) const CHAT_RETRY_EVENT: &str = "chat-retry";

pub(crate) fn announce_stream_retry(
    app: &AppHandle,
    opts: &StreamOptions<'_>,
    content: &str,
    reasoning: &str,
) {
    // Main chat only: sub-agent tokens land in task rows that have no clear
    // event, and UI-less callers (summaries, planner) have nothing to clear.
    if opts.task_id.is_none() && opts.emit_tokens {
        let _ = app.emit(
            CHAT_RETRY_EVENT,
            json!({ "content": content, "reasoning": reasoning }),
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
                let _ = app.emit("chat-error", err.clone());
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
                let _ = app.emit("chat-error", err.clone());
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
) -> Result<StreamResult, String> {
    stream_llm_request(
        app,
        client,
        url,
        api_key,
        req_body,
        cancelled,
        StreamOptions {
            token_event: "chat-token",
            task_id: None,
            reasoning_event: Some("chat-reasoning-token"),
            emit_usage: true,
            emit_tokens: true,
            emit_errors: true,
            usage_max_tokens,
            ds_format,
            headers,
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
