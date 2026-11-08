//! Page layout → IR blocks.
//!
//! This is where a page stops being geometry and becomes a document. The input
//! is [`PageLayout`] (lines grouped into columns and paragraphs, plus rules and
//! images); the output is a stream of [`Block`]s in reading order.
//!
//! The order of the decisions matters:
//!
//! 1. **Tables first** — they *consume* lines, and any paragraph that still
//!    holds a consumed line must not be emitted a second time as prose.
//! 2. **Captions** are attached to the figure they sit next to, and removed from
//!    the paragraph stream (otherwise "Figure 3: …" would appear twice).
//! 3. **Reading order** is decided per page by [`crate::layout::order`], which
//!    knows about columns, gutter-spanning blocks and footnotes. Sorting by `x`
//!    or by `y` alone is exactly the bug that interleaves two columns.
//! 4. **Classification** of each paragraph: display formula, heading, code,
//!    list item, or body text. The heading detector has the final say because it
//!    keeps the outline consistent across the whole document.
//!
//! Everything here is deliberately conservative: when the evidence is weak the
//! paragraph stays a paragraph. A lost heading costs a `#`; a false heading
//! breaks the table of contents.

use std::collections::HashSet;

use crate::assets::AssetWriter;
use crate::error::Result;
use crate::extract::pdfoxide::{ImagePlacement, Line, PdfInfo};
use crate::formula::HeuristicFormula;
use crate::geom::Rect;
use crate::ir::{Block, Document, Metadata, Page};
use crate::layout::{order, PageLayout, Paragraph};
use crate::options::Options;
use crate::router::PdfProfile;

use super::headings::{self, HeadingDetector};
use super::tables::{self, TableCandidate};

/// What the builder did, for logs and for `--verbose` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildReport {
    pub headings: usize,
    pub tables: usize,
    pub images: usize,
    pub formulas: usize,
    pub lists: usize,
    pub code: usize,
    /// Paragraphs that vanished because a table had already claimed their lines.
    pub absorbed_paragraphs: usize,
    /// Captions attached to a figure.
    pub captions: usize,
}

