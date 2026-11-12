//! Automatic memory recording for messages that read like a conclusion.

use super::*;

// ─── Automatic conclusion memory recording ───────────────────────────────────

/// Heuristic: is this final answer a substantive conclusion worth persisting?
/// Keeps the automatic fallback conservative so we don't spam memory with
/// greetings or trivial acknowledgments.
pub(crate) fn should_record_conclusion(content: &str) -> bool {
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
pub(crate) fn looks_like_conclusion(content: &str) -> bool {
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
pub(crate) fn record_conclusion_to_memory(
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
