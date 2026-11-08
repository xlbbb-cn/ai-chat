//! Reading order for multi-column pages.
//!
//! Sorting a page by `x` and then by `y` is the classic bug: it interleaves the
//! two columns, so the left column's first paragraph is cut in half by the right
//! column's first paragraph. Sorting by `y` and then `x` is just as wrong — it
//! reads across the gutter line by line.
//!
//! What actually works, and what this module implements, is a banded column
//! sweep:
//!
//! 1. every item that crosses a gutter (title, abstract, full-width figure,
//!    displayed equation, wide table) becomes a **separator**;
//! 2. separators cut the page into horizontal bands;
//! 3. inside a band the columns are read independently, column by column, top to
//!    bottom — so the left column is finished before the right one starts;
//! 4. footnote-band items are moved to the end of the page.
//!
//! A page with no gutter (single column) degenerates to a plain top-to-bottom
//! sort, which is exactly right there.

use crate::geom::Rect;

use super::ColumnLayout;

/// Sort `items` into reading order.
///
/// * `items` — `(bounding box, payload)`; the payload is returned in order.
/// * `columns` — the page's column layout.
/// * `footnote_from` — y coordinate below which items are treated as footnotes
///   and moved to the end of the page.
pub fn order<T>(
    items: Vec<(Rect, T)>,
    columns: &ColumnLayout,
    footnote_from: Option<f32>,
) -> Vec<T> {
    if items.is_empty() {
        return Vec::new();
    }
    let multi_column = columns.is_multi_column();
    let separators: Vec<Rect> = if multi_column {
        items
            .iter()
            .map(|(rect, _)| *rect)
            .filter(|rect| columns.is_spanning(rect))
            .collect()
    } else {
        Vec::new()
    };

    const FOOTNOTE_BAND: usize = usize::MAX / 2;

    let mut keyed: Vec<(usize, usize, f32, f32, T)> = Vec::with_capacity(items.len());
    for (rect, value) in items {
        let footnote = footnote_from.map(|y| rect.y0 >= y).unwrap_or(false);
        let spanning = multi_column && columns.is_spanning(&rect);
        let band = if footnote {
            FOOTNOTE_BAND
        } else {
            separators
                .iter()
                .filter(|separator| separator.y1 <= rect.y0 + 0.5)
                .count()
        };
        // A spanning item closes its band: no column rank can beat it.
        let column_rank = if spanning { columns.len() } else { columns.column_of(&rect) };
        keyed.push((band, column_rank, rect.y0, rect.x0, value));
    }

    keyed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal))
    });
    keyed.into_iter().map(|(_, _, _, _, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Column;

    fn two_columns() -> ColumnLayout {
        ColumnLayout {
            columns: vec![Column { x0: 60.0, x1: 290.0 }, Column { x0: 320.0, x1: 550.0 }],
            gutters: vec![(290.0, 320.0)],
        }
    }

    fn left(y: f32) -> Rect {
        Rect::new(60.0, y, 285.0, y + 10.0)
    }

    fn right(y: f32) -> Rect {
        Rect::new(320.0, y, 545.0, y + 10.0)
    }

    fn full(y: f32) -> Rect {
        Rect::new(60.0, y, 545.0, y + 80.0)
    }

    #[test]
    fn two_columns_are_read_column_major() {
        let items = vec![
            (left(100.0), "L1"),
            (left(112.0), "L2"),
            (right(100.0), "R1"),
            (right(112.0), "R2"),
        ];
        let ordered = order(items, &two_columns(), None);
        assert_eq!(ordered, vec!["L1", "L2", "R1", "R2"]);
    }

    #[test]
    fn full_width_figure_splits_the_page_into_bands() {
        let items = vec![
            (left(100.0), "L1"),
            (right(100.0), "R1"),
            (left(200.0), "L2"),
            (right(200.0), "R2"),
            (full(400.0), "FIG"),
            (left(500.0), "L3"),
            (right(500.0), "R3"),
        ];
        let ordered = order(items, &two_columns(), None);
        assert_eq!(ordered, vec!["L1", "L2", "R1", "R2", "FIG", "L3", "R3"]);
    }

    #[test]
    fn footnotes_come_last() {
        let items = vec![
            (left(100.0), "body"),
            (right(100.0), "body2"),
            (left(760.0), "note"),
            (right(760.0), "note2"),
        ];
        let ordered = order(items, &two_columns(), Some(750.0));
        assert_eq!(ordered, vec!["body", "body2", "note", "note2"]);
    }

    #[test]
    fn single_column_is_a_plain_vertical_sort() {
        let columns = ColumnLayout {
            columns: vec![Column { x0: 60.0, x1: 550.0 }],
            gutters: Vec::new(),
        };
        let items = vec![
            (Rect::new(60.0, 200.0, 500.0, 210.0), "second"),
            (Rect::new(60.0, 100.0, 500.0, 110.0), "first"),
        ];
        assert_eq!(order(items, &columns, None), vec!["first", "second"]);
    }
}
