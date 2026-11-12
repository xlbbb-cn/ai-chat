//! Context-window accounting: token estimates, mechanical trimming and the
//! LLM-backed compressor.

use super::*;

pub(crate) fn summarize_message_for_context(msg: &Value) -> Option<String> {
    let role = msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    if role == "tool" {
        return None;
    }

    if msg
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|calls| !calls.is_empty())
        .unwrap_or(false)
    {
        return None;
    }

    let content = flatten_text(&msg.get("content").map(content_to_text).unwrap_or_default());

    if content.is_empty() {
        return None;
    }

    Some(format!("- {role}: {}", truncate_text(&content, 260)))
}

pub(crate) fn is_pinned_context_message(msg: &Value) -> bool {
    if msg.get("role").and_then(|v| v.as_str()) == Some("system") {
        return true;
    }
    msg.get("content")
        .map(content_to_text)
        .map(|content| {
            content.starts_with("INTERNAL CONTEXT - ACTIVE SKILLS")
                || content.starts_with("INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL")
        })
        .unwrap_or(false)
}

/// Body of a session-summary message: the marker stays on the first line
/// (stable parse target), followed by the continuation framing, then the
/// summary itself.
pub(crate) fn render_summary_message_body(body: &str) -> String {
    format!("{CONTEXT_SUMMARY_MARKER}\n{CONTEXT_SUMMARY_CONTINUATION_PREFIX}\n\n{body}")
}

pub(crate) fn extract_summary_body(content: &str) -> Option<String> {
    let rest = content.strip_prefix(CONTEXT_SUMMARY_MARKER)?;
    // Skip the continuation framing when present; legacy rows go straight
    // from the marker line to the body.
    let rest = rest.trim_start_matches('\n');
    let rest = rest
        .strip_prefix(CONTEXT_SUMMARY_CONTINUATION_PREFIX)
        .unwrap_or(rest);
    let body = rest.trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Is this user-role message one the USER actually wrote, as opposed to a
/// backend-generated notice? Pinned blocks (skills context, summaries, todo
/// lists, mission reports) are classified before this check; what remains to
/// exclude are the mid-turn injection, background results, the turn-budget
/// marker, the post-compression continuation nudge, the output-truncation
/// continuation and the goal-loop nudge — each with a stable text prefix.
pub(crate) fn is_frontend_user_message(msg: &Value) -> bool {
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    let text = content_to_text(msg.get("content").unwrap_or(&Value::Null));
    !text.starts_with(MID_TURN_INJECTION_PREFIX)
        && !text.starts_with(BACKGROUND_RESULT_PREFIX)
        && !text.starts_with(CONTEXT_BUDGET_MARKER)
        && !text.starts_with(CONTEXT_CONTINUE_MARKER)
        && !text.starts_with(CONTEXT_LENGTH_MARKER)
        && !text.starts_with(CONTEXT_GOAL_MARKER)
}

/// Count the frontend (UI) messages inside a message slice — the coverage a
/// persisted summary stands in for — and record the flattened prefix of the
/// LAST one. That boundary text is what a later turn re-verifies before
/// dropping the covered prefix of its history.
pub(crate) fn count_frontend_messages(messages: &[Value]) -> (usize, String) {
    let mut covered = 0usize;
    let mut boundary = String::new();
    for msg in messages {
        let is_frontend = match msg.get("role").and_then(|v| v.as_str()) {
            Some("user") => is_frontend_user_message(msg),
            // Assistant messages without tool calls: the UI history carries
            // one final assistant message per turn. Backend-only interim
            // messages (background holds) can slip in here — the boundary
            // verification turns any such miscount into a SAFE fallback
            // (summary discarded, full history replayed), never into lost
            // context.
            Some("assistant") => msg
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|calls| calls.is_empty())
                .unwrap_or(true),
            _ => false,
        };
        if is_frontend {
            covered += 1;
            boundary = truncate_text(
                &flatten_text(&content_to_text(msg.get("content").unwrap_or(&Value::Null))),
                CONTEXT_LEDGER_MSG_CHARS,
            );
        }
    }
    (covered, boundary)
}

/// May a persisted summary replace the first `covered` messages of the
/// incoming history? Pure decision over the recorded coverage: `covered` and
/// `boundary` come from the saved row, `boundary_message_text` is the raw
/// text of `history[covered - 1]` (None when covered runs past the history —
/// the summary is stale). Returns the trim count on a verified match.
pub(crate) fn summary_trim_count(
    covered: usize,
    boundary: &str,
    boundary_message_text: Option<&str>,
) -> Option<usize> {
    if covered == 0 || boundary.trim().is_empty() {
        return None;
    }
    let text = flatten_text(boundary_message_text?);
    if text.starts_with(boundary.trim()) {
        Some(covered)
    } else {
        None
    }
}

