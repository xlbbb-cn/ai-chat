//! Conversion options.
//!
//! Everything the pipeline does is driven from here: nothing reads the
//! environment, nothing is static. The defaults are tuned for "the file is a
//! paper more often than not", because that is the hard case; plain-text PDFs
//! behave sensibly with the same defaults.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::formula::FormulaRecognizer;

/// Which layout strategy to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParseMode {
    /// Detect from the PDF itself (see [`crate::router`]).
    #[default]
    Auto,
    /// Plain-text PDF: top-down reading order, conservative heading detection.
    PlainText,
    /// Academic paper: column layout, title/abstract/captions/formulas.
    Paper,
    /// Force the GROBID backend (requires the `grobid` feature and a server).
    ForceGrobid,
}

/// Where the page-layout analysis comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutBackend {
    /// Built-in geometry analysis (`layout` + `reading_order`).
    #[default]
    Builtin,
    /// GROBID TEI output (needs the `grobid` feature and a reachable server).
    Grobid,
}

/// GROBID connection settings.
#[derive(Debug, Clone)]
pub struct GrobidConfig {
    /// Base URL, e.g. `http://localhost:8070`.
    pub base_url: String,
    /// Give up on the server after this long.
    pub timeout_secs: u64,
    /// Consolidate header metadata through CrossRef.
    pub consolidate_header: bool,
}

impl Default for GrobidConfig {
    fn default() -> Self {
        GrobidConfig {
            base_url: "http://localhost:8070".to_string(),
            timeout_secs: 60,
            consolidate_header: false,
        }
    }
}

/// Header/footer and paragraph clean-up behaviour.
#[derive(Debug, Clone)]
pub struct Clean {
    /// Drop running headers/footers and page numbers.
    pub remove_headers_footers: bool,
    /// A line counts as running furniture when it appears on at least this
    /// fraction of pages (0..=1).
    pub header_footer_threshold: f32,
    /// Size of the top and bottom band, as a fraction of page height.
    pub band_ratio: f32,
    /// Re-join words hyphenated across a line break (`inter-` + `national`).
    pub join_hyphenation: bool,
    /// Merge paragraphs that continue across a page or column break.
    pub merge_paragraphs: bool,
    /// Collapse repeated spaces inside a line (PDF text layers often have them).
    pub collapse_whitespace: bool,
    /// Drop blocks whose text is empty after cleaning.
    pub drop_empty: bool,
}

impl Default for Clean {
    fn default() -> Self {
        Clean {
            remove_headers_footers: true,
            header_footer_threshold: 0.5,
            band_ratio: 0.08,
            join_hyphenation: true,
            merge_paragraphs: true,
            collapse_whitespace: true,
            drop_empty: true,
        }
    }
}

/// How the Markdown writer renders the IR.
#[derive(Debug, Clone)]
pub struct Markdown {
    /// Emit a YAML front-matter block with title/author/page count.
    pub front_matter: bool,
    /// What to write at a [`crate::ir::Block::PageBreak`].
    pub page_breaks: PageBreaks,
    /// Added to every heading level (so `1` can become `2` under a document title).
    pub heading_offset: u8,
    /// GFM pipe tables or raw HTML tables.
    pub table_style: TableStyle,
    /// `$…$` / `$$…$$` or a plain-text fallback.
    pub formula_style: FormulaStyle,
    /// Wrap inline math-looking runs (`x[n]`, `α≤β`) in `$…$`.
    pub inline_math: bool,
    /// Escape Markdown metacharacters in body text.
    pub escape: bool,
    /// Keep the image caption as a following italic line as well as alt text.
    pub image_caption_line: bool,
}

impl Default for Markdown {
    fn default() -> Self {
        Markdown {
            front_matter: true,
            page_breaks: PageBreaks::Comment,
            heading_offset: 0,
            table_style: TableStyle::Gfm,
            formula_style: FormulaStyle::Latex,
            inline_math: true,
            escape: true,
            image_caption_line: false,
        }
    }
}

/// Page-break rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageBreaks {
    /// No marker at all — the Markdown flows continuously.
    Off,
    /// `<!-- page 3 -->`.
    #[default]
    Comment,
    /// A horizontal rule.
    Rule,
}

/// Table rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableStyle {
    /// GitHub-flavoured pipe table (first row becomes the header).
    #[default]
    Gfm,
    /// `<table>` HTML, which survives colspan-less merged cells better.
    Html,
    /// Indented plain text, cell-aligned.
    Text,
}

/// Formula rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormulaStyle {
    /// `$$…$$` / `$…$`.
    #[default]
    Latex,
    /// Plain text, surrounded by blank lines.
    Plain,
}

