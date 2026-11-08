//! Table detection.
//!
//! Two independent signals, because PDFs draw tables in two completely
//! different ways:
//!
//! * **Ruled tables** — horizontal and vertical rules form a grid, and the words
//!   fall between them. Reliable when the lines exist, which is why it runs
//!   first and wins when both signals fire.
//! * **Aligned text** — no rules at all, just words arranged in columns with
//!   large inter-column gaps repeated across consecutive lines (very common in
//!   papers, especially in result tables).
//!
//! Cells are plain strings; the renderer decides between GFM, HTML and text.

use crate::extract::pdfoxide::{Line, Rule};
use crate::geom::Rect;
use crate::layout::PageLayout;

/// A detected table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCandidate {
    /// Row-major cell text. Rows are normalised to the same width.
    pub rows: Vec<Vec<String>>,
    pub rect: Rect,
    /// Indices into [`PageLayout::lines`] that were consumed by this table.
    pub consumed_lines: Vec<usize>,
    /// `true` when the first row reads as a header (all cells filled, no units).
    pub has_header: bool,
    /// Which signal found it — `"rules"` or `"aligned-text"`.
    pub source: &'static str,
}

impl TableCandidate {
    /// Number of rows.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Number of columns (of the widest row).
    pub fn column_count(&self) -> usize {
        self.rows.iter().map(|r| r.len()).max().unwrap_or(0)
    }
}

