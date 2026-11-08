//! Strip running headers, footers and page numbers from the page geometry.
//!
//! This has to happen **before** paragraphs are grouped: a header that survives
//! becomes a paragraph, and once it is a paragraph no text-level rule can
//! distinguish it from content without risking real text.
//!
//! Two signals are used, both cheap and explainable:
//!
//! * **Repetition** — the same line (after normalising digits to `#`, so
//!   "Page 3 of 12" and "Page 4 of 12" match) in the top or bottom band of at
//!   least half the pages is furniture. One page is not enough evidence, so the
//!   detector needs at least two pages.
//! * **Shape** — a band line that is nothing but a number, a roman numeral, or
//!   `Page N` / `N / M` / `- N -` is a page number even on a single page.
//!
//! Nothing outside the bands is touched: a repeated sentence in the body of a
//! paper (a running title quoted in the text) must survive.

use crate::layout::PageLayout;
use crate::options::Clean;

/// The set of line shapes that must be dropped.
#[derive(Debug, Clone, Default)]
pub struct Furniture {
    keys: std::collections::HashSet<String>,
    /// Fraction of the page height considered a header/footer band.
    band_ratio: f32,
    /// `true` when page numbers are stripped even without repetition.
    strip_page_numbers: bool,
}

impl Furniture {
    /// `true` when nothing will be removed.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && !self.strip_page_numbers
    }

    /// Number of repeated shapes found.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// `true` when `text` (already normalised or not) is running furniture.
    pub fn contains(&self, text: &str) -> bool {
        if self.keys.contains(&normalize(text)) {
            return true;
        }
        self.strip_page_numbers && is_page_number(text)
    }

    /// The band ratio the detection used.
    pub fn band_ratio(&self) -> f32 {
        self.band_ratio
    }

    /// `true` when `y` is inside the header or footer band.
    pub fn in_band(&self, page_height: f32, y: f32) -> bool {
        if self.band_ratio <= 0.0 {
            return false;
        }
        let band = page_height * self.band_ratio;
        y <= band || y >= page_height - band
    }
}

/// Learn the document's running furniture.
pub fn detect(pages: &[PageLayout], clean: &Clean) -> Furniture {
    if !clean.remove_headers_footers || pages.is_empty() {
        return Furniture::default();
    }
    let band_ratio = clean.band_ratio.clamp(0.01, 0.25);
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for page in pages {
        let mut on_this_page: Vec<String> = Vec::new();
        for line in &page.lines {
            if !in_band(page.height, band_ratio, line.rect.center_y()) {
                continue;
            }
            let key = normalize(&line.text);
            if key.is_empty() {
                continue;
            }
            if !on_this_page.contains(&key) {
                on_this_page.push(key);
            }
        }
        for key in on_this_page {
            *seen.entry(key).or_insert(0) += 1;
        }
    }

    // A shape repeated on at least half the pages (and on at least two) is a
    // running header/footer. With a single page there is no repetition signal.
    let threshold = if pages.len() < 2 {
        usize::MAX
    } else {
        ((pages.len() as f32 * clean.header_footer_threshold).ceil() as usize).max(2)
    };
    let keys = seen
        .into_iter()
        .filter(|(key, count)| *count >= threshold && meaningful(key))
        .map(|(key, _)| key)
        .collect();

    Furniture { keys, band_ratio, strip_page_numbers: clean.remove_headers_footers }
}

/// Remove furniture lines from every page. Returns the number of lines dropped.
pub fn strip(pages: &mut [PageLayout], furniture: &Furniture) -> usize {
    if furniture.is_empty() {
        return 0;
    }
    let mut dropped = 0usize;
    for page in pages.iter_mut() {
        let height = page.height;
        let band_ratio = furniture.band_ratio;
        page.lines.retain(|line| {
            let in_band = in_band(height, band_ratio, line.rect.center_y());
            if !in_band {
                return true;
            }
            if furniture.contains(&line.text) {
                dropped += 1;
                return false;
            }
            true
        });
    }
    dropped
}

fn in_band(height: f32, band_ratio: f32, y: f32) -> bool {
    let band = height * band_ratio;
    y <= band || y >= height - band
}

/// A key must carry at least two letters/digits to be worth removing; a lone
/// dash or a bullet in the margin is not evidence of anything.
fn meaningful(key: &str) -> bool {
    key.chars().filter(|c| c.is_alphanumeric()).count() >= 2 || is_page_number(key)
}

/// Lowercase, collapse whitespace, turn digits into `#`.
///
/// `#` is what makes "Page 3 of 12" and "Page 4 of 12" the same shape without
/// also collapsing "Chapter 3" into "Chapter 4".
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        let ch = if ch.is_ascii_digit() { '#' } else { ch };
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
            continue;
        }
        last_space = false;
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out.trim().to_string()
}