/// Build the IR for a whole document.
///
/// `layouts` must be in page order. `assets` may be disabled; figure blocks are
/// still emitted (with an empty path) so the reader knows something was there.
pub fn build(
    layouts: &[PageLayout],
    _profile: &PdfProfile,
    info: &PdfInfo,
    options: &Options,
    assets: &mut AssetWriter,
) -> Result<(Document, BuildReport)> {
    let mut report = BuildReport::default();
    let body_size = document_body_size(layouts);
    let title = title_candidate(layouts.first(), info, body_size);
    let emit_title = title.is_some() && !options.markdown.front_matter;

    // The document title occupies level 1 only when it is written into the body;
    // with front matter the first real section is already level 1.
    let base_level = if emit_title { 2 } else { 1 };
    let mut headings = HeadingDetector::new(body_size, base_level);
    let formula = HeuristicFormula;

    let mut metadata = Metadata {
        title: info
            .title
            .as_deref()
            .map(one_line)
            .filter(|t| !t.is_empty())
            .or_else(|| title.as_ref().map(|t| t.text.clone())),
        author: info.authors.clone(),
        page_count: layouts.len(),
    };
    // The line the authors came from is metadata, not content: it must not be
    // repeated as a paragraph under the title.
    let author_source = author_line(layouts.first());
    if metadata.author.is_empty() {
        if let Some(author) = &author_source {
            metadata.author = author.authors.clone();
        }
    }

    let mut document = Document::new(metadata);
    let mut title_emitted = false;

    for (page_index, layout) in layouts.iter().enumerate() {
        document.push_page(Page {
            page_number: layout.number,
            width: layout.width,
            height: layout.height,
        });

        let paragraphs = layout.paragraphs();
        let mut state = PageState::default();

        // 1. Tables claim their lines before any paragraph is emitted.
        state.collect_tables(layout);
        report.absorbed_paragraphs += paragraphs
            .iter()
            .filter(|paragraph| state.paragraph_consumed(paragraph))
            .count();
        // 2. Figure captions leave the paragraph stream.
        let captions = state.collect_captions(layout, &paragraphs, &mut report);
        // 3. Reading order over everything that is left.
        let ordered = order(state.items(layout, &paragraphs, &captions), &layout.columns, Some(layout.footer_band.start));

        // 4. Emit.
        for item in ordered {
            match item {
                Item::Table(table) => {
                    document.push(Block::Table { rows: table.rows });
                    report.tables += 1;
                }
                Item::Image(index, caption) => {
                    let image = &layout.images[index];
                    let path = write_image(layout.number, image, assets)?;
                    document.push(Block::Image { path, caption });
                    report.images += 1;
                }
                Item::Rule => document.push(Block::Rule),
                Item::Paragraph(index) => {
                    let paragraph = &paragraphs[index];
                    let text = state.paragraph_text(layout, paragraph);
                    if text.trim().is_empty() {
                        continue;
                    }
                    let single = one_line(&text);

                    // The "by Ada Lovelace and Alan Turing" line.
                    let is_author = page_index == 0
                        && author_source
                            .as_ref()
                            .is_some_and(|author| author.matches(&paragraph.rect, &single));
                    if is_author {
                        continue;
                    }

                    // The document title, taken from page 1.
                    let is_title = !title_emitted
                        && page_index == 0
                        && title.as_ref().is_some_and(|t| t.matches(&paragraph.rect, &single));
                    if is_title {
                        if emit_title {
                            document.push(Block::Heading { level: 1, text: single });
                            headings.reset();
                            report.headings += 1;
                        }
                        title_emitted = true;
                        continue;
                    }

                    // Display formula (only when the profile asked for it).
                    if options.formula.detect && !(paragraph.bold && paragraph.size > body_size * 1.05)
                    {
                        if let Some(candidate) = formula.detect(layout, paragraph) {
                            document.push(formula.block(&candidate));
                            report.formulas += 1;
                            continue;
                        }
                    }

                    // Source code.
                    if is_code(paragraph, &text) {
                        document.push(Block::Code {
                            language: None,
                            source: text.trim_end().to_string(),
                        });
                        report.code += 1;
                        continue;
                    }

                    // Lists: bullets always; numbers only when the line does not
                    // look like a section heading (body size, not bold).
                    if let Some(ordered) = list_marker(&single) {
                        let heading_like = ordered
                            && (paragraph.bold || paragraph.size > body_size * 1.08)
                            && headings::numbering_depth(&single).is_some();
                        if !heading_like {
                            let level = list_level(paragraph, layout, body_size);
                            let item = strip_list_marker(&single).to_string();
                            document.push(Block::ListItem { text: item, ordered, level });
                            report.lists += 1;
                            continue;
                        }
                    }

                    // Headings.
                    if let Some(level) = headings.infer(&single, paragraph.size, paragraph.bold) {
                        document.push(Block::Heading { level, text: single });
                        report.headings += 1;
                        continue;
                    }

                    // Body text.
                    if single.starts_with('>') {
                        document.push(Block::Quote {
                            text: single.trim_start_matches('>').trim_start().to_string(),
                        });
                    } else {
                        document.push(Block::Paragraph { text });
                    }
                }
            }
        }

        if page_index + 1 < layouts.len() {
            document.push(Block::PageBreak);
        }
    }

    Ok((document, report))
}

/// Per-page bookkeeping: which lines and paragraphs are already spoken for.
#[derive(Debug, Default)]
struct PageState {
    consumed_lines: HashSet<usize>,
    consumed_paragraphs: HashSet<usize>,
    tables: Vec<TableCandidate>,
}

impl PageState {
    fn collect_tables(&mut self, layout: &PageLayout) {
        for table in tables::ruled_tables(layout) {
            self.push_table(&table);
        }
        // Aligned-text tables, from the top of the page down. A line can only
        // start a table if an earlier table did not claim it.
        let mut index = 0;
        while index < layout.lines.len() {
            if self.consumed_lines.contains(&index) {
                index += 1;
                continue;
            }
            match tables::aligned_table(layout, index) {
                Some(table) if table.rows.len() >= 2 && table.column_count() >= 2 => {
                    let last = table.consumed_lines.iter().copied().max().unwrap_or(index);
                    self.push_table(&table);
                    index = last + 1;
                }
                _ => index += 1,
            }
        }
    }