/// Find every ruled table on the page.
pub fn ruled_tables(layout: &PageLayout) -> Vec<TableCandidate> {
    let horizontals = merge_collinear(
        layout
            .rules
            .iter()
            .filter(|r| r.is_horizontal())
            .map(|r| (r.rect.center_y(), r.rect.x0, r.rect.x1))
            .collect(),
        true,
    );
    let verticals_raw: Vec<(f32, f32, f32)> = layout
        .rules
        .iter()
        .filter(|r| r.is_vertical())
        .map(|r| (r.rect.center_x(), r.rect.y0, r.rect.y1))
        .collect();
    let verticals = merge_collinear(verticals_raw, false);
    if horizontals.len() < 2 || verticals.len() < 2 {
        return Vec::new();
    }

    let mut tables = Vec::new();
    for cluster in cluster_rows(&horizontals) {
        if cluster.len() < 2 {
            continue;
        }
        let top = cluster.first().unwrap().0;
        let bottom = cluster.last().unwrap().0;
        if bottom - top < 10.0 {
            continue;
        }
        // Vertical rules that actually span this table.
        let mut xs: Vec<f32> = verticals
            .iter()
            .filter(|(_, y0, y1)| *y0 <= top + 2.0 && *y1 >= bottom - 2.0)
            .map(|(x, _, _)| *x)
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // A grid with a single column is not a table: it is a box around some
        // text (a wire frame, a callout), and emitting it as one would destroy
        // the paragraph.
        if xs.len() < 3 {
            continue;
        }
        let left = xs.first().copied().unwrap_or(0.0).min(
            cluster.iter().map(|c| c.1).fold(f32::MAX, f32::min),
        );
        let right = xs.last().copied().unwrap_or(0.0).max(
            cluster.iter().map(|c| c.2).fold(f32::MIN, f32::max),
        );
        let rect = Rect::new(left, top, right, bottom);

        let mut rows: Vec<Vec<String>> = Vec::new();
        for pair in cluster.windows(2) {
            let (y_start, y_end) = (pair[0].0, pair[1].0);
            if y_end - y_start < 1.0 {
                continue;
            }
            let mut row: Vec<String> = Vec::new();
            for window in xs.windows(2) {
                let cell = Rect::new(window[0], y_start, window[1], y_end);
                // Assign *words*, not whole lines: one line routinely holds the
                // text of several cells ("Method    Score"), and taking the
                // line's centre would push all of it into a single cell.
                let mut parts: Vec<(f32, &str)> = Vec::new();
                for line in &layout.lines {
                    for word in &line.words {
                        if cell.contains_point(word.rect.center_x(), word.rect.center_y()) {
                            parts.push((word.rect.x0, word.text.trim()));
                        }
                    }
                }
                parts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                row.push(
                    parts
                        .iter()
                        .map(|(_, text)| *text)
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            if row.iter().any(|c| !c.is_empty()) {
                rows.push(row);
            }
        }
        if rows.len() < 2 {
            continue;
        }
        // A grid often has more vertical rules than the text uses ("Method |
        // Score | <empty> | <empty>"). Columns that carry no text in any row are
        // noise from the drawing, not part of the table.
        while rows.first().is_some_and(|row| row.last().is_some_and(String::is_empty)) {
            for row in &mut rows {
                row.pop();
            }
        }
        normalize_rows(&mut rows);
        if rows.first().map_or(true, |row| row.is_empty()) {
            continue;
        }
        let consumed_lines = lines_inside(layout, &rect);
        let has_header = rows.first().map(|r| r.iter().all(|c| !c.is_empty())).unwrap_or(false);
        tables.push(TableCandidate { rows, rect, consumed_lines, has_header, source: "rules" });
    }
    tables
}

/// Detect a table made only of aligned words, starting at `start`.
///
/// Returns the table and how many lines it consumed.
pub fn aligned_table(layout: &PageLayout, start: usize) -> Option<TableCandidate> {
    let first = layout.lines.get(start)?;
    if first.words.len() < 2 {
        return None;
    }
    let mut indices = vec![start];
    let mut previous = first;
    for index in start + 1..layout.lines.len() {
        let line = &layout.lines[index];
        if line.words.len() < 2 {
            break;
        }
        let gap = line.rect.y0 - previous.rect.y1;
        if gap < -2.0 || gap > 1.4 * line.size.max(first.size) {
            break;
        }
        // The column must be the same, otherwise this is the next column.
        if layout.columns.column_of(&line.rect) != layout.columns.column_of(&first.rect) {
            break;
        }
        indices.push(index);
        previous = line;
    }
    if indices.len() < 3 {
        return None;
    }

    // Are the word starts aligned across the candidate lines?
    let starts: Vec<Vec<f32>> = indices
        .iter()
        .map(|i| layout.lines[*i].words.iter().map(|w| w.rect.x0).collect())
        .collect();
    let mut columns: Vec<f32> = Vec::new();
    for row in &starts {
        for x in row {
            if !columns.iter().any(|existing| (existing - x).abs() <= 6.0) {
                columns.push(*x);
            }
        }
    }
    columns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if columns.len() < 2 {
        return None;
    }
    let aligned_rows = starts
        .iter()
        .filter(|row| {
            row.iter().filter(|x| columns.iter().any(|c| (*c - **x).abs() <= 6.0)).count() >= 2
        })
        .count();
    if aligned_rows as f32 / (indices.len() as f32) < 0.6 {
        return None;
    }

    // Split each line into cells at gaps wider than a space.
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut cell_starts: Vec<Vec<f32>> = Vec::new();
    for index in &indices {
        let line = &layout.lines[*index];
        let threshold = (line.size * 1.1).max(6.0);
        let mut cells: Vec<String> = Vec::new();
        let mut starts: Vec<f32> = Vec::new();
        let mut current = String::new();
        let mut previous_x1: Option<f32> = None;
        for word in &line.words {
            if let Some(x1) = previous_x1 {
                if word.rect.x0 - x1 > threshold {
                    cells.push(current.trim().to_string());
                    starts.push(word.rect.x0);
                    current.clear();
                }
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&word.text);
            previous_x1 = Some(word.rect.x1);
        }
        if !current.trim().is_empty() {
            cells.push(current.trim().to_string());
        }
        if cells.len() >= 2 {
            rows.push(cells);
            cell_starts.push(starts);
        }
    }
    if rows.len() < 3 {
        return None;
    }
    // A table is a *grid*: the same number of cells, starting at the same x, on
    // most rows. Scattered labels (a schematic, a poster, a diagram) also have
    // wide gaps between words; what they never have is a repeated grid, and
    // without this test they would all turn into one giant table.
    let width = grid_width(&cell_starts)?;
    rows.retain(|row| row.len() == width);
    if rows.len() < 3 {
        return None;
    }
    normalize_rows(&mut rows);
    let rect = indices
        .iter()
        .fold(layout.lines[*indices.first().unwrap()].rect, |acc, i| acc.union(&layout.lines[*i].rect));
    Some(TableCandidate {
        rows,
        rect,
        consumed_lines: indices,
        has_header: true,
        source: "aligned-text",
    })
}

/// Number of columns of the grid `cell_starts` describes, if it is one.
///
/// `cell_starts[i]` holds the left edges of the cells of row `i` *after* the
/// first cell, so the row has `len + 1` cells. The modal count wins, and at
/// least 60 % of the rows must both have that count and start each cell within
/// six points of the first such row. That is the difference between a table and
/// a page of scattered labels.
fn grid_width(cell_starts: &[Vec<f32>]) -> Option<usize> {
    if cell_starts.len() < 3 {
        return None;
    }
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for starts in cell_starts {
        let cells = starts.len() + 1;
        match counts.iter_mut().find(|(width, _)| *width == cells) {
            Some((_, rows)) => *rows += 1,
            None => counts.push((cells, 1)),
        }
    }
    let (width, _) = counts.into_iter().max_by_key(|(width, rows)| (*rows, *width))?;
    if width < 2 {
        return None;
    }
    let reference = cell_starts.iter().find(|starts| starts.len() + 1 == width)?;
    let consistent = cell_starts
        .iter()
        .filter(|starts| {
            starts.len() + 1 == width
                && starts.iter().zip(reference).all(|(x, reference)| (x - reference).abs() <= 6.0)
        })
        .count();
    if consistent >= 3 && consistent * 10 >= cell_starts.len() * 6 {
        Some(width)
    } else {
        None
    }
}

/// Pad rows to a rectangle.
fn normalize_rows(rows: &mut [Vec<String>]) {
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    for row in rows.iter_mut() {
        while row.len() < width {
            row.push(String::new());
        }
    }
}

/// Merge rules that sit on the same coordinate into their extent.
fn merge_collinear(mut items: Vec<(f32, f32, f32)>, horizontal: bool) -> Vec<(f32, f32, f32)> {
    items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f32, f32, f32)> = Vec::new();
    for (coordinate, start, end) in items {
        match merged.last_mut() {
            Some(last) if (last.0 - coordinate).abs() <= 2.0 => {
                last.1 = last.1.min(start);
                last.2 = last.2.max(end);
            }
            _ => merged.push((coordinate, start, end)),
            }
    }
    let _ = horizontal;
    merged
}

/// Group horizontal rules into tables by vertical proximity.
fn cluster_rows(rules: &[(f32, f32, f32)]) -> Vec<Vec<(f32, f32, f32)>> {
    let mut clusters: Vec<Vec<(f32, f32, f32)>> = Vec::new();
    for rule in rules {
        match clusters.last_mut() {
            Some(cluster)
                if rule.0 - cluster.last().map(|r| r.0).unwrap_or(rule.0) <= 80.0
                    && cluster
                        .iter()
                        .any(|r| r.1 < rule.2 && rule.1 < r.2) =>
            {
                cluster.push(*rule);
            }
            _ => clusters.push(vec![*rule]),
        }
    }
    clusters.retain(|cluster| cluster.len() >= 2);
    clusters
}

fn lines_inside(layout: &PageLayout, rect: &Rect) -> Vec<usize> {
    layout
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let inside = rect.intersection_area(&line.rect);
            inside >= line.rect.area() * 0.5 && line.rect.width() > 0.0
        })
        .map(|(index, _)| index)
        .collect()
}

