//! Unit tests for `llm_complete.rs`.

use super::*;
// The streaming transport moved to `llm_client`; keep the historical test
// namespace (`stream::…`) so the assertions read the same.
use crate::llm_client as stream;

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

/// The memory hint is prefixed to the user's text, as its own paragraph, and
/// the stored content stays clean (the hint is request-only).
#[test]
fn the_memory_hint_is_prefixed_to_a_string_message() {
    let content = json!("改一下 tauri build");
    let hinted = turn::prepend_memory_hint(&content, "MEMORY (c5): tag first");
    assert_eq!(
        hinted,
        json!("MEMORY (c5): tag first\n\n改一下 tauri build")
    );
    assert_eq!(content, json!("改一下 tauri build"));
}

/// Multimodal content keeps every part in order — the hint goes in front.
#[test]
fn the_memory_hint_goes_in_front_of_every_multimodal_part() {
    let content = json!([
        { "type": "text", "text": "describe this: " },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,XXX" } },
    ]);
    let hinted = turn::prepend_memory_hint(&content, "MEMORY (c5): tag first");
    let parts = hinted.as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(
        parts[0],
        json!({ "type": "text", "text": "MEMORY (c5): tag first" })
    );
    assert_eq!(parts[1], content[0]);
    assert_eq!(parts[2], content[1]);
}