    fn push_table(&mut self, table: &TableCandidate) {
        self.tables.push(table.clone());
        for line in &table.consumed_lines {
            self.consumed_lines.insert(*line);
        }
    }

    /// Find captions and pair each figure with the closest one.
    fn collect_captions(
        &mut self,
        layout: &PageLayout,
        paragraphs: &[Paragraph],
        report: &mut BuildReport,
    ) -> Vec<Option<String>> {
        let mut captions: Vec<Option<String>> = vec![None; layout.images.len()];
        for (image_index, image) in layout.images.iter().enumerate() {
            if !image.is_figure_sized(24.0) {
                continue;
            }
            let mut best: Option<(f32, usize)> = None;
            for (index, paragraph) in paragraphs.iter().enumerate() {
                if self.consumed_paragraphs.contains(&index) {
                    continue;
                }
                if self.paragraph_consumed(paragraph) {
                    continue;
                }
                let text = one_line(&layout.paragraph_text(paragraph));
                if !is_caption(&text) {
                    continue;
                }
                let below = paragraph.rect.y0 >= image.rect.y1 - 2.0
                    && paragraph.rect.y0 - image.rect.y1 < layout.height * 0.12;
                let above = paragraph.rect.y1 <= image.rect.y0 + 2.0
                    && image.rect.y0 - paragraph.rect.y1 < layout.height * 0.06;
                if !below && !above {
                    continue;
                }
                if paragraph.rect.horizontal_gap(&image.rect) > layout.width * 0.2 {
                    continue;
                }
                let distance = if below {
                    paragraph.rect.y0 - image.rect.y1
                } else {
                    image.rect.y0 - paragraph.rect.y1
                };
                if best.map(|(d, _)| distance < d).unwrap_or(true) {
                    best = Some((distance, index));
                }
            }
            if let Some((_, index)) = best {
                captions[image_index] =
                    Some(one_line(&layout.paragraph_text(&paragraphs[index])));
                self.consumed_paragraphs.insert(index);
                report.captions += 1;
            }
        }
        captions
    }

    fn paragraph_consumed(&self, paragraph: &Paragraph) -> bool {
        paragraph.lines.iter().all(|line| self.consumed_lines.contains(line))
    }

    /// Text of the paragraph with table-consumed lines removed.
    fn paragraph_text(&self, layout: &PageLayout, paragraph: &Paragraph) -> String {
        paragraph
            .lines
            .iter()
            .filter(|line| !self.consumed_lines.contains(line))
            .map(|line| layout.lines[*line].text.trim())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn items(
        &self,
        layout: &PageLayout,
        paragraphs: &[Paragraph],
        captions: &[Option<String>],
    ) -> Vec<(Rect, Item)> {
        let mut items = Vec::new();
        for (index, paragraph) in paragraphs.iter().enumerate() {
            if self.consumed_paragraphs.contains(&index) || self.paragraph_consumed(paragraph) {
                continue;
            }
            if self.inside_table(&paragraph.rect) {
                continue;
            }
            items.push((paragraph.rect, Item::Paragraph(index)));
        }
        for table in &self.tables {
            items.push((table.rect, Item::Table(table.clone())));
        }
        let page_area = (layout.width * layout.height).max(1.0);
        for (index, image) in layout.images.iter().enumerate() {
            if !image.is_figure_sized(24.0) || image.rect.area() > page_area * 0.85 {
                continue;
            }
            let caption = captions.get(index).cloned().flatten();
            items.push((image.rect, Item::Image(index, caption)));
        }
        for rule in &layout.rules {
            if !rule.is_horizontal() || rule.rect.width() < layout.text_box.width() * 0.3 {
                continue;
            }
            if layout.header_band.contains(rule.rect.center_y())
                || layout.footer_band.contains(rule.rect.center_y())
            {
                continue;
            }
            if self.inside_table(&rule.rect) {
                continue;
            }
            items.push((rule.rect, Item::Rule));
        }
        items
    }

    fn inside_table(&self, rect: &Rect) -> bool {
        self.tables.iter().any(|table| table.rect.expand(1.0).overlaps(rect, 0.6))
    }
}

#[derive(Debug, Clone)]
enum Item {
    Paragraph(usize),
    Table(TableCandidate),
    Image(usize, Option<String>),
    Rule,
}

/// The title of the first page, if there is one.
#[derive(Debug, Clone)]
struct TitleCandidate {
    text: String,
    rect: Rect,
}

impl TitleCandidate {
    fn matches(&self, rect: &Rect, text: &str) -> bool {
        self.text == text && self.rect.overlaps(rect, 0.2)
    }
}

fn title_candidate(
    page: Option<&PageLayout>,
    info: &PdfInfo,
    body_size: f32,
) -> Option<TitleCandidate> {
    let page = page?;
    if page.lines.is_empty() {
        return None;
    }
    let limit = page.height * 0.45;
    let info_title = info.title.as_deref().map(one_line);
    let mut best: Option<(f32, String, Rect)> = None;

    let mut candidates: Vec<&Line> = page
        .lines
        .iter()
        .filter(|line| line.rect.y0 < limit)
        .filter(|line| line.size >= body_size * 1.18)
        .filter(|line| !page.header_band.contains(line.rect.center_y()))
        .collect();
    candidates.sort_by(|a, b| {
        b.size
            .partial_cmp(&a.size)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.rect.y0.partial_cmp(&b.rect.y0).unwrap_or(std::cmp::Ordering::Equal))
    });

