//! Clean-up: from "lines on a page" to readable Markdown blocks.
//!
//! Three jobs, in this order:
//!
//! 1. **Geometry-level** ([`furniture`]) — running headers, footers and page
//!    numbers are removed from the page *lines* before paragraphs exist. This
//!    module only re-exports it, because the pipeline has to call it between
//!    layout analysis and parsing.
//! 2. **Text-level** ([`text`]) — line breaks become spaces, hyphenated words are
//!    rejoined, repeated spaces collapse.
//! 3. **Document-level** (this module) — a paragraph broken by a page or column
//!    break becomes one paragraph again, empty blocks disappear, and page breaks
//!    at the edges of the document are dropped.
//!
//! Nothing here guesses content: it only removes artefacts of the medium.

pub mod furniture;
pub mod text;

pub use furniture::Furniture;

use crate::ir::{Block, Document};
use crate::options::Clean;

/// What the clean-up stage changed, for logs and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanReport {
    /// Paragraphs that were joined and re-flowed.
    pub reflowed: usize,
    /// Paragraphs merged into their predecessor across a page/column break.
    pub merged: usize,
    /// Consecutive duplicate paragraphs removed.
    pub deduped: usize,
    /// Empty blocks removed.
    pub dropped: usize,
    /// Standalone page numbers removed.
    pub page_numbers: usize,
    /// Leading/trailing/doubled page breaks removed.
    pub page_breaks: usize,
}

/// Clean a whole document in place.
pub fn document(doc: &mut Document, clean: &Clean) -> CleanReport {
    let mut report = CleanReport::default();
    if !clean.collapse_whitespace && !clean.join_hyphenation && !clean.merge_paragraphs && !clean.drop_empty
    {
        return report;
    }

    // 1. Re-flow every text block.
    for block in &mut doc.blocks {
        match block {
            Block::Paragraph { text } | Block::Quote { text } | Block::ListItem { text, .. } => {
                let joined = text::join_lines(text, clean.join_hyphenation);
                *text = if clean.collapse_whitespace {
                    text::collapse_whitespace(&joined)
                } else {
                    joined
                };
                report.reflowed += 1;
            }
            Block::Heading { text, .. } => {
                let joined = text::join_lines(text, false);
                *text = text::collapse_whitespace(&joined);
                report.reflowed += 1;
            }
            Block::Code { source, .. } => {
                if source.contains('\r') {
                    *source = source.replace("\r\n", "\n").replace('\r', "\n");
                }
            }
            Block::Formula { .. }
            | Block::Table { .. }
            | Block::Image { .. }
            | Block::PageBreak
            | Block::Rule => {}
        }
    }

    // 2. Merge continuations and drop duplicates / empties.
    let blocks = std::mem::take(&mut doc.blocks);
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Paragraph { text } if clean.merge_paragraphs => {
                if merge_into_previous(&mut out, &text) {
                    report.merged += 1;
                    continue;
                }
                out.push(Block::Paragraph { text });
            }
            Block::Paragraph { text } => out.push(Block::Paragraph { text }),
            other => out.push(other),
        }
    }

    // 3. Drop consecutive duplicate paragraphs (a leftover running title that
    //    repeated inside the body, or a table caption emitted twice).
    if clean.drop_empty {
        let mut deduped: Vec<Block> = Vec::with_capacity(out.len());
        for block in out {
            let duplicate = match (&block, deduped.last()) {
                (Block::Paragraph { text }, Some(Block::Paragraph { text: previous })) => {
                    !text.is_empty() && text == previous
                }
                _ => false,
            };
            if duplicate {
                report.deduped += 1;
                continue;
            }
            deduped.push(block);
        }
        out = deduped;
    }

    // 4. Drop standalone page numbers that sit at a page boundary.
    if clean.remove_headers_footers {
        out = drop_page_numbers(out, &mut report);
    }

    // 5. Drop empty blocks.
    if clean.drop_empty {
        let before = out.len();
        out.retain(|block| !block.is_empty());
        report.dropped += before - out.len();
    }

    // 6. Tidy page breaks: none at the start/end, none doubled.
    tidy_page_breaks(&mut out, &mut report);

    doc.blocks = out;
    report
}

/// Append `text` to the last paragraph if it continues it, removing any page
/// break between them. Returns `true` when the merge happened.
fn merge_into_previous(out: &mut Vec<Block>, text: &str) -> bool {
    let Some(position) = out.iter().rposition(|block| !matches!(block, Block::PageBreak)) else {
        return false;
    };
    let Block::Paragraph { text: previous } = &mut out[position] else {
        return false;
    };
    if !text::continues(previous, text) {
        return false;
    }
    if !previous.ends_with(' ') && !text::is_cjk(previous.chars().last().unwrap_or(' ')) {
        previous.push(' ');
    }
    previous.push_str(text);
    // Everything after the paragraph was the page break we just bridged.
    out.truncate(position + 1);
    true
}