/// A recent window may only start on a message that is not itself part of a
/// tool round — otherwise the kept tail would begin with an orphaned `tool`
/// message or an assistant whose tool results were summarized away, and
/// `sanitize_tool_pairs` would silently drop them on the next request.
pub(crate) fn is_clean_split_boundary(msg: &Value) -> bool {
    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
    if role == "tool" {
        return false;
    }
    let has_calls = msg
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|calls| !calls.is_empty())
        .unwrap_or(false);
    !has_calls
}

pub(crate) fn merge_summary_lines(existing_summary: Option<&str>, new_lines: &[String]) -> String {
    let mut merged: Vec<String> = Vec::new();

    if let Some(existing) = existing_summary {
        merged.extend(
            existing
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string()),
        );
    }

    merged.extend(new_lines.iter().cloned());

    if merged.len() > CONTEXT_SUMMARY_MAX_LINES {
        merged = merged.split_off(merged.len() - CONTEXT_SUMMARY_MAX_LINES);
    }

    merged.join("\n")
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

/// Classification of one compression pass over the message list.
pub(crate) struct ContextSplit {
    /// System prompt + pinned INTERNAL CONTEXT blocks — always kept verbatim.
    pub(crate) pinned: Vec<Value>,
    /// Summarized region: user/assistant history INCLUDING tool rounds, so
    /// the summary can preserve what was actually done.
    pub(crate) older: Vec<Value>,
    /// Recent tail kept verbatim (starts on a clean round boundary).
    pub(crate) recent: Vec<Value>,
    pub(crate) existing_summary_body: Option<String>,
    pub(crate) todo_block: Option<Value>,
    /// Frontend (UI) messages inside `older` — the coverage a persisted
    /// summary stands in for on later turns.
    pub(crate) covered_frontend: usize,
    /// Flattened prefix of the last covered frontend message.
    pub(crate) boundary_text: String,
}

/// Classify the message list for compression. Unlike the pre-LLM summarizer,
/// tool rounds stay in `older` instead of being dropped without a trace.
pub(crate) fn split_context_for_compression(all_messages: Vec<Value>, keep_recent: usize) -> ContextSplit {
    let mut pinned: Vec<Value> = Vec::new();
    let mut compressible: Vec<Value> = Vec::new();
    let mut existing_summary_body: Option<String> = None;
    let mut todo_block: Option<Value> = None;

    for msg in all_messages {
        if let Some(summary_body) = msg
            .get("content")
            .map(content_to_text)
            .and_then(|s| extract_summary_body(&s))
        {
            existing_summary_body = Some(summary_body);
            continue;
        }

        // The active todo list is re-rendered from disk every turn. Keep it
        // verbatim at the tail (cache-friendly) instead of pinning it to the
        // head or folding it into the summary.
        let is_todo_block = msg
            .get("content")
            .map(content_to_text)
            .map(|content| content.starts_with(CONTEXT_TODO_MARKER))
            .unwrap_or(false);
        if is_todo_block {
            todo_block = Some(msg);
            continue;
        }

        if is_pinned_context_message(&msg) {
            pinned.push(msg);
        } else {
            compressible.push(msg);
        }
    }

    if compressible.len() <= keep_recent + 2 {
        return ContextSplit {
            pinned,
            older: Vec::new(),
            recent: compressible,
            existing_summary_body,
            todo_block,
            covered_frontend: 0,
            boundary_text: String::new(),
        };
    }

    let mut split_idx = compressible.len() - keep_recent;
    while split_idx < compressible.len() && !is_clean_split_boundary(&compressible[split_idx]) {
        split_idx += 1;
    }
    let recent = compressible.split_off(split_idx);
    let older = compressible;
    let (covered_frontend, boundary_text) = count_frontend_messages(&older);

    ContextSplit {
        pinned,
        older,
        recent,
        existing_summary_body,
        todo_block,
        covered_frontend,
        boundary_text,
    }
}

