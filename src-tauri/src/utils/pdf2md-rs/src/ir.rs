//! The unified document intermediate representation (IR).
//!
//! Every backend — the built-in PDFOxide text extractor, the page-layout
//! analyser, or an optional GROBID run — produces *this* structure, and every
//! renderer consumes it. Nothing downstream of the IR is allowed to look at
//! glyphs, coordinates or PDF objects again, which is what keeps the Markdown
//! writer (and any future renderer: HTML, DOCX, plain text, an editor view)
//! independent from the parsing side.
//!
//! The IR is deliberately flat: a single [`Document::blocks`] sequence in
//! reading order, with [`Block::PageBreak`] marking page boundaries. Page
//! geometry lives in [`Document::pages`] for consumers that need it (caption
//! anchoring, hit-testing in a viewer) but the renderer never has to care.
//!
//! ```
//! use pdf2md_rs::ir::{Block, Document, Metadata, Page};
//!
//! let mut doc = Document::new(Metadata::default());
//! doc.push_page(Page { page_number: 1, width: 595.0, height: 842.0 });
//! doc.push(Block::Heading { level: 1, text: "A Title".into() });
//! doc.push(Block::Paragraph { text: "Body text.".into() });
//! assert_eq!(doc.blocks.len(), 2);
//! ```

use serde::{Deserialize, Serialize};

/// A parsed document: metadata, page geometry and the block stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Document {
    pub metadata: Metadata,
    pub pages: Vec<Page>,
    pub blocks: Vec<Block>,
}

/// Document-level metadata, taken from the PDF information dictionary when it
/// is present and filled in by the layout analysis otherwise (title guess).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub title: Option<String>,
    /// One entry per author. A single `/Author` string containing `;`, `,` or
    /// ` and ` is split into several entries.
    pub author: Vec<String>,
    pub page_count: usize,
}

/// Page geometry, in PDF points with a top-left origin.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Page {
    /// 1-based page number.
    pub page_number: usize,
    pub width: f32,
    pub height: f32,
}

/// One unit of content in reading order.
///
/// The first seven variants are the canonical set; the rest are additive
/// extensions that a renderer may ignore when it only supports the core set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        text: String,
    },
    Table {
        rows: Vec<Vec<String>>,
    },
    Image {
        /// Path as it must appear in the Markdown document (POSIX separators,
        /// relative to the document unless [`crate::Assets::link_absolute`]).
        path: String,
        caption: Option<String>,
    },
    Formula {
        /// LaTeX source. A native PDF parser cannot recover this: the built-in
        /// [`crate::formula::HeuristicFormula`] transcriber emits an escaped
        /// plain-text approximation (`\text{…}`) and a real recognizer can be
        /// plugged in through [`crate::formula::FormulaExtractor`].
        latex: String,
        /// Display formula (`$$…$$`) versus inline (`$…$`).
        display: bool,
    },
    Code {
        language: Option<String>,
        source: String,
    },
    /// Emitted where the PDF starts a new page (`Options::markdown.page_breaks`).
    PageBreak,

    // ── Extensions ───────────────────────────────────────────────────────────
    /// A list item. `ordered` distinguishes `1.` from `•`; `level` is the
    /// nesting depth deduced from indentation (0 = top level).
    ListItem {
        text: String,
        ordered: bool,
        level: u8,
    },
    /// Block quote / indented quotation.
    Quote {
        text: String,
    },
    /// A horizontal rule (a drawn separator that is not a table border).
    Rule,
}

impl Block {
    /// `true` for blocks that render as a single short line.
    pub fn is_heading(&self) -> bool {
        matches!(self, Block::Heading { .. })
    }

    /// `true` when the block separates rather than contains content.
    pub fn is_break(&self) -> bool {
        matches!(self, Block::PageBreak | Block::Rule)
    }

    /// The plain text of the block, if it has any.
    pub fn text(&self) -> Option<&str> {
        match self {
            Block::Heading { text, .. }
            | Block::Paragraph { text }
            | Block::Quote { text }
            | Block::Code { source: text, .. }
            | Block::ListItem { text, .. } => Some(text),
            Block::Formula { latex, .. } => Some(latex),
            Block::Image { caption, .. } => caption.as_deref(),
            Block::Table { .. } | Block::PageBreak | Block::Rule => None,
        }
    }

    /// `true` when the block carries no content worth writing.
    pub fn is_empty(&self) -> bool {
        match self {
            Block::Table { rows } => rows.iter().all(|r| r.iter().all(|c| c.trim().is_empty())),
            Block::Paragraph { text } | Block::Quote { text } => text.trim().is_empty(),
            Block::Heading { text, .. } => text.trim().is_empty(),
            Block::Code { source, .. } => source.trim().is_empty(),
            Block::ListItem { text, .. } => text.trim().is_empty(),
            Block::Formula { latex, .. } => latex.trim().is_empty(),
            Block::Image { .. } | Block::PageBreak | Block::Rule => false,
        }
    }
}

impl Document {
    /// An empty document with the given metadata.
    pub fn new(metadata: Metadata) -> Self {
        Document { metadata, pages: Vec::new(), blocks: Vec::new() }
    }

    /// Append one block to the reading-order stream.
    pub fn push(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// Register a page's geometry. Page numbers are 1-based and must be
    /// appended in order.
    pub fn push_page(&mut self, page: Page) {
        self.pages.push(page);
    }

    /// The page whose geometry belongs to the given block index, i.e. the
    /// number of [`Block::PageBreak`]s before it plus one.
    pub fn page_of_block(&self, index: usize) -> usize {
        1 + self.blocks[..index.min(self.blocks.len())]
            .iter()
            .filter(|b| matches!(b, Block::PageBreak))
            .count()
    }

    /// Serialise the whole IR (used when the Tauri layer hands it to the UI).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Round-trip helper for [`Document::to_json`].
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Total number of non-break blocks.
    pub fn content_block_count(&self) -> usize {
        self.blocks.iter().filter(|b| !b.is_break()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Document {
        let mut doc = Document::new(Metadata {
            title: Some("Demo".into()),
            author: vec!["Ada".into()],
            page_count: 2,
        });
        doc.push_page(Page { page_number: 1, width: 595.0, height: 842.0 });
        doc.push(Block::Heading { level: 1, text: "Demo".into() });
        doc.push(Block::Paragraph { text: "Hello".into() });
        doc.push(Block::PageBreak);
        doc.push(Block::Paragraph { text: "World".into() });
        doc
    }

    #[test]
    fn page_of_block_counts_page_breaks() {
        let doc = sample();
        assert_eq!(doc.page_of_block(0), 1);
        assert_eq!(doc.page_of_block(2), 1);
        assert_eq!(doc.page_of_block(3), 2);
        assert_eq!(doc.page_of_block(4), 2);
    }

    #[test]
    fn json_round_trip_keeps_blocks() {
        let doc = sample();
        let json = doc.to_json().unwrap();
        let back = Document::from_json(&json).unwrap();
        assert_eq!(back.blocks, doc.blocks);
        assert_eq!(back.metadata.author, vec!["Ada".to_string()]);
        assert_eq!(back.content_block_count(), 3);
    }

    #[test]
    fn empty_blocks_are_detected() {
        assert!(Block::Paragraph { text: "  ".into() }.is_empty());
        assert!(Block::Table { rows: vec![vec!["".into()]] }.is_empty());
        assert!(!Block::PageBreak.is_empty());
        assert!(!Block::Rule.is_empty());
    }
}