/// A shape the request cannot carry a hint in is left untouched.
#[test]
fn the_memory_hint_leaves_other_content_shapes_untouched() {
    assert_eq!(turn::prepend_memory_hint(&json!(null), "x"), json!(null));
    assert_eq!(turn::prepend_memory_hint(&json!(42), "x"), json!(42));
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

fn role(id: &str, name: &str, description: &str, prompt: &str) -> db::EmbeddedRole {
    db::EmbeddedRole {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        prompt: prompt.to_string(),
    }
}

#[test]
fn embedded_roles_render_nothing_without_content() {
    assert_eq!(render_embedded_roles(&[]), "");
    // A blank prompt contributes nothing — not even the framing block.
    assert_eq!(
        render_embedded_roles(&[role("a", "A", "desc", "   ")]),
        ""
    );
}

#[test]
fn embedded_roles_render_in_selection_order_with_one_framing_block() {
    let block = render_embedded_roles(&[
        role("a", "Embedded A", "Does A things.", "Prompt A."),
        role("b", "", "", "Prompt B."),
    ]);

    assert_eq!(block.matches("EMBEDDED ROLES").count(), 1);
    assert_eq!(block.matches("## Embedded role:").count(), 2);

    let first = block.find("## Embedded role: Embedded A").unwrap();
    let second = block.find("## Embedded role: b").unwrap();
    assert!(first < second, "selection order is preserved");

    assert!(block.contains("Does A things.\nPrompt A."));
    assert!(block.contains("Prompt B."));
    // The safety/tool precedence rule travels with the roles.
    assert!(block.contains("Safety, tool and approval rules win"));
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
        format!("{CONTEXT_CONTINUE_MARKER}\ncontinue"),
        format!("{CONTEXT_LENGTH_MARKER}\ncontinue"),
        format!("{CONTEXT_GOAL_MARKER}\ncontinue"),
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
fn wait_evt_marker_is_parsed_and_stripped() {
    // A marker with a reason yields the reason, trimmed.
    assert_eq!(
        wait_evt_reason("waiting for the CI notification [WAIT_EVT: the CI run]").as_deref(),
        Some("the CI run")
    );
    // An empty body still produces a usable phrase for the notice.
    assert_eq!(
        wait_evt_reason("blocked [WAIT_EVT:]"),
        Some("an external event".to_string())
    );
    // A marker that never closed is not a marker yet.
    assert_eq!(wait_evt_reason("[WAIT_EVT: still typing"), None);
    assert_eq!(wait_evt_reason("plain answer"), None);

    // Both answer-level control markers are removed.
    assert_eq!(strip_control_markers("done [goal:DONE]"), "done");
    assert_eq!(
        strip_control_markers("waiting for the build [WAIT_EVT: the build]"),
        "waiting for the build"
    );
    assert_eq!(
        strip_control_markers("a [goal:DONE] b [WAIT_EVT: x] c [WAIT_EVT: y]"),
        "a  b  c"
    );
    // An unterminated marker stays in the text.
    assert_eq!(
        strip_control_markers("tail [WAIT_EVT: unterminated"),
        "tail [WAIT_EVT: unterminated"
    );
}

/// A reply that carried only control markers is an empty answer: the turn must
/// fail loudly instead of closing on a blank bubble the user reads as "the
/// model never answered" (the model declaring `[goal:DONE]` alone did exactly
/// that).
#[test]
fn marker_only_answers_are_empty_after_stripping() {
    assert!(strip_control_markers(GOAL_DONE_MARKER)
        .trim()
        .is_empty());
    assert!(strip_control_markers("[WAIT_EVT: the CI run]")
        .trim()
        .is_empty());
    assert!(strip_control_markers(&format!("  {GOAL_DONE_MARKER}\n\n"))
        .trim()
        .is_empty());

    // A real answer keeps its text.
    assert_eq!(
        strip_control_markers(&format!("Done.\n\n{GOAL_DONE_MARKER}")),
        "Done."
    );
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
fn retryable_statuses_cover_the_transient_relay_errors() {
    for status in [408, 429, 500, 502, 503, 504, 529] {
        assert!(
            is_retryable_status(reqwest::StatusCode::from_u16(status).unwrap()),
            "{status} should be retryable"
        );
    }
    for status in [400, 401, 403, 404, 422] {
        assert!(
            !is_retryable_status(reqwest::StatusCode::from_u16(status).unwrap()),
            "{status} must not be retried"
        );
    }
}

#[test]
fn retry_after_header_parses_seconds_and_caps() {
    let mut headers = reqwest::header::HeaderMap::new();
    assert_eq!(parse_retry_after_header(&headers), None);

    headers.insert("retry-after", "7".parse().unwrap());
    assert_eq!(parse_retry_after_header(&headers), Some(Duration::from_secs(7)));

    // Hostile values are capped so a header cannot stall the turn.
    headers.insert("retry-after", "9999".parse().unwrap());
    assert_eq!(
        parse_retry_after_header(&headers),
        Some(Duration::from_secs(30))
    );

    // The HTTP-date form is deliberately ignored (falls back to backoff).
    headers.insert(
        "retry-after",
        "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
    );
    assert_eq!(parse_retry_after_header(&headers), None);
}

#[test]
fn premature_connection_close_is_retryable() {
    // hyper's message when a chunked body ends before the terminator.
    assert!(is_retryable_network_error(
        "error sending request for url (https://relay/v1): connection closed before message completed"
    ));
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

// ─── Empty-completion detection ─────────────────────────────────────────────
//
// A relay can close an aborted generation with a usage chunk and `[DONE]` and
// nothing else (no content, no tool call, no `finish_reason`). Once accepted as
// an answer it ended the turn silently, leaving the previous partial text on
// screen and the task unfinished — `stream_llm_request` now treats it as a
// failed attempt instead.

fn tool_call(id: &str) -> (String, String, String) {
    (id.to_string(), "run_shell".to_string(), "{}".to_string())
}

#[test]
fn empty_completion_is_unusable_without_content_or_tool_call() {
    assert!(stream::is_empty_completion("", &[]));
    // Whitespace-only replies are empty too (models emit those on a stall).
    assert!(stream::is_empty_completion("  \n\t ", &[]));
}

#[test]
fn empty_completion_accepts_content_or_tool_call() {
    assert!(!stream::is_empty_completion("done", &[]));
    // A tool call is a usable round even without narration.
    assert!(!stream::is_empty_completion("", &[tool_call("call_1")]));
    assert!(!stream::is_empty_completion("reading", &[tool_call("call_1")]));
}

// ─── Tool-round history message ─────────────────────────────────────────────
//
// The round's narration must survive into the request history: the UI shows it,
// and a model that cannot see what it just told the user re-reads files and
// repeats itself mid-turn. A round with no narration keeps `null`.

fn streamed_call(id: &str) -> Value {
    json!({ "id": id, "type": "function", "function": { "name": "run_shell", "arguments": "{}" } })
}

#[test]
fn tool_round_message_keeps_the_narration() {
    let msg = stream::assistant_tool_message("我先读一下稿件。", "", vec![streamed_call("c1")]);
    assert_eq!(msg["role"], json!("assistant"));
    assert_eq!(msg["content"], json!("我先读一下稿件。"));
    assert_eq!(msg["tool_calls"], json!([streamed_call("c1")]));
    assert!(msg.get("reasoning_content").is_none());
}

#[test]
fn tool_round_message_leaves_silent_rounds_null() {
    let msg = stream::assistant_tool_message("   ", "", vec![streamed_call("c1")]);
    assert_eq!(msg["content"], Value::Null);
}

#[test]
fn tool_round_message_carries_thinking_along() {
    let msg = stream::assistant_tool_message("done", "让我看看目录", vec![]);
    assert_eq!(msg["reasoning_content"], json!("让我看看目录"));
}

// ─── Content-part normalization ─────────────────────────────────────────────
//
// One attachment must never fail a whole turn: a gateway that only takes text
// and images answers a `file` part with
// `400 invalid_request_error … file content parts are not supported here; send
// the image as an image_url part`. The payload decides, the capability decides,
// and neither is left to the caller.

fn user_body(parts: Value) -> Value {
    json!({
        "messages": [
            { "role": "system", "content": "sys" },
            { "role": "user", "content": parts },
        ]
    })
}

fn file_part(name: &str, data: &str) -> Value {
    json!({ "type": "file", "file": { "filename": name, "file_data": data } })
}

#[test]
fn image_inside_a_file_part_becomes_an_image_url() {
    // A mis-classified screenshot: the part type says file, the bytes say png.
    let mut body = user_body(json!([
        { "type": "text", "text": "看图" },
        file_part("screenshot.avif", "data:image/avif;base64,AAAA"),
    ]));
    stream::normalize_content_parts(&mut body);
    let parts = body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["type"], json!("text"));
    assert_eq!(parts[1]["type"], json!("image_url"));
    assert_eq!(parts[1]["image_url"]["url"], json!("data:image/avif;base64,AAAA"));
}

// ─── Strict tool-pair sanitization ──────────────────────────────────────────
//
// DashScope validates a tighter shape than the lenient sanitizer pairs for:
// every `tool` response must sit in the block directly following its
// assistant, and that block must answer every declared id exactly once. Where
// the two rules diverge the lenient version blesses a sequence the provider
// then rejects with `400 … The tool_call_id in the response message does not
// match the tool_call_id in the tool_call`. On the sequences the chat loop
// builds the strict pass is a no-op.

fn round_assistant(ids: &[&str]) -> Value {
    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": ids.iter().map(|id| streamed_call(id)).collect::<Vec<_>>(),
    })
}

fn tool_response(id: &str) -> Value {
    json!({ "role": "tool", "tool_call_id": id, "content": "ok" })
}

#[test]
fn strict_sanitize_keeps_healthy_rounds_and_between_round_messages() {
    let mut messages = vec![
        json!({ "role": "user", "content": "start" }),
        round_assistant(&["c1"]),
        tool_response("c1"),
        // Injections and narrations between rounds are fine: they sit after
        // the whole block, never inside one.
        json!({ "role": "user", "content": "injected note" }),
        round_assistant(&["c2", "c3"]),
        tool_response("c2"),
        tool_response("c3"),
        json!({ "role": "assistant", "content": "answer" }),
    ];
    let expected = messages.clone();
    sanitize_tool_pairs_strict(&mut messages);
    assert_eq!(messages, expected);
}

#[test]
fn strict_sanitize_drops_a_response_not_adjacent_to_its_assistant() {
    // The lenient sanitizer pairs T(c1) with A(c1) across the interleaved
    // user message; DashScope rejects exactly this shape — so the strict
    // version must drop the orphan response AND the now-incomplete round.
    let mut messages = vec![
        json!({ "role": "user", "content": "start" }),
        round_assistant(&["c1"]),
        json!({ "role": "user", "content": "interrupted" }),
        tool_response("c1"),
    ];
    sanitize_tool_pairs_strict(&mut messages);
    assert_eq!(
        messages,
        vec![
            json!({ "role": "user", "content": "start" }),
            json!({ "role": "user", "content": "interrupted" }),
        ]
    );
}

#[test]
fn strict_sanitize_drops_duplicate_and_orphaned_responses() {
    // One response per declared id, in the block: the second T(c1) is a
    // duplicate, T(c9) answers nothing.
    let mut messages = vec![
        round_assistant(&["c1", "c2"]),
        tool_response("c1"),
        tool_response("c1"),
        tool_response("c9"),
        tool_response("c2"),
        json!({ "role": "assistant", "content": "answer" }),
    ];
    sanitize_tool_pairs_strict(&mut messages);
    assert_eq!(
        messages,
        vec![
            round_assistant(&["c1", "c2"]),
            tool_response("c1"),
            tool_response("c2"),
            json!({ "role": "assistant", "content": "answer" }),
        ]
    );
}

#[test]
fn strict_sanitize_drops_an_incomplete_round_with_its_block() {
    // T(c2) never arrived (a dropped/cancelled round): the assistant and the
    // whole block go — a partial round is rejected wholesale.
    let mut messages = vec![
        round_assistant(&["c1", "c2"]),
        tool_response("c1"),
        json!({ "role": "user", "content": "next" }),
    ];
    sanitize_tool_pairs_strict(&mut messages);
    assert_eq!(messages, vec![json!({ "role": "user", "content": "next" })]);
}

#[test]
fn strict_sanitize_drops_a_duplicated_id_only_once_per_occurrence() {
    // Two calls sharing one id (a relay repeat): ensure_tool_call_ids
    // normally prevents this, but the strict pass must still consume one
    // response per occurrence and keep the round when both are answered.
    let mut messages = vec![
        round_assistant(&["c1", "c1"]),
        tool_response("c1"),
        tool_response("c1"),
    ];
    let expected = messages.clone();
    sanitize_tool_pairs_strict(&mut messages);
    assert_eq!(messages, expected);
}

// ─── Tool-pairing error classifier ──────────────────────────────────────────
//
// The turn can repair a pairing rejection (drop the offending round, retry
// once); it must be classified instead of staying fatal.

#[test]
fn pairing_error_classifier_matches_the_provider_phrasings() {
    assert!(is_tool_pairing_error(
        "<400> InternalError.Algo.InvalidParameter: The tool_call_id in the response message does not match the tool_call_id in the tool_call: messages: message[24]."
    ));
    assert!(is_tool_pairing_error(
        "400: An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'."
    ));
    assert!(is_tool_pairing_error(
        "400: Messages with role 'tool' must be a response to a preceding message with 'tool_calls'"
    ));
    assert!(is_tool_pairing_error(
        "400 InvalidParameter: The following tool_call_ids did not have response messages: message[296].role"
    ));
    // Ordinary upstream failures must not trigger the repair.
    assert!(!is_tool_pairing_error("429: rate limited"));
    assert!(!is_tool_pairing_error(
        "400: maximum context length exceeded"
    ));
    assert!(!is_tool_pairing_error("stream closed without finish_reason/[DONE]"));
}

#[test]
fn reported_message_index_parses_the_reported_position() {
    assert_eq!(
        reported_message_index(
            "<400> InternalError.Algo.InvalidParameter: The tool_call_id in the response message does not match the tool_call_id in the tool_call: messages: message[24]."
        ),
        Some(24)
    );
    // The `messages:` prefix must not swallow the bracketed index.
    assert_eq!(
        reported_message_index("…must be followed by tool messages: message[296].role"),
        Some(296)
    );
    assert_eq!(reported_message_index("400: tool_call_id mismatch"), None);
}

#[test]
fn message_id_map_lists_roles_and_ids_without_content() {
    let messages = vec![
        json!({ "role": "system", "content": "a very long system prompt" }),
        round_assistant(&["c1", "c2"]),
        tool_response("c1"),
        json!({ "role": "user", "content": "injected" }),
    ];
    let map = message_id_map(&messages);
    assert!(map.contains("[0] system\n"));
    assert!(map.contains("[1] assistant calls=c1,c2\n"));
    assert!(map.contains("[2] tool for=c1\n"));
    assert!(map.contains("[3] user\n"));
    assert!(!map.contains("very long system prompt"));
}

// ─── Pairing-repair round drop ──────────────────────────────────────────────
//
// `drop_tool_round` removes the round the provider's `message[N]` points at
// (assistant + its following tool block), keeping everything after it.

#[test]
fn drop_tool_round_removes_the_reported_round_and_keeps_the_rest() {
    let mut messages = vec![
        json!({ "role": "user", "content": "start" }),
        round_assistant(&["c1"]),
        tool_response("c1"),
        round_assistant(&["c2"]),
        tool_response("c2"),
        json!({ "role": "user", "content": "injected mid-turn" }),
    ];
    // message[3] is the second round's assistant.
    assert!(drop_tool_round(&mut messages, Some(3)));
    assert_eq!(
        messages,
        vec![
            json!({ "role": "user", "content": "start" }),
            round_assistant(&["c1"]),
            tool_response("c1"),
            json!({ "role": "user", "content": "injected mid-turn" }),
        ]
    );
}

#[test]
fn drop_tool_round_removes_the_whole_block_when_the_index_names_a_response() {
    let mut messages = vec![
        round_assistant(&["c1", "c2"]),
        tool_response("c1"),
        tool_response("c2"),
        json!({ "role": "user", "content": "after" }),
    ];
    // message[2] is the block's second tool response: the round goes whole.
    assert!(drop_tool_round(&mut messages, Some(2)));
    assert_eq!(messages, vec![json!({ "role": "user", "content": "after" })]);
}

#[test]
fn drop_tool_round_falls_back_to_the_last_round_without_a_usable_index() {
    let mut messages = vec![
        round_assistant(&["c1"]),
        tool_response("c1"),
        round_assistant(&["c2"]),
        tool_response("c2"),
    ];
    assert!(drop_tool_round(&mut messages, None));
    assert_eq!(
        messages,
        vec![round_assistant(&["c1"]), tool_response("c1")]
    );

    // An index outside the array behaves the same way.
    let mut messages = vec![round_assistant(&["c1"]), tool_response("c1")];
    assert!(drop_tool_round(&mut messages, Some(999)));
    assert!(messages.is_empty());
}

#[test]
fn drop_tool_round_is_a_noop_without_any_round() {
    let mut messages = vec![json!({ "role": "user", "content": "hi" })];
    assert!(!drop_tool_round(&mut messages, None));
    assert_eq!(messages.len(), 1);
}

#[test]
fn image_data_url_detection_ignores_case() {
    let mut body = user_body(json!([file_part("a.png", "DATA:IMAGE/PNG;base64,CCCC")]));
    stream::normalize_content_parts(&mut body);
    assert_eq!(body["messages"][1]["content"][0]["type"], json!("image_url"));
}

#[test]
fn unreadable_file_part_degrades_to_text_instead_of_failing_the_turn() {
    let mut body = user_body(json!([
        { "type": "text", "text": "这份稿子" },
        file_part("spec.docx", "data:application/vnd.openxmlformats-officedocument.wordprocessingml.document;base64,AAAA"),
    ]));
    stream::normalize_content_parts(&mut body);
    let parts = body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["type"], json!("text"));
    assert_eq!(parts[1]["type"], json!("text"));
    let note = parts[1]["text"].as_str().unwrap();
    // The transcript still names the file, so the model can ask about it.
    assert!(note.contains("spec.docx"), "{note}");
    assert!(note.contains("not sent"), "{note}");
    // No `file` part survives on the wire.
    assert!(!body.to_string().contains("\"type\":\"file\""));
}