/// Reassemble the message list after a (non-)compression pass. The summary is
/// a `user` message with continuation framing — mid-conversation `system`
/// messages are rejected by strict OpenAI-compatible gateways (the same
/// reason `MID_TURN_INJECTION_PREFIX` is a user message).
pub(crate) fn rebuild_context_messages(
    pinned: Vec<Value>,
    existing_summary_body: Option<String>,
    todo_block: Option<Value>,
    rest: Vec<Value>,
) -> Vec<Value> {
    let mut rebuilt = Vec::with_capacity(pinned.len() + rest.len() + 2);
    rebuilt.extend(pinned);
    if let Some(body) = existing_summary_body {
        rebuilt.push(json!({
            "role": "user",
            "content": render_summary_message_body(&body)
        }));
    }
    if let Some(todo) = todo_block {
        rebuilt.push(todo);
    }
    rebuilt.extend(rest);
    rebuilt
}

/// Render the summarized region as a compact transcript for the LLM
/// summarizer: user/assistant text kept at length, tool rounds folded into a
/// one-line ledger (`name(args) → result preview`) so what was DONE survives
/// as summarizer input. Chronological; `cap_transcript` bounds the size.
pub(crate) fn build_summary_transcript(messages: &[Value]) -> String {
    let mut out = String::new();
    // tool_call_id -> (name, arguments) from the assistant round that
    // declared them, so each `tool` message renders with its tool's identity.
    let mut pending_calls: std::collections::HashMap<&str, (&str, &str)> =
        std::collections::HashMap::new();
    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "assistant" => {
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let name = tc
                            .pointer("/function/name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = tc
                            .pointer("/function/arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !id.is_empty() {
                            pending_calls.insert(id, (name, args));
                        }
                    }
                }
                let content =
                    flatten_text(&content_to_text(msg.get("content").unwrap_or(&Value::Null)));
                if !content.is_empty() {
                    out.push_str(&format!(
                        "[assistant] {}\n\n",
                        truncate_text(&content, CONTEXT_LEDGER_MSG_CHARS)
                    ));
                }
            }
            "user" => {
                let content =
                    flatten_text(&content_to_text(msg.get("content").unwrap_or(&Value::Null)));
                if !content.is_empty() {
                    out.push_str(&format!(
                        "[user] {}\n\n",
                        truncate_text(&content, CONTEXT_LEDGER_MSG_CHARS)
                    ));
                }
            }
            "tool" => {
                let id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                let (name, args) = match pending_calls.remove(id) {
                    Some(pair) => pair,
                    None => ("", ""),
                };
                let result =
                    flatten_text(&content_to_text(msg.get("content").unwrap_or(&Value::Null)));
                let total_chars = result.chars().count();
                let suffix = if total_chars > CONTEXT_LEDGER_RESULT_CHARS {
                    format!(" …({total_chars} chars total)")
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "[tool] {}({}) → {}{}\n\n",
                    name,
                    truncate_text(args, CONTEXT_LEDGER_ARG_CHARS),
                    truncate_text(&result, CONTEXT_LEDGER_RESULT_CHARS),
                    suffix
                ));
            }
            _ => {}
        }
    }
    out
}

/// Keep the transcript within the summarizer's input budget by dropping the
/// OLDEST content (the tail matters most), cut at a line boundary.
pub(crate) fn cap_transcript(transcript: &str) -> String {
    let total = transcript.chars().count();
    if total <= CONTEXT_SUMMARY_INPUT_CHARS {
        return transcript.to_string();
    }
    let suffix: String = transcript
        .chars()
        .skip(total - CONTEXT_SUMMARY_INPUT_CHARS)
        .collect();
    let trimmed = match suffix.split_once('\n') {
        Some((_, rest)) => rest,
        None => suffix.as_str(),
    };
    format!("(earlier messages omitted)\n\n{trimmed}")
}

/// Mechanical fallback summarizer: one line per message. Tool rounds render
/// as a compact ledger (`- tool name(args) → result preview`) so even without
/// the LLM path the summary preserves which tools ran and what came back.
pub(crate) fn summarize_older_mechanically(older: &[Value]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut pending_calls: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for msg in older {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        match role.as_str() {
            "assistant" => {
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let name = tc
                            .pointer("/function/name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = tc
                            .pointer("/function/arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !id.is_empty() {
                            pending_calls.insert(id.to_string(), (name, args));
                        }
                    }
                }
                if let Some(line) = summarize_message_for_context(msg) {
                    lines.push(line);
                }
            }
            "tool" => {
                let id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                let (name, args) = pending_calls
                    .remove(id)
                    .unwrap_or_else(|| (String::new(), String::new()));
                let result =
                    flatten_text(&content_to_text(msg.get("content").unwrap_or(&Value::Null)));
                let total_chars = result.chars().count();
                let suffix = if total_chars > CONTEXT_LEDGER_RESULT_CHARS {
                    format!(" …({total_chars} chars total)")
                } else {
                    String::new()
                };
                lines.push(format!(
                    "- tool {}({}) → {}{}",
                    name,
                    truncate_text(&args, CONTEXT_LEDGER_ARG_CHARS),
                    truncate_text(&result, CONTEXT_LEDGER_RESULT_CHARS),
                    suffix
                ));
            }
            _ => {
                if let Some(line) = summarize_message_for_context(msg) {
                    lines.push(line);
                }
            }
        }
    }
    lines
}

