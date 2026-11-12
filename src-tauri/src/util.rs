//! Small helpers shared by more than one module.
//!
//! Each of these used to exist as a private copy in two or more modules, and
//! the copies had drifted apart — three truncation policies under one name,
//! five copies of the same clock read, three slugifiers with different caps.
//! The shared version puts the policy in the name so a caller picks one on
//! purpose instead of inheriting whichever copy its module happened to hold.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch (0 if the clock predates it).
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Milliseconds since the Unix epoch as `u64` (log timestamps, ring buffers).
pub(crate) fn now_ms_u64() -> u64 {
    now_ms().max(0) as u64
}

/// Cut `text` to at most `max` characters, on a char boundary. No marker: the
/// caller decides how to signal the cut (see the two `…` variants below).
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// Cut `text` so the result — including its trailing `…` — is at most `max`
/// characters. Used for titles that must stay inside a fixed budget.
pub(crate) fn truncate_with_ellipsis(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Cut `text` to `max` characters and append a separated ` …` when anything was
/// dropped, so the marker sits *on top of* the budget. Used where the preview
/// has no hard ceiling (tool output snippets).
pub(crate) fn truncate_with_trailing_ellipsis(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str(" …");
    out
}

/// Lowercase slug: alphanumeric runs joined by single dashes, trimmed at both
/// ends. Unicode letters/digits survive, so a Chinese name keeps a readable
/// file name. `max` caps the length while building (0 = uncapped); the result
/// may be empty — the caller owns the fallback.
pub(crate) fn slugify(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in text.trim().chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            last_dash = false;
        } else if !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
        if max > 0 && out.chars().count() >= max {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_policies_stay_distinct() {
        let text = "上海市浦东新区";
        assert_eq!(truncate_chars(text, 3), "上海市");
        assert_eq!(truncate_with_ellipsis(text, 3), "上海…");
        assert_eq!(truncate_with_trailing_ellipsis(text, 3), "上海市 …");
        assert_eq!(truncate_with_ellipsis(text, 0), "");
        assert_eq!(truncate_chars(text, 99), text);
    }

    #[test]
    fn slugify_keeps_unicode_and_caps() {
        assert_eq!(slugify("Weather Tools!", 0), "weather-tools");
        assert_eq!(slugify("天气 工具", 0), "天气-工具");
        assert_eq!(slugify("  --  ", 0), "");
        assert_eq!(slugify("abcdefghij", 4), "abcd");
    }
}