/// `true` when the rule set is dense enough to be a table grid.
pub fn looks_like_grid(rules: &[Rule]) -> bool {
    let horizontal = rules.iter().filter(|r| r.is_horizontal()).count();
    let vertical = rules.iter().filter(|r| r.is_vertical()).count();
    horizontal >= 2 && vertical >= 2
}

/// Convenience for callers that only have lines (no page layout).
pub fn lines_are_tabular(lines: &[&Line]) -> bool {
    lines.len() >= 3 && lines.iter().all(|l| l.words.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::pdfoxide::Word;

    fn line_with_words(texts: &[(&str, f32, f32)], y: f32, size: f32) -> Line {
        let words: Vec<Word> = texts
            .iter()
            .map(|(text, x0, x1)| Word {
                text: (*text).to_string(),
                rect: Rect::new(*x0, y, *x1, y + size),
            })
            .collect();
        let rect = words.iter().fold(words[0].rect, |acc, w| acc.union(&w.rect));
        Line {
            text: texts.iter().map(|(t, _, _)| *t).collect::<Vec<_>>().join(" "),
            rect,
            size,
            bold: false,
            italic: false,
            mono: false,
            math_ratio: 0.0,
            words,
            length: texts.iter().map(|(t, _, _)| t.len()).sum(),
        }
    }

    #[test]
    fn aligned_words_become_a_table() {
        let mut layout = PageLayout::default();
        layout.columns.columns = vec![crate::layout::Column { x0: 40.0, x1: 560.0 }];
        for row in 0..4 {
            let y = 100.0 + row as f32 * 14.0;
            layout.lines.push(line_with_words(
                &[("Method", 60.0, 110.0), ("Accuracy", 300.0, 360.0), ("Time", 480.0, 510.0)],
                y,
                10.0,
            ));
        }
        let table = aligned_table(&layout, 0).expect("a table");
        assert_eq!(table.row_count(), 4);
        assert_eq!(table.column_count(), 3);
        assert_eq!(table.rows[0], vec!["Method", "Accuracy", "Time"]);
        assert_eq!(table.source, "aligned-text");
    }

    #[test]
    fn plain_prose_is_not_a_table() {
        let mut layout = PageLayout::default();
        layout.columns.columns = vec![crate::layout::Column { x0: 40.0, x1: 560.0 }];
        for row in 0..4 {
            let y = 100.0 + row as f32 * 12.0;
            layout.lines.push(line_with_words(&[("ordinary", 60.0, 110.0)], y, 10.0));
        }
        assert!(aligned_table(&layout, 0).is_none());
    }

    #[test]
    fn collinear_rules_are_merged() {
        let merged = merge_collinear(
            vec![(10.0, 0.0, 50.0), (10.5, 40.0, 100.0), (30.0, 0.0, 10.0)],
            true,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], (10.0, 0.0, 100.0));
    }

    #[test]
    fn normalize_pads_rows() {
        let mut rows = vec![vec!["a".to_string()], vec!["b".to_string(), "c".to_string()]];
        normalize_rows(&mut rows);
        assert_eq!(rows[0].len(), 2);
    }
}
