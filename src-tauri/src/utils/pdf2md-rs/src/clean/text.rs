//! Text-level clean-up: line joining, hyphenation, paragraph continuation.
//!
//! The parse stage keeps a paragraph's lines separated by `\n`, because the
//! information "the line ended here" is needed exactly once: to decide whether a
//! trailing hyphen is a real hyphen or a word broken by the typesetter. After
//! that the newlines must go, and this module is where they go.

/// Join the lines of one paragraph into a single line.
///
/// * a trailing hyphen followed by a lowercase continuation is removed
///   (`inter-` + `national` → `international`);
/// * CJK on either side of the break joins without a space;
/// * everything else joins with one space.
pub fn join_lines(text: &str, join_hyphenation: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n').map(str::trim).filter(|line| !line.is_empty()) {
        if out.is_empty() {
            out.push_str(line);
            continue;
        }
        if join_hyphenation && breaks_word(&out, line) {
            out.pop();
            out.push_str(line);
        } else if cjk_boundary(&out, line) {
            out.push_str(line);
        } else {
            out.push(' ');
            out.push_str(line);
        }
    }
    out
}

/// `true` when the hyphen at the end of `left` is a hyphenation break.
fn breaks_word(left: &str, right: &str) -> bool {
    let Some(last) = left.chars().last() else {
        return false;
    };
    if !matches!(last, '-' | '\u{2010}') {
        return false;
    }
    // The word before the hyphen must have some substance, and the continuation
    // must look like the rest of a word, not the start of a new one.
    let word_len = left
        .trim_end_matches(['-', '\u{2010}'])
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric())
        .count();
    let Some(first) = right.chars().next() else {
        return false;
    };
    word_len >= 2 && first.is_lowercase()
}

fn cjk_boundary(left: &str, right: &str) -> bool {
    let left_cjk = left.chars().last().is_some_and(is_cjk);
    let right_cjk = right.chars().next().is_some_and(is_cjk);
    left_cjk || right_cjk
}

/// `true` for CJK ideographs, kana and full-width punctuation.
pub fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x2E80..=0x9FFF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x20000..=0x2FA1F
    )
}

/// Collapse runs of spaces/tabs to a single space.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch == ' ' || ch == '\t' || ch == '\u{00A0}' {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    out
}

/// `true` when `right` continues the paragraph that `left` ends.
///
/// Deliberately shy: a capitalised continuation, a finished sentence, a colon
/// (which introduces something) and any short line that does not end in a
/// comma/hyphen are all treated as a new paragraph.
pub fn continues(left: &str, right: &str) -> bool {
    let left = left.trim_end();
    let right = right.trim_start();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if ends_sentence(left) {
        return false;
    }
    let last = left.chars().last();
    let soft = matches!(last, Some(',' | ';' | '-' | '\u{2010}' | '—' | '(' | '[' | '/'));
    let starts_lower = right
        .chars()
        .next()
        .is_some_and(|ch| ch.is_lowercase() || is_cjk(ch));
    let long_enough = left.chars().count() >= 40;
    soft || (starts_lower && long_enough)
}

/// `true` when the text ends a sentence.
pub fn ends_sentence(text: &str) -> bool {
    let trimmed = text.trim_end();
    let mut chars = trimmed.chars().rev().peekable();
    // Skip closing quotes and brackets: `… done.”` still ends a sentence.
    while let Some(ch) = chars.peek() {
        if matches!(ch, '"' | '\'' | '”' | '’' | ')' | ']' | '}' | '»') {
            chars.next();
        } else {
            break;
        }
    }
    matches!(
        chars.next(),
        Some('.' | '!' | '?' | ':' | ';' | '。' | '！' | '？' | '：' | '；' | '…')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenation_is_rejoined_only_for_lowercase_continuations() {
        assert_eq!(join_lines("inter-\nnational", true), "international");
        assert_eq!(join_lines("inter-\nNational", true), "inter- National");
        assert_eq!(join_lines("inter-\nnational", false), "inter- national");
    }

    #[test]
    fn cjk_lines_join_without_a_space() {
        assert_eq!(join_lines("这是一个\n测试", true), "这是一个测试");
        assert_eq!(join_lines("hello\n世界", true), "hello世界");
    }

    #[test]
    fn ordinary_lines_join_with_one_space() {
        assert_eq!(join_lines("hello\nworld  again", true), "hello world  again");
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(collapse_whitespace("a   b\t\tc  "), "a b c");
        assert_eq!(collapse_whitespace("  leading"), "leading");
        assert_eq!(collapse_whitespace("中文  空格"), "中文 空格");
    }

    #[test]
    fn continuation_rules() {
        assert!(continues(
            "This paragraph was cut by the page break and continues",
            "on the next page."
        ));
        assert!(continues("see the following,", "which is important"));
        assert!(!continues("The end.", "A new paragraph."));
        assert!(!continues("Short heading", "Next line"));
        assert!(!continues("Results:", "42"));
    }

    #[test]
    fn sentence_end_ignores_closing_quotes() {
        assert!(ends_sentence("He said “done.”"));
        assert!(ends_sentence("完。"));
        assert!(!ends_sentence("and then"));
    }
}
