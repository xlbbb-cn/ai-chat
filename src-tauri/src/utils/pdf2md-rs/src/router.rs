//! PDF type and layout detection.
//!
//! Every conversion needs one decision before parsing: is this a plain-text PDF,
//! an academic paper, or a scan without a text layer? Getting it wrong is not
//! fatal (both parsers share the same IR) but it is expensive: papers need column
//! analysis, captions and formula detection, plain text needs none of it, and a
//! scan cannot be parsed at all without OCR.
//!
//! The detector is a small, explainable scoring function. Every point it adds is
//! recorded in [`PdfProfile::reason`], so a surprising classification can be read
//! off the result instead of guessed.

use crate::extract::pdfoxide::PdfInfo;
use crate::layout::PageLayout;
use crate::options::ParseMode;

/// What kind of document this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentClass {
    /// One column, prose, headings by typography.
    PlainText,
    /// Academic paper: columns, abstract, captions, equations, tables.
    Paper,
    /// No usable text layer (image-only pages).
    Scanned,
}

impl DocumentClass {
    /// `plain-text` / `paper` / `scanned`.
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentClass::PlainText => "plain-text",
            DocumentClass::Paper => "paper",
            DocumentClass::Scanned => "scanned",
        }
    }
}

/// The detection result, with the evidence that produced it.
#[derive(Debug, Clone)]
pub struct PdfProfile {
    pub class: DocumentClass,
    /// Number of columns on the majority of pages (1 for single column).
    pub columns: usize,
    pub has_text_layer: bool,
    pub image_count: usize,
    pub rule_count: usize,
    /// Share of characters drawn in a math font (0..=1).
    pub math_density: f32,
    pub char_count: usize,
    pub page_count: usize,
    /// Why the class was chosen, in plain words.
    pub reason: Vec<&'static str>,
}

impl PdfProfile {
    /// `true` when column analysis is worth running.
    pub fn is_multi_column(&self) -> bool {
        self.columns > 1
    }
}

/// Average characters per page below which the PDF is treated as a scan.
const SCANNED_CHARS_PER_PAGE: usize = 40;

/// Classify a document from its analysed pages.
pub fn detect(pages: &[PageLayout], info: &PdfInfo, mode: ParseMode) -> PdfProfile {
    let page_count = pages.len();
    let char_count: usize = pages.iter().map(|p| p.char_count()).sum();
    let image_count: usize = pages.iter().map(|p| p.images.len()).sum();
    let rule_count: usize = pages.iter().map(|p| p.rules.len()).sum();
    let math_chars: usize = pages
        .iter()
        .map(|p| {
            p.lines
                .iter()
                .map(|l| (l.math_ratio * l.length as f32) as usize)
                .sum::<usize>()
        })
        .sum();
    let math_density = if char_count > 0 { math_chars as f32 / char_count as f32 } else { 0.0 };
    let columns = dominant_column_count(pages);

    let mut reason: Vec<&'static str> = Vec::new();
    let per_page = if page_count > 0 { char_count / page_count } else { 0 };

    if mode == ParseMode::Paper {
        reason.push("forced by ParseMode::Paper");
        return PdfProfile {
            class: DocumentClass::Paper,
            columns,
            has_text_layer: per_page >= SCANNED_CHARS_PER_PAGE,
            image_count,
            rule_count,
            math_density,
            char_count,
            page_count,
            reason,
        };
    }
    if mode == ParseMode::PlainText {
        reason.push("forced by ParseMode::PlainText");
        return PdfProfile {
            class: DocumentClass::PlainText,
            columns: 1,
            has_text_layer: per_page >= SCANNED_CHARS_PER_PAGE,
            image_count,
            rule_count,
            math_density,
            char_count,
            page_count,
            reason,
        };
    }

    if per_page < SCANNED_CHARS_PER_PAGE {
        reason.push("fewer than 40 characters per page (no text layer)");
        return PdfProfile {
            class: DocumentClass::Scanned,
            columns,
            has_text_layer: false,
            image_count,
            rule_count,
            math_density,
            char_count,
            page_count,
            reason,
        };
    }

    let mut score = 0;
    if columns >= 2 {
        score += 2;
        reason.push("two or more text columns");
    }
    if math_density > 0.004 {
        score += 1;
        reason.push("math/symbol font characters present");
    }
    if has_any(pages, &["abstract", "摘要"]) {
        score += 2;
        reason.push("abstract section found");
    }
    if has_any(pages, &["references", "bibliography", "参考文献"]) {
        score += 1;
        reason.push("references section found");
    }
    if page_count > 0 && image_count * 2 >= page_count {
        score += 1;
        reason.push("at least one figure every other page");
    }
    if !info.authors.is_empty() && info.title.is_some() {
        score += 1;
        reason.push("title and authors in the information dictionary");
    }

    let class = if score >= 3 { DocumentClass::Paper } else { DocumentClass::PlainText };
    reason.push(if class == DocumentClass::Paper { "paper score >= 3" } else { "paper score < 3" });
    PdfProfile {
        class,
        columns,
        has_text_layer: true,
        image_count,
        rule_count,
        math_density,
        char_count,
        page_count,
        reason,
    }
}