/// Structured-summary system prompt, adapted from Claude Code's compact
/// prompt (`services/compact/prompt.ts`): the numbered sections keep user
/// intent, exact file paths and the error→fix history alive across
/// compression. Section 6 (all user messages) is the critical one — user
/// feedback must never dissolve into a paraphrase.
pub(crate) const CONTEXT_SUMMARY_SYSTEM_PROMPT: &str = "You compress a conversation transcript into a durable working summary for an AI coding assistant. Respond with TEXT ONLY. Do NOT call any tools. Produce plain text with exactly these numbered sections:\n\n\
1. Primary Request and Intent: the user's explicit requests and the goal driving them\n\
2. Key Technical Concepts: technologies, constraints, decisions and their reasons\n\
3. Files and Code Sections: every file touched or read, with exact paths and what was done or found\n\
4. Errors and Fixes: each error hit and how it was resolved, so it is not repeated\n\
5. Problem Solving: approaches tried, what worked, what did not\n\
6. All User Messages: list every user message, condensed but never dropped\n\
7. Pending Tasks: what is not finished yet\n\
8. Current Work: precisely what was being done when this summary was requested\n\
9. Next Step: the immediate next action, quoting the instruction that drives it\n\n\
Keep file paths, commands, identifiers and code references EXACT. Be dense and factual; no preamble, no closing remarks.";

/// Everything the context compressor needs to run its (optional) LLM
/// summarizer. The summary call reuses the main streaming helper with every
/// UI event off: it is an internal step, never a chat turn of its own.
pub(crate) struct ContextCompressor<'a> {
    pub(crate) app: &'a AppHandle,
    pub(crate) client: &'a Client,
    pub(crate) url: &'a str,
    pub(crate) api_key: &'a str,
    pub(crate) model: &'a str,
    pub(crate) ds_format: bool,
    pub(crate) headers: &'a [(String, String)],
    pub(crate) cancelled: &'a AtomicBool,
}

/// Bookkeeping threaded through a turn: how many frontend messages the
/// currently-injected summary already covers (`offset`) and the boundary
/// text recorded with it.
#[derive(Default, Clone)]
pub(crate) struct CoverageState {
    pub(crate) offset: usize,
    pub(crate) boundary: String,
}

/// Result of one successful compression pass.
pub(crate) struct CompressionOutcome {
    pub(crate) summary: String,
    /// Total frontend messages covered (previous offset + this pass).
    pub(crate) covered_messages: usize,
    /// Boundary text to persist alongside the summary.
    pub(crate) boundary_text: String,
}