/// `true` for a line that is only a page number in one of its common shapes.
pub fn is_page_number(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 24 {
        return false;
    }
    let mut words = trimmed.split_whitespace();
    // CJK page numbers: 第 12 页.
    if trimmed.starts_with('第') && trimmed.ends_with('页') {
        let inner = trimmed.trim_start_matches('第').trim_end_matches('页');
        if !inner.trim().is_empty() && inner.chars().all(is_number_char) {
            return true;
        }
    }
    if let Some(first) = words.next() {
        let lower = first.to_ascii_lowercase();
        if matches!(lower.as_str(), "page" | "p." | "p" | "pg." | "seite" | "页" | "第") {
            let rest: String = words.collect::<Vec<_>>().join(" ");
            return !rest.is_empty() && rest.chars().all(is_number_char);
        }
    }
    // "- 12 -", "12", "iv", "12 / 30", "12 of 30", "12/30".
    let stripped: String = trimmed.chars().filter(|c| !matches!(c, '-' | '–' | '—')).collect();
    let stripped = stripped.trim();
    if stripped.chars().all(is_number_char) && stripped.chars().any(|c| c.is_alphanumeric()) {
        return true;
    }
    let lower = stripped.to_ascii_lowercase();
    for separator in [" of ", "/", " von ", " de "] {
        if let Some((left, right)) = lower.split_once(separator) {
            let left = left.trim();
            let right = right.trim();
            if !left.is_empty()
                && !right.is_empty()
                && left.chars().all(is_number_char)
                && right.chars().all(is_number_char)
                && left.chars().any(|c| c.is_alphanumeric())
            {
                return true;
            }
        }
    }
    false
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit()
        || c.is_whitespace()
        || matches!(c, '.' | '-' | '–' | '—' | '/' | ':' | '(' | ')' | '[' | ']')
        || matches!(c, 'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm' | 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::pdfoxide::Line;
    use crate::geom::Rect;
    use crate::layout::{Column, ColumnLayout};

    fn line(text: &str, y: f32) -> Line {
        Line {
            text: text.to_string(),
            rect: Rect::new(10.0, y, 200.0, y + 10.0),
            size: 9.0,
            bold: false,
            italic: false,
            mono: false,
            math_ratio: 0.0,
            words: Vec::new(),
            length: text.len(),
        }
    }

    fn page(number: usize, header: &str, footer: &str) -> PageLayout {
        let mut layout = PageLayout {
            number,
            width: 595.0,
            height: 842.0,
            ..PageLayout::default()
        };
        layout.columns = ColumnLayout { columns: vec![Column { x0: 0.0, x1: 595.0 }], gutters: vec![] };
        layout.lines = vec![
            line(header, 20.0),
            line("Real body text stays here.", 400.0),
            line(footer, 820.0),
        ];
        layout
    }

    #[test]
    fn repeated_header_and_page_numbers_are_learned() {
        let pages: Vec<PageLayout> = (1..=4)
            .map(|n| page(n, "Journal of Things 2024", &format!("- {n} -")))
            .collect();
        let clean = Clean::default();
        let furniture = detect(&pages, &clean);
        assert!(furniture.contains("Journal of things 2024"), "header shape, digits normalised");
        assert!(furniture.contains("- 7 -"));
        assert!(!furniture.contains("Real body text stays here."));
    }

    #[test]
    fn a_single_page_never_loses_its_text() {
        let pages = vec![page(1, "Some Title", "1")];
        let furniture = detect(&pages, &Clean::default());
        assert_eq!(furniture.len(), 0, "no repetition on one page");
        assert!(!furniture.contains("Some Title"), "a header needs repetition to be dropped");
        // …but a bare number in the band is still a page number.
        assert!(is_page_number("1"));
        assert!(furniture.contains("1"));
    }

    #[test]
    fn stripping_removes_only_band_lines() {
        let mut pages: Vec<PageLayout> = (1..=3)
            .map(|n| page(n, "Running title", &format!("{n}")))
            .collect();
        let furniture = detect(&pages, &Clean::default());
        let dropped = strip(&mut pages, &furniture);
        assert_eq!(dropped, 6); // header + footer on three pages
        assert_eq!(pages[0].lines.len(), 1);
        assert_eq!(pages[0].lines[0].text, "Real body text stays here.");
    }

    #[test]
    fn page_number_shapes() {
        for text in ["12", "2024", "- 12 -", "Page 12", "p. 12", "12 / 30", "12 of 30", "iv", "第 12 页"] {
            assert!(is_page_number(text), "{text}");
        }
        for text in ["Chapter 3", "12 March 2024", "Figure 2: results", "ISO 9001"] {
            assert!(!is_page_number(text), "{text}");
        }
    }

    #[test]
    fn normalisation_keeps_shape_but_not_digits() {
        assert_eq!(normalize("Page   3 of 12"), "page # of ##");
        assert_eq!(normalize("Ünïcode OK"), "ünïcode ok");
    }
}