#[test]
fn text_payload_inside_a_file_part_is_inlined_instead_of_dropped() {
    // `@scripts/user_admin` was stored by an older build as an
    // `application/octet-stream` `file` part, and it succeeded the provider
    // question only once the bytes themselves reached the prompt.
    let script = "IyEvdXNyL2Jpbi9lbnYgYmFzaAo="; // "#!/usr/bin/env bash\n"
    let data = format!("data:application/octet-stream;base64,{script}");
    let mut body = user_body(json!([file_part("scripts/user_admin", &data)]));
    stream::normalize_content_parts(&mut body);
    let part = &body["messages"][1]["content"][0];
    assert_eq!(part["type"], json!("text"));
    let text = part["text"].as_str().unwrap();
    assert!(text.contains("#!/usr/bin/env bash"), "{text}");
    assert!(text.contains("scripts/user_admin"), "{text}");
    assert!(!text.contains("not sent"), "{text}");
}

#[test]
fn a_payload_that_is_not_base64_degrades_to_a_note() {
    let mut body = user_body(json!([file_part("raw.bin", "not-a-data-url")]));
    stream::normalize_content_parts(&mut body);
    let part = &body["messages"][1]["content"][0];
    assert_eq!(part["type"], json!("text"));
    assert!(part["text"].as_str().unwrap().contains("raw.bin"));
}

