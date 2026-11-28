// What one tool call produced, in the shape the transcript row shows.
//
// The result string itself never leaves the turn: it is the model's context
// (up to 8000 characters, see `truncate_tool_output`) and a transcript row must
// not carry it. What a reader needs instead is a handful of numbers plus a
// clipped head — how many lines came back, which range was read, the exit code,
// and the line that says why a call failed.
//
// `measure` is the one place a result becomes those numbers. Every tool goes
// through it, so a tool added tomorrow shows its line count, its duration and
// its error without any UI work; the per-tool `match` below only adds what the
// generic rules cannot guess (a read range, a search hit count, a patch's
// `+n −m`).

use serde::Serialize;
use serde_json::Value;

use crate::turn_guard::tool_output_is_error;

/// Head of the result kept for the expandable output block.
const PREVIEW_LINES: usize = 12;
const PREVIEW_CHARS: usize = 800;

/// The failure line is one row of the transcript, cut with an ellipsis; the
/// full text is in the preview behind the expander.
const ERROR_CHARS: usize = 200;

/// Longest label the transcript keeps on one line.
const LABEL_CHARS: usize = 120;

/// Structured outcome of one tool call, serialized into the `tool-call` event
/// under `outcome` and stored on the entry as `outcome`.
///
/// Every field is optional except the tool name and the duration, so a reader
/// can tell "no value" from "zero" — a `0` is never rendered for something the
/// tool did not report.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ToolOutcome {
    /// Raw tool name (`run_shell`, `file_actions`), for the UI to label the row.
    pub tool: String,
    /// First line of a failure, clipped. Present only when the call failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Process exit code (`run_cmd` / `run_shell`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// First line `file_actions read` actually returned (1-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_from: Option<u64>,
    /// Last line `file_actions read` actually returned (1-based, inclusive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_to: Option<u64>,
    /// Lines in the result body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<u64>,
    /// Bytes in the result body (the written content, for a `write`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The result was clipped by `truncate_tool_output` before the model saw it.
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// Search hits and the number of distinct files they came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<u64>,
    /// Lines a patch / diff added and removed (`+n −m`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u64>,
    /// Wall-clock time the call took; 0 when it was never timed.
    pub duration_ms: u64,
    /// Head of the result (≤ 12 lines / 800 chars), for the expander.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Measure one finished call. Pure: no IO, no clock.
pub(crate) fn measure(tool: &str, args: &Value, result: &str, duration_ms: u64) -> ToolOutcome {
    let mut outcome = ToolOutcome {
        tool: tool.to_string(),
        lines: Some(result.lines().count() as u64),
        bytes: Some(result.len() as u64),
        truncated: result.contains("...[truncated "),
        duration_ms,
        preview: preview_of(result),
        ..ToolOutcome::default()
    };

    if tool_output_is_error(result) {
        outcome.error = Some(error_excerpt(result));
    }
    outcome.exit_code = exit_code_of(result);

    // Per-tool fields the generic rules cannot read off the text.
    let action = args["action"].as_str().unwrap_or("");
    match (tool, action) {
        ("file_actions", "read") => {
            // A ranged read returns exactly the lines it read, joined without a
            // trailing newline, so the count is the range's length. A whole-file
            // read starts at line 1.
            let from = args["start_line"].as_u64().unwrap_or(1);
            outcome.line_from = Some(from);
            outcome.line_to = Some(from + outcome.lines.unwrap_or(1).saturating_sub(1));
        }
        ("file_actions", "search") => {
            let (matches, files) = count_search_rows(result);
            outcome.matches = Some(matches);
            outcome.files = Some(files);
        }
        ("file_actions", "write") => {
            // What was written is the interesting size, not the one-line
            // "Successfully wrote to …" that came back.
            outcome.bytes = args["content"].as_str().map(|text| text.len() as u64);
            outcome.lines = None;
        }
        ("file_actions", "patch") => {
            // A patch defines its own size; counting the result ("Successfully
            // patched …") would say one line.
            let (added, removed) = count_patch_lines(&patch_text(args));
            outcome.added = Some(added);
            outcome.removed = Some(removed);
        }
        ("file_actions", "diff") => {
            let (added, removed) = count_patch_lines(result);
            outcome.added = Some(added);
            outcome.removed = Some(removed);
        }
        _ => {}
    }

    // A failure explains itself with its error line, not with a metric that
    // counts the error message.
    if outcome.error.is_some() {
        outcome.lines = None;
        outcome.line_from = None;
        outcome.line_to = None;
        outcome.matches = None;
        outcome.files = None;
        outcome.added = None;
        outcome.removed = None;
    }
    outcome
}

