//! Heading level inference.
//!
//! Font size alone is not enough, and neither is numbering: a document can show
//!
//! ```text
//! 1. Introduction
//! 1.1 Background
//! 1.1.1 Problem Definition
//! ```
//!
//! at one identical size, and it can show an unnumbered `Abstract` at body size.
//! So the detector combines, in this order of authority:
//!
//! 1. **Numbering** — `1`, `1.2`, `1.2.3`, `IV.`, `一、`, `(a)` give the depth
//!    directly, regardless of the font;
//! 2. **Structural keywords** — `Abstract`, `References`, `Appendix`, `Acknowl‑
//!    edgements`, `摘要`, `参考文献` are always a top-level section;
//! 3. **Typography** — size relative to the body size, weight, shortness;
//! 4. **Continuity** — a heading never skips a level (`1` → `3` becomes `1` → `2`),
//!    which keeps the Markdown outline valid.

/// Infers Markdown heading levels for one document.
#[derive(Debug, Clone)]
pub struct HeadingDetector {
    body_size: f32,
    /// Level of an unnumbered top-level section (1 without a document title,
    /// 2 when the title already took level 1).
    base_level: u8,
    last_level: u8,
    headings_seen: usize,
}

impl HeadingDetector {
    /// `base_level` is 1 when the renderer emits no document title of its own.
    pub fn new(body_size: f32, base_level: u8) -> Self {
        HeadingDetector {
            body_size: body_size.max(1.0),
            base_level: base_level.clamp(1, 5),
            last_level: 0,
            headings_seen: 0,
        }
    }

    /// How many headings were detected so far.
    pub fn seen(&self) -> usize {
        self.headings_seen
    }

    /// The body font size the detector compares against.
    pub fn body_size(&self) -> f32 {
        self.body_size
    }

    /// Reset the continuity state (used when a document's first page is skipped).
    pub fn reset(&mut self) {
        self.last_level = 0;
    }

    /// Decide whether `text` is a heading, and at which level.
    pub fn infer(&mut self, text: &str, size: f32, bold: bool) -> Option<u8> {
        let trimmed = text.trim();
        if !looks_like_heading_text(trimmed) {
            return None;
        }
        let candidate = if let Some(depth) = numbering_depth(trimmed) {
            // Numbering is authoritative: `1.2.3` is a third-level section even
            // when it is printed at body size.
            self.base_level + (depth as u8).saturating_sub(1)
        } else if is_structural_keyword(trimmed) {
            self.base_level
        } else {
            let ratio = size / self.body_size;
            if ratio >= 1.45 {
                self.base_level
            } else if ratio >= 1.2 {
                self.base_level + 1
            } else if ratio >= 1.06 && bold {
                self.base_level + 1
            } else if bold && self.headings_seen > 0 && ratio >= 0.98 && word_count(trimmed) <= 8 {
                // A short bold line in an already-structured document reads as
                // a sub-heading (`Methods`, `Results`).
                self.base_level + 1
            } else {
                return None;
            }
        };

        let level = self.clamp_level(candidate);
        self.last_level = level;
        self.headings_seen += 1;
        Some(level)
    }

    /// Keep the outline valid: no level jumps, never below `base_level`.
    fn clamp_level(&self, candidate: u8) -> u8 {
        let floor = self.base_level;
        let ceiling = if self.last_level == 0 { floor + 1 } else { self.last_level + 1 };
        candidate.clamp(floor, ceiling.min(6))
    }
}

/// Text-level sanity checks shared by every heuristic.
pub fn looks_like_heading_text(text: &str) -> bool {
    if text.is_empty() || text.chars().count() > 160 {
        return false;
    }
    let last = text.chars().last().unwrap_or(' ');
    if matches!(last, ',' | ';' | '，' | '；' | '、') {
        return false;
    }
    // A sentence that happens to be short is not a heading.
    if word_count(text) > 16 && !text.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    // `........` (dot leaders) and pure punctuation are not headings.
    let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
    letters >= 2
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Depth encoded in a numbering prefix: `1` → 1, `1.2` → 2, `1.2.3` → 3.
pub fn numbering_depth(text: &str) -> Option<usize> {
    let trimmed = text.trim_start();
    if let Some((components, rest)) = numeric_prefix(trimmed) {
        if rest.is_empty() || !components.is_empty() {
            let depth = components.len();
            // `2023 was a good year` must not become a heading: a single number
            // needs a separator or a following capitalised word.
            if depth == 1 && !has_separator_after_number(trimmed) {
                return None;
            }
            return Some(depth.clamp(1, 4));
        }
    }
    if let Some(depth) = roman_or_letter_prefix(trimmed) {
        return Some(depth);
    }
    cjk_prefix(trimmed)
}

/// `1`, `1.`, `1.2`, `1.2.3.` — returns the components and the remaining text.
fn numeric_prefix(text: &str) -> Option<(Vec<u32>, &str)> {
    let mut components = Vec::new();
    let mut rest = text;
    loop {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        let value: u32 = digits.parse().ok()?;
        components.push(value);
        rest = &rest[digits.len()..];
        if let Some(stripped) = rest.strip_prefix('.') {
            // A dot followed by another digit continues the numbering, otherwise
            // it terminates it (`1. Introduction`).
            if stripped.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                rest = stripped;
                continue;
            }
            rest = stripped;
            break;
        }
        break;
    }
    if components.is_empty() {
        return None;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        // `1.2.3` alone on a line is still a heading.
        return Some((components, rest));
    }
    // Require a space or a closing parenthesis after the number.
    let had_space = text
        .find(rest)
        .map(|index| text[..index].ends_with([' ', ')', '）', '.']))
        .unwrap_or(false);
    if had_space {
        Some((components, rest))
    } else {
        None
    }
}

