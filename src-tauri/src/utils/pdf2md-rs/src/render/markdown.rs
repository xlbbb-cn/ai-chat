//! The Markdown renderer.
//!
//! Everything the [`crate::options::Markdown`] struct promises is implemented
//! here: front matter, page-break markers, heading offsets, three table styles,
//! formulas, escaped body text and inline math.
//!
//! The one rule that matters: **blocks are separated by exactly one blank
//! line**, except consecutive list items, which are separated by a single
//! newline. Anything else produces broken lists or collapsed code fences in
//! every Markdown implementation.

use crate::formula;
use crate::ir::{Block, Document};
use crate::options::{FormulaStyle, Markdown, PageBreaks, TableStyle};

/// Render a document as Markdown.
pub fn render(document: &Document, options: &Markdown) -> String {
    let mut out = String::with_capacity(document.blocks.len() * 80);
    if options.front_matter {
        write_front_matter(&mut out, document);
    }

    let mut page = 1usize;
    let mut previous_was_list = false;
    let mut ordered_counter = 0usize;
    let mut previous_level = 0u8;

    for block in &document.blocks {
        match block {
            Block::PageBreak => {
                match options.page_breaks {
                    PageBreaks::Off => {}
                    PageBreaks::Comment => {
                        separate(&mut out);
                        out.push_str(&format!("<!-- page {} -->\n", page + 1));
                    }
                    PageBreaks::Rule => {
                        separate(&mut out);
                        out.push_str("---\n");
                    }
                }
                page += 1;
                previous_was_list = false;
                ordered_counter = 0;
                previous_level = 0;
                continue;
            }
            Block::ListItem { text, ordered, level } => {
                if !previous_was_list {
                    separate(&mut out);
                }
                if *ordered {
                    if !previous_was_list || *level < previous_level {
                        ordered_counter = 0;
                    }
                    ordered_counter += 1;
                } else {
                    ordered_counter = 0;
                }
                let indent = "  ".repeat(*level as usize);
                let marker = if *ordered {
                    format!("{ordered_counter}. ")
                } else {
                    "- ".to_string()
                };
                out.push_str(&indent);
                out.push_str(&marker);
                out.push_str(&format_inline(text, options));
                out.push('\n');
                previous_was_list = true;
                previous_level = *level;
                continue;
            }
            _ => {}
        }

        separate(&mut out);
        previous_was_list = false;
        ordered_counter = 0;
        previous_level = 0;

        match block {
            Block::Heading { level, text } => {
                let level = (*level as i32 + i32::from(options.heading_offset)).clamp(1, 6) as usize;
                out.push_str(&"#".repeat(level));
                out.push(' ');
                out.push_str(&format_inline(text, options));
                out.push('\n');
            }
            Block::Paragraph { text } => {
                out.push_str(&format_inline(text, options));
                out.push('\n');
            }
            Block::Quote { text } => {
                for line in text.lines() {
                    out.push_str("> ");
                    out.push_str(&format_inline(line, options));
                    out.push('\n');
                }
            }
            Block::Table { rows } => {
                write_table(&mut out, rows, options.table_style);
            }
            Block::Image { path, caption } => {
                write_image(&mut out, path, caption, options);
            }
            Block::Formula { latex, display } => {
                write_formula(&mut out, latex, *display, options.formula_style);
            }
            Block::Code { language, source } => {
                let fence = "```";
                let fence = if source.contains(fence) { "````" } else { fence };
                out.push_str(fence);
                if let Some(language) = language {
                    out.push_str(language);
                }
                out.push('\n');
                out.push_str(source.trim_end_matches('\n'));
                out.push('\n');
                out.push_str(fence);
                out.push('\n');
            }
            Block::Rule => out.push_str("---\n"),
            Block::ListItem { .. } | Block::PageBreak => unreachable!("handled above"),
        }
    }

    // Exactly one trailing newline, no trailing blank lines.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Ensure the output ends with a blank line before a new block.
fn separate(out: &mut String) {
    if out.is_empty() {
        return;
    }
    if out.ends_with("\n\n") {
        return;
    }
    if out.ends_with('\n') {
        out.push('\n');
    } else {
        out.push_str("\n\n");
    }
}

fn write_front_matter(out: &mut String, document: &Document) {
    let metadata = &document.metadata;
    out.push_str("---\n");
    if let Some(title) = &metadata.title {
        out.push_str(&format!("title: {}\n", yaml_string(title)));
    }
    if !metadata.author.is_empty() {
        out.push_str("author:\n");
        for author in &metadata.author {
            out.push_str(&format!("  - {}\n", yaml_string(author)));
        }
    }
    out.push_str(&format!("pages: {}\n", metadata.page_count));
    out.push_str(&format!("generator: pdf2md-rs {}\n", crate::VERSION));
    out.push_str("---\n\n");
}

/// Quote a YAML scalar: always double-quoted, so `:`, `#` and `-` are safe.
fn yaml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn write_image(out: &mut String, path: &str, caption: &Option<String>, options: &Markdown) {
    let alt = caption
        .as_deref()
        .map(|c| c.replace(['[', ']'], "").replace('\n', " "))
        .unwrap_or_default();
    if path.is_empty() {
        // Images were not extracted (or the payload could not be decoded): keep
        // a marker rather than silently dropping the figure.
        out.push_str(&format!("<!-- image omitted: {} -->\n", if alt.is_empty() { "no alt" } else { &alt }));
        return;
    }
    out.push_str(&format!("![{alt}]({path})\n"));
    if options.image_caption_line {
        if let Some(caption) = caption {
            if !caption.trim().is_empty() {
                out.push('\n');
                out.push('*');
                out.push_str(caption.trim());
                out.push('*');
                out.push('\n');
            }
        }
    }
}

fn write_formula(out: &mut String, latex: &str, display: bool, style: FormulaStyle) {
    match style {
        FormulaStyle::Latex => {
            if display {
                out.push_str("$$\n");
                out.push_str(latex.trim());
                out.push_str("\n$$\n");
            } else {
                out.push('$');
                out.push_str(latex.trim());
                out.push_str("$\n");
            }
        }
        FormulaStyle::Plain => {
            out.push_str(latex.trim());
            out.push('\n');
        }
    }
}

fn write_table(out: &mut String, rows: &[Vec<String>], style: TableStyle) {
    let columns = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if columns == 0 {
        return;
    }
    match style {
        TableStyle::Gfm => {
            let cell = |row: &Vec<String>, index: usize| -> String {
                row.get(index)
                    .map(|value| value.replace('|', "\\|").replace('\n', " "))
                    .unwrap_or_default()
            };
            let header = rows.first().cloned().unwrap_or_default();
            out.push('|');
            for index in 0..columns {
                out.push(' ');
                out.push_str(&cell(&header, index));
                out.push_str(" |");
            }
            out.push('\n');
            out.push('|');
            for _ in 0..columns {
                out.push_str(" --- |");
            }
            out.push('\n');
            for row in rows.iter().skip(1) {
                out.push('|');
                for index in 0..columns {
                    out.push(' ');
                    out.push_str(&cell(row, index));
                    out.push_str(" |");
                }
                out.push('\n');
            }
        }
        TableStyle::Html => {
            out.push_str("<table>\n");
            for (row_index, row) in rows.iter().enumerate() {
                out.push_str("<tr>");
                let tag = if row_index == 0 { "th" } else { "td" };
                for index in 0..columns {
                    let cell = row.get(index).map(String::as_str).unwrap_or("");
                    out.push_str(&format!("<{tag}>{}</{tag}>", escape_html(cell)));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</table>\n");
        }
        TableStyle::Text => {
            let mut widths = vec![0usize; columns];
            for row in rows {
                for (index, cell) in row.iter().enumerate() {
                    widths[index] = widths[index].max(display_width(cell));
                }
            }
            for (row_index, row) in rows.iter().enumerate() {
                let mut line = String::new();
                for (index, width) in widths.iter().enumerate() {
                    let cell = row.get(index).map(String::as_str).unwrap_or("");
                    line.push_str(cell);
                    let padding = width.saturating_sub(display_width(cell));
                    if index + 1 < columns {
                        line.push_str(&" ".repeat(padding + 2));
                    }
                }
                out.push_str(line.trim_end());
                out.push('\n');
                if row_index == 0 {
                    let total: usize = widths.iter().sum::<usize>() + 2 * (columns - 1);
                    out.push_str(&"-".repeat(total));
                    out.push('\n');
                }
            }
        }
    }
}

fn display_width(text: &str) -> usize {
    text.chars().map(|ch| if crate::clean::text::is_cjk(ch) { 2 } else { 1 }).sum()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Format one inline run: collapse newlines, optionally wrap math-looking
/// tokens in `$…$`, and escape Markdown metacharacters outside math.
pub fn format_inline(text: &str, options: &Markdown) -> String {
    let flat = text.replace('\n', " ");
    let with_math = if options.inline_math {
        formula::wrap_inline_math(&flat)
    } else {
        flat
    };
    if options.escape {
        escape_preserving_math(&with_math)
    } else {
        with_math
    }
}

/// Escape Markdown punctuation, but never inside `$…$`.
fn escape_preserving_math(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut in_math = false;
    let mut line_start = true;
    for ch in text.chars() {
        match ch {
            '$' => {
                in_math = !in_math;
                out.push('$');
                line_start = false;
                continue;
            }
            '\n' => {
                out.push('\n');
                line_start = true;
                continue;
            }
            _ => {}
        }
        if in_math {
            out.push(ch);
            line_start = false;
            continue;
        }
        if line_start && ch == '#' {
            out.push_str("\\#");
            line_start = false;
            continue;
        }
        if line_start && ch == '-' {
            out.push_str("\\-");
            line_start = false;
            continue;
        }
        match ch {
            '\\' => out.push_str("\\\\"),
            '*' => out.push_str("\\*"),
            '_' => out.push_str("\\_"),
            '[' => out.push_str("\\["),
            ']' => out.push_str("\\]"),
            '`' => out.push_str("\\`"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '|' => out.push_str("\\|"),
            '~' => out.push_str("\\~"),
            _ => out.push(ch),
        }
        line_start = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Metadata, Page};

    fn doc(blocks: Vec<Block>) -> Document {
        let mut document = Document::new(Metadata {
            title: Some("A \"quoted\" title".into()),
            author: vec!["Ada Lovelace".into(), "Alan Turing".into()],
            page_count: 2,
        });
        document.push_page(Page { page_number: 1, width: 595.0, height: 842.0 });
        document.blocks = blocks;
        document
    }

    #[test]
    fn front_matter_is_valid_yaml() {
        let out = render(&doc(vec![Block::Paragraph { text: "Body".into() }]), &Markdown::default());
        assert!(out.starts_with("---\n"));
        assert!(out.contains("title: \"A \\\"quoted\\\" title\""));
        assert!(out.contains("  - \"Ada Lovelace\""));
        assert!(out.contains("pages: 2"));
        assert!(out.contains("---\n\nBody\n"));
    }

    #[test]
    fn headings_honour_the_offset_and_are_clamped() {
        let options = Markdown { heading_offset: 2, ..Markdown::default() };
        let out = render(&doc(vec![Block::Heading { level: 1, text: "Title".into() }]), &options);
        assert!(out.contains("### Title\n"));

        let options = Markdown { heading_offset: 9, ..Markdown::default() };
        let out = render(&doc(vec![Block::Heading { level: 1, text: "Title".into() }]), &options);
        assert!(out.contains("###### Title\n"));
    }

    #[test]
    fn gfm_table_uses_the_first_row_as_header() {
        let table = Block::Table {
            rows: vec![
                vec!["Method".into(), "Score".into()],
                vec!["ours".into(), "97.5".into()],
            ],
        };
        let out = render(&doc(vec![table]), &Markdown::default());
        assert!(out.contains("| Method | Score |\n| --- | --- |\n| ours | 97.5 |\n"), "{out}");
    }

    #[test]
    fn pipes_inside_cells_are_escaped() {
        let table = Block::Table { rows: vec![vec!["a|b".into()], vec!["c".into()]] };
        let out = render(&doc(vec![table]), &Markdown::default());
        assert!(out.contains("a\\|b"));
    }

    #[test]
    fn list_items_are_tight_and_ordered_lists_count() {
        let blocks = vec![
            Block::ListItem { text: "first".into(), ordered: true, level: 0 },
            Block::ListItem { text: "second".into(), ordered: true, level: 0 },
            Block::ListItem { text: "nested".into(), ordered: false, level: 1 },
        ];
        let out = render(&doc(blocks), &Markdown::default());
        assert!(out.contains("1. first\n2. second\n  - nested\n"), "{out}");
    }

    #[test]
    fn display_formula_becomes_a_dollar_block() {
        let out = render(
            &doc(vec![Block::Formula { latex: "y[n] = h[n] * x[n]".into(), display: true }]),
            &Markdown::default(),
        );
        assert!(out.contains("$$\ny[n] = h[n] * x[n]\n$$\n"));
    }

    #[test]
    fn inline_math_is_not_escaped() {
        let options = Markdown { escape: true, ..Markdown::default() };
        let out = render(&doc(vec![Block::Paragraph { text: "where x[n] = h[n] is small".into() }]), &options);
        assert!(out.contains("$x[n] = h[n]$"), "{out}");
    }

    #[test]
    fn page_breaks_are_numbered() {
        let blocks = vec![
            Block::Paragraph { text: "one".into() },
            Block::PageBreak,
            Block::Paragraph { text: "two".into() },
        ];
        let out = render(&doc(blocks), &Markdown::default());
        assert!(out.contains("<!-- page 2 -->"));
    }

    #[test]
    fn missing_images_leave_a_comment() {
        let out = render(
            &doc(vec![Block::Image { path: String::new(), caption: Some("Figure 1: x".into()) }]),
            &Markdown::default(),
        );
        assert!(out.contains("<!-- image omitted: Figure 1: x -->"));
    }

    #[test]
    fn images_render_with_alt_text() {
        let out = render(
            &doc(vec![Block::Image { path: "assets/page-0001-img-01.png".into(), caption: Some("A [box]".into()) }]),
            &Markdown::default(),
        );
        assert!(out.contains("![A box](assets/page-0001-img-01.png)"));
    }

    #[test]
    fn body_text_is_escaped_outside_math() {
        let out = render(&doc(vec![Block::Paragraph { text: "a*b _c_ $x*y$".into() }]), &Markdown::default());
        assert!(out.contains("a\\*b \\_c\\_ $x*y$"), "{out}");
    }

    #[test]
    fn code_fences_grow_when_the_source_contains_one() {
        let options = Markdown { front_matter: false, ..Markdown::default() };
        let out = render(
            &doc(vec![Block::Code { language: None, source: "```\nx\n```".into() }]),
            &options,
        );
        assert!(out.starts_with("````\n"), "{out}");
    }
}