    for line in candidates {
        let text = one_line(&line.text);
        if text.is_empty() || text.chars().count() < 4 {
            continue;
        }
        if info_title.as_deref() == Some(text.as_str()) {
            return Some(TitleCandidate { text, rect: line.rect });
        }
        if best.as_ref().map(|(size, _, _)| line.size > *size).unwrap_or(true) {
            best = Some((line.size, text, line.rect));
        }
    }

    best.map(|(_, text, rect)| TitleCandidate { text, rect })
}

/// Median body size weighted by characters, over the whole document.
fn document_body_size(layouts: &[PageLayout]) -> f32 {
    let mut weighted: Vec<(f32, usize)> = Vec::new();
    for layout in layouts {
        for line in &layout.lines {
            weighted.push((line.size, line.length.max(1)));
        }
    }
    if weighted.is_empty() {
        return 10.0;
    }
    weighted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let total: usize = weighted.iter().map(|(_, n)| *n).sum();
    let mut acc = 0usize;
    for (size, count) in &weighted {
        acc += count;
        if acc * 2 >= total {
            return *size;
        }
    }
    weighted.last().map(|(s, _)| *s).unwrap_or(10.0)
}

/// `true` when the paragraph reads like source code.
fn is_code(paragraph: &Paragraph, text: &str) -> bool {
    if !paragraph.mono {
        return false;
    }
    let codeish = text.contains('{')
        || text.contains('}')
        || text.contains("();")
        || text.contains("=>")
        || text.contains("#!/")
        || text.contains("fn ")
        || text.contains("def ")
        || text.contains("</");
    if paragraph.lines.len() >= 3 {
        return true;
    }
    if paragraph.lines.len() >= 2 && codeish {
        return true;
    }
    codeish
        && matches!(text.trim_end().chars().last(), Some(';' | '}' | '{'))
}

/// Detect a bullet or number prefix. Returns `Some(true)` for ordered items.
fn list_marker(text: &str) -> Option<bool> {
    let trimmed = text.trim_start();
    let first = trimmed.chars().next()?;
    if matches!(first, '•' | '·' | '●' | '○' | '▪' | '◦' | '‣' | '⁃' | '–' | '—') {
        return Some(false);
    }
    // A hyphen, asterisk or plus only counts when followed by a space and text.
    if matches!(first, '-' | '*' | '+') {
        let rest = trimmed[1..].trim_start();
        return if trimmed[1..].starts_with(' ') && !rest.is_empty() { Some(false) } else { None };
    }
    if first == '(' {
        let closing = trimmed.find(')')?;
        let body = &trimmed[1..closing];
        if !body.is_empty()
            && body.chars().all(|c| c.is_ascii_alphanumeric())
            && trimmed[closing + 1..].starts_with(' ')
            && trimmed[closing + 1..].trim().len() > 1
        {
            return Some(true);
        }
        return None;
    }
    if first.is_ascii_digit() {
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = &trimmed[digits.len()..];
        let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
        return if rest.starts_with(' ') && rest.trim().len() > 1 { Some(true) } else { None };
    }
    if first.is_ascii_lowercase() {
        let rest = &trimmed[1..];
        if let Some(rest) = rest.strip_prefix(')').or_else(|| rest.strip_prefix('.')) {
            return if first <= 'h' && rest.starts_with(' ') && rest.trim().len() > 1 {
                Some(true)
            } else {
                None
            };
        }
    }
    None
}