fn has_separator_after_number(text: &str) -> bool {
    let digits = text.chars().take_while(|c| c.is_ascii_digit()).count();
    text[digits..].starts_with('.') || text[digits..].starts_with(')') || text[digits..].starts_with('、')
}

/// `IV. Methods`, `(a) Setup` → depth 1 / 2.
fn roman_or_letter_prefix(text: &str) -> Option<usize> {
    let trimmed = text.trim_start_matches('(').trim_start_matches('（');
    let letters: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() || letters.len() > 6 {
        return None;
    }
    let rest = trimmed[letters.len()..].trim_start();
    let closed = trimmed[letters.len()..].starts_with(')') || trimmed[letters.len()..].starts_with('）');
    let rest = rest.trim_start_matches([')', '）', '.']).trim_start();
    if rest.is_empty() {
        return None;
    }
    let upper = letters.chars().all(|c| c.is_ascii_uppercase());
    let is_roman = upper
        && letters
            .chars()
            .all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'));
    if is_roman && (closed || trimmed[letters.len()..].starts_with('.')) {
        return Some(1);
    }
    if (letters.len() == 1 || closed) && (closed || trimmed[letters.len()..].starts_with('.')) {
        return Some(2);
    }
    None
}

/// `一、引言`, `第一章 …`, `（一）…` — Chinese documents' numbering.
fn cjk_prefix(text: &str) -> Option<usize> {
    let trimmed = text.trim_start_matches(['（', '(']);
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if !matches!(first as u32, 0x4E00..=0x9FFF) {
        return None;
    }
    let rest: String = chars.collect();
    if rest.starts_with('、') || rest.starts_with('．') || rest.starts_with('.') {
        return Some(1);
    }
    if trimmed.starts_with("第") && (rest.contains('章') || rest.contains('节')) {
        return Some(1);
    }
    None
}

/// Section names that are headings in every paper, at body size or not.
pub fn is_structural_keyword(text: &str) -> bool {
    let lower = text.trim().trim_end_matches([':', '：']).to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "abstract"
            | "introduction"
            | "related work"
            | "background"
            | "method"
            | "methods"
            | "methodology"
            | "approach"
            | "experiments"
            | "experimental results"
            | "results"
            | "results and discussion"
            | "discussion"
            | "conclusion"
            | "conclusions"
            | "future work"
            | "acknowledgements"
            | "acknowledgments"
            | "references"
            | "bibliography"
            | "appendix"
            | "摘要"
            | "引言"
            | "前言"
            | "相关工作"
            | "方法"
            | "实验结果"
            | "实验"
            | "结论"
            | "参考文献"
            | "致谢"
            | "附录"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_depth_wins_over_identical_font_sizes() {
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(detector.infer("1. Introduction", 10.0, false), Some(1));
        assert_eq!(detector.infer("1.1 Background", 10.0, false), Some(2));
        assert_eq!(detector.infer("1.1.1 Problem Definition", 10.0, false), Some(3));
    }

    #[test]
    fn levels_never_skip() {
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(detector.infer("1. Introduction", 10.0, false), Some(1));
        // A jump to 1.1.1 is clamped to one deeper than the previous heading.
        assert_eq!(detector.infer("1.1.1 Deep", 10.0, false), Some(2));
    }

    #[test]
    fn size_and_weight_produce_levels() {
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(detector.infer("A Big Title", 18.0, true), Some(1));
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(detector.infer("A Medium Heading", 12.5, true), Some(2));
    }

    #[test]
    fn body_text_is_not_a_heading() {
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(
            detector.infer("This is a full sentence of ordinary body text that goes on.", 10.0, false),
            None
        );
        assert_eq!(detector.infer("……", 10.0, false), None);
    }

    #[test]
    fn structural_keywords_are_sections() {
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(detector.infer("Abstract", 10.0, false), Some(1));
        let mut detector = HeadingDetector::new(10.0, 1);
        assert_eq!(detector.infer("References", 9.0, false), Some(1));
    }

    #[test]
    fn base_level_two_offsets_every_level() {
        let mut detector = HeadingDetector::new(10.0, 2);
        assert_eq!(detector.infer("1. Introduction", 10.0, false), Some(2));
        assert_eq!(detector.infer("1.1 Background", 10.0, false), Some(3));
    }

    #[test]
    fn a_bare_year_is_not_numbering() {
        assert_eq!(numbering_depth("2023 was a good year"), None);
        assert_eq!(numbering_depth("2. Methods"), Some(1));
        assert_eq!(numbering_depth("2.1 Methods"), Some(2));
        assert_eq!(numbering_depth("IV. Results"), Some(1));
        assert_eq!(numbering_depth("一、引言"), Some(1));
    }
}
