//! Page layout analysis: text area, columns, paragraphs.
//!
//! The input is the line stream PDFOxide produced; the output is the same lines
//! grouped into columns and paragraphs, with a body font size the heading and
//! caption heuristics compare against.
//!
//! The column detector is deliberately conservative: a gutter is only accepted
//! when it is wide, never crossed by ordinary body text, and leaves at least two
//! substantially used columns. A false gutter is far more damaging than a missed
//! one — it interleaves the columns and destroys the reading order, which is
//! exactly the failure mode this module exists to avoid.

pub mod reading_order;

pub use reading_order::order;

use crate::extract::pdfoxide::{ExtractedPage, ImagePlacement, Line, Rule};
use crate::geom::{Band, Rect};

/// A horizontal column of the page, given as an x range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub x0: f32,
    pub x1: f32,
}

impl Column {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn center(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }

    pub fn contains(&self, x: f32) -> bool {
        x >= self.x0 && x <= self.x1
    }
}

/// The column structure of one page.
#[derive(Debug, Clone)]
pub struct ColumnLayout {
    pub columns: Vec<Column>,
    /// The x ranges between the columns (empty for a single-column page).
    pub gutters: Vec<(f32, f32)>,
}

impl Default for ColumnLayout {
    fn default() -> Self {
        ColumnLayout { columns: vec![Column { x0: 0.0, x1: f32::MAX }], gutters: Vec::new() }
    }
}

impl ColumnLayout {
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// `true` for a 2+ column page.
    pub fn is_multi_column(&self) -> bool {
        self.columns.len() > 1
    }

    /// Column that holds most of `rect` horizontally.
    pub fn column_of(&self, rect: &Rect) -> usize {
        let mut best = 0;
        let mut best_overlap = -1.0;
        for (index, column) in self.columns.iter().enumerate() {
            let overlap =
                (rect.x1.min(column.x1) - rect.x0.max(column.x0)).max(0.0);
            if overlap > best_overlap {
                best_overlap = overlap;
                best = index;
            }
        }
        best
    }

    /// `true` when `rect` crosses a gutter, i.e. it belongs to the whole page
    /// rather than to one column (a full-width figure, title or equation).
    pub fn is_spanning(&self, rect: &Rect) -> bool {
        if self.columns.len() < 2 {
            return true;
        }
        if rect.width() <= 1.0 {
            return false;
        }
        let center_window = rect.width() * 0.15;
        self.gutters.iter().any(|(start, end)| {
            let mid = (start + end) * 0.5;
            rect.x0 < mid - center_window && rect.x1 > mid + center_window
        })
    }
}

/// Lines grouped into a paragraph candidate.
#[derive(Debug, Clone)]
pub struct Paragraph {
    /// Indices into [`PageLayout::lines`], in reading order.
    pub lines: Vec<usize>,
    pub rect: Rect,
    /// Dominant font size of the paragraph.
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub math_ratio: f32,
    pub column: usize,
    /// `true` when the paragraph sits in the bottom band (footnote candidate).
    pub footnote: bool,
}

impl Paragraph {
    /// Number of characters in the paragraph.
    pub fn length(&self) -> usize {
        self.lines.len()
    }
}

/// A page, analysed.
#[derive(Debug, Clone, Default)]
pub struct PageLayout {
    pub number: usize,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<Line>,
    pub rules: Vec<Rule>,
    pub images: Vec<ImagePlacement>,
    /// Box enclosing the text (excluding margins).
    pub text_box: Rect,
    pub columns: ColumnLayout,
    /// Most common font size, weighted by character count.
    pub body_size: f32,
    /// Header band and footer band of the page.
    pub header_band: Band,
    pub footer_band: Band,
}

