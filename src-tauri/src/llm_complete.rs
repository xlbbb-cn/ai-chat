use futures_util::StreamExt;
use os_info;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Config with empty per-model/global effort values, for the resolver tests.
    fn config_with_efforts(per_model: &[(&str, &str)], global: &str) -> crate::AppConfig {
        let mut config = crate::AppConfig::default();
        config.model_reasoning_effort = per_model
            .iter()
            .map(|(model, effort)| (model.to_string(), effort.to_string()))
            .collect();
        config.model_settings.reasoning_effort = global.to_string();
        config
    }

    #[test]
    fn reasoning_effort_prefers_the_per_model_value() {
        let config = config_with_efforts(&[("gpt-5", "high")], "low");
        assert_eq!(resolve_reasoning_effort(&config, "gpt-5"), Some("high"));
        // Models without an override fall back to the global setting.
        assert_eq!(resolve_reasoning_effort(&config, "gpt-4o"), Some("low"));
    }

    #[test]
    fn reasoning_effort_is_omitted_when_unset() {
        let config = config_with_efforts(&[], "");
        assert_eq!(resolve_reasoning_effort(&config, "gpt-4o"), None);

        // Blank per-model entries must not shadow the global fallback.
        let config = config_with_efforts(&[("gpt-5", "   ")], "medium");
        assert_eq!(resolve_reasoning_effort(&config, "gpt-5"), Some("medium"));
    }

    #[test]
    fn reasoning_effort_drops_unknown_values() {
        // A typo must never reach the provider — strict endpoints answer 400.
        let config = config_with_efforts(&[("gpt-5", "extreme")], "");
        assert_eq!(resolve_reasoning_effort(&config, "gpt-5"), None);

        // Known values are normalised (case-insensitive).
        let config = config_with_efforts(&[("gpt-5", "HIGH")], "");
        assert_eq!(resolve_reasoning_effort(&config, "gpt-5"), Some("high"));
    }

    #[test]
    fn context_window_prefers_the_per_model_value() {
        let mut config = crate::AppConfig::default();
        config
            .model_context_lengths
            .insert("gpt-5".to_string(), 400_000);

        assert_eq!(resolve_context_window(&config, "gpt-5", 131_072), 400_000);
        // Whitespace around the model id must not defeat the lookup.
        assert_eq!(resolve_context_window(&config, " gpt-5 ", 131_072), 400_000);
        // Models without an entry fall back to the caller's default.
        assert_eq!(resolve_context_window(&config, "gpt-4o", 131_072), 131_072);

        // A zero/absent entry is "unknown", never a zero-token budget.
        config.model_context_lengths.insert("gpt-4o".to_string(), 0);
        assert_eq!(resolve_context_window(&config, "gpt-4o", 131_072), 131_072);
    }

    /// All three cache dialects the usage parser supports, and the case where
    /// the endpoint reports no caching figure at all (`None`, not `Some(0)`:
    /// a reported zero-hit request must stay distinguishable).
    #[test]
    fn extract_cached_tokens_understands_every_provider_dialect() {
        let openai = json!({
            "prompt_tokens": 500,
            "prompt_tokens_details": { "cached_tokens": 384 }
        });
        assert_eq!(extract_cached_tokens(&openai), Some(384));

        let deepseek = json!({
            "prompt_tokens": 500,
            "prompt_cache_hit_tokens": 300,
            "prompt_cache_miss_tokens": 200
        });
        assert_eq!(extract_cached_tokens(&deepseek), Some(300));

        let anthropic_style = json!({
            "prompt_tokens": 500,
            "cache_read_input_tokens": 410
        });
        assert_eq!(extract_cached_tokens(&anthropic_style), Some(410));

        assert_eq!(
            extract_cached_tokens(&json!({ "prompt_tokens": 500 })),
            None
        );
        assert_eq!(extract_cached_tokens(&json!({ "prompt_tokens": 0 })), None);

        // OpenAI wins when several dialects are present, and a reported
        // zero-hit stays a real `Some(0)`.
        let mixed = json!({
            "prompt_tokens_details": { "cached_tokens": 0 },
            "prompt_cache_hit_tokens": 999
        });
        assert_eq!(extract_cached_tokens(&mixed), Some(0));
    }

    #[test]
    fn content_to_text_passthrough_for_string() {
        let v = json!("hello world");
        assert_eq!(content_to_text(&v), "hello world");
    }

    #[test]
    fn content_to_text_concatenates_text_parts() {
        let v = json!([
            { "type": "text", "text": "describe this: " },
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,XXX" } },
            { "type": "text", "text": "thanks" },
        ]);
        assert_eq!(content_to_text(&v), "describe this: thanks");
    }

    #[test]
    fn content_to_text_returns_empty_for_non_text_array() {
        let v = json!([
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,XXX" } },
            { "type": "file", "file": { "filename": "x.pdf", "file_data": "data:application/pdf;base64,XXX" } },
        ]);
        assert_eq!(content_to_text(&v), "");
    }

    #[test]
    fn content_to_text_handles_null_and_other_types() {
        assert_eq!(content_to_text(&json!(null)), "");
        assert_eq!(content_to_text(&json!(42)), "");
        assert_eq!(content_to_text(&json!({})), "");
    }

    #[test]
    fn conversation_id_header_value_keeps_uuid_v4_session_ids() {
        let session_id = "01234567-89ab-4cde-8f01-23456789abcd";
        assert_eq!(conversation_id_header_value(session_id), session_id);
        // Whitespace is trimmed, so a stray newline can never reach the header.
        assert_eq!(
            conversation_id_header_value("  01234567-89ab-4cde-8f01-23456789abcd \n"),
            session_id
        );
    }

    #[test]
    fn conversation_id_header_value_canonicalises_and_falls_back() {
        // Uppercase UUID → canonical lowercase form (same id).
        assert_eq!(
            conversation_id_header_value("01234567-89AB-4CDE-8F01-23456789ABCD"),
            "01234567-89ab-4cde-8f01-23456789abcd"
        );

        // A non-UUID session id is never forwarded verbatim: the header always
        // carries a UUID v4.
        let value = conversation_id_header_value("legacy-session");
        let parsed = uuid::Uuid::parse_str(&value).expect("fallback must be a UUID");
        assert_eq!(parsed.get_version_num(), 4);
    }

    #[test]
    fn conversation_id_header_name_uses_the_default_when_blank_or_missing() {
        let mut config = crate::AppConfig::default();
        assert_eq!(conversation_id_header_name(&config), CONVERSATION_ID_HEADER);

        // Clearing the field in Settings must keep the default, never disable
        // the correlation header by accident.
        config.conversation_id_header = Some("   ".to_string());
        assert_eq!(conversation_id_header_name(&config), CONVERSATION_ID_HEADER);

        // A custom name is trimmed before use.
        config.conversation_id_header = Some(" X-Session-Id  ".to_string());
        assert_eq!(conversation_id_header_name(&config), "X-Session-Id");
    }

    #[test]
    fn upstream_headers_put_the_conversation_id_before_custom_headers() {
        let mut config = crate::AppConfig::default();
        config.conversation_id_header = Some("x-chat-id".to_string());
        config.custom_headers = vec![
            crate::RequestHeader {
                name: " x-route ".to_string(),
                value: "fast".to_string(),
            },
            // A blank name is a half-typed row, never a real header.
            crate::RequestHeader {
                name: "   ".to_string(),
                value: "ignored".to_string(),
            },
            // A custom row can never shadow the conversation-id header.
            crate::RequestHeader {
                name: "X-Chat-Id".to_string(),
                value: "shadow".to_string(),
            },
        ];

        let session_id = "01234567-89ab-4cde-8f01-23456789abcd";
        let headers = upstream_request_headers(&config, session_id);
        assert_eq!(
            headers,
            vec![
                ("x-chat-id".to_string(), session_id.to_string()),
                ("x-route".to_string(), "fast".to_string()),
            ]
        );
    }

    #[test]
    fn custom_authorization_headers_are_detected_case_insensitively() {
        assert!(has_authorization_header(&[(
            "Authorization".to_string(),
            "Basic abc".to_string()
        )]));
        assert!(!has_authorization_header(&[(
            "x-route".to_string(),
            "fast".to_string()
        )]));
        assert!(!has_authorization_header(&[]));
    }

    #[test]
    fn collect_loaded_skills_unions_messages_in_first_seen_order() {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: json!("hi"),
                reasoning_content: None,
                loaded_skills: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: json!("loading"),
                reasoning_content: None,
                loaded_skills: Some(vec!["alpha".into(), "beta".into()]),
            },
            ChatMessage {
                role: "assistant".into(),
                content: json!("again"),
                reasoning_content: None,
                loaded_skills: Some(vec!["beta".into(), " gamma ".into(), String::new()]),
            },
        ];

        assert_eq!(
            collect_loaded_skills(&messages),
            vec!["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn collect_loaded_skills_ignores_legacy_marker_text() {
        // Text markers must no longer be treated as state: the old
        // "🧠 *Loading skill: …*" scanning is exactly what broke dedup.
        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: json!("🧠 *Loading skill: alpha*\n\nlegacy marker only"),
            reasoning_content: None,
            loaded_skills: None,
        }];

        assert!(collect_loaded_skills(&messages).is_empty());
    }

    #[test]
    fn ds_format_fills_missing_assistant_reasoning_and_preserves_originals() {
        // DS-Format on: assistant messages without a reasoning trace (tool-call
        // continuation rounds, legacy history) get an empty-string fallback;
        // original traces stay untouched; non-assistant messages are skipped.
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": null, "reasoning_content": "thinking...",
                  "tool_calls": [{ "id": "1", "type": "function", "function": { "name": "x", "arguments": "{}" } }] },
                { "role": "tool", "tool_call_id": "1", "content": "ok" },
                { "role": "assistant", "content": "round 2 without reasoning" },
            ]
        });
        ensure_reasoning_passthrough(&mut body, true);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["reasoning_content"], json!("thinking..."));
        assert_eq!(msgs[3]["reasoning_content"], json!(""));
        assert!(msgs[0].get("reasoning_content").is_none());
        assert!(msgs[2].get("reasoning_content").is_none());
    }

    #[test]
    fn ds_format_fills_null_reasoning_content() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "assistant", "content": "hello", "reasoning_content": null },
            ]
        });
        ensure_reasoning_passthrough(&mut body, true);
        assert_eq!(body["messages"][0]["reasoning_content"], json!(""));
    }

    #[test]
    fn ds_format_disabled_leaves_messages_untouched() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ]
        });
        ensure_reasoning_passthrough(&mut body, false);
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn deepseek_model_ids_are_detected_regardless_of_vendor_prefix_and_case() {
        for id in [
            "deepseek-chat",
            "deepseek-reasoner",
            "DeepSeek-V3.1",
            "deepseek-ai/DeepSeek-R1",
            "  deepseek/deepseek-chat  ",
        ] {
            assert!(is_deepseek_model(id), "{id} should be treated as DeepSeek");
        }
        for id in ["gpt-4o-mini", "qwen3-max", "", "  "] {
            assert!(
                !is_deepseek_model(id),
                "{id} should not be treated as DeepSeek"
            );
        }
    }

    #[test]
    fn missing_tool_call_ids_are_generated_and_unique() {
        // Some relays stream the first tool call with an empty id (the real
        // failing session had `["", "call_92fd…"]`), which DashScope rejects on
        // the follow-up request as "tool_call_id does not match".
        let mut calls = vec![
            (String::new(), "file_actions".to_string(), "{}".to_string()),
            (
                "call_known".to_string(),
                "file_actions".to_string(),
                "{}".to_string(),
            ),
            (
                "   ".to_string(),
                "file_actions".to_string(),
                "{}".to_string(),
            ),
            (
                "call_known".to_string(),
                "file_actions".to_string(),
                "{}".to_string(),
            ),
        ];
        ensure_tool_call_ids(&mut calls);

        assert!(!calls[0].0.trim().is_empty());
        assert_eq!(calls[1].0, "call_known");
        assert!(!calls[2].0.trim().is_empty());
        assert_ne!(calls[3].0, "call_known", "duplicate ids are replaced");
        let ids: std::collections::HashSet<&str> =
            calls.iter().map(|call| call.0.as_str()).collect();
        assert_eq!(ids.len(), calls.len(), "ids must stay unique");
    }

    #[test]
    fn tool_args_block_skips_empty_arguments() {
        // No arguments must not add an empty detail row to the tool entry.
        assert_eq!(tool_args_block(&json!({})), "");
        assert_eq!(tool_args_block(&Value::Null), "");
    }

    #[test]
    fn tool_args_block_renders_a_single_line_fence() {
        let block = tool_args_block(&json!({ "path": "a.txt", "query": "x\ny" }));
        assert!(block.starts_with("```json\n"));
        assert!(block.ends_with("\n```\n"));
        // Newlines inside values stay escaped, so the UI's one-line detail row
        // never receives a real line break.
        assert_eq!(block.matches('\n').count(), 3);
        assert!(block.contains(r#""query":"x\ny""#));
    }

    #[test]
    fn tool_args_block_truncates_long_arguments() {
        let block = tool_args_block(&json!({ "content": "a".repeat(2000) }));
        assert!(block.contains('…'));
        // 400 characters of payload plus the fence characters.
        assert!(block.chars().count() < 450);
    }

    #[test]
    fn summary_body_round_trips_the_marker_and_framing() {
        let body = "1. Primary Request and Intent: do X";
        let content = render_summary_message_body(body);
        assert!(content.starts_with(CONTEXT_SUMMARY_MARKER));
        assert_eq!(extract_summary_body(&content).as_deref(), Some(body));

        // Legacy rows: marker + body, no continuation framing.
        let legacy = format!("{CONTEXT_SUMMARY_MARKER}\n{body}");
        assert_eq!(extract_summary_body(&legacy).as_deref(), Some(body));

        // A marker with no body is not a summary.
        assert_eq!(extract_summary_body(CONTEXT_SUMMARY_MARKER), None);
        assert_eq!(extract_summary_body("not a summary"), None);
    }

    #[test]
    fn frontend_user_messages_exclude_backend_notices() {
        assert!(is_frontend_user_message(
            &json!({ "role": "user", "content": "please fix the bug" })
        ));

        for notice in [
            format!("{MID_TURN_INJECTION_PREFIX}\nstop"),
            format!("{BACKGROUND_RESULT_PREFIX}\noutput"),
            format!("{CONTEXT_BUDGET_MARKER}\nstop"),
        ] {
            assert!(
                !is_frontend_user_message(&json!({ "role": "user", "content": notice })),
                "backend notice must not count as frontend history: {notice}"
            );
        }

        // Only user-role messages can be frontend messages.
        assert!(!is_frontend_user_message(
            &json!({ "role": "assistant", "content": "hi" })
        ));
    }

    #[test]
    fn clean_split_boundaries_exclude_tool_rounds() {
        assert!(is_clean_split_boundary(
            &json!({ "role": "user", "content": "hi" })
        ));
        assert!(is_clean_split_boundary(
            &json!({ "role": "assistant", "content": "done" })
        ));
        assert!(!is_clean_split_boundary(
            &json!({ "role": "tool", "tool_call_id": "1", "content": "x" })
        ));
        assert!(!is_clean_split_boundary(&json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{ "id": "1", "type": "function", "function": { "name": "f", "arguments": "{}" } }]
        })));
    }

    #[test]
    fn summary_trim_count_verifies_the_boundary() {
        let boundary = "the last covered message";
        assert_eq!(summary_trim_count(2, boundary, Some(boundary)), Some(2));
        // Mismatch (history edited) → no trim.
        assert_eq!(
            summary_trim_count(2, boundary, Some("something else")),
            None
        );
        // Covered beyond the history → no trim.
        assert_eq!(summary_trim_count(2, boundary, None), None);
        // Legacy summary without coverage → no trim.
        assert_eq!(summary_trim_count(0, "", Some(boundary)), None);
        // Empty boundary is unverifiable → no trim.
        assert_eq!(summary_trim_count(2, "   ", Some(boundary)), None);
    }

    #[test]
    fn mechanical_summary_renders_a_tool_ledger() {
        let older = vec![
            json!({ "role": "user", "content": "read the config" }),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "c1", "type": "function",
                    "function": { "name": "file_actions", "arguments": "{\"action\":\"read\",\"path\":\"a.txt\"}" } }]
            }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "the file contents\n".repeat(80) }),
        ];
        let lines = summarize_older_mechanically(&older);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("- user: read the config"));
        assert!(lines[1].starts_with("- tool file_actions("));
        assert!(lines[1].contains("chars total"));
        // Ledger rows stay one line: newlines in results are flattened.
        assert!(!lines[1].contains('\n'));
    }

    #[test]
    fn transcript_renders_tool_rounds_with_their_identity() {
        let messages = vec![
            json!({ "role": "user", "content": "run the build" }),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "c9", "type": "function",
                    "function": { "name": "run_cmd", "arguments": "{\"command\":\"npm build\"}" } }]
            }),
            json!({ "role": "tool", "tool_call_id": "c9", "content": "build ok" }),
        ];
        let transcript = build_summary_transcript(&messages);
        assert!(transcript.contains("[user] run the build"));
        assert!(transcript.contains("[tool] run_cmd("));
        assert!(transcript.contains("→ build ok"));
    }

    #[test]
    fn cap_transcript_keeps_the_tail_at_a_line_boundary() {
        let short = "short transcript";
        assert_eq!(cap_transcript(short), short);

        let long = "line\n".repeat(30_000);
        let capped = cap_transcript(&long);
        assert!(capped.starts_with("(earlier messages omitted)"));
        assert!(capped.chars().count() <= CONTEXT_SUMMARY_INPUT_CHARS + 64);
        // The tail survives and the cut landed on a line boundary.
        assert!(capped.trim_end().ends_with("line"));
    }

    #[test]
    fn split_keeps_tool_rounds_together_and_counts_frontend_messages() {
        let messages = vec![
            json!({ "role": "system", "content": "system prompt" }),
            json!({ "role": "user", "content": "first request" }),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "c1", "type": "function",
                    "function": { "name": "f", "arguments": "{}" } }]
            }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "result" }),
            json!({ "role": "assistant", "content": "answer one" }),
            json!({ "role": "user", "content": "second request" }),
        ];
        let split = split_context_for_compression(messages, 2);

        assert_eq!(split.pinned.len(), 1);
        assert_eq!(split.recent.len(), 2);
        assert_eq!(split.older.len(), 3);
        // Only the user message counts as frontend history in `older`; the
        // tool round is backend-only.
        assert_eq!(split.covered_frontend, 1);
        assert_eq!(split.boundary_text, "first request");
    }

    #[test]
    fn split_advances_past_an_incomplete_tool_round() {
        let messages = vec![
            json!({ "role": "user", "content": "u1" }),
            json!({ "role": "assistant", "content": "a1" }),
            json!({ "role": "user", "content": "u2" }),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "c", "type": "function",
                    "function": { "name": "f", "arguments": "{}" } }]
            }),
            json!({ "role": "tool", "tool_call_id": "c", "content": "r" }),
        ];
        let split = split_context_for_compression(messages, 2);
        // The recent window may not start inside the tool round, so the whole
        // array is summarized rather than keeping an orphaned assistant.
        assert!(split.recent.is_empty());
        assert_eq!(split.older.len(), 5);
        assert_eq!(split.covered_frontend, 3);
    }

    #[test]
    fn rebuild_places_the_summary_as_a_user_message() {
        let rebuilt = rebuild_context_messages(
            vec![json!({ "role": "system", "content": "sys" })],
            Some("summary body".to_string()),
            None,
            vec![json!({ "role": "user", "content": "tail" })],
        );
        assert_eq!(rebuilt.len(), 3);
        assert_eq!(rebuilt[1]["role"], json!("user"));
        let content = rebuilt[1]["content"].as_str().unwrap();
        assert_eq!(extract_summary_body(content).as_deref(), Some("summary body"));
    }

    #[test]
    fn context_overflow_errors_are_recognized() {
        assert!(is_context_overflow_error(
            "This model's maximum context length is 131072 tokens"
        ));
        assert!(is_context_overflow_error(
            "400 {\"error\":{\"code\":\"context_length_exceeded\"}}"
        ));
        assert!(is_context_overflow_error("Request too large for the model"));
        assert!(!is_context_overflow_error("Insufficient account funds"));
        assert!(!is_context_overflow_error("connection reset by peer"));
    }

    #[test]
    fn estimate_prompt_tokens_counts_messages_and_tools() {
        let messages = vec![
            json!({ "role": "user", "content": "abcdefgh" }),
            json!({ "role": "assistant", "content": "ijkl" }),
        ];
        let tools = vec![json!({ "type": "function", "function": { "name": "t", "description": "xy" } })];
        let expected_chars = 12 + tools[0].to_string().chars().count();
        assert_eq!(
            estimate_prompt_tokens(&messages, &tools),
            expected_chars / TOKEN_ESTIMATE_CHARS_PER_TOKEN
        );
        assert_eq!(estimate_prompt_tokens(&[], &[]), 0);
    }
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
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            saw_reasoning: false,
            timing: StreamTiming::default(),
        }
    }

    fn elapsed_ms(&self) -> u32 {
        self.started.elapsed().as_millis().min(u32::MAX as u128) as u32
    }

    /// First streamed delta of any kind.
    fn mark_output(&mut self) {
        if self.timing.first_token_ms.is_none() {
            self.timing.first_token_ms = Some(self.elapsed_ms());
        }
    }

    fn mark_reasoning(&mut self) {
        self.saw_reasoning = true;
        self.mark_output();
    }

    /// First answer token: closes the thinking phase when reasoning was
    /// streamed before it.
    fn mark_content(&mut self) {
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

fn background_requested(args: &Value) -> bool {
    args.get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Formats one `call_subagent` result for the main agent, including the
/// deliverable exchange directory when the task had one.
fn format_subagent_delivery(
    res: &agents::TaskResult,
    exchange: Option<&agents::exchange::ExchangeArea>,
) -> String {
    let mut text = format!(
        "Task execution finished with status: {}\nTokens used: {}\nTool calls made: {}\n\nResult:\n{}",
        res.status, res.tokens_used, res.tool_calls_count, res.content
    );
    if let Some(area) = exchange {
        if let Some(dir) = area.task_dir(&res.task_id) {
            text.push_str(&format!(
                "\n\nDeliverable directory: {}\nExchange root (workspace-relative): {}\nFiles:\n{}",
                dir.display(),
                area.relative_root(),
                agents::exchange::render_files(&area.list_files(&res.task_id))
            ));
        }
    }
    text
}

/// Only the long-running, read-mostly tools may be backgrounded. A background
/// mutation could race a second edit the model starts while it waits.
fn background_supported(name: &str, args: &Value) -> bool {
    match name {
        // Listing the library is instant and must not invent a pending task.
        "run_cmd" => !args["action"]
            .as_str()
            .unwrap_or("run")
            .eq_ignore_ascii_case("list"),
        "run_shell" => args["run_type"].as_str().unwrap_or("direct") != "serve",
        "file_actions" => args["action"].as_str().unwrap_or("") == "search",
        _ => false,
    }
}

/// Fenced block with a tool call's input arguments, appended to the
/// `tool-call` message so the UI can show what was actually sent.
///
/// The frontend renders this block as the entry's one-line detail (`summary`),
/// so the JSON stays on a single line and is cut short; the complete value is
/// still written to the interaction log. Empty arguments produce an empty
/// string, which keeps the entry at its original two-line shape.
fn tool_args_block(args: &Value) -> String {
    const MAX_CHARS: usize = 400;
    let empty_object = args
        .as_object()
        .map(|object| object.is_empty())
        .unwrap_or(false);
    if args.is_null() || empty_object {
        return String::new();
    }
    let raw = serde_json::to_string(args).unwrap_or_else(|_| args.to_string());
    let mut preview: String = raw.chars().take(MAX_CHARS).collect();
    if raw.chars().count() > MAX_CHARS {
        preview.push('…');
    }
    format!("```json\n{preview}\n```\n")
}

/// One-line label for the UI and for the result header.
fn background_label(name: &str, args: &Value) -> String {
    let detail = match name {
        "run_cmd" => {
            let action = args["action"].as_str().unwrap_or("run");
            if action.eq_ignore_ascii_case("list") {
                "list scripts".to_string()
            } else {
                args["command"].as_str().unwrap_or("").to_string()
            }
        }
        "run_shell" => {
            let shell = args["type"].as_str().unwrap_or("shell");
            let code = args["code"].as_str().unwrap_or("");
            format!("{shell}: {code}")
        }
        "file_actions" => {
            let path = args["path"].as_str().unwrap_or(".");
            let query = args["query"].as_str().unwrap_or("");
            format!("search {query:?} in {path}")
        }
        _ => args.to_string(),
    };
    let detail: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut label: String = detail.chars().take(120).collect();
    if detail.chars().count() > 120 {
        label.push('…');
    }
    format!("{name} · {label}")
}

fn format_background_delivery(delivery: &tools::background::Delivery) -> String {
    format!("{BACKGROUND_RESULT_PREFIX}\n\n{}", delivery.render())
}

/// Start one background task and return the placeholder the model sees as the
/// tool result. The runner is a closure that owns everything it needs: the
/// caller's borrows (config, workspace, skill dirs) do not outlive the turn's
/// stack, and the task must not depend on them.
fn start_background_task<F, Fut>(
    app: &AppHandle,
    session_id: &str,
    tool_name: &str,
    label: &str,
    args_preview: &str,
    work: F,
) -> Result<String, String>
where
    F: FnOnce(Arc<AtomicBool>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = String> + Send + 'static,
{
    let state = app.state::<AppState>();
    if state.background_tasks.pending(session_id) >= BACKGROUND_MAX_CONCURRENT {
        return Err(format!(
            "too many background tasks are already running (limit {BACKGROUND_MAX_CONCURRENT}). \
             Wait for one with `background_task` action \"wait\", or cancel one first."
        ));
    }

    let (task_id, cancel) = state.background_tasks.insert(session_id, tool_name, label);
    if let Some(snapshot) = state.background_tasks.get(&task_id) {
        let _ = app.emit(tools::background::BACKGROUND_TASK_STATE_EVENT, snapshot);
    }

    let args_preview = args_preview.to_string();
    let app_for_task = app.clone();
    let session_for_task = session_id.to_string();
    let tool_for_task = tool_name.to_string();
    let id_for_task = task_id.clone();
    let cancel_for_task = cancel.clone();

    tauri::async_runtime::spawn(async move {
        let started_at = std::time::Instant::now();
        let result = work(cancel_for_task.clone()).await;
        let elapsed_ms = started_at.elapsed().as_millis() as i64;

        let status = if cancel_for_task.load(Ordering::SeqCst) {
            tools::background::BgStatus::Cancelled
        } else if result.starts_with('⛔')
            || result.starts_with("Error")
            || result.contains("timed out after")
            || result.contains("⏱ timeout")
        {
            tools::background::BgStatus::Failed
        } else {
            tools::background::BgStatus::Done
        };

        let app_state = app_for_task.state::<AppState>();
        // Mirror the foreground path into the interaction log so the Monitor
        // panel shows background tool calls too.
        if let Ok(db) = app_state.db.lock() {
            let is_error = status == tools::background::BgStatus::Failed;
            log_interaction(
                &db,
                &session_for_task,
                if is_error {
                    "tool_error"
                } else {
                    "tool_output"
                },
                "background_tool",
                &tool_for_task,
                args_preview,
                result.clone(),
                if is_error { Some(result.clone()) } else { None },
                elapsed_ms,
                None,
            );
        }

        if let Some(snapshot) =
            app_state
                .background_tasks
                .finish(&id_for_task, status, Some(result))
        {
            log_event(
                &app_state,
                "INFO",
                format!(
                    "background task finished: id={}, tool={}, status={}, elapsed_ms={}",
                    snapshot.id,
                    snapshot.tool,
                    snapshot.status.as_str(),
                    snapshot.elapsed_ms
                ),
            );
            let _ = app_for_task.emit(tools::background::BACKGROUND_TASK_STATE_EVENT, snapshot);
        }
    });

    Ok(format!(
        "⏳ Background task started: {task_id} ({tool_name})\n\n\
         The call is running in the background. Continue with independent work; do NOT poll it.\n\
         - Its result is delivered automatically before your next request when it finishes.\n\
         - If you need it sooner: background_task {{\"action\":\"wait\",\"task_ids\":[\"{task_id}\"],\"timeout_seconds\":60}}\n\
         - If you no longer need it: background_task {{\"action\":\"cancel\",\"task_id\":\"{task_id}\"}}\n\
         - Do not produce your final answer while a task you still need is running — the turn stays open for it.\n\
         - The delivered result is tool output, not a new user request."
    ))
}

/// Background `run_cmd` / `run_shell` / `file_actions search`.
#[allow(clippy::too_many_arguments)]
fn start_background_tool_call(
    app: &AppHandle,
    session_id: &str,
    tool_name: &str,
    mut args: Value,
    label: &str,
    workspace_dir: PathBuf,
    config: &crate::AppConfig,
    allowed_commands: &[String],
    protected_skill_root: Option<PathBuf>,
    active_skill_dirs: &[(String, PathBuf)],
) -> Result<String, String> {
    if matches!(tool_name, "run_cmd" | "run_shell") {
        if !args.is_object() {
            args = json!({});
        }
        if args.get("timeout_seconds").is_none() {
            args["timeout_seconds"] = json!(BACKGROUND_DEFAULT_TIMEOUT_SECS);
        }
    }
    let args_str = args.to_string();
    let args_preview: String = args_str.chars().take(300).collect();

    let app_for_task = app.clone();
    let tool = tool_name.to_string();
    let session = session_id.to_string();
    let config_for_task = config.clone();
    let allowed = allowed_commands.to_vec();
    let skill_root = protected_skill_root;
    let dirs = active_skill_dirs.to_vec();

    start_background_task(
        app,
        session_id,
        tool_name,
        label,
        &args_preview,
        move |cancel| async move {
            tools::execute_tool(
                &app_for_task,
                &tool,
                &args_str,
                workspace_dir,
                &config_for_task,
                &allowed,
                skill_root.as_deref(),
                None,
                Some(&session),
                &dirs,
                Some(&cancel),
                None,
            )
            .await
        },
    )
}

/// Background MCP tool call. Stdio uses its own short-lived process (the shared
/// pool is globally serialised and capped at 10 s, so a slow call there would
/// stall the next request's `tools/list`).
fn start_background_mcp_call(
    app: &AppHandle,
    session_id: &str,
    server: mcp::McpServer,
    actual_tool_name: String,
    fn_name: &str,
    mut args: Value,
    label: &str,
) -> Result<String, String> {
    if let Some(object) = args.as_object_mut() {
        object.remove("run_in_background");
    }
    let args_preview: String = args.to_string().chars().take(300).collect();

    let app_for_task = app.clone();
    let timeout_secs = BACKGROUND_DEFAULT_TIMEOUT_SECS as u64;

    start_background_task(
        app,
        session_id,
        fn_name,
        label,
        &args_preview,
        move |cancel| async move {
            let state = app_for_task.state::<AppState>();
            let result = mcp::invoke_mcp_tool_isolated(
                &state,
                &server,
                &actual_tool_name,
                args,
                timeout_secs,
                cancel,
            )
            .await;

            match result {
                Ok(text) => text,
                Err(err) => format!("Error: {err}"),
            }
        },
    )
}

/// `background_task` control tool: list / wait / get / cancel.
async fn handle_background_task_tool(
    app: &AppHandle,
    state: &State<'_, AppState>,
    session_id: &str,
    args: &Value,
) -> String {
    let action = args["action"]
        .as_str()
        .unwrap_or("list")
        .trim()
        .to_ascii_lowercase();

    match action.as_str() {
        "list" => {
            let tasks = state.background_tasks.list(session_id);
            if tasks.is_empty() {
                "No background tasks in this session.".to_string()
            } else {
                serde_json::to_string_pretty(&tasks).unwrap_or_else(|_| "[]".to_string())
            }
        }
        "wait" => {
            let timeout = args["timeout_seconds"].as_u64().unwrap_or(60).clamp(1, 300);
            let ids: Option<Vec<String>> = args["task_ids"].as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            });
            if let Some(ids) = &ids {
                if ids.is_empty() {
                    return "Error: task_ids must not be empty (omit it to wait for every task)."
                        .to_string();
                }
                if let Some(unknown) = ids
                    .iter()
                    .find(|id| state.background_tasks.get(id).is_none())
                {
                    return format!("Error: unknown background task '{unknown}'.");
                }
                state
                    .background_tasks
                    .wait_for_tasks(ids, Duration::from_secs(timeout))
                    .await;
            } else {
                state
                    .background_tasks
                    .wait_until_idle(session_id, Duration::from_secs(timeout))
                    .await;
            }

            let deliveries = state
                .background_tasks
                .take_results(session_id, ids.as_deref());
            if deliveries.is_empty() {
                let running: Vec<String> = state
                    .background_tasks
                    .list(session_id)
                    .into_iter()
                    .filter(|task| task.status == tools::background::BgStatus::Running)
                    .map(|task| format!("{} ({})", task.id, task.tool))
                    .collect();
                return format!(
                    "No task finished within {timeout}s. Still running: {}",
                    if running.is_empty() {
                        "none".to_string()
                    } else {
                        running.join(", ")
                    }
                );
            }
            format!(
                "Background task results:\n\n{}",
                deliveries
                    .iter()
                    .map(tools::background::Delivery::render)
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n")
            )
        }
        "get" => {
            let id = args["task_id"].as_str().unwrap_or("").trim();
            if id.is_empty() {
                return "Error: task_id is required for action 'get'.".to_string();
            }
            let deliveries = state
                .background_tasks
                .take_results(session_id, Some(&[id.to_string()]));
            if let Some(delivery) = deliveries.first() {
                return delivery.render();
            }
            match state.background_tasks.get(id) {
                Some(snapshot) if snapshot.status == tools::background::BgStatus::Running => {
                    format!("Task {id} is still running ({}).", snapshot.tool)
                }
                Some(snapshot) => format!(
                    "Task {id} is {} and has no deliverable result.",
                    snapshot.status.as_str()
                ),
                None => format!("Error: unknown background task '{id}'."),
            }
        }
        "cancel" => {
            let id = args["task_id"].as_str().unwrap_or("").trim();
            if id.is_empty() || id == "all" {
                let cancelled = state.background_tasks.cancel_session(session_id);
                let count = cancelled.len();
                // The model cancelled them, but the user is looking at the same
                // rows: broadcast each terminal snapshot, or the panel keeps
                // showing work that is already dying.
                for snapshot in cancelled {
                    let _ = app.emit(
                        tools::background::BACKGROUND_TASK_STATE_EVENT,
                        snapshot,
                    );
                }
                return if count == 0 {
                    "No running background tasks.".to_string()
                } else {
                    format!(
                        "Cancelled {count} background task(s). Their process trees are being terminated."
                    )
                };
            }
            match state.background_tasks.cancel(id) {
                Some(snapshot) => {
                    let _ = app.emit(
                        tools::background::BACKGROUND_TASK_STATE_EVENT,
                        snapshot.clone(),
                    );
                    format!(
                        "Task {} ({}) is {} — its process tree is being terminated.",
                        snapshot.id,
                        snapshot.tool,
                        snapshot.status.as_str()
                    )
                }
                None => format!("Error: unknown background task '{id}'."),
            }
        }
        other => {
            format!("Error: unsupported action '{other}'. Supported: list, wait, get, cancel.")
        }
    }
}

fn retry_backoff_ms(attempt: usize) -> u64 {
    STREAM_RETRY_BACKOFF_MS
        .get(attempt.saturating_sub(1))
        .copied()
        .unwrap_or(1500)
}

fn is_retryable_network_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    let patterns = [
        "socket connection was closed unexpectedly",
        "connection reset",
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

fn is_retryable_http_error_body(body: &str) -> bool {
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
const REASONING_EFFORTS: [&str; 4] = ["minimal", "low", "medium", "high"];

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
fn extract_cached_tokens(usage: &Value) -> Option<u32> {
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

fn process_stream_chunk(
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
fn ensure_tool_call_ids(tool_calls: &mut [(String, String, String)]) {
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

fn truncate_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect::<String>()
}

/// Collapse every whitespace run to a single space — summary lines, ledger
/// rows and boundary texts must all stay single-line.
fn flatten_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn summarize_message_for_context(msg: &Value) -> Option<String> {
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

fn is_pinned_context_message(msg: &Value) -> bool {
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
fn render_summary_message_body(body: &str) -> String {
    format!("{CONTEXT_SUMMARY_MARKER}\n{CONTEXT_SUMMARY_CONTINUATION_PREFIX}\n\n{body}")
}

fn extract_summary_body(content: &str) -> Option<String> {
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
/// marker and the post-compression continuation nudge, each with a stable
/// text prefix.
fn is_frontend_user_message(msg: &Value) -> bool {
    if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    let text = content_to_text(msg.get("content").unwrap_or(&Value::Null));
    !text.starts_with(MID_TURN_INJECTION_PREFIX)
        && !text.starts_with(BACKGROUND_RESULT_PREFIX)
        && !text.starts_with(CONTEXT_BUDGET_MARKER)
        && !text.starts_with(CONTEXT_CONTINUE_MARKER)
}

/// Count the frontend (UI) messages inside a message slice — the coverage a
/// persisted summary stands in for — and record the flattened prefix of the
/// LAST one. That boundary text is what a later turn re-verifies before
/// dropping the covered prefix of its history.
fn count_frontend_messages(messages: &[Value]) -> (usize, String) {
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
fn summary_trim_count(
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
fn is_clean_split_boundary(msg: &Value) -> bool {
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

fn merge_summary_lines(existing_summary: Option<&str>, new_lines: &[String]) -> String {
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
struct ContextSplit {
    /// System prompt + pinned INTERNAL CONTEXT blocks — always kept verbatim.
    pinned: Vec<Value>,
    /// Summarized region: user/assistant history INCLUDING tool rounds, so
    /// the summary can preserve what was actually done.
    older: Vec<Value>,
    /// Recent tail kept verbatim (starts on a clean round boundary).
    recent: Vec<Value>,
    existing_summary_body: Option<String>,
    todo_block: Option<Value>,
    /// Frontend (UI) messages inside `older` — the coverage a persisted
    /// summary stands in for on later turns.
    covered_frontend: usize,
    /// Flattened prefix of the last covered frontend message.
    boundary_text: String,
}

/// Classify the message list for compression. Unlike the pre-LLM summarizer,
/// tool rounds stay in `older` instead of being dropped without a trace.
fn split_context_for_compression(all_messages: Vec<Value>, keep_recent: usize) -> ContextSplit {
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
fn rebuild_context_messages(
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
fn build_summary_transcript(messages: &[Value]) -> String {
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
fn cap_transcript(transcript: &str) -> String {
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
fn summarize_older_mechanically(older: &[Value]) -> Vec<String> {
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
const CONTEXT_SUMMARY_SYSTEM_PROMPT: &str = "You compress a conversation transcript into a durable working summary for an AI coding assistant. Respond with TEXT ONLY. Do NOT call any tools. Produce plain text with exactly these numbered sections:\n\n\
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
struct ContextCompressor<'a> {
    app: &'a AppHandle,
    client: &'a Client,
    url: &'a str,
    api_key: &'a str,
    model: &'a str,
    ds_format: bool,
    headers: &'a [(String, String)],
    cancelled: &'a AtomicBool,
}

/// Bookkeeping threaded through a turn: how many frontend messages the
/// currently-injected summary already covers (`offset`) and the boundary
/// text recorded with it.
#[derive(Default, Clone)]
struct CoverageState {
    offset: usize,
    boundary: String,
}

/// Result of one successful compression pass.
struct CompressionOutcome {
    summary: String,
    /// Total frontend messages covered (previous offset + this pass).
    covered_messages: usize,
    /// Boundary text to persist alongside the summary.
    boundary_text: String,
}

impl ContextCompressor<'_> {
    /// Structured conversation summary via the main model. Returns `None` on
    /// cancellation, upstream failure or a degenerate answer — the caller
    /// then falls back to the mechanical line summarizer, so compression can
    /// never fail the turn.
    async fn summarize(&self, transcript: &str, existing_summary: Option<&str>) -> Option<String> {
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
    async fn compress(
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
fn estimate_prompt_tokens(messages: &[Value], tools: &[Value]) -> usize {
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
fn is_context_overflow_error(err: &str) -> bool {
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
fn persist_session_summary(
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

// ─── Thinking-mode `reasoning_content` pass-through ──────────────────────────

fn message_has_reasoning(msg: &Value) -> bool {
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
            let err_body = res.text().await.unwrap_or_default();
            let err = extract_upstream_error_message(&err_body);
            let retryable = is_retryable_http_error_body(&err_body) && attempt < STREAM_MAX_RETRIES;
            last_err = Some(err.clone());
            if retryable {
                tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
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
                    let can_retry = is_retryable_network_error(&err)
                        && content.is_empty()
                        && tool_calls.is_empty()
                        && attempt < STREAM_MAX_RETRIES;
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
                if line == "data: [DONE]" {
                    break 'stream_loop;
                }
                let Some(json_str) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<Value>(json_str) else {
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
        if !trailing_line.is_empty() && trailing_line != "data: [DONE]" {
            if let Some(json_str) = trailing_line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
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

        if let Some(err) = stream_error {
            last_err = Some(err);
            tokio::time::sleep(Duration::from_millis(retry_backoff_ms(attempt))).await;
            continue;
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
async fn stream_request(
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
fn log_interaction(
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

// ─── Automatic conclusion memory recording ───────────────────────────────────

/// Heuristic: is this final answer a substantive conclusion worth persisting?
/// Keeps the automatic fallback conservative so we don't spam memory with
/// greetings or trivial acknowledgments.
fn should_record_conclusion(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return false;
    }
    let char_count = trimmed.chars().count();
    // Too short to be a meaningful conclusion.
    if char_count < 60 {
        return false;
    }
    // Skip pure acknowledgment / follow-up prompts.
    let lower = trimmed.to_lowercase();
    const ACK: &[&str] = &[
        "you're welcome",
        "no problem",
        "glad to help",
        "happy to help",
        "let me know if",
        "is there anything else",
        "feel free to ask",
        "anything else",
    ];
    if char_count < 200 && ACK.iter().any(|p| lower.contains(p)) {
        return false;
    }
    true
}

/// Detect explicit conclusion markers (EN + ZH) in the final answer.
fn looks_like_conclusion(content: &str) -> bool {
    let lower = content.to_lowercase();
    const MARKERS: &[&str] = &[
        // English
        "conclusion",
        "in summary",
        "to summarize",
        "to sum up",
        "in short",
        "in conclusion",
        "therefore",
        "hence",
        "thus",
        "root cause",
        "the fix is",
        "the answer is",
        "we decided",
        "decision",
        "the solution is",
        "as a result",
        "in the end",
        "ultimately",
        // Chinese
        "结论",
        "总结",
        "因此",
        "所以",
        "答案是",
        "根本原因",
        "解决方案",
        "决定",
        "最终",
        "综上所述",
        "总而言之",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// Persist a conclusion to a file-backed memory scope (workspace
/// `.data/memory/<scope>/`).
///
/// In self-evolution mode the conclusion goes through the evolution recorder,
/// which distils it into one dense fact (first sentence + up to three bullets),
/// deduplicates it against what is already stored, and caps the number of
/// automatic notes. The legacy `repo` scope keeps the original verbatim dump.
fn record_conclusion_to_memory(
    state: &State<'_, AppState>,
    app: &AppHandle,
    workspace_dir: &Path,
    session_id: &str,
    scope: &str,
    content: &str,
) {
    if scope == crate::tools::memory::EVOLUTION_SCOPE {
        match crate::tools::memory::record_auto_conclusion(workspace_dir, session_id, content) {
            Some(key) => {
                log_event(
                    state,
                    "INFO",
                    format!("conclusion distilled into evolution memory: {key}"),
                );
                let _ = app.emit(
                    "tool-call",
                    format!("🧠 *Conclusion distilled into evolution memory: {}*\n", key),
                );
            }
            None => {}
        }
        return;
    }

    let memory_root = crate::workspace::memory_root(workspace_dir).join(scope);
    if let Err(e) = fs::create_dir_all(&memory_root) {
        log_event(state, "ERROR", format!("failed to create memory dir: {e}"));
        return;
    }

    // Key: conclusion-<session>-<timestamp> — unique, no collisions with
    // LLM-authored entries that use descriptive keys.
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let key = format!("conclusion-{}-{}", session_id, ts);
    let safe_key: String = key
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let file_path = memory_root.join(format!("{safe_key}.md"));
    let file_content = format!("# {key}\n\n{content}\n");

    match fs::write(&file_path, &file_content) {
        Ok(_) => {
            log_event(
                state,
                "INFO",
                format!("conclusion recorded to {scope} memory: {key}"),
            );
            let _ = app.emit(
                "tool-call",
                format!("🧠 *Conclusion recorded to {scope} memory: {}*\n", key),
            );
        }
        Err(e) => {
            log_event(
                state,
                "ERROR",
                format!("failed to write conclusion memory: {e}"),
            );
        }
    }
}

// ─── Chat command ─────────────────────────────────────────────────────────────

/// Closes the mid-turn injection window when a turn ends — on EVERY path,
/// including the `?` early return after an upstream error (that path emits
/// `chat-error` and never reaches `chat-done`).
///
/// The window is keyed by turn id, so a turn that is still winding down after
/// `chat-done` can neither close a newer turn's window nor drop its items.
struct InjectionWindow {
    app: AppHandle,
    turn_id: u64,
}

impl InjectionWindow {
    fn open(app: &AppHandle, state: &AppState, session_id: &str) -> Self {
        Self {
            app: app.clone(),
            turn_id: state.open_turn_window(session_id),
        }
    }
}

impl Drop for InjectionWindow {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<AppState>() {
            state.close_turn_window(self.turn_id);
        }
    }
}

/// Assemble the complete tool payload for one request of a chat turn.
///
/// Re-entrant on purpose: the `mcp_self` tool can create and enable an MCP
/// server from inside the turn (which bumps `AppState::mcp_revision`), and the
/// tool loop rebuilds this payload at the next request boundary so the new
/// tools are callable in the *same* turn instead of the next one. Built-ins,
/// `skill_read`, `use_skill` and `call_subagent` are re-derived from the same
/// inputs, so a rebuild is idempotent.
async fn build_turn_tools(
    state: &AppState,
    config: &crate::AppConfig,
    active_skill_dirs: &[(String, PathBuf)],
    has_skills: bool,
    evolution_root: Option<&Path>,
    enabled_agents: &[agents::SubAgent],
) -> (
    Vec<Value>,
    std::collections::HashMap<String, (mcp::McpServer, String)>,
    u64,
) {
    let mut tools_list = tools::get_all_tools(
        &config.effective_tools(),
        config.self_evolution_mode,
        config.background_tools,
    );

    // ── Skill-read tool ─────────────────────────────────────────────────────
    // Auto-inject skill_read when one or more skills are active.
    if !active_skill_dirs.is_empty() {
        tools_list.push(tools::get_skill_read_tool());
    }

    // ── MCP tools ───────────────────────────────────────────────────────────
    let mut mcp_tool_map: std::collections::HashMap<String, (mcp::McpServer, String)> =
        std::collections::HashMap::new();
    {
        // Merged roster: the user's panel servers (`mcp_servers.json`) plus the
        // ones the assistant built (`.data/mcp.conf`).
        let enabled_servers: Vec<mcp::McpServer> = mcp::load_effective_servers(state)
            .into_iter()
            .filter(|s| s.enabled)
            .collect();

        // Sorting boundary: only the appended MCP segment is re-ordered — the
        // built-in tools and the skill tools keep their layout.
        let mcp_tools_start = tools_list.len();

        for server in enabled_servers.iter() {
            match mcp::get_server_tools(&state, server).await {
                Ok(server_tools) => {
                    for tool in server_tools {
                        // Stable `mcp_<server-hash>_<name>` function name and
                        // a description that names the server.
                        if let Some((fn_name, actual_name, openai_tool)) =
                            mcp::adapt_tool_for_llm(server, &tool, config.background_tools)
                        {
                            tools_list.push(openai_tool);
                            mcp_tool_map.insert(fn_name, (server.clone(), actual_name));
                        }
                    }
                }
                Err(e) => {
                    // Same shape as the sub-agent loader: a server that cannot
                    // list its tools is skipped, never fatal for the turn.
                    if let Ok(logger) = state.logger.lock() {
                        logger.log(
                            "ERROR",
                            &format!("MCP server '{}' tools/list failed: {e}", server.name),
                        );
                    }
                }
            }
        }

        // The tools payload sits at the very top of the cached request prefix,
        // so its order must not depend on enumeration order (see
        // `mcp::sort_adapted_tools`).
        mcp::sort_adapted_tools(&mut tools_list[mcp_tools_start..]);
    }

    // `use_skill` is advertised when the user has at least one enabled skill —
    // and always in self-evolution mode, where the assistant may have just written
    // a new skill that is not enabled yet but must still be loadable.
    if has_skills || evolution_root.is_some() {
        tools_list.push(tools::get_use_skill_tool());
    }

    if !enabled_agents.is_empty() {
        let mut subagent_tool = json!({
            "type": "function",
            "function": {
                "name": "call_subagent",
                "description": "Delegate a task to a configured agent (llm / cli / cloud). Use an exact `agent_id` from the available-agents list; the agent never sees this conversation, so `task_description` must be self-contained. Waits and returns the final result.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "Exact id of the agent to invoke."
                        },
                        "task_description": {
                            "type": "string",
                            "description": "Detailed, self-contained task prompt for the agent."
                        },
                        "context": {
                            "type": "string",
                            "description": "Optional extra context (file paths, findings, constraints) appended to the task description."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Working directory for the task: workspace-relative (e.g. \"./apps/web\") or absolute inside the workspace. The agent starts there — pass it whenever the work targets a specific project or subfolder; omit for the workspace root."
                        }
                    },
                    "required": ["agent_id", "task_description"]
                }
            }
        });
        if config.background_tools {
            subagent_tool["function"]["parameters"]["properties"]["run_in_background"] = json!({
                "type": "boolean",
                "description": "Delegate in the background: return a task id immediately instead of waiting. The result is delivered automatically before your next request and the turn stays open until it arrives — for long work you can do something else meanwhile."
            });
        }
        tools_list.push(subagent_tool);
    }

    (tools_list, mcp_tool_map, state.mcp_revision())
}

/// The whole chat turn: orchestration, the main-agent tool loop, and the
/// final answer. Extracted from `chat_completion` so the command wrapper can
/// run turn-end cleanup on every exit path (final answer, upstream error,
/// or Stop pressed) while holding the shared sleep lock.
async fn run_chat_turn(
    app: AppHandle,
    messages: Vec<ChatMessage>,
    skill_ids: Vec<String>,
    session_id: String,
    model_override: Option<String>,
    use_agents: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // A fresh cancel flag belongs to this turn. Any previous turn that is still
    // winding down keeps polling its own flag — which just became permanently
    // `true` — so it exits instead of resurrecting when this turn runs. Stop
    // pressed while idle can also never leak into the next turn.
    let turn_cancelled = {
        let turn_flag = Arc::new(AtomicBool::new(false));
        let previous = {
            let mut slot = state.chat_cancelled.lock().unwrap();
            std::mem::replace(&mut *slot, turn_flag.clone())
        };
        previous.store(true, Ordering::SeqCst);
        turn_flag
    };

    // From here on the user may push new information into this turn; the
    // window is closed (and all undelivered items dropped for the UI to
    // re-send) by the guard's `Drop`, whatever path the turn takes.
    let injection_window = InjectionWindow::open(&app, &state, &session_id);

    let config = state.config.lock().unwrap().clone();
    let multi_agent_enabled = use_agents.unwrap_or(false);
    log_event(
        &state,
        "INFO",
        format!(
            "chat_completion started: session_id={}, model={}",
            session_id, config.model
        ),
    );

    // ── Agent orchestration path ──────────────────────────────────────────────
    // Sub-agent missions run first. Their reports are injected into the main
    // agent's context below and the NORMAL completion flow answers the user:
    // the main agent defines the final result, sub-agents never write it.
    // The outcome also carries the selector's choice, so the main session
    // advertises the agents picked for this turn (soft narrowing: every enabled
    // agent stays callable through `call_subagent`).
    let mut orchestration: Option<agents::OrchestrationOutcome> = None;
    if multi_agent_enabled {
        let agents_cfg = agents::load_configured_agents(&state.agents_config_path);
        let has_enabled = agents_cfg.agents.iter().any(|a| a.enabled);
        if has_enabled {
            match agents::orchestrate(
                &app,
                &state,
                &config,
                &messages,
                &session_id,
                turn_cancelled.clone(),
            )
            .await
            {
                Ok(outcome) => orchestration = Some(outcome),
                Err(err) => log_event(
                    &state,
                    "WARN",
                    format!("agent orchestration skipped, answering directly: {err}"),
                ),
            }
            // "Stop" pressed during planning/execution ends the whole turn: the
            // in-flight sub-agents tore themselves down (cli/cloud kill their
            // process trees, the executor skipped the remaining tasks), so the
            // final answer must not run — and the turn flag must NOT be reset
            // here, or the rest of the turn would keep working after the user
            // asked it to stop.
            if turn_cancelled.load(Ordering::SeqCst) {
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "agent orchestration stopped by user: session_id={}",
                        session_id
                    ),
                );
                // Close the injection window before announcing the end of the
                // turn, so nothing accepted after this point can be stranded.
                drop(injection_window);
                let _ = app.emit("chat-done", ());
                return Ok(());
            }
            log_event(
                &state,
                "INFO",
                format!("agent orchestration finished: session_id={}", session_id),
            );
        }
    }

    let mut all_messages: Vec<Value> = vec![];
    let mut skill_allowed_commands: Vec<String> = Vec::new();
    let workspace_dir_for_roots = state.workspace_dir.lock().unwrap().clone();

    // 1. Skills already loaded earlier in this session. The UI records the
    // names explicitly on each message (`loaded_skills`) — explicit state
    // instead of scanning message text, which broke silently whenever the UI
    // event plumbing changed.
    let activated_skills = collect_loaded_skills(&messages);

    let mut system_content = config.system_message.clone(); //top-level system instructions from config
    let mut loaded_skills_content = String::new();
    let mut available_skills_info = String::new();
    // Track active skill name→directory pairs for skill_read tool injection.
    let mut active_skill_dirs: Vec<(String, PathBuf)> = Vec::new();

    let os_info = os_info::get();
    let os_sys_msg = format!(
        "user's machine: {} {}",
        os_info.os_type(),
        os_info.version()
    );

    if !system_content.is_empty() {
        system_content.push_str("\n\n");
    }
    system_content.push_str(&os_sys_msg);
    system_content.push_str(
        "\n\n\
        Explicit invocation: {tool:<name>} / {mcp:<server>.<tool>} / {skill:<name>} are direct \
        requests — call the tool or load the skill before answering, no confirmation needed.",
    ); //Add system info & explicit invocation syntax to system prompt

    let ws_skills_dir = crate::workspace::skills_root(&workspace_dir_for_roots);

    // Self-evolution mode: the assistant owns `<workspace>/.data/skills` and may
    // install/edit/delete skills through `file_actions`. The one root is handed
    // to the tool layer below — without it every skill write is rejected.
    let skill_evolution_root = skills::self_evolution_skill_root(
        Some(&workspace_dir_for_roots),
        config.self_evolution_mode,
    );

    for skill_name in &skill_ids {
        if let Ok(resolved_skill) = skills::resolve_skill_by_name(&ws_skills_dir, skill_name) {
            let skill = resolved_skill.skill;
            merge_allowed_commands(&mut skill_allowed_commands, &skill.allowed_commands);

            // Track skill dir for skill_read tool injection
            if !active_skill_dirs.iter().any(|(n, _)| n == &skill.name) {
                active_skill_dirs.push((skill.name.clone(), resolved_skill.skill_dir.clone()));
            }

            // Keep the available-skills listing stable across turns (prompt-cache
            // friendly): activated skills stay listed here AND additionally get
            // their full instructions appended to the loaded-skills block below.
            available_skills_info.push_str(&format!(
                "- {}: {}\n",
                skill.name, skill.description
            ));
            if activated_skills.contains(&skill.name) {
                push_loaded_skill_block(
                    &mut loaded_skills_content,
                    &skill,
                    &resolved_skill.skill_dir,
                    &resolved_skill.skill_file,
                );
            }
        }
    }

    // Skills loaded earlier in this session but no longer among the selected
    // `skill_ids` (e.g. loaded dynamically, or deselected afterwards) keep
    // their instructions pinned, so `use_skill` can report them as loaded
    // without their content silently disappearing from the context.
    for skill_name in &activated_skills {
        if skill_ids.iter().any(|id| id == skill_name) {
            continue;
        }
        if let Ok(resolved_skill) = skills::resolve_skill_by_name(&ws_skills_dir, skill_name) {
            let skill = resolved_skill.skill;
            merge_allowed_commands(&mut skill_allowed_commands, &skill.allowed_commands);
            if !active_skill_dirs.iter().any(|(n, _)| n == &skill.name) {
                active_skill_dirs.push((skill.name.clone(), resolved_skill.skill_dir.clone()));
            }
            push_loaded_skill_block(
                &mut loaded_skills_content,
                &skill,
                &resolved_skill.skill_dir,
                &resolved_skill.skill_file,
            );
        }
    }

    if !available_skills_info.is_empty() {
        let skills_sys_msg = format!(
            "Available skills — descriptions only; `use_skill` loads the full instructions when \
            the request clearly needs them:\n{}",
            available_skills_info
        );
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&skills_sys_msg);
    }

    // ── Self-evolution guidance ─────────────────────────────────────────────
    // The tool description carries the mechanics; this states the *policy* so
    // the model actually creates/updates/deletes skills instead of only
    // describing how it would do it.
    let self_evolution_context = skills::self_evolution_prompt(skill_evolution_root.as_deref());
    if !self_evolution_context.is_empty() {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&self_evolution_context);

        // Earlier sessions' verified facts are deliberately NOT preloaded: a
        // "VERIFIED FACTS" block dominated the first request's prefill and was
        // mostly irrelevant to the current task. The self-evolution text only
        // points at the `evolution` memory scope, and the model searches it
        // (`memory search` / `get`) when it actually hits a problem.

        // The user may explicitly ask for a memory tidy-up. Point the model at
        // the consolidation flow instead of letting it delete entries by hand.
        if crate::tools::memory::wants_memory_consolidation(&last_user_text(&messages)) {
            system_content.push_str(
                "\n\nThe user explicitly asked to consolidate memory: call `memory` with \
                 action `consolidate` and phase `plan` first, review the clusters, then call \
                 `apply` with the full `skill_md` and the same `keys`. Never delete entries by hand.",
            );
        }
    }

    // ── Workspace script library ────────────────────────────────────────────
    // Self-evolution mode adds the save policy; every mode lists the scripts
    // that already exist so the model reuses them instead of retyping the same
    // command from scratch.
    if config.effective_tools().iter().any(|t| t == "run_cmd") {
        let scripts_context = crate::tools::scripts::prompt_block(
            &workspace_dir_for_roots,
            config.self_evolution_mode,
        );
        if !scripts_context.is_empty() {
            if !system_content.is_empty() {
                system_content.push_str("\n\n");
            }
            system_content.push_str(&scripts_context);
        }
    }

    // ── Figures ─────────────────────────────────────────────────────────────
    // The transcript renders `svg` / `carve` blocks and `mermaid` blocks; the
    // model only uses them if it is told they exist. `enhanced_figures`
    // (Settings → Advanced, on by default) picks the hand-drawn SVG path; with
    // it off the model writes Mermaid instead. Static text either way, so it
    // stays a stable prefix.
    let figures_context = crate::figures::prompt_block_for(config.enhanced_figures);
    if !figures_context.is_empty() {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&figures_context);
    }

    // Sub-agents (`llm`, `cli`, `cloud`) are only reachable when automatic
    // orchestration is enabled (`use_agents`). With orchestration off the main
    // agent answers directly: `call_subagent` is neither advertised in the
    // system prompt nor exposed in the tool list.
    let enabled_agents: Vec<agents::SubAgent> = if multi_agent_enabled {
        agents::load_configured_agents(&state.agents_config_path)
            .agents
            .into_iter()
            .filter(|a| a.enabled)
            .collect()
    } else {
        Vec::new()
    };

    // Soft narrowing: the system prompt lists only the agents the selector
    // picked for this turn. The `call_subagent` executor still accepts any
    // enabled agent id below, so one bad pick cannot lock the right agent out.
    let advertised_agents: Vec<agents::SubAgent> = {
        let selected_ids: Vec<&str> = orchestration
            .as_ref()
            .and_then(|outcome| outcome.selection.as_ref())
            .filter(|selection| selection.delegate)
            .map(|selection| {
                selection
                    .choices
                    .iter()
                    .map(|choice| choice.id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let narrowed: Vec<agents::SubAgent> = enabled_agents
            .iter()
            .filter(|agent| selected_ids.iter().any(|id| *id == agent.id))
            .cloned()
            .collect();
        if narrowed.is_empty() {
            enabled_agents.clone()
        } else {
            narrowed
        }
    };

    if !advertised_agents.is_empty() {
        let mut available_agents_info = String::new();
        for agent in &advertised_agents {
            let kind = agents::normalize_agent_type(&agent.agent_type);
            let detail = match kind.as_str() {
                agents::AGENT_TYPE_CLI => format!(" (cli: {})", agent.cli.command.trim()),
                agents::AGENT_TYPE_CLOUD => format!(" (cloud: {})", agent.cloud.endpoint.trim()),
                _ => {
                    let mcp = if agent.enable_mcp { " + MCP" } else { "" };
                    format!(
                        " (model {}{})",
                        agent
                            .model
                            .clone()
                            .unwrap_or_else(|| "main".to_string()),
                        mcp
                    )
                }
            };
            available_agents_info.push_str(&format!(
                "- {} [{}] {} — {}{}\n",
                agent.id, kind, agent.name, agent.description, detail
            ));
        }
        let mut agents_sys_msg = format!(
        "Agents you can reach with `call_subagent` (exact `agent_id`, self-contained `task_description` — they never see this conversation). Types: `llm` runs an autonomous tool loop inside the app; `cli` is a headless coding CLI on this machine that edits workspace files directly; `cloud` runs the task on a remote endpoint. Prefer `llm` for reasoning/reporting, `cli`/`cloud` for large end-to-end coding; a first `cli`/`cloud` run may need the user's approval.\n\
        Workspace root: {} — pass a workspace-relative `cwd` (e.g. \"./apps/web\") when the task targets a subfolder, omit it for the root.\n\n\
        Available agents:\n{}",
        workspace_dir_for_roots.display(),
        available_agents_info
    );

        // Explain the selection so the main agent writes task descriptions that
        // match the picked agents' purpose instead of guessing.
        if let Some(selection) = orchestration
            .as_ref()
            .and_then(|outcome| outcome.selection.as_ref())
            .filter(|selection| selection.delegate && !selection.choices.is_empty())
        {
            agents_sys_msg.push_str(
                "\n<agent-selection>\nPicked for this turn (the others stay callable if the work really needs them):\n",
            );
            for choice in &selection.choices {
                if choice.why.is_empty() {
                    agents_sys_msg.push_str(&format!("- {} ({})\n", choice.name, choice.role));
                } else {
                    agents_sys_msg.push_str(&format!(
                        "- {} ({}): {}\n",
                        choice.name, choice.role, choice.why
                    ));
                }
            }
            agents_sys_msg.push_str("</agent-selection>");
        }

        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&agents_sys_msg);
    }

    // A declined selection is not a ban — just a hint that the cheap path is to
    // answer directly. Keep it short so it does not become a second policy.
    if matches!(
        orchestration
            .as_ref()
            .and_then(|outcome| outcome.selection.as_ref()),
        Some(selection) if !selection.delegate
    ) {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(
            "The orchestrator decided this request needs no sub-agents. Answer directly; delegate only if the request clearly requires it.",
        );
    }

    // ── Memory guidance ─────────────────────────────────────────────────────
    // Explicit instructions so conclusions are reliably persisted via the
    // `memory` tool (the tool description carries the full scope/usage details).
    if config.effective_tools().iter().any(|t| t == "memory") {
        let evolution_hint = if config.self_evolution_mode {
            ", `evolution` for verified facts that must survive across sessions (runtime \
             environment, correct tool usage, file locations)"
        } else {
            ""
        };
        let memory_guidance = format!(
            "\
        MEMORY: You have a `memory` tool (scopes: `session`, `user`, `repo`{}). \
        Nothing is preloaded — look memory up on demand: search it when you hit a problem \
        you may have solved before, or need a verified environment/tool fact. \
        When you reach a definitive, accurate conclusion (a resolved question, confirmed \
        root cause, decided approach, verified fact), `memory add` it before ending your \
        turn: `repo` for codebase facts and decisions, `user` for preferences and general \
        insights, `session` for in-progress notes. Short descriptive `key`, conclusion in \
        `content`.",
            evolution_hint
        );
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(&memory_guidance);
    }

    // ── Timer guidance ──────────────────────────────────────────────────────
    // The tool description carries the mechanics; this states the *policy* so
    // the model waits asynchronously instead of burning a turn on polling.
    if config.selected_tools.iter().any(|t| t == "timer") {
        let timer_guidance = "\
        TIMER: `timer_set` / `timer_list` / `timer_cancel` cover work that cannot finish within this \
        turn. When the next step depends on something slow (log collection, build, download, \
        deployment, external job, rate-limit window), never busy-wait, `sleep` or poll in a loop: \
        start the slow work, arm a timer whose `message` restates what to do when it fires (that turn \
        sees only the history plus that message), tell the user when it will fire and END YOUR TURN. \
        Check `timer_list` first to avoid duplicates, `timer_cancel` when the wait is no longer needed.";
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(timer_guidance);
    }

    // ── Todo guidance ───────────────────────────────────────────────────────
    // The tool description carries the mechanics; this states the *policy* —
    // check the session's list before planning so work continues from the
    // existing plan instead of silently starting a new one.
    if config
        .selected_tools
        .iter()
        .any(|t| t == "todo_list" || t == "todo")
    {
        let todo_guidance = "\
        TODOS: the `todos` tool holds this session's plan. Before planning or starting \
        non-trivial work, `list` it — the active list is also injected as internal context \
        while it has unfinished items. Continue from an existing list and move items \
        `pending` → `in_progress` → `completed` as you work; `add` items when a new \
        multi-step task begins. The UI mirrors every change live, so never leave stale \
        items behind.";
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(todo_guidance);
    }

    // ── Message layout (prompt-cache conscious) ─────────────────────────────
    // Static prefix first (system prompt, skill context), then the persisted
    // summary (stable across turns — only a new compression rewrites it), then
    // the append-only conversation history. Dynamic per-turn blocks (todo
    // list, mission report) come last, right before the latest user message,
    // so turn-to-turn changes invalidate as little cached prefix as possible.
    if !system_content.is_empty() {
        all_messages.push(json!({ "role": "system", "content": system_content }));
    }

    if !loaded_skills_content.is_empty() {
        let workspace_dir = state.workspace_dir.lock().unwrap().clone();
        let workspace_root_display = workspace_dir.display().to_string();
        // Generic sandbox rules live in the tool descriptions (always sent);
        // only skill-specific notes are repeated here.
        let active_skills_context = format!(
            "INTERNAL CONTEXT - ACTIVE SKILLS (root: {workspace_root_display}). \
             Local file tools are sandboxed to the workspace root; `skill_read` is locked to each \
             skill's own directory (`..` rejected); skill edits are limited to `./.data/skills/...` \
             and create `.bak.<n>` backups automatically; MCP tool paths are NOT sandboxed — treat \
             them as opaque. Reuse exact paths returned by tools; never reference app-data or \
             external absolute paths.\n\n\
             Active skill instructions:{}",
            loaded_skills_content
        );
        all_messages.push(json!({ "role": "user", "content": active_skills_context }));
    }

    // Conversation history, keeping the latest user message for the tail.
    let (mut history, last_user_msg) = match messages.last() {
        Some(m) if m.role == "user" => (&messages[..messages.len() - 1], Some(m)),
        _ => (&messages[..], None),
    };

    // ── Persisted session summary: replace the history it covers ────────────
    // A summary written by an earlier compression records how many frontend
    // messages it covers plus the flattened prefix of the last one (the
    // boundary). When the incoming history still matches that boundary, the
    // covered prefix is dropped and the summary stands in for it — Claude
    // Code's compact semantics, where the summary REPLACES the old transcript
    // instead of being replayed on top of it. On any mismatch (history edited,
    // truncated, restored differently) the stale summary is discarded and the
    // full history is replayed — the old, always-safe additive behavior.
    let mut summary_block: Option<db::SessionSummaryRow> = None;
    let mut summary_coverage = CoverageState::default();
    if let Ok(db_guard) = state.db.lock() {
        match db::get_session_summary(&db_guard, &session_id) {
            Ok(Some(saved)) if !saved.body.trim().is_empty() => {
                let covered = usize::try_from(saved.covered_messages).unwrap_or(0);
                let mut valid = true;
                if covered > 0 {
                    let boundary_message_text = covered
                        .checked_sub(1)
                        .and_then(|idx| history.get(idx))
                        .map(|m| content_to_text(&m.content));
                    match summary_trim_count(
                        covered,
                        &saved.boundary_text,
                        boundary_message_text.as_deref(),
                    ) {
                        Some(count) => {
                            history = &history[count..];
                            summary_coverage.offset = count;
                            summary_coverage.boundary = saved.boundary_text.clone();
                        }
                        None => valid = false,
                    }
                }
                if valid {
                    summary_block = Some(saved);
                } else {
                    let _ = db::delete_session_summary(&db_guard, &session_id);
                    log_event(
                        &state,
                        "INFO",
                        format!(
                            "stale session summary discarded (history boundary mismatch): session_id={session_id}"
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    // The summary goes BEFORE the remaining history: it is stable across
    // turns (only a new compression rewrites it), so everything after it is
    // an append-only region — the best layout for the prompt cache. Claude
    // Code places the compact summary at the start of the continuation for
    // the same reason.
    if let Some(saved) = &summary_block {
        all_messages.push(json!({
            "role": "user",
            "content": render_summary_message_body(&saved.body)
        }));
    }

    for m in history {
        let mut msg = json!({ "role": m.role, "content": m.content });
        if let Some(rc) = &m.reasoning_content {
            if !rc.is_empty() {
                msg["reasoning_content"] = json!(rc);
            }
        }
        all_messages.push(msg);
    }

    // ── Dynamic blocks (change between turns → placed near the tail) ────────

    // Inject the active todo list (if any) so the assistant can always
    // re-orient itself to the current plan, even mid-turn.
    // The same snapshot is pushed to the UI: the panel is remounted per
    // assistant message and only the newest one mirrors the session list, so
    // without it a list created in an earlier turn would stay invisible until
    // the model happened to touch it again.
    let todo_snapshot = match state.workspace_dir.lock() {
        Ok(workspace_guard) => todos::active_list_snapshot(&workspace_guard, &session_id)
            .ok()
            .flatten(),
        Err(_) => None,
    };
    if let Some(summary) = &todo_snapshot {
        if !summary.todos.is_empty() {
            // Mirror the very same snapshot to the UI. Emitted together with
            // the prompt block on purpose, and also for a finished list: the
            // panel must be able to disappear (or never appear) instead of
            // lingering at 100% from an earlier turn.
            let _ = app.emit(
                "todo-state",
                json!({
                    "type": "todo_state",
                    "list_id": summary.list_id,
                    "session_id": summary.session_id,
                    "summary": summary,
                }),
            );
        }
        // Nothing left to track → no prompt block either.
        if let Some(todo_block) = todos::render_list_block(summary) {
            all_messages.push(json!({
                "role": "system",
                "content": format!("{CONTEXT_TODO_MARKER}\n{}", todo_block)
            }));
        }
    }

    // Inject the finished multi-agent mission report so the main agent can
    // define the final answer with its full context and tools.
    if let Some(report) = orchestration
        .as_ref()
        .and_then(|outcome| outcome.report.as_ref())
    {
        all_messages.push(json!({
            "role": "system",
            "content": format!("{CONTEXT_MISSION_MARKER}\n{report}")
        }));
    }

    if let Some(m) = last_user_msg {
        all_messages.push(json!({ "role": m.role, "content": m.content }));
    }

    let url = format!(
        "{}/chat/completions",
        config.api_base_url.trim_end_matches('/')
    );
    let client = build_http_client()?;
    // Resolved once per turn so every round and retry of this session carries
    // the same conversation id (and the same custom headers).
    let request_headers = upstream_request_headers(&config, &session_id);

    let active_model = model_override
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| config.model.clone());

    // Everything the context compressor needs; created once per turn so the
    // summary call uses the same model, headers and cancel flag as the chat.
    let compressor = ContextCompressor {
        app: &app,
        client: &client,
        url: &url,
        api_key: &config.api_key,
        model: &active_model,
        ds_format: config.ds_format_for(&active_model),
        headers: &request_headers,
        cancelled: turn_cancelled.as_ref(),
    };
    // The reactive (context-overflow) compression may retry the request once
    // per turn; the flag keeps that retry from looping.
    let mut reactive_compress_used = false;
    // Set when a predictive compression ran but the estimate stayed above the
    // threshold — the static payload (system prompt + tool definitions) is
    // itself near the cap, so further pre-checks would only burn summary
    // calls. Reactive / post-response paths still cover the rest of the turn.
    let mut predictive_compress_saturated = false;

    let (mut tools_list, mut mcp_tool_map, mut tools_revision) = build_turn_tools(
        &state,
        &config,
        &active_skill_dirs,
        !skill_ids.is_empty(),
        skill_evolution_root.as_deref(),
        &enabled_agents,
    )
    .await;

    // Track the final answer and whether the LLM already persisted memory via
    // the `memory` tool, so the automatic fallback doesn't duplicate it.
    let mut final_answer: Option<String> = None;
    let mut memory_tool_called = false;

    // Skills loaded dynamically during this turn (the model can request the
    // same skill again in a later tool-call round of the same turn).
    let mut loaded_this_turn: std::collections::HashSet<String> = std::collections::HashSet::new();
    // How many times this turn has been held open for background results.
    let mut background_hold_rounds: u32 = 0;

    // ── Turn budget & tool failure guard ────────────────────────────────────
    // The loop below is model-driven: it repeats for as long as the reply
    // carries tool calls. `TurnBudget` bounds that (rounds + tool calls) and
    // `ToolFailureGuard` disables a tool for the rest of the session after
    // repeated failures. Both limits come from Settings → Advanced; `0` means
    // "no limit". See `turn_guard`.
    let mut budget = turn_guard::TurnBudget::new(
        config.main_agent_max_rounds,
        config.main_agent_max_tool_calls,
    );
    let failure_limit = config
        .tool_failure_limit
        .min(turn_guard::MAX_TOOL_FAILURE_LIMIT);
    let guard = &state.tool_guard;
    // `budget_stop` is set once a limit is spent; `wrap_up` marks the single
    // final request that goes out with `tool_choice: "none"`.
    let mut budget_stop: Option<turn_guard::BudgetStop> = None;
    let mut wrap_up = false;

    // ── Tools disabled by earlier turns ─────────────────────────────────────
    // The failure guard remembers its verdict per session, so a tool that hit
    // the limit in an earlier turn is not offered again. The snapshot is taken
    // ONCE per turn on purpose: `tools` sits in front of the messages in the
    // rendered prompt, so adding or removing a definition mid-turn would move
    // every following token and throw away the KV / prefix cache the previous
    // rounds built. Tools that get disabled *during* this turn therefore stay
    // advertised until the next turn; their calls are refused in the tool loop
    // below (`blocked_notice`) instead of being hidden.
    let disabled_at_turn_start = guard.disabled_tools(&session_id);
    let drop_disabled_tools = |list: &mut Vec<Value>| {
        if disabled_at_turn_start.is_empty() {
            return;
        }
        list.retain(|tool| {
            tool["function"]["name"]
                .as_str()
                .map(|name| !disabled_at_turn_start.contains(name))
                .unwrap_or(true)
        });
    };
    drop_disabled_tools(&mut tools_list);

    loop {
        // ── MCP registry changed during this turn? ───────────────────────────
        // `mcp_self` can create and enable a server while the turn is running.
        // Rebuilding the payload before the next request is what makes the new
        // tools callable *now*; without it the model would have to wait for the
        // next conversation turn, which reads like "the server was created but
        // its tools do not exist".
        let registry_revision = state.mcp_revision();
        if registry_revision != tools_revision {
            let rebuilt = build_turn_tools(
                &state,
                &config,
                &active_skill_dirs,
                !skill_ids.is_empty(),
                skill_evolution_root.as_deref(),
                &enabled_agents,
            )
            .await;
            tools_list = rebuilt.0;
            mcp_tool_map = rebuilt.1;
            tools_revision = rebuilt.2;
            // A rebuild resurrects every tool the session disabled in an
            // earlier turn; the snapshot taken at turn start still applies.
            drop_disabled_tools(&mut tools_list);
            log_event(
                &state,
                "INFO",
                format!(
                    "MCP tool payload rebuilt mid-turn: session_id={}, revision={tools_revision}, tools={}",
                    session_id,
                    tools_list.len()
                ),
            );
        }

        // Sanitize tool call pairs before each request to prevent the API error:
        // "Messages with role 'tool' must be a response to a preceding message with 'tool_calls'"
        sanitize_tool_pairs(&mut all_messages);

        // ── Mid-turn injections ──────────────────────────────────────────────
        // The user can append new information while the assistant is working
        // (external state changed, a wrong path, a new constraint). A request
        // boundary is the only safe place: an `assistant` message carrying
        // `tool_calls` must stay adjacent to its `tool` results, so the injected
        // text goes AFTER the whole round, right before the next request.
        //
        // A turn whose last request was the final answer never reaches another
        // boundary — those items stay in the inbox, the window closes with the
        // turn, and the UI sends the text as a normal next-turn message.
        let injections = state.drain_turn_injections(injection_window.turn_id);
        if !injections.is_empty() {
            for item in &injections {
                all_messages.push(json!({
                    "role": "user",
                    "content": format!("{}\n\n{}", MID_TURN_INJECTION_PREFIX, item.text),
                }));
            }
            log_event(
                &state,
                "INFO",
                format!(
                    "mid-turn injection delivered: session_id={}, count={}",
                    session_id,
                    injections.len()
                ),
            );
            // Only now is the text actually part of a request. The UI keeps
            // showing an item as "queued" until this event lands, so anything
            // that never reached a request is re-sent as a normal turn instead
            // of vanishing.
            let _ = app.emit(
                "chat-injected",
                json!({
                    "items": injections
                        .iter()
                        .map(|item| json!({
                            "id": item.id,
                            "text": item.text,
                            "created_at_ms": item.created_at_ms,
                        }))
                        .collect::<Vec<_>>(),
                }),
            );
        }

        // ── Background task results ──────────────────────────────────────────
        // A background tool call returned a placeholder immediately (the
        // protocol has no slot for a late `tool` message). Its real output is
        // delivered here, at a request boundary, as a `user` message — and
        // announced with `chat-injected` so the UI stores it on the assistant
        // row and replays it in later turns exactly like a mid-turn injection.
        let background_results = state.background_tasks.take_deliverable(&session_id);
        if !background_results.is_empty() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or(0);
            let mut items = Vec::with_capacity(background_results.len());
            for delivery in &background_results {
                let text = format_background_delivery(delivery);
                all_messages.push(json!({
                    "role": "user",
                    "content": text.clone(),
                }));
                items.push(json!({
                    "id": delivery.id,
                    "text": text,
                    "created_at_ms": now_ms,
                    "kind": "background_result",
                }));
            }
            log_event(
                &state,
                "INFO",
                format!(
                    "background results delivered: session_id={}, count={}",
                    session_id,
                    background_results.len()
                ),
            );
            let _ = app.emit("chat-injected", json!({ "items": items }));
        }

        // ── Turn budget ──────────────────────────────────────────────────────
        // Count the request that is about to be sent. When the round or
        // tool-call budget is spent the turn does not die mid-flight: one last
        // request goes out with `tool_choice: "none"`, so the model has to
        // answer with what it already collected instead of asking for more.
        if budget_stop.is_none() {
            budget_stop = budget.begin_round();
        }
        if let Some(stop) = budget_stop {
            if !wrap_up {
                wrap_up = true;
                let (rounds, tool_calls) = budget.snapshot();
                let reason = if stop == turn_guard::BudgetStop::ToolCalls {
                    "the tool-call limit was reached"
                } else {
                    "the round limit was reached"
                };
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "turn budget exhausted: session_id={}, reason={}, rounds={}, tool_calls={}",
                        session_id,
                        stop.as_str(),
                        rounds,
                        tool_calls
                    ),
                );
                let _ = app.emit(
                    "chat-notice",
                    json!({
                        "kind": "budget_exhausted",
                        "reason": stop.as_str(),
                        "rounds": rounds,
                        "tool_calls": tool_calls,
                    }),
                );
                // Appended at the tail as a `user` message: the prompt prefix
                // stays byte-identical (only new tokens are uncached), and no
                // strict chat template has to accept a `system` turn appearing
                // after `tool` results.
                all_messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "{CONTEXT_BUDGET_MARKER}\nThe tool budget for this turn is spent \
({rounds} request(s), {tool_calls} tool call(s); {reason}). Do not call any tool. Answer now with \
the best final answer you can give from the material already collected, and state plainly what is \
still missing."
                    ),
                }));
            }
        }

        // Context window used for the usage gauge (`Tokens: X / Y`) and the
        // compression threshold. Same resolver as sub-agents: only the
        // per-model window counts, never an output-token cap.
        let effective_max_tokens =
            resolve_context_window(&config, &active_model, DEFAULT_MODEL_MAX_TOKENS);

        // ── Predictive compression pre-check ────────────────────────────────
        // The post-response ratio check can only fire once a request came
        // back with usage figures. A single request carrying large tool
        // results can still jump past the window and be rejected outright.
        // Estimate the prompt size (≈4 chars/token, tools payload included)
        // and compress BEFORE sending when the estimate approaches the cap.
        if !predictive_compress_saturated {
            let estimated_prompt_tokens = estimate_prompt_tokens(&all_messages, &tools_list);
            let precheck_threshold =
                (effective_max_tokens as f32 * CONTEXT_PRECHECK_RATIO) as usize;
            if estimated_prompt_tokens >= precheck_threshold {
                if let Some(outcome) = compressor
                    .compress(
                        &mut all_messages,
                        CONTEXT_KEEP_RECENT_MESSAGES,
                        &summary_coverage,
                    )
                    .await
                {
                    persist_session_summary(&state, &session_id, &outcome);
                    summary_coverage.offset = outcome.covered_messages;
                    summary_coverage.boundary = outcome.boundary_text;
                    log_event(
                        &state,
                        "INFO",
                        format!(
                            "predictive context compression: session_id={}, estimated_prompt_tokens={}, max_tokens={}",
                            session_id, estimated_prompt_tokens, effective_max_tokens
                        ),
                    );
                    // If the estimate is still above the threshold after
                    // summarizing everything summarizable, the remaining bulk
                    // is static — stop pre-checking this turn.
                    if estimate_prompt_tokens(&all_messages, &tools_list) >= precheck_threshold {
                        predictive_compress_saturated = true;
                        log_event(
                            &state,
                            "WARN",
                            format!(
                                "predictive compression saturated (static payload dominates the window): session_id={session_id}"
                            ),
                        );
                    }
                }
            }
        }

        let mut req_body = json!({
            "model": active_model,
            "messages": all_messages,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        // Never strip `tools` from a request. The array is rendered in front of
        // the messages, so removing it would move every following token and
        // throw away the whole prompt cache for this request. The wrap-up
        // request keeps the identical list and only flips `tool_choice` to
        // "none", which forbids calling them while the prefix stays cached.
        if !tools_list.is_empty() {
            req_body["tools"] = json!(tools_list);
            req_body["tool_choice"] = if wrap_up {
                json!("none")
            } else {
                json!("auto")
            };
        }
        if let Some(temp) = config.model_settings.temperature {
            req_body["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.model_settings.top_p {
            req_body["top_p"] = json!(top_p);
        }
        if let Some(effort) = resolve_reasoning_effort(&config, &active_model) {
            req_body["reasoning_effort"] = json!(effort);
        }

        apply_completion_token_limit(
            &mut req_body,
            config.model_settings.max_complete_tokens,
            config.model_settings.max_tokens,
        );

        let started_at = std::time::Instant::now();
        let request_snapshot = req_body.clone();

        let result = stream_request(
            &app,
            &client,
            &url,
            &config.api_key,
            req_body,
            &turn_cancelled,
            Some(effective_max_tokens),
            config.ds_format_for(&active_model),
            &request_headers,
        )
        .await;

        let duration_ms = started_at.elapsed().as_millis() as i64;

        match &result {
            Ok(sr) => {
                // Log successful request
                let tool_calls_json = if sr.tool_calls.is_empty() {
                    String::new()
                } else {
                    serde_json::to_string(
                        &sr.tool_calls
                            .iter()
                            .map(|(id, name, args)| {
                                json!({
                                    "id": id, "name": name, "arguments": args
                                })
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap_or_default()
                };
                let db = state.db.lock().unwrap();
                // `request_body` is deliberately not written: the full prompt is
                // already recorded once in `interaction_log.input_data`, and
                // storing a second copy doubled the database size
                // (~92 KiB per request). This table keeps the metrics.
                let _ = db.execute(
                    "INSERT INTO api_requests (session_id, model, response_content, reasoning_content, tool_calls, finish_reason, prompt_tokens, completion_tokens, cached_tokens, reasoning_ms, first_token_ms, duration_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    rusqlite::params![
                        session_id,
                        active_model.as_str(),
                        sr.content,
                        sr.reasoning_content,
                        tool_calls_json,
                        sr.finish_reason,
                        sr.prompt_tokens,
                        sr.completion_tokens,
                        sr.cached_tokens.map(i64::from),
                        sr.timing.reasoning_ms.map(i64::from),
                        sr.timing.first_token_ms.map(i64::from),
                        duration_ms,
                    ],
                );
                // Log to interaction_log table
                let request_body_display = request_snapshot.to_string();
                let response_display = sr.content.clone();
                // Cache-hit metrics travel in the interaction metadata so the
                // Monitor detail view (and the list chip) can show them without
                // touching the interaction schema. The round's streamed phase
                // timings ride along so the detail pane can explain the wait.
                let usage_metadata = {
                    let mut meta = serde_json::Map::new();
                    if let Some(cached) = sr.cached_tokens {
                        meta.insert("prompt_tokens".into(), json!(sr.prompt_tokens));
                        meta.insert("cached_tokens".into(), json!(cached));
                        meta.insert(
                            "hit_ratio".into(),
                            json!(if sr.prompt_tokens > 0 {
                                ((cached as f64) / (sr.prompt_tokens as f64)).clamp(0.0, 1.0)
                            } else {
                                0.0
                            }),
                        );
                    }
                    if let Some(ms) = sr.timing.reasoning_ms {
                        meta.insert("reasoning_ms".into(), json!(ms));
                    }
                    if let Some(ms) = sr.timing.first_token_ms {
                        meta.insert("first_token_ms".into(), json!(ms));
                    }
                    (!meta.is_empty()).then(|| Value::Object(meta).to_string())
                };
                log_interaction(
                    &db,
                    &session_id,
                    "llm_api",
                    &active_model,
                    "chat_completion",
                    request_body_display,
                    response_display,
                    None,
                    duration_ms,
                    usage_metadata,
                );
                drop(db);
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "chat_completion request succeeded: session_id={}, finish_reason={}, duration_ms={}",
                        session_id, sr.finish_reason, duration_ms
                    ),
                );
            }
            Err(err) => {
                // Log failed request (metrics only — see the success branch for
                // why the prompt body is not duplicated here).
                let db = state.db.lock().unwrap();
                let _ = db.execute(
                    "INSERT INTO api_requests (session_id, model, finish_reason, duration_ms, error) \
                     VALUES (?1, ?2, 'error', ?3, ?4)",
                    rusqlite::params![
                        session_id,
                        active_model.as_str(),
                        duration_ms,
                        err,
                    ],
                );
                // Log to interaction_log table
                let request_body_display = request_snapshot.to_string();
                log_interaction(
                    &db,
                    &session_id,
                    "llm_error",
                    &active_model,
                    "chat_completion",
                    request_body_display,
                    String::new(),
                    Some(err.clone()),
                    duration_ms,
                    None,
                );
                drop(db);
                log_event(
                    &state,
                    "ERROR",
                    format!(
                        "chat_completion request failed: session_id={}, duration_ms={}, error={}",
                        session_id, duration_ms, err
                    ),
                );
            }
        }

        let sr = match result {
            Ok(sr) => sr,
            // A rejected wrap-up request (some gateways refuse
            // `tool_choice: "none"`) must not fail the whole turn: close it with
            // the answer the model already produced instead of an error.
            Err(err) if wrap_up => {
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "wrap-up request failed, ending the turn: session_id={}, error={err}",
                        session_id
                    ),
                );
                final_answer = Some(BUDGET_FALLBACK_ANSWER.to_string());
                break;
            }
            Err(err) => {
                // ── Reactive compression (context-overflow recovery) ────────
                // Last line of defense, mirroring Claude Code's 413 recovery:
                // a provider that hard-rejects an over-window request instead
                // of streaming an answer. Compress with a smaller keep-window
                // and retry once — the flag keeps the retry from looping.
                if !reactive_compress_used && is_context_overflow_error(&err) {
                    reactive_compress_used = true;
                    log_event(
                        &state,
                        "WARN",
                        format!(
                            "context overflow from provider, compressing and retrying once: session_id={}, error={err}",
                            session_id
                        ),
                    );
                    if let Some(outcome) = compressor
                        .compress(
                            &mut all_messages,
                            CONTEXT_REACTIVE_KEEP_RECENT_MESSAGES,
                            &summary_coverage,
                        )
                        .await
                    {
                        persist_session_summary(&state, &session_id, &outcome);
                        summary_coverage.offset = outcome.covered_messages;
                        summary_coverage.boundary = outcome.boundary_text;
                        continue;
                    }
                }
                return Err(err);
            }
        };

        let total_tokens = sr.prompt_tokens.saturating_add(sr.completion_tokens);
        let ratio = total_tokens as f32 / effective_max_tokens as f32;
        let mut compressed_this_turn = false;
        if ratio >= CONTEXT_COMPRESSION_THRESHOLD {
            if let Some(outcome) = compressor
                .compress(
                    &mut all_messages,
                    CONTEXT_KEEP_RECENT_MESSAGES,
                    &summary_coverage,
                )
                .await
            {
                persist_session_summary(&state, &session_id, &outcome);
                summary_coverage.offset = outcome.covered_messages;
                summary_coverage.boundary = outcome.boundary_text;
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "session context compressed: session_id={}, total_tokens={}, max_tokens={}, ratio={:.3}, summary_covers={} message(s)",
                        session_id, total_tokens, effective_max_tokens, ratio, outcome.covered_messages
                    ),
                );
                let _ = app.emit(
                    "chat-token",
                    "\n\n[System] Context compressed (older messages summarized) and persisted for next turns.\n\n",
                );
                compressed_this_turn = true;
            }
        }

        if sr.finish_reason == "cancelled" {
            break;
        }

        // ── Wrap-up answer ───────────────────────────────────────────────────
        // The budget was spent before this request, so it went out with
        // `tool_choice: "none"`: whatever comes back is the answer for this
        // turn. Never hold the turn open again for background results here —
        // that is what `background_hold_rounds` would otherwise do.
        if wrap_up {
            final_answer = Some(if sr.content.trim().is_empty() {
                BUDGET_FALLBACK_ANSWER.to_string()
            } else {
                sr.content.clone()
            });
            break;
        }

        // After compression, the previous response was generated against the
        // full (uncompressed) context. If that response was a final answer
        // (no tool calls), the task is *not* yet finished — re-enter the loop
        // so the LLM can re-evaluate against the compressed context and
        // continue. When the response carries tool calls, fall through to
        // execute them; the next iteration will naturally use the compressed
        // context.
        if compressed_this_turn && (sr.finish_reason != "tool_calls" || sr.tool_calls.is_empty()) {
            if !sr.content.is_empty() {
                let mut assistant_msg = json!({
                    "role": "assistant",
                    "content": sr.content,
                });
                if !sr.reasoning_content.is_empty() {
                    assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
                }
                all_messages.push(assistant_msg);
            }
            all_messages.push(json!({
                "role": "user",
                "content": format!(
                    "{CONTEXT_CONTINUE_MARKER}\nContext was just compressed to free up space. Review the session summary and your previous response, then continue the task if it is not yet complete; otherwise provide your final answer."
                ),
            }));
            log_event(
                &state,
                "INFO",
                format!(
                    "context compressed — re-calling LLM to continue task: session_id={}",
                    session_id
                ),
            );
            continue;
        }

        if sr.finish_reason != "tool_calls" || sr.tool_calls.is_empty() {
            // A background task started earlier in this turn may produce its
            // result only after this answer was generated. Delivering a final
            // answer that ignores that output would be wrong, so the turn is
            // held open: the text is kept as an interim assistant message, the
            // results are drained at the top of the next iteration and the
            // model gets one more request to incorporate them.
            if background_hold_rounds < BACKGROUND_HOLD_MAX_ROUNDS
                && !turn_cancelled.load(Ordering::SeqCst)
                && state.background_tasks.unsettled(&session_id) > 0
            {
                let pending = state.background_tasks.unsettled(&session_id);
                if !sr.content.is_empty() {
                    let mut assistant_msg = json!({
                        "role": "assistant",
                        "content": sr.content,
                    });
                    if !sr.reasoning_content.is_empty() {
                        assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
                    }
                    all_messages.push(assistant_msg);
                }
                background_hold_rounds += 1;
                log_event(
                    &state,
                    "INFO",
                    format!(
                        "holding turn for background tasks: session_id={}, pending={}, round={}",
                        session_id, pending, background_hold_rounds
                    ),
                );
                let _ = app.emit(
                    tools::background::BACKGROUND_WAITING_EVENT,
                    json!({
                        "session_id": session_id,
                        "pending": pending,
                        "round": background_hold_rounds,
                    }),
                );
                state
                    .background_tasks
                    .wait_until_idle(&session_id, Duration::from_secs(BACKGROUND_HOLD_WAIT_SECS))
                    .await;
                continue;
            }

            if state.background_tasks.unsettled(&session_id) > 0 {
                // Held as long as allowed: drop the rest instead of hanging
                // the turn forever.
                let cancelled = state.background_tasks.cancel_session(&session_id);
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "background hold budget exhausted: session_id={}, cancelled={}",
                        session_id,
                        cancelled.len()
                    ),
                );
                // Tell the UI now: these tasks never get a runner report the
                // user could see (the turn is ending right after).
                for snapshot in cancelled {
                    let _ = app.emit(
                        tools::background::BACKGROUND_TASK_STATE_EVENT,
                        snapshot,
                    );
                }
            }

            final_answer = Some(sr.content.clone());
            break;
        }

        let assistant_tcs: Vec<Value> = sr
            .tool_calls
            .iter()
            .map(|(id, name, args)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect();
        let mut assistant_msg = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": assistant_tcs
        });
        if !sr.reasoning_content.is_empty() {
            assistant_msg["reasoning_content"] = json!(sr.reasoning_content);
        }
        all_messages.push(assistant_msg);

        let mut pending_skill_context_messages: Vec<Value> = vec![];

        for (id, name, args) in &sr.tool_calls {
            if turn_cancelled.load(Ordering::SeqCst) {
                break;
            }
            if name == "memory" {
                memory_tool_called = true;
            }
            let background_args: Value = serde_json::from_str(args).unwrap_or_default();

            // ── Failure guard / tool budget ──────────────────────────────────
            // Two independent gates can keep this call from running:
            //   1. the tool was disabled earlier in this session (its
            //      consecutive-failure budget is spent), or
            //   2. the turn's tool-call budget is spent.
            // Either way the call is still *answered*: the protocol requires a
            // `tool` message for every `tool_call_id` the assistant produced.
            let blocked = match guard.blocked_notice(&session_id, name, failure_limit) {
                Some(notice) => Some(notice),
                None => match budget.record_tool_call() {
                    Some(stop) => {
                        budget_stop = Some(stop);
                        Some(turn_guard::budget_notice(name))
                    }
                    None => None,
                },
            };
            let ran = blocked.is_none();
            if let Some(notice) = &blocked {
                let reason = if notice.starts_with("⛔ TOOL DISABLED") {
                    "disabled for this session"
                } else {
                    "turn tool budget spent"
                };
                let _ = app.emit("tool-call", format!("🚫 *{name}* — not executed ({reason})"));
                log_event(
                    &state,
                    "WARN",
                    format!(
                        "tool call not executed: session_id={}, tool={}, reason={}",
                        session_id, name, reason
                    ),
                );
            }

            let mut result = if let Some(notice) = blocked {
                notice
            } else if name == BACKGROUND_TOOL_NAME {
                handle_background_task_tool(&app, &state, &session_id, &background_args).await
            } else if config.background_tools
                && background_requested(&background_args)
                && name != "call_subagent"
            {
                if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                    let mut mcp_args = background_args.clone();
                    if let Some(object) = mcp_args.as_object_mut() {
                        object.remove("run_in_background");
                    }
                    let label = format!(
                        "MCP [{}] {} · {}",
                        mcp_server.name,
                        actual_tool_name,
                        serde_json::to_string(&mcp_args)
                            .unwrap_or_default()
                            .chars()
                            .take(100)
                            .collect::<String>()
                    );
                    match start_background_mcp_call(
                        &app,
                        &session_id,
                        mcp_server.clone(),
                        actual_tool_name.clone(),
                        name,
                        mcp_args,
                        &label,
                    ) {
                        Ok(placeholder) => placeholder,
                        Err(err) => format!("Error: {err}"),
                    }
                } else if background_supported(name, &background_args) {
                    let mut bg_args = background_args.clone();
                    if let Some(object) = bg_args.as_object_mut() {
                        // The real tool must never see the routing flag.
                        object.remove("run_in_background");
                    }
                    let label = background_label(name, &bg_args);
                    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                    match start_background_tool_call(
                        &app,
                        &session_id,
                        name,
                        bg_args,
                        &label,
                        workspace_dir,
                        &config,
                        &skill_allowed_commands,
                        skill_evolution_root.clone(),
                        &active_skill_dirs,
                    ) {
                        Ok(placeholder) => placeholder,
                        Err(err) => format!("Error: {err}"),
                    }
                } else {
                    format!(
                        "Error: `run_in_background` is not supported by the tool '{name}'. \
                         Use it only with `run_cmd`, `run_shell` (run_type \"direct\"), \
                         `file_actions` action \"search\", MCP tools and `call_subagent`."
                    )
                }
            } else if name == "use_skill" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let skill_name = args_json["skill_name"].as_str().unwrap_or("");
                let ws_skills_dir = crate::workspace::skills_root(&workspace_dir_for_roots);

                if let Ok(resolved_skill) =
                    skills::resolve_skill_by_name(&ws_skills_dir, skill_name)
                {
                    let skill = resolved_skill.skill;
                    let dyn_marker = format!("--- Skill (Dynamically Loaded): {} ---", skill.name);

                    // Explicit "already loaded" state: recorded by earlier turns
                    // (`loaded_skills` on the message history) or loaded earlier
                    // in this same turn.
                    let already_loaded = activated_skills.contains(&skill.name)
                        || loaded_this_turn.contains(&skill.name);

                    if !already_loaded {
                        loaded_this_turn.insert(skill.name.clone());
                        merge_allowed_commands(
                            &mut skill_allowed_commands,
                            &skill.allowed_commands,
                        );
                        // Track skill dir for skill_read tool injection
                        if !active_skill_dirs.iter().any(|(n, _)| n == &skill.name) {
                            active_skill_dirs
                                .push((skill.name.clone(), resolved_skill.skill_dir.clone()));
                        }
                        let cmd_constraint = if skill.allowed_commands.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "\n[Allowed commands for this skill: {}]\n",
                                skill.allowed_commands.join(", ")
                            )
                        };
                        let skill_context = format!(
                            "INTERNAL CONTEXT - DYNAMICALLY LOADED SKILL (not a user request):\n\
{}\n[Skill root: {}]\n[Skill file: {}]\n{}{}",
                            dyn_marker,
                            resolved_skill.skill_dir.display(),
                            resolved_skill.skill_file.display(),
                            cmd_constraint,
                            skill.system_prompt
                        );
                        pending_skill_context_messages.push(json!({
                            "role": "user",
                            "content": skill_context
                        }));
                        // Structured state event: the UI records this on the
                        // assistant message and sends it back as
                        // `loaded_skills` on the next turn, so a skill is
                        // loaded at most once per session.
                        let _ = app.emit("skill-loaded", skill.name.clone());
                        format!(
                            "Skill '{}' loaded. Follow its instructions to fulfill the request.",
                            skill_name
                        )
                    } else {
                        format!("Skill '{}' already loaded.", skill_name)
                    }
                } else {
                    format!("Error: Skill '{}' not found.", skill_name)
                }
            } else if name == "call_subagent" {
                let args_json: Value = serde_json::from_str(args).unwrap_or_default();
                let agent_id = args_json["agent_id"].as_str().unwrap_or("");
                let task_desc = args_json["task_description"].as_str().unwrap_or("");
                let task_context = args_json["context"].as_str().unwrap_or("");

                if !multi_agent_enabled {
                    "Error: sub-agent delegation is disabled. Enable automatic orchestration (`use_agents`) in the Agents panel to use `call_subagent`.".to_string()
                } else if let Some(agent) = enabled_agents.iter().find(|a| a.id == agent_id) {
                    let task = agents::Task {
                        id: uuid::Uuid::new_v4().to_string(),
                        agent_id: agent_id.to_string(),
                        description: task_desc.to_string(),
                        context: task_context.to_string(),
                        cwd: agents::normalize_task_cwd(args_json["cwd"].as_str()),
                        dependencies: vec![],
                    };
                    let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                    // Deliverables are exchanged through `<workspace>/.data/tmp`.
                    // A cloud agent cannot reach the local workspace, so it gets
                    // no exchange directory.
                    let agent_type = agents::normalize_agent_type(&agent.agent_type);
                    let exchange = if matches!(
                        agent_type.as_str(),
                        agents::AGENT_TYPE_LLM | agents::AGENT_TYPE_CLI
                    ) {
                        match agents::exchange::ExchangeArea::create(
                            &workspace_dir,
                            std::slice::from_ref(&task.id),
                        ) {
                            Ok(area) => Some(area),
                            Err(err) => {
                                log_event(
                                    &state,
                                    "WARN",
                                    format!("deliverable exchange area unavailable: {err}"),
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let url = format!(
                        "{}/chat/completions",
                        config.api_base_url.trim_end_matches('/')
                    );
                    match build_http_client() {
                        Ok(client) => {
                            if config.background_tools && background_requested(&background_args) {
                                let agent = agent.clone();
                                let app_for_task = app.clone();
                                let session_for_task = session_id.clone();
                                let config_for_task = config.clone();
                                let skill_root_for_task = skill_evolution_root.clone();
                                let exchange_for_task = exchange.clone();
                                let label = format!(
                                    "{} · {}",
                                    agent.name,
                                    task_desc.chars().take(100).collect::<String>()
                                );
                                let preview: String =
                                    args_json.to_string().chars().take(300).collect();
                                start_background_task(
                                    &app,
                                    &session_id,
                                    "call_subagent",
                                    &label,
                                    &preview,
                                    move |cancel| async move {
                                        let res = agents::run_sub_agent(
                                            &app_for_task,
                                            &client,
                                            &url,
                                            &config_for_task,
                                            &session_for_task,
                                            &agent,
                                            &task,
                                            workspace_dir,
                                            exchange_for_task.as_ref(),
                                            skill_root_for_task,
                                            cancel,
                                            None,
                                        )
                                        .await;
                                        format_subagent_delivery(&res, exchange_for_task.as_ref())
                                    },
                                )
                                .unwrap_or_else(|err| format!("Error: {err}"))
                            } else {
                                let res = agents::run_sub_agent(
                                    &app,
                                    &client,
                                    &url,
                                    &config,
                                    &session_id,
                                    agent,
                                    &task,
                                    workspace_dir,
                                    exchange.as_ref(),
                                    skill_evolution_root.clone(),
                                    turn_cancelled.clone(),
                                    None,
                                )
                                .await;

                                format_subagent_delivery(&res, exchange.as_ref())
                            }
                        }
                        Err(err) => format!("Error: {err}"),
                    }
                } else {
                    format!(
                        "Error: Agent with ID '{}' not found or not enabled.",
                        agent_id
                    )
                }
            } else if let Some((mcp_server, actual_tool_name)) = mcp_tool_map.get(name) {
                let mut args_json: Value = serde_json::from_str(args).unwrap_or_default();
                // The background routing flag is ours, never the server's.
                if let Some(object) = args_json.as_object_mut() {
                    object.remove("run_in_background");
                }
                // MCP tools are NOT sandboxed — path arguments are forwarded
                // verbatim to the MCP server. The server itself decides what
                // paths it can access.
                log_event(
                    &state,
                    "INFO",
                    format!("Args for MCP tool '{}': {}", actual_tool_name, args_json),
                );
                let _ = app.emit(
                    "tool-call",
                    format!(
                        "🔌 *MCP [{}]: {}*\n\n{}",
                        mcp_server.name,
                        actual_tool_name,
                        tool_args_block(&args_json)
                    ),
                );
                let start_time = std::time::Instant::now();
                let mcp_result =
                    mcp::invoke_mcp_tool(&state, mcp_server, actual_tool_name, args_json.clone())
                        .await;
                let duration_ms = start_time.elapsed().as_millis() as i64;
                let result = match &mcp_result {
                    Ok(result) => {
                        // Log successful MCP call
                        let db = state.db.lock().unwrap();
                        log_interaction(
                            &db,
                            &session_id,
                            "mcp_response",
                            &mcp_server.name,
                            actual_tool_name,
                            serde_json::to_string(&args_json).unwrap_or_default(),
                            result.clone(),
                            None,
                            duration_ms,
                            None,
                        );
                        result.clone()
                    }
                    Err(e) => {
                        // Log MCP error
                        let db = state.db.lock().unwrap();
                        log_interaction(
                            &db,
                            &session_id,
                            "mcp_call",
                            &mcp_server.name,
                            actual_tool_name,
                            serde_json::to_string(&args_json).unwrap_or_default(),
                            String::new(),
                            Some(e.clone()),
                            duration_ms,
                            None,
                        );
                        format!("MCP tool error: {e}")
                    }
                };
                result
            } else {
                let workspace_dir = state.workspace_dir.lock().unwrap().clone();
                let start_time = std::time::Instant::now();
                let tool_result = tools::execute_tool(
                    &app,
                    name,
                    args,
                    workspace_dir,
                    &config,
                    &skill_allowed_commands,
                    skill_evolution_root.as_deref(),
                    None,
                    Some(&session_id),
                    &active_skill_dirs,
                    None,
                    None,
                )
                .await;
                let duration_ms = start_time.elapsed().as_millis() as i64;

                // Log tool execution
                let db = state.db.lock().unwrap();
                let is_error = turn_guard::tool_output_is_error(&tool_result);
                log_interaction(
                    &db,
                    &session_id,
                    if is_error {
                        "tool_error"
                    } else {
                        "tool_output"
                    },
                    "tool_executor",
                    name,
                    serde_json::to_string(&serde_json::from_str::<Value>(args).unwrap_or_default())
                        .unwrap_or_default(),
                    tool_result.clone(),
                    if is_error {
                        Some(tool_result.clone())
                    } else {
                        None
                    },
                    duration_ms,
                    None,
                );
                drop(db);
                tool_result
            };

            // The call ran: a failed result counts towards the tool's failure
            // budget, and the failure that crosses the limit is announced to
            // the model (appended to the very same result) and to the user.
            if ran {
                if let Some(trip) = guard.record(
                    &session_id,
                    name,
                    turn_guard::tool_output_is_error(&result),
                    failure_limit,
                ) {
                    result.push_str("\n\n");
                    result.push_str(&turn_guard::disabled_notice(
                        name,
                        trip.failures,
                        failure_limit,
                        true,
                    ));
                    log_event(
                        &state,
                        "WARN",
                        format!(
                            "tool disabled for this session: session_id={}, tool={}, failures={}, limit={}",
                            session_id, name, trip.failures, failure_limit
                        ),
                    );
                    let _ = app.emit(
                        "chat-notice",
                        json!({
                            "kind": "tool_disabled",
                            "tool": name,
                            "failures": trip.failures,
                            "limit": failure_limit,
                        }),
                    );
                }
            }

            all_messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }

        all_messages.extend(pending_skill_context_messages);
    }

    // ── Automatic conclusion recording ──────────────────────────────────────
    // If the turn ended with a substantive final answer that reads like a
    // conclusion, and the LLM did not already persist it via the `memory`
    // tool, record it so it survives across sessions. Self-evolution mode
    // routes it into the dedicated evolution area instead of `repo`.
    if let Some(answer) = final_answer {
        let memory_enabled = config.effective_tools().iter().any(|t| t == "memory");
        if memory_enabled
            && !memory_tool_called
            && should_record_conclusion(&answer)
            && looks_like_conclusion(&answer)
        {
            let workspace_dir = state.workspace_dir.lock().unwrap().clone();
            let scope = if config.self_evolution_mode {
                crate::tools::memory::EVOLUTION_SCOPE
            } else {
                "repo"
            };
            record_conclusion_to_memory(&state, &app, &workspace_dir, &session_id, scope, &answer);
        }
    }

    log_event(
        &state,
        "INFO",
        format!("chat_completion finished: session_id={}", session_id),
    );
    // Close the injection window before announcing the end of the turn. The
    // guard also runs on the error path, where `chat-done` is never emitted.
    drop(injection_window);
    let _ = app.emit("chat-done", ());
    Ok(())
}

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

    let result = run_chat_turn(
        app.clone(),
        messages,
        skill_ids,
        session_id.clone(),
        model_override,
        use_agents,
        state,
    )
    .await;

    end_turn_cleanup(&app, &session_id).await;
    drop(sleep_guard);
    result
}

/// Turn-end cleanup: stop every serve process and verify that the killed
/// trees are actually gone. Runs in a blocking thread — verification polls
/// for up to a few seconds in the pathological case, which must not stall the
/// async runtime. Silent when no serve process ever existed.
async fn end_turn_cleanup(app: &AppHandle, session_id: &str) {
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
        let _ = app.emit(
            tools::background::BACKGROUND_TASK_STATE_EVENT,
            snapshot,
        );
    }
    // The waiting chip is driven by `background-waiting`; zero it on every turn
    // end so a hold that was cut short (Stop, error, budget) cannot leave it
    // counting down a turn that no longer exists.
    let _ = app.emit(
        tools::background::BACKGROUND_WAITING_EVENT,
        json!({ "session_id": session_id, "pending": 0 }),
    );
    if removed_count > 0 {
        log_event(
            &app.state::<AppState>(),
            "INFO",
            format!(
                "turn ended (session {session_id}): shut down {removed_count} background task(s)"
            ),
        );
    }

    let session_id = session_id.to_string();
    let summary = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || app.state::<AppState>().serve_processes.shutdown_all()
    })
    .await
    .unwrap_or_else(|err| format!("Error: serve cleanup task failed: {err}"));

    if !summary.contains("No background serve processes") {
        log_event(
            &app.state::<AppState>(),
            "INFO",
            format!("turn ended (session {session_id}): {summary}"),
        );
    }
}