#[test]
fn normalization_leaves_text_and_media_parts_alone() {
    let mut body = user_body(json!([
        { "type": "text", "text": "听一下" },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,DDDD" } },
        { "type": "input_audio", "input_audio": { "data": "EEEE", "format": "wav" } },
    ]));
    let before = body.clone();
    stream::normalize_content_parts(&mut body);
    assert_eq!(body, before);
}

#[test]
fn normalization_ignores_plain_string_content() {
    let mut body = json!({
        "messages": [ { "role": "user", "content": "just text" } ]
    });
    let before = body.clone();
    stream::normalize_content_parts(&mut body);
    assert_eq!(body, before);
}

#[test]
fn parts_signature_names_the_message_index() {
    // The provider reports `.messages[N]`, so the log line must use the same
    // numbering; text-only messages are left out on purpose.
    let body = user_body(json!([
        { "type": "text", "text": "看图" },
        file_part("spec.docx", "data:application/msword;base64,AAAA"),
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,FFFF" } },
    ]));
    assert_eq!(
        stream::content_parts_signature(&body),
        "parts=m1=file+image_url"
    );
    let plain = json!({ "messages": [ { "role": "user", "content": "hi" } ] });
    assert_eq!(stream::content_parts_signature(&plain), "parts=text-only");
    assert_eq!(
        stream::content_parts_signature(&json!({})),
        "parts=n/a"
    );
}