fn drop_page_numbers(blocks: Vec<Block>, report: &mut CleanReport) -> Vec<Block> {
    let length = blocks.len();
    let mut out = Vec::with_capacity(length);
    for (index, block) in blocks.iter().enumerate() {
        let boundary = index == 0
            || index + 1 == length
            || matches!(blocks.get(index + 1), Some(Block::PageBreak))
            || matches!(blocks.get(index.wrapping_sub(1)), Some(Block::PageBreak));
        if let Block::Paragraph { text } = block {
            if boundary && furniture::is_page_number(text) {
                report.page_numbers += 1;
                continue;
            }
        }
        out.push(block.clone());
    }
    out
}

fn tidy_page_breaks(blocks: &mut Vec<Block>, report: &mut CleanReport) {
    let before = blocks.len();
    // No leading page break.
    while matches!(blocks.first(), Some(Block::PageBreak)) {
        blocks.remove(0);
    }
    // No trailing page break.
    while matches!(blocks.last(), Some(Block::PageBreak)) {
        blocks.pop();
    }
    // No doubled page break.
    let mut previous_break = false;
    blocks.retain(|block| {
        let is_break = matches!(block, Block::PageBreak);
        let keep = !(is_break && previous_break);
        previous_break = is_break;
        keep
    });
    report.page_breaks += before.saturating_sub(blocks.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Metadata;

    fn document_with(blocks: Vec<Block>) -> Document {
        let mut doc = Document::new(Metadata::default());
        doc.blocks = blocks;
        doc
    }

    #[test]
    fn a_paragraph_split_by_a_page_break_is_merged() {
        let mut doc = document_with(vec![
            Block::Paragraph {
                text: "The experiment ran for three weeks and produced a large\n\
                       amount of data that we then analysed".into(),
            },
            Block::PageBreak,
            Block::Paragraph { text: "with a simple linear model.".into() },
        ]);
        let report = document(&mut doc, &Clean::default());
        assert_eq!(report.merged, 1);
        assert_eq!(doc.blocks.len(), 1);
        let Block::Paragraph { text } = &doc.blocks[0] else { panic!("expected a paragraph") };
        assert!(text.contains("analysed with a simple linear model."), "{text}");
    }

    #[test]
    fn a_finished_paragraph_is_not_merged() {
        let mut doc = document_with(vec![
            Block::Paragraph { text: "Results are shown below.".into() },
            Block::PageBreak,
            Block::Paragraph { text: "A new section starts here.".into() },
        ]);
        document(&mut doc, &Clean::default());
        assert_eq!(doc.blocks.len(), 3);
    }

    #[test]
    fn hyphenation_and_whitespace_are_cleaned() {
        let mut doc = document_with(vec![Block::Paragraph {
            text: "This is an inter-\nnational   example.".into(),
        }]);
        document(&mut doc, &Clean::default());
        let Block::Paragraph { text } = &doc.blocks[0] else { panic!() };
        assert_eq!(text, "This is an international example.");
    }

    #[test]
    fn leading_trailing_and_double_page_breaks_go_away() {
        let mut doc = document_with(vec![
            Block::PageBreak,
            Block::Paragraph { text: "one".into() },
            Block::PageBreak,
            Block::PageBreak,
            Block::Paragraph { text: "two".into() },
            Block::PageBreak,
        ]);
        document(&mut doc, &Clean::default());
        assert_eq!(doc.blocks.len(), 3);
    }

    #[test]
    fn standalone_page_numbers_at_boundaries_are_dropped() {
        let mut doc = document_with(vec![
            Block::Paragraph { text: "text".into() },
            Block::Paragraph { text: "12".into() },
            Block::PageBreak,
            Block::Paragraph { text: "more".into() },
        ]);
        document(&mut doc, &Clean::default());
        assert_eq!(doc.blocks.len(), 3);
        assert!(!doc.blocks.iter().any(|b| b.text() == Some("12")));
    }

    #[test]
    fn empty_blocks_are_dropped() {
        let mut doc = document_with(vec![
            Block::Paragraph { text: "  ".into() },
            Block::Table { rows: vec![vec!["".into()]] },
            Block::Paragraph { text: "kept".into() },
        ]);
        document(&mut doc, &Clean::default());
        assert_eq!(doc.blocks.len(), 1);
    }
}