/// One-line label for a call that is about to run.
///
/// Every call gets one, emitted when the round starts, so a tool that reports
/// no progress of its own (`memory`, `todos`, `calculator`, `timer_list`) still
/// shows *what it was asked to do* instead of appearing as a bare name when it
/// finishes. A tool with a line of its own overwrites this one a moment later,
/// so a specific verb phrase always wins over the generic digest.
pub(crate) fn call_label(name: &str, args: &Value) -> String {
    let action = args["action"].as_str().unwrap_or("");
    let path = || args["path"].as_str().unwrap_or(".");

    let detail = match (name, action) {
        ("file_actions", "read") => format!(
            "read {}{}",
            path(),
            line_range_suffix(args["start_line"].as_i64(), args["end_line"].as_i64()),
        ),
        ("file_actions", "search") => {
            let query = args["query"].as_str().unwrap_or("");
            let limit = args["max_results"]
                .as_u64()
                .map(|value| format!(" · max {value}"))
                .unwrap_or_default();
            format!("search {query:?} in {}{limit}", path())
        }
        ("file_actions", "") => path().to_string(),
        ("file_actions", other) => format!("{other} {}", path()),
        ("run_cmd", "list") => "list scripts".to_string(),
        ("run_cmd", _) => {
            let command = args["command"].as_str().unwrap_or("");
            if command.is_empty() {
                "run".to_string()
            } else {
                format!("run {command}")
            }
        }
        ("run_shell", _) => format!(
            "{}: {}",
            crate::tools::RUN_SHELL_TYPE,
            first_line(args["code"].as_str().unwrap_or(""))
        ),
        ("call_subagent", _) => args["prompt"]
            .as_str()
            .map(first_line)
            .unwrap_or_else(|| "delegate".to_string()),
        (_, "") => String::new(),
        _ => args_digest(args),
    };

    let detail = sanitize(&detail);
    if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {detail}")
    }
}

/// ` (lines 40–80)` / ` (from line 40)` / ` (to line 80)` / `""`.
pub(crate) fn line_range_suffix(from: Option<i64>, to: Option<i64>) -> String {
    match (from, to) {
        (None, None) => String::new(),
        (Some(from), Some(to)) => format!(" (lines {from}–{to})"),
        (Some(from), None) => format!(" (from line {from})"),
        (None, Some(to)) => format!(" (to line {to})"),
    }
}

// ─── Extraction helpers ──────────────────────────────────────────────────────

/// The line that says what broke.
///
/// `format_process_output` writes stdout, then a `STDERR:` marker, then the
/// error stream, then the exit code — so for a shell failure the useful line is
/// the one *after* the marker, not the marker itself.
fn error_excerpt(result: &str) -> String {
    let lines: Vec<&str> = result
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let picked = match lines.first() {
        Some(&"STDERR:") => lines.get(1).copied().unwrap_or("STDERR:"),
        Some(first) => first,
        None => "(no output)",
    };
    clip(picked, ERROR_CHARS)
}

/// `Exit code: 1` — the trailer `format_process_output` appends to a failure.
fn exit_code_of(result: &str) -> Option<i32> {
    result.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix("Exit code: ")?
            .trim()
            .parse()
            .ok()
    })
}

/// Head of the result for the expander.
fn preview_of(result: &str) -> Option<String> {
    let text = result.trim();
    if text.is_empty() {
        return None;
    }
    let total = text.lines().count();
    let mut preview = text
        .lines()
        .take(PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if preview.chars().count() > PREVIEW_CHARS {
        preview = preview.chars().take(PREVIEW_CHARS).collect();
    } else if total <= PREVIEW_LINES {
        return Some(preview);
    }
    preview.push('…');
    Some(preview)
}

/// Rows of a search result: `path:line:text`, one hit per line, plus the
/// `STDERR:` / notes trailer that must not be counted as a hit.
fn count_search_rows(result: &str) -> (u64, u64) {
    let mut matches = 0u64;
    let mut files = std::collections::HashSet::new();
    for line in result.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(path), Some(number), Some(_text)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        matches += 1;
        files.insert(path);
    }
    (matches, files.len() as u64)
}

/// `+n −m` as a unified diff spells it; `+++` / `---` headers are not changes.
fn count_patch_lines(patch: &str) -> (u64, u64) {
    let mut added = 0u64;
    let mut removed = 0u64;
    for line in patch.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    (added, removed)
}

