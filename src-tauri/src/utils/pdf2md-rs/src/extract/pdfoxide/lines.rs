//! Glyph → line → word assembly.
//!
//! Raw glyph runs carry no notion of a line: a PDF may emit a paragraph as one
//! operator, or one operator per character, in any order. This module recovers
//! the visual lines that the layout analysis and the Markdown writer work with,
//! and the word boxes that table detection needs.

use std::cmp::Ordering;

use crate::geom::Rect;

use super::content::Glyph;

/// A word inside a line, with its box.
#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    pub rect: Rect,
}

/// One visual line of text.
#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
    pub rect: Rect,
    /// Dominant font size, in points.
    pub size: f32,
    /// Style of the majority of the line's characters.
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    /// Fraction of characters drawn in a math/symbol font (0..=1).
    pub math_ratio: f32,
    pub words: Vec<Word>,
    /// Number of characters, spaces excluded — a cheap "weight" for statistics.
    pub length: usize,
}

impl Line {
    /// Number of words.
    pub fn word_count(&self) -> usize {
        self.words.len()
    }
}

/// Group glyphs into lines, then into words.
///
/// Glyphs drawn under a rotation or shear are dropped: their boxes are
/// meaningless in page space, and keeping them would corrupt every line they
/// touch (rotated watermarks are the common case).
pub fn build(glyphs: &[Glyph]) -> Vec<Line> {
    let mut items: Vec<&Glyph> = glyphs
        .iter()
        .filter(|g| !g.rotated && !g.text.trim().is_empty())
        .collect();
    if items.is_empty() {
        return Vec::new();
    }
    items.sort_by(|a, b| {
        cmp(a.rect.center_y(), b.rect.center_y()).then(cmp(a.rect.x0, b.rect.x0))
    });

    let mut groups: Vec<Vec<&Glyph>> = Vec::new();
    let mut baseline = 0.0f32;
    let mut line_size = 0.0f32;
    let mut count = 0usize;
    for glyph in items {
        let joins = count > 0 && {
            let tolerance = 0.42 * line_size.min(glyph.size).max(1.0);
            (glyph.rect.center_y() - baseline).abs() <= tolerance
        };
        if joins {
            baseline = (baseline * count as f32 + glyph.rect.center_y()) / (count + 1) as f32;
            line_size = line_size.max(glyph.size);
            count += 1;
            groups.last_mut().expect("a group exists once count > 0").push(glyph);
        } else {
            groups.push(vec![glyph]);
            baseline = glyph.rect.center_y();
            line_size = glyph.size;
            count = 1;
        }
    }

    let mut lines: Vec<Line> = groups.into_iter().filter_map(build_line).collect();
    lines.sort_by(|a, b| cmp(a.rect.y0, b.rect.y0).then(cmp(a.rect.x0, b.rect.x0)));
    lines
}

fn build_line(mut group: Vec<&Glyph>) -> Option<Line> {
    group.sort_by(|a, b| cmp(a.rect.x0, b.rect.x0).then(a.seq.cmp(&b.seq)));

    let mut words: Vec<Word> = Vec::new();
    let mut word_text = String::new();
    let mut word_rect: Option<Rect> = None;
    let mut previous: Option<&Glyph> = None;
    let mut sizes: Vec<f32> = Vec::with_capacity(group.len());
    let mut bold_chars = 0usize;
    let mut italic_chars = 0usize;
    let mut mono_chars = 0usize;
    let mut math_chars = 0usize;
    let mut chars = 0usize;

    for glyph in group {
        let gap = previous.map(|p| glyph.rect.x0 - p.rect.x1).unwrap_or(0.0);
        let splits_word = previous.is_some() && gap > 0.18 * glyph.size.max(6.0);
        if splits_word && !word_text.is_empty() {
            if let Some(rect) = word_rect.take() {
                words.push(Word { text: std::mem::take(&mut word_text), rect });
            }
        }
        word_text.push_str(&glyph.text);
        word_rect = Some(match word_rect {
            Some(rect) => rect.union(&glyph.rect),
            None => glyph.rect,
        });
        sizes.push(glyph.size);
        let n = glyph.text.chars().count().max(1);
        chars += n;
        if glyph.bold {
            bold_chars += n;
        }
        if glyph.italic {
            italic_chars += n;
        }
        if glyph.mono {
            mono_chars += n;
        }
        if glyph.math {
            math_chars += n;
        }
        previous = Some(glyph);
    }
    if let Some(rect) = word_rect {
        if !word_text.is_empty() {
            words.push(Word { text: word_text, rect });
        }
    }
    if words.is_empty() || chars == 0 {
        return None;
    }

    // Build the line box from the words, so the per-glyph bookkeeping above
    // cannot leak into the statistics.
    let line_rect = words.iter().fold(words[0].rect, |acc, w| acc.union(&w.rect));
    let text = words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    sizes.sort_by(|a, b| cmp(*a, *b));
    let size = sizes[sizes.len() / 2];

    Some(Line {
        text,
        rect: line_rect,
        size,
        bold: bold_chars * 2 > chars,
        italic: italic_chars * 2 > chars,
        mono: mono_chars * 2 > chars,
        math_ratio: math_chars as f32 / chars as f32,
        words,
        length: chars,
    })
}

fn cmp(a: f32, b: f32) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(text: &str, x: f32, y: f32, size: f32) -> Glyph {
        Glyph {
            text: text.to_string(),
            rect: Rect::new(x, y, x + size * 0.5, y + size),
            size,
            bold: false,
            italic: false,
            mono: false,
            math: false,
            seq: 0,
            rotated: false,
        }
    }

    #[test]
    fn glyphs_on_one_baseline_form_one_line() {
        let mut glyphs = Vec::new();
        for (index, ch) in "Hello".chars().enumerate() {
            glyphs.push(glyph(&ch.to_string(), 10.0 + index as f32 * 5.0, 100.0, 10.0));
        }
        let lines = build(&glyphs);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello");
        assert_eq!(lines[0].length, 5);
    }

    #[test]
    fn a_wide_gap_starts_a_new_word() {
        let glyphs = vec![
            glyph("a", 10.0, 100.0, 10.0),
            glyph("b", 15.0, 100.0, 10.0),
            glyph("c", 40.0, 100.0, 10.0),
        ];
        let lines = build(&glyphs);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].word_count(), 2);
        assert_eq!(lines[0].text, "ab c");
    }

    #[test]
    fn lines_further_apart_are_separate() {
        let glyphs = vec![glyph("A", 10.0, 100.0, 10.0), glyph("B", 10.0, 130.0, 10.0)];
        let lines = build(&glyphs);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn rotated_glyphs_are_dropped() {
        let mut g = glyph("A", 10.0, 100.0, 10.0);
        g.rotated = true;
        assert!(build(&[g]).is_empty());
    }

    #[test]
    fn style_is_decided_by_the_majority() {
        let mut bold = glyph("Hi", 10.0, 100.0, 10.0);
        bold.bold = true;
        let lines = build(&[bold]);
        assert!(lines[0].bold);
    }
}