/// Nesting level from the left indent relative to the column edge.
fn list_level(paragraph: &Paragraph, layout: &PageLayout, body_size: f32) -> u8 {
    let column = layout
        .columns
        .columns
        .get(paragraph.column)
        .copied()
        .or_else(|| layout.columns.columns.first().copied())
        .unwrap_or(crate::layout::Column { x0: 0.0, x1: layout.width });
    let indent = (paragraph.rect.x0 - column.x0).max(0.0);
    let step = (body_size * 1.5).max(6.0);
    ((indent / step).round() as u8).min(3)
}

fn write_image(page: usize, image: &ImagePlacement, assets: &mut AssetWriter) -> Result<String> {
    if !assets.is_enabled() {
        return Ok(String::new());
    }
    let Some(data) = &image.data else {
        return Ok(String::new());
    };
    Ok(assets.write(page, data)?.unwrap_or_default())
}

/// `true` when the line reads as a figure/table caption.
fn is_caption(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    let prefixes = [
        "figure", "fig.", "fig ", "table", "tab.", "algorithm", "listing", "scheme", "chart",
    ];
    if prefixes.iter().any(|prefix| lower.starts_with(prefix)) {
        return lower.chars().take(16).any(|c| c.is_ascii_digit()) || lower.contains(':');
    }
    ["图", "表"].iter().any(|prefix| lower.starts_with(prefix))
}

/// Lines under the title that read as `by A. Author` / `Authors: …`.
///
/// The line itself is returned too, so the page loop can drop it from the body:
/// the author belongs in the front matter, not in a paragraph of its own.
#[derive(Debug, Clone)]
struct AuthorLine {
    authors: Vec<String>,
    text: String,
    rect: Rect,
}

impl AuthorLine {
    fn matches(&self, rect: &Rect, text: &str) -> bool {
        self.text == text && self.rect.overlaps(rect, 0.2)
    }
}

fn author_line(page: Option<&PageLayout>) -> Option<AuthorLine> {
    let page = page?;
    for line in page.lines.iter().take(12) {
        let text = one_line(&line.text);
        let lower = text.to_ascii_lowercase();
        let rest = lower
            .strip_prefix("by ")
            .map(|_| &text[3..])
            .or_else(|| lower.strip_prefix("authors:").map(|_| &text[8..]))
            .or_else(|| lower.strip_prefix("author:").map(|_| &text[7..]));
        if let Some(rest) = rest {
            let authors = crate::extract::pdfoxide::split_authors(rest);
            if !authors.is_empty() && authors.len() <= 12 {
                return Some(AuthorLine { authors, text, rect: line.rect });
            }
        }
    }
    None
}

/// Text of a list item with its marker removed: `• Foo` → `Foo`, `1. Foo` → `Foo`.
///
/// The renderer writes its own marker, so keeping the original one would produce
/// `- • Foo`.
fn strip_list_marker(text: &str) -> &str {
    let trimmed = text.trim_start();
    let mut chars = trimmed.char_indices();
    let Some((_, first)) = chars.next() else {
        return trimmed;
    };
    if matches!(first, '•' | '·' | '●' | '○' | '▪' | '◦' | '‣' | '⁃' | '–' | '—')
        || (matches!(first, '-' | '*' | '+')
            && trimmed[first.len_utf8()..].starts_with(' '))
    {
        return trimmed[first.len_utf8()..].trim_start();
    }
    // `(iv) Foo` and `1.2 Foo` / `a) Foo` styles.
    if first == '(' {
        if let Some(close) = trimmed.find(')') {
            return trimmed[close + 1..].trim_start();
        }
    }
    let marker_len: usize = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric())
        .map(char::len_utf8)
        .sum();
    let rest = &trimmed[marker_len..];
    if let Some(rest) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
        return rest.trim_start();
    }
    trimmed
}

/// Collapse all whitespace runs into single spaces.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