/// `file_actions patch` accepts one string or an array of chunks.
fn patch_text(args: &Value) -> String {
    match &args["patch"] {
        Value::String(text) => text.clone(),
        Value::Array(chunks) => chunks
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Short `key=value` digest of a call whose arguments have no known shape.
fn args_digest(args: &Value) -> String {
    let Some(object) = args.as_object() else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in object {
        if key == "run_in_background" {
            continue;
        }
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Null => continue,
            other => other.to_string(),
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        // A payload (`content`, `patch`, `code`) is worth a hint, not a copy.
        let budget = if matches!(key.as_str(), "content" | "patch" | "code") {
            48
        } else {
            80
        };
        parts.push(format!("{key}={}", clip(&text, budget)));
        if parts.len() == 3 {
            break;
        }
    }
    parts.join(" ")
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Cut to `limit` characters, marking the cut.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

/// One line, no markup characters the transcript's italic-label parser would
/// read as syntax: the label is wrapped in `*…*` by the emitter and the
/// frontend takes everything between the first and the last asterisk.
fn sanitize(text: &str) -> String {
    let flattened: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['*', '`'], "");
    clip(flattened.trim(), LABEL_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_range_is_measured_from_the_arguments() {
        let outcome = measure(
            "file_actions",
            &json!({ "action": "read", "path": "src/a.ts", "start_line": 40, "end_line": 80 }),
            "line forty\nline forty-one",
            12,
        );
        assert_eq!(outcome.line_from, Some(40));
        // Two lines came back, not the 41 the caller asked for: the range is
        // what the tool *read*, and the row must not claim more.
        assert_eq!(outcome.line_to, Some(41));
        assert_eq!(outcome.lines, Some(2));
        assert!(outcome.error.is_none());
        assert_eq!(outcome.duration_ms, 12);
    }

    #[test]
    fn whole_file_read_starts_at_line_one() {
        let outcome = measure(
            "file_actions",
            &json!({ "action": "read", "path": "a.ts" }),
            "a\nb\nc",
            0,
        );
        assert_eq!(outcome.line_from, Some(1));
        assert_eq!(outcome.line_to, Some(3));
    }

    #[test]
    fn search_counts_rows_and_files() {
        let outcome = measure(
            "file_actions",
            &json!({ "action": "search", "query": "needle" }),
            "src/a.ts:4:needle here\nsrc/a.ts:9:needle again\nsrc/b.rs:1:needle\n",
            3,
        );
        assert_eq!(outcome.matches, Some(3));
        assert_eq!(outcome.files, Some(2));
    }

    #[test]
    fn search_notes_are_not_hits() {
        let outcome = measure(
            "file_actions",
            &json!({ "action": "search", "query": "x" }),
            "src/a.ts:4:x\n\nstopped early at the 20-match limit — narrow the path\n",
            0,
        );
        assert_eq!(outcome.matches, Some(1));
    }

    #[test]
    fn failed_shell_call_reports_the_stderr_line_and_the_exit_code() {
        let outcome = measure(
            "run_shell",
            &json!({ "code": "npm run build" }),
            "STDERR:\nboom: missing dependency\n\nExit code: 1",
            812,
        );
        assert_eq!(outcome.error.as_deref(), Some("boom: missing dependency"));
        assert_eq!(outcome.exit_code, Some(1));
        // A failure shows its reason, never a line count of the error message.
        assert!(outcome.lines.is_none());
        assert!(outcome.preview.is_some());
    }

    #[test]
    fn a_successful_call_has_no_error() {
        let outcome = measure("run_shell", &json!({ "code": "true" }), "ok", 5);
        assert!(outcome.error.is_none());
        assert!(outcome.exit_code.is_none());
        assert_eq!(outcome.lines, Some(1));
    }

    #[test]
    fn a_huge_result_is_clipped_and_flagged() {
        let raw = format!(
            "{}\n\n...[truncated 9000 of 17000 chars — output too large]...\n\ntail",
            "x".repeat(20_000)
        );
        let outcome = measure("run_shell", &json!({ "code": "dump" }), &raw, 1);
        assert!(outcome.truncated);
        let preview = outcome.preview.unwrap();
        assert!(preview.chars().count() <= PREVIEW_CHARS + 1, "{preview}");
    }

    #[test]
    fn patch_reports_added_and_removed_lines() {
        let patch = "--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n same\n";
        let outcome = measure("file_actions", &json!({ "action": "patch", "patch": patch }), "Successfully patched x", 3);
        assert_eq!(outcome.added, Some(2));
        assert_eq!(outcome.removed, Some(1));
    }

    #[test]
    fn write_reports_the_written_size_not_the_reply() {
        let outcome = measure(
            "file_actions",
            &json!({ "action": "write", "path": "a.ts", "content": "0123456789" }),
            "Successfully wrote to a.ts",
            1,
        );
        assert_eq!(outcome.bytes, Some(10));
        assert!(outcome.lines.is_none());
    }

    #[test]
    fn labels_carry_the_arguments_and_stay_on_one_line() {
        let label = call_label(
            "file_actions",
            &json!({ "action": "read", "path": "src/a.ts", "start_line": 40, "end_line": 80 }),
        );
        assert_eq!(label, "file_actions · read src/a.ts (lines 40–80)");

        let label = call_label("memory", &json!({ "action": "get", "key": "repo/fact" }));
        assert_eq!(label, "memory · action=get key=repo/fact");

        // A multi-line shell script collapses, and markup characters the label
        // parser would read as syntax are dropped.
        let label = call_label("run_shell", &json!({ "code": "echo *hi*\nrm -rf build" }));
        assert_eq!(label, "run_shell · pwsh: echo hi");
        assert!(!label.contains('\n'));
    }

    #[test]
    fn a_call_without_arguments_is_just_its_name() {
        assert_eq!(call_label("timer_list", &json!({})), "timer_list");
    }
}