/// Extracted-image handling.
#[derive(Debug, Clone)]
pub struct Assets {
    /// Directory the images are written to. `None` disables extraction (the
    /// renderer then emits an HTML comment placeholder).
    pub dir: Option<PathBuf>,
    /// Sub-directory (relative to `dir`) holding the images.
    pub folder: String,
    /// Images smaller than this (in pixels, both sides) are dropped — they are
    /// usually rules, bullets or artefacts.
    pub min_pixels: u32,
    /// Write identical image payloads only once.
    pub dedupe: bool,
    /// Keep JPEG data as-is instead of re-encoding to PNG.
    pub keep_jpeg: bool,
    /// Emit absolute paths in the Markdown instead of `folder/name`.
    pub link_absolute: bool,
}

impl Default for Assets {
    fn default() -> Self {
        Assets {
            dir: None,
            folder: "assets".to_string(),
            min_pixels: 48,
            dedupe: true,
            keep_jpeg: true,
            link_absolute: false,
        }
    }
}

impl Assets {
    /// Write images into `dir`.
    pub fn writing_to(dir: impl Into<PathBuf>) -> Self {
        Assets { dir: Some(dir.into()), ..Assets::default() }
    }
}

/// Formula recognition settings.
#[derive(Clone, Default)]
pub struct FormulaOptions {
    /// Recognise display formulas from text geometry (default off: the
    /// heuristics are useful for papers, noisy for plain text).
    pub detect: bool,
    /// Image-based recognizer used when a formula is an embedded image.
    pub recognizer: Option<Arc<dyn FormulaRecognizer>>,
}

impl fmt::Debug for FormulaOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormulaOptions")
            .field("detect", &self.detect)
            .field("recognizer", &self.recognizer.as_ref().map(|r| r.name()))
            .finish()
    }
}

impl FormulaOptions {
    /// Enable text-geometry formula detection.
    pub fn enabled() -> Self {
        FormulaOptions { detect: true, recognizer: None }
    }

    /// Attach an image-based recognizer (also enables detection).
    pub fn with_recognizer(mut self, recognizer: Arc<dyn FormulaRecognizer>) -> Self {
        self.detect = true;
        self.recognizer = Some(recognizer);
        self
    }
}

/// The full option set for one conversion.
#[derive(Debug, Clone)]
pub struct Options {
    pub mode: ParseMode,
    pub layout_backend: LayoutBackend,
    pub grobid: GrobidConfig,
    /// Convert only pages `from..=to` (1-based, inclusive).
    pub page_range: Option<(usize, usize)>,
    /// Stop after this many pages.
    pub max_pages: Option<usize>,
    /// Password for encrypted PDFs (owner/user, tried in that order).
    pub password: Option<String>,
    pub clean: Clean,
    pub markdown: Markdown,
    pub assets: Assets,
    pub formula: FormulaOptions,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: ParseMode::Auto,
            layout_backend: LayoutBackend::Builtin,
            grobid: GrobidConfig::default(),
            page_range: None,
            max_pages: None,
            password: None,
            clean: Clean::default(),
            markdown: Markdown::default(),
            assets: Assets::default(),
            formula: FormulaOptions::default(),
        }
    }
}

impl Options {
    /// Plain-text profile: no column analysis, no formula detection.
    pub fn plain_text() -> Self {
        Options {
            mode: ParseMode::PlainText,
            markdown: Markdown { front_matter: false, ..Markdown::default() },
            ..Options::default()
        }
    }

    /// Academic-paper profile: column analysis on, formulas detected.
    pub fn paper() -> Self {
        Options {
            mode: ParseMode::Paper,
            formula: FormulaOptions::enabled(),
            ..Options::default()
        }
    }

    /// Write extracted images into `dir`.
    pub fn with_assets(mut self, dir: impl Into<PathBuf>) -> Self {
        self.assets.dir = Some(dir.into());
        self
    }

    /// Restrict the conversion to a page range (1-based, inclusive).
    pub fn with_page_range(mut self, from: usize, to: usize) -> Self {
        self.page_range = Some((from.max(1), to.max(from)));
        self
    }

    /// Keep at most `n` pages.
    pub fn with_max_pages(mut self, n: usize) -> Self {
        self.max_pages = Some(n);
        self
    }

    /// `true` when the resolved page index (1-based) should be converted.
    pub fn includes_page(&self, page: usize) -> bool {
        if let Some((from, to)) = self.page_range {
            if page < from || page > to {
                return false;
            }
        }
        if let Some(max) = self.max_pages {
            let first = self.page_range.map(|(from, _)| from).unwrap_or(1);
            if page >= first + max {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_selection_honours_range_and_limit() {
        let opts = Options::default().with_page_range(2, 9).with_max_pages(3);
        assert!(!opts.includes_page(1));
        assert!(opts.includes_page(2));
        assert!(opts.includes_page(4));
        assert!(!opts.includes_page(5));
        assert!(!opts.includes_page(9));
    }

    #[test]
    fn plain_text_profile_disables_front_matter() {
        let opts = Options::plain_text();
        assert_eq!(opts.mode, ParseMode::PlainText);
        assert!(!opts.markdown.front_matter);
        assert!(!opts.formula.detect);
        assert!(Options::paper().formula.detect);
    }
}