/// The column count most pages agree on.
fn dominant_column_count(pages: &[PageLayout]) -> usize {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for page in pages {
        let columns = page.columns.len();
        match counts.iter_mut().find(|(value, _)| *value == columns) {
            Some((_, weight)) => *weight += 1,
            None => counts.push((columns, 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, weight)| *weight).map(|(value, _)| value).unwrap_or(1)
}

/// Does any page contain one of these (lowercase) section names as a line?
fn has_any(pages: &[PageLayout], needles: &[&str]) -> bool {
    pages.iter().any(|page| {
        page.lines.iter().any(|line| {
            let text = line.text.trim().trim_end_matches([':', '：']).to_ascii_lowercase();
            needles.iter().any(|needle| text == *needle)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::pdfoxide::Line;
    use crate::geom::Rect;
    use crate::layout::Column;

    fn page(lines: Vec<(&str, f32)>, columns: usize) -> PageLayout {
        let mut layout = PageLayout {
            number: 1,
            width: 595.0,
            height: 842.0,
            ..Default::default()
        };
        layout.columns = if columns > 1 {
            ColumnLayoutTest::two()
        } else {
            crate::layout::ColumnLayout {
                columns: vec![Column { x0: 60.0, x1: 550.0 }],
                gutters: Vec::new(),
            }
        };
        for (index, (text, math)) in lines.into_iter().enumerate() {
            layout.lines.push(Line {
                text: text.to_string(),
                rect: Rect::new(60.0, 80.0 + index as f32 * 12.0, 550.0, 90.0 + index as f32 * 12.0),
                size: 10.0,
                bold: false,
                italic: false,
                mono: false,
                math_ratio: math,
                words: Vec::new(),
                length: text.chars().count(),
            });
        }
        layout
    }

    struct ColumnLayoutTest;
    impl ColumnLayoutTest {
        fn two() -> crate::layout::ColumnLayout {
            crate::layout::ColumnLayout {
                columns: vec![Column { x0: 60.0, x1: 290.0 }, Column { x0: 320.0, x1: 550.0 }],
                gutters: vec![(290.0, 320.0)],
            }
        }
    }

    #[test]
    fn scan_without_text_is_detected() {
        let pages = vec![page(vec![], 1)];
        let profile = detect(&pages, &PdfInfo::default(), ParseMode::Auto);
        assert_eq!(profile.class, DocumentClass::Scanned);
        assert!(!profile.has_text_layer);
    }

    #[test]
    fn two_column_paper_with_abstract_is_a_paper() {
        let body = "The quick brown fox jumps over the lazy dog and keeps on running";
        let pages = vec![
            page(vec![("Abstract", 0.0), (body, 0.0), (body, 0.0), (body, 0.0)], 2),
            page(vec![("References", 0.0), (body, 0.0), (body, 0.0)], 2),
        ];
        let profile = detect(&pages, &PdfInfo::default(), ParseMode::Auto);
        assert_eq!(profile.class, DocumentClass::Paper);
        assert_eq!(profile.columns, 2);
        assert!(profile.reason.contains(&"abstract section found"));
    }

    #[test]
    fn single_column_prose_stays_plain() {
        let body = "A perfectly ordinary sentence in a plain text document, nothing academic";
        let pages = vec![page(vec![(body, 0.0), (body, 0.0), (body, 0.0)], 1)];
        let profile = detect(&pages, &PdfInfo::default(), ParseMode::Auto);
        assert_eq!(profile.class, DocumentClass::PlainText);
    }

    #[test]
    fn mode_overrides_detection() {
        let pages = vec![page(vec![("text", 0.0)], 1)];
        assert_eq!(detect(&pages, &PdfInfo::default(), ParseMode::Paper).class, DocumentClass::Paper);
        assert_eq!(
            detect(&pages, &PdfInfo::default(), ParseMode::PlainText).columns,
            1
        );
    }
}