impl PageLayout {
    /// Analyse an extracted page.
    pub fn analyse(page: &ExtractedPage) -> Self {
        let lines = page.lines.clone();
        let mut text_box = text_box(&lines, page.width, page.height);
        extend_to_reading_area(&mut text_box, page.width);
        let columns = detect_columns(&lines, text_box, page.width);
        let body_size = body_size(&lines).max(1.0);
        PageLayout {
            number: page.number,
            width: page.width,
            height: page.height,
            lines,
            rules: page.rules.clone(),
            images: page.images.clone(),
            text_box,
            columns,
            body_size,
            header_band: Band { start: 0.0, end: page.height * 0.07 },
            footer_band: Band { start: page.height * 0.93, end: page.height },
        }
    }

    /// Total characters on the page.
    pub fn char_count(&self) -> usize {
        self.lines.iter().map(|l| l.length).sum()
    }

    /// `true` when the page has a text layer worth parsing.
    pub fn has_text(&self) -> bool {
        self.char_count() >= 8
    }

    /// Group lines into paragraph candidates, per column, top to bottom.
    ///
    /// This runs *before* the reading order is decided: grouping only needs
    /// vertical proximity inside one column, which is local and reliable, while
    /// the order between columns/figures is decided afterwards.
    pub fn paragraphs(&self) -> Vec<Paragraph> {
        let mut paragraphs = Vec::new();
        let columns = self.columns.len().max(1);
        for column_index in 0..columns {
            let mut indices: Vec<usize> = (0..self.lines.len())
                .filter(|i| self.columns.column_of(&self.lines[*i].rect) == column_index)
                .collect();
            if indices.is_empty() {
                continue;
            }
            indices.sort_by(|a, b| {
                let (la, lb) = (&self.lines[*a], &self.lines[*b]);
                la.rect
                    .y0
                    .partial_cmp(&lb.rect.y0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        la.rect
                            .x0
                            .partial_cmp(&lb.rect.x0)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            });

            let column = self.columns.columns[column_index];
            let pitch = self.line_pitch(&indices);
            let mut current: Vec<usize> = Vec::new();
            for index in indices {
                let line = &self.lines[index];
                if let Some(previous_index) = current.last() {
                    let previous = &self.lines[*previous_index];
                    if self.continues(previous, line, &column, pitch) {
                        current.push(index);
                        continue;
                    }
                    paragraphs.push(self.paragraph_from(&current, column_index));
                    current.clear();
                }
                current.push(index);
            }
            if !current.is_empty() {
                paragraphs.push(self.paragraph_from(&current, column_index));
            }
        }
        paragraphs
    }

    /// The smallest vertical gap between two consecutive lines of a column.
    ///
    /// That gap is the page's own leading or a fraction of it (the tightest
    /// pair is normally two ordinary body lines), and paragraphs are separated
    /// by a multiple of it. Taking the *minimum* rather than the median is what
    /// makes sparse pages work: a page with a figure, an equation and a list has
    /// more irregular gaps than regular ones, and an average would be dragged up
    /// until every block merged.
    fn line_pitch(&self, indices: &[usize]) -> Option<f32> {
        indices
            .windows(2)
            .map(|pair| self.lines[pair[1]].rect.y0 - self.lines[pair[0]].rect.y1)
            .filter(|gap| *gap >= 0.5)
            .fold(None, |smallest: Option<f32>, gap| {
                Some(smallest.map_or(gap, |smallest| smallest.min(gap)))
            })
    }

    /// Does `line` continue the paragraph that ends with `previous`?
    fn continues(&self, previous: &Line, line: &Line, column: &Column, pitch: Option<f32>) -> bool {
        let gap = line.rect.y0 - previous.rect.y1;
        let size = previous.size.max(line.size).max(1.0);
        // Inside a paragraph the baseline distance is constant; between two
        // paragraphs it is noticeably larger. `1.15 * size` caps the estimate so
        // that a page whose tightest pair already sits a line apart cannot make
        // every block merge.
        let leading = pitch.map(|pitch| (pitch * 1.45).min(size * 1.15)).unwrap_or(0.0);
        if gap > (0.62 * size).max(leading) {
            return false;
        }
        if gap < -0.6 * size {
            // Overlapping boxes: a new text object on the same line region.
            return false;
        }
        // A style change starts a new paragraph.
        let size_delta = (line.size - previous.size).abs() / size;
        if size_delta > 0.18 || line.bold != previous.bold || line.mono != previous.mono {
            return false;
        }
        // A deliberately short line ("the last line of the paragraph") is *not*
        // used as a signal: ragged-right text has short lines everywhere, and
        // PDFs generated by word processors rarely indent the next paragraph.
        // Geometry that actually differs — gap, indent, style — is enough.
        // A line starting far right of the column edge is a new (indented) block.
        if line.rect.x0 > column.x0 + size * 2.0 && previous.rect.x0 < line.rect.x0 - size {
            return false;
        }
        true
    }

    fn paragraph_from(&self, indices: &[usize], column: usize) -> Paragraph {
        let mut rect = self.lines[indices[0]].rect;
        let mut sizes: Vec<f32> = Vec::new();
        let mut bold_chars = 0usize;
        let mut italic_chars = 0usize;
        let mut mono_chars = 0usize;
        let mut math = 0.0f32;
        let mut chars = 0usize;
        for index in indices {
            let line = &self.lines[*index];
            rect = rect.union(&line.rect);
            sizes.push(line.size);
            let n = line.length.max(1);
            chars += n;
            if line.bold {
                bold_chars += n;
            }
            if line.italic {
                italic_chars += n;
            }
            if line.mono {
                mono_chars += n;
            }
            math += line.math_ratio * n as f32;
        }
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let size = sizes[sizes.len() / 2];
        Paragraph {
            lines: indices.to_vec(),
            rect,
            size,
            bold: bold_chars * 2 > chars,
            italic: italic_chars * 2 > chars,
            mono: mono_chars * 2 > chars,
            math_ratio: if chars > 0 { math / chars as f32 } else { 0.0 },
            column,
            footnote: rect.y0 > self.footer_band.start,
        }
    }

    /// Raw text of a paragraph: lines joined with `\n` (hyphenation is resolved
    /// later, in the clean-up stage, where it can see the whole document).
    pub fn paragraph_text(&self, paragraph: &Paragraph) -> String {
        paragraph
            .lines
            .iter()
            .map(|i| self.lines[*i].text.trim())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The column edge a paragraph should reach to be "full width".
    pub fn column_right_edge(&self, column: usize) -> f32 {
        self.columns
            .columns
            .get(column)
            .map(|c| c.x1)
            .unwrap_or(self.text_box.x1)
    }
}

/// Box around the text, ignoring the extremes so a stray header cannot widen it.
fn text_box(lines: &[Line], width: f32, height: f32) -> Rect {
    if lines.is_empty() {
        return Rect::new(0.0, 0.0, width, height);
    }
    let mut x0s: Vec<f32> = lines.iter().map(|l| l.rect.x0).collect();
    let mut x1s: Vec<f32> = lines.iter().map(|l| l.rect.x1).collect();
    x0s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    x1s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let y0 = lines.iter().map(|l| l.rect.y0).fold(f32::MAX, f32::min);
    let y1 = lines.iter().map(|l| l.rect.y1).fold(f32::MIN, f32::max);
    Rect::new(
        percentile(&x0s, 0.05).max(0.0),
        y0.max(0.0),
        percentile(&x1s, 0.95).min(width.max(20.0)),
        y1.min(height.max(20.0)),
    )
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f32 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Grow the measured text box to the reading area the page must have.
///
/// The percentile of the right edges is dragged left by the widest *centred*
/// line: a title or a display equation is often the only content near the middle
/// of the page, so the raw box ends where the equation does. Pages printed with
/// symmetric margins — that is, almost all of them — give the missing half back
/// by mirroring the measured left margin, and the centring checks in the formula
/// heuristic then have a real reference to compare both margins against.
fn extend_to_reading_area(text_box: &mut Rect, page_width: f32) {
    if text_box.x0 <= 1.0 || page_width <= 20.0 || text_box.x0 > page_width * 0.5 {
        return;
    }
    let mirrored = page_width - text_box.x0;
    if mirrored > text_box.x1 {
        text_box.x1 = mirrored.min(page_width);
    }
}

/// Most common font size, weighted by characters.
fn body_size(lines: &[Line]) -> f32 {
    use std::collections::HashMap;
    let mut histogram: HashMap<i32, usize> = HashMap::new();
    for line in lines {
        let bucket = (line.size * 2.0).round() as i32;
        *histogram.entry(bucket).or_default() += line.length.max(1);
    }
    histogram
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(bucket, _)| bucket as f32 / 2.0)
        .unwrap_or(10.0)
}

/// Find the gutters, and therefore the columns, of a page.
fn detect_columns(lines: &[Line], text_box: Rect, page_width: f32) -> ColumnLayout {
    let single = ColumnLayout {
        columns: vec![Column { x0: text_box.x0, x1: text_box.x1.max(text_box.x0 + 1.0) }],
        gutters: Vec::new(),
    };
    let body: Vec<&Line> = lines.iter().filter(|l| l.length >= 8).collect();
    if body.len() < 12 || text_box.width() < page_width * 0.3 {
        return single;
    }

    let bin = (text_box.width() / 240.0).max(0.75);
    let bins = ((text_box.width() / bin).ceil() as usize).max(1);
    let mut covered = vec![false; bins];
    for line in &body {
        let from = (((line.rect.x0 - text_box.x0) / bin).floor() as isize).clamp(0, bins as isize - 1);
        let to = (((line.rect.x1 - text_box.x0) / bin).ceil() as isize).clamp(0, bins as isize - 1);
        for index in from..=to {
            covered[index as usize] = true;
        }
    }

    // Candidate gutters: uncovered runs of at least ~1.8% of the page width.
    let min_gap_bins = ((page_width * 0.018) / bin).round().max(3.0) as usize;
    let mut candidates: Vec<(f32, f32)> = Vec::new();
    let mut run_start: Option<usize> = None;
    for (index, covered) in covered.iter().enumerate() {
        match (*covered, run_start) {
            (false, None) => run_start = Some(index),
            (true, Some(start)) => {
                if index - start >= min_gap_bins {
                    candidates.push((
                        text_box.x0 + start as f32 * bin,
                        text_box.x0 + index as f32 * bin,
                    ));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = run_start {
        if bins - start >= min_gap_bins {
            candidates.push((text_box.x0 + start as f32 * bin, text_box.x0 + bins as f32 * bin));
        }
    }
    // Gutters must be interior: a gap at either edge is just a margin.
    candidates.retain(|(start, end)| {
        *start > text_box.x0 + 10.0 && *end < text_box.x1 - 10.0 && end - start > 4.0
    });
    if candidates.is_empty() {
        return single;
    }

    // A gutter crossed by a large share of the lines is not a gutter: those
    // lines are full-width text that merely happens not to cover every bin.
    let crossing_limit = (body.len() as f32 * 0.12).max(2.0);
    let mut gutters: Vec<(f32, f32)> = candidates
        .into_iter()
        .take(2)
        .filter(|(start, end)| {
            let mid = (start + end) * 0.5;
            let crossings = body.iter().filter(|l| l.rect.x0 < mid && l.rect.x1 > mid).count();
            (crossings as f32) <= crossing_limit
        })
        .collect();
    gutters.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if gutters.is_empty() {
        return single;
    }

    let mut columns = Vec::new();
    let mut cursor = text_box.x0;
    for (start, end) in &gutters {
        if start - cursor > 8.0 {
            columns.push(Column { x0: cursor, x1: *start });
        }
        cursor = *end;
    }
    if text_box.x1 - cursor > 8.0 {
        columns.push(Column { x0: cursor, x1: text_box.x1 });
    }

    // Every column must carry a real share of the text, otherwise the "gutter"
    // was a table column separator or a wide indent.
    let total = body.len() as f32;
    let keep = columns
        .iter()
        .map(|column| {
            let count = body
                .iter()
                .filter(|l| column.contains(l.rect.center_x()))
                .count() as f32;
            count / total
        })
        .collect::<Vec<_>>();
    if columns.len() < 2 || keep.iter().any(|share| *share < 0.15) {
        return single;
    }

    ColumnLayout { columns, gutters }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x0: f32, x1: f32, y0: f32, size: f32) -> Line {
        Line {
            text: text.to_string(),
            rect: Rect::new(x0, y0, x1, y0 + size),
            size,
            bold: false,
            italic: false,
            mono: false,
            math_ratio: 0.0,
            words: Vec::new(),
            length: text.chars().count(),
        }
    }

    fn two_column_page() -> ExtractedPage {
        let mut lines = Vec::new();
        for row in 0..14 {
            let y = 80.0 + row as f32 * 14.0;
            lines.push(line("The left column holds a full line of body text here", 60.0, 290.0, y, 10.0));
            lines.push(line("The right column holds a full line of body text here", 320.0, 550.0, y, 10.0));
        }
        ExtractedPage {
            number: 1,
            width: 595.0,
            height: 842.0,
            lines,
            ..Default::default()
        }
    }

    #[test]
    fn two_column_page_is_split_at_the_gutter() {
        let page = PageLayout::analyse(&two_column_page());
        assert!(page.columns.is_multi_column(), "expected two columns");
        assert_eq!(page.columns.len(), 2);
        let gutter = page.columns.gutters[0];
        assert!(gutter.0 > 285.0 && gutter.1 < 325.0, "gutter at {gutter:?}");
    }

    #[test]
    fn full_width_paragraph_is_not_split_into_columns() {
        let mut lines = Vec::new();
        for row in 0..16 {
            let y = 80.0 + row as f32 * 14.0;
            lines.push(line("A single column paragraph that spans the whole text width nicely", 60.0, 550.0, y, 10.0));
        }
        let page = PageLayout::analyse(&ExtractedPage {
            number: 1,
            width: 595.0,
            height: 842.0,
            lines,
            ..Default::default()
        });
        assert!(!page.columns.is_multi_column());
    }

    #[test]
    fn spanning_rect_crosses_the_gutter() {
        let columns = ColumnLayout {
            columns: vec![Column { x0: 60.0, x1: 290.0 }, Column { x0: 320.0, x1: 550.0 }],
            gutters: vec![(290.0, 320.0)],
        };
        assert!(columns.is_spanning(&Rect::new(60.0, 0.0, 550.0, 10.0)));
        assert!(!columns.is_spanning(&Rect::new(60.0, 0.0, 289.0, 10.0)));
        assert_eq!(columns.column_of(&Rect::new(330.0, 0.0, 400.0, 10.0)), 1);
    }

    #[test]
    fn paragraphs_split_on_a_blank_gap() {
        let mut lines = Vec::new();
        for row in 0..3 {
            lines.push(line("Body text of the first paragraph", 60.0, 550.0, 80.0 + row as f32 * 12.0, 10.0));
        }
        for row in 0..3 {
            lines.push(line("Body text of the second paragraph", 60.0, 550.0, 150.0 + row as f32 * 12.0, 10.0));
        }
        let page = PageLayout::analyse(&ExtractedPage {
            number: 1,
            width: 595.0,
            height: 842.0,
            lines,
            ..Default::default()
        });
        let paragraphs = page.paragraphs();
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].lines.len(), 3);
    }

    #[test]
    fn body_size_is_the_dominant_weighted_size() {
        let mut lines = Vec::new();
        for row in 0..10 {
            lines.push(line("body text line of reasonable length", 60.0, 550.0, 100.0 + row as f32 * 12.0, 10.0));
        }
        lines.push(line("Heading", 60.0, 200.0, 60.0, 18.0));
        let page = PageLayout::analyse(&ExtractedPage {
            number: 1,
            width: 595.0,
            height: 842.0,
            lines,
            ..Default::default()
        });
        assert_eq!(page.body_size, 10.0);
    }
}