impl ContextCompressor<'_> {
    /// Structured conversation summary via the main model. Returns `None` on
    /// cancellation, upstream failure or a degenerate answer — the caller
    /// then falls back to the mechanical line summarizer, so compression can
    /// never fail the turn.
    pub(crate) async fn summarize(&self, transcript: &str, existing_summary: Option<&str>) -> Option<String> {
        if self.cancelled.load(Ordering::SeqCst) {
            return None;
        }
        let existing_block = existing_summary
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                format!("Previous summary (merge it — do not lose its information):\n\n{s}\n\n")
            })
            .unwrap_or_default();
        let req_body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": CONTEXT_SUMMARY_SYSTEM_PROMPT },
                { "role": "user", "content": format!(
                    "{existing_block}Transcript to summarize:\n\n{transcript}"
                ) },
            ],
            "stream": true,
        });
        let result = stream_llm_request(
            self.app,
            self.client,
            self.url,
            self.api_key,
            req_body,
            self.cancelled,
            StreamOptions {
                token_event: "chat-token",
                task_id: None,
                reasoning_event: None,
                emit_usage: false,
                emit_tokens: false,
                emit_errors: false,
                usage_max_tokens: None,
                ds_format: self.ds_format,
                headers: self.headers,
            },
        )
        .await;

        let sr = match result {
            Ok(sr) => sr,
            Err(err) => {
                log_event(
                    &self.app.state::<AppState>(),
                    "WARN",
                    format!("LLM context summary failed, falling back to line summary: {err}"),
                );
                return None;
            }
        };
        if sr.finish_reason == "cancelled" {
            return None;
        }
        let text = sr.content.trim();
        if text.chars().count() < CONTEXT_SUMMARY_MIN_CHARS {
            log_event(
                &self.app.state::<AppState>(),
                "WARN",
                format!(
                    "LLM context summary degenerate ({} chars), falling back to line summary",
                    text.chars().count()
                ),
            );
            return None;
        }
        Some(truncate_text(text, CONTEXT_SUMMARY_MAX_CHARS))
    }

    /// One compression pass: split the context, summarize the older region
    /// (LLM first, mechanical fallback), rebuild the message list and report
    /// the updated coverage. `None` = nothing to compress, `*all_messages`
    /// left normalized but unchanged in content.
    pub(crate) async fn compress(
        &self,
        all_messages: &mut Vec<Value>,
        keep_recent: usize,
        coverage: &CoverageState,
    ) -> Option<CompressionOutcome> {
        if all_messages.len() <= keep_recent + 2 {
            return None;
        }

        let split = split_context_for_compression(std::mem::take(all_messages), keep_recent);

        if split.older.is_empty() {
            *all_messages = rebuild_context_messages(
                split.pinned,
                split.existing_summary_body,
                split.todo_block,
                split.recent,
            );
            return None;
        }

        let transcript = cap_transcript(&build_summary_transcript(&split.older));
        let llm_summary = self
            .summarize(&transcript, split.existing_summary_body.as_deref())
            .await;

        let merged_summary = match llm_summary {
            Some(summary) => summary,
            None => {
                let new_lines = summarize_older_mechanically(&split.older);
                if new_lines.is_empty() {
                    *all_messages = rebuild_context_messages(
                        split.pinned,
                        split.existing_summary_body,
                        split.todo_block,
                        split.recent,
                    );
                    return None;
                }
                merge_summary_lines(split.existing_summary_body.as_deref(), &new_lines)
            }
        };

        let covered_messages = coverage.offset + split.covered_frontend;
        let boundary_text = if split.covered_frontend > 0 {
            split.boundary_text
        } else {
            coverage.boundary.clone()
        };

        let summary_message = json!({
            "role": "user",
            "content": render_summary_message_body(&merged_summary)
        });
        let mut rebuilt =
            Vec::with_capacity(split.pinned.len() + 2 + split.recent.len());
        rebuilt.extend(split.pinned);
        rebuilt.push(summary_message);
        if let Some(todo) = split.todo_block {
            rebuilt.push(todo);
        }
        rebuilt.extend(split.recent);
        *all_messages = rebuilt;

        Some(CompressionOutcome {
            summary: merged_summary,
            covered_messages,
            boundary_text,
        })
    }
}

/// Rough prompt size estimate in tokens (≈4 chars/token), tools payload
/// included — the `tools` array is rendered before the messages in the
/// prompt, so its JSON counts toward every request.
pub(crate) fn estimate_prompt_tokens(messages: &[Value], tools: &[Value]) -> usize {
    let mut chars: usize = 0;
    for msg in messages {
        chars += content_to_text(msg.get("content").unwrap_or(&Value::Null)).chars().count();
        if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in calls {
                chars += tc.to_string().chars().count();
            }
        }
    }
    for tool in tools {
        chars += tool.to_string().chars().count();
    }
    chars / TOKEN_ESTIMATE_CHARS_PER_TOKEN
}

/// Does an upstream error mean the request exceeded the model's context
/// window? Providers phrase it a dozen ways; a false positive costs one
/// harmless compression+retry, a false negative fails the turn.
pub(crate) fn is_context_overflow_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    const PATTERNS: &[&str] = &[
        "context_length_exceeded",
        "maximum context length",
        "context length exceeded",
        "context window",
        "too many tokens",
        "prompt is too long",
        "request too large",
        "payload too large",
        "exceeds the context",
        "input length exceeds",
        "token limit",
    ];
    PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

/// Persist a compression outcome (summary + coverage bookkeeping) for the
/// session. Best-effort: a failed write only means the next turn replays the
/// full history (the old additive behavior).
pub(crate) fn persist_session_summary(
    state: &State<'_, AppState>,
    session_id: &str,
    outcome: &CompressionOutcome,
) {
    if let Ok(db_guard) = state.db.lock() {
        let _ = db::save_session_summary(
            &db_guard,
            session_id,
            &outcome.summary,
            outcome.covered_messages as i64,
            &outcome.boundary_text,
        );
    }
}
