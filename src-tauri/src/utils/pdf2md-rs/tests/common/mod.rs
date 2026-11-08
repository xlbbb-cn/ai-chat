//! A PDF fixture built with `lopdf`, shared by the integration tests.
//!
//! It is a small three-page "paper" that exercises every stage of the pipeline
//! on purpose:
//!
//! * a running header and page numbers (furniture removal);
//! * a title and an author line (metadata + title detection);
//! * numbered headings `1`, `2`, `3` and a subsection `1.1` (levels),
//! * a paragraph split across a page break (merge), with a hyphenated word;
//! * a ruled table (grid extraction);
//! * a bulleted list;
//! * a centred display formula (text-layer formula detection).
//!
//! Everything is real PDF: text operators, filled rectangles as rules, several
//! pages. No external fixture file is needed.

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Object, Stream};

/// One page's content stream under construction.
#[derive(Default)]
pub struct PageBuilder {
    operations: Vec<Operation>,
}

impl PageBuilder {
    pub fn new() -> Self {
        PageBuilder { operations: Vec::new() }
    }

    /// Absolute-positioned text (`Tm`, so each call is independent).
    pub fn text(&mut self, x: f32, y: f32, size: f32, bold: bool, text: &str) {
        let font = if bold { "F2" } else { "F1" };
        self.operations.push(Operation::new("BT", vec![]));
        self.operations.push(Operation::new(
            "Tf",
            vec![Object::Name(font.as_bytes().to_vec()), size.into()],
        ));
        self.operations.push(Operation::new("Tm", vec![1.into(), 0.into(), 0.into(), 1.into(), x.into(), y.into()]));
        self.operations
            .push(Operation::new("Tj", vec![Object::string_literal(escape(text))]));
        self.operations.push(Operation::new("ET", vec![]));
    }

    /// Text centred on an A4 page, with a rough Helvetica width estimate.
    pub fn centered(&mut self, y: f32, size: f32, bold: bool, text: &str) {
        let width = text.chars().count() as f32 * size * 0.5;
        let x = ((595.0 - width) * 0.5).max(40.0);
        self.text(x, y, size, bold, text);
    }

    /// A paragraph: one text line every `size * 1.35`, from `y` downwards.
    /// Returns the y of the next free line.
    pub fn paragraph(&mut self, x: f32, y: f32, size: f32, lines: &[&str]) -> f32 {
        let mut y = y;
        for line in lines {
            self.text(x, y, size, false, line);
            y -= size * 1.35;
        }
        y
    }

    /// A filled rectangle — how the extractor sees a rule.
    pub fn rule(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.operations.push(Operation::new(
            "re",
            vec![x.into(), y.into(), width.into(), height.into()],
        ));
        self.operations.push(Operation::new("f", vec![]));
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            _ => out.push(ch),
        }
    }
    out
}

/// Build the three-page fixture.
pub fn sample_pdf() -> Vec<u8> {
    let mut pdf = lopdf::Document::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let regular = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let bold = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica-Bold",
    });

    let mut pages: Vec<PageBuilder> = Vec::new();

    // ── Page 1 ──────────────────────────────────────────────────────────────
    let mut page = PageBuilder::new();
    page.text(72.0, 800.0, 8.0, false, "Journal of Testing and Examples");
    page.centered(730.0, 18.0, true, "Understanding Widgets: A Study");
    page.centered(705.0, 11.0, false, "by Ada Lovelace and Alan Turing");
    page.text(72.0, 670.0, 12.0, true, "Abstract");
    page.paragraph(
        72.0,
        645.0,
        11.0,
        &[
            "We present a study of widgets and the pipelines that produce",
            "them. The widget pipeline is measured twice on every page.",
        ],
    );
    page.text(72.0, 580.0, 12.0, true, "1 Introduction");
    page.paragraph(
        72.0,
        555.0,
        11.0,
        &[
            "Widgets are important because everything is made of them.",
            "This paragraph is deliberately cut by a page break and",
            "continues on the next page so the cleaner has to stitch it.",
            "In this section we describe the widget",
        ],
    );
    page.text(72.0, 40.0, 9.0, false, "- 1 -");
    pages.push(page);

    // ── Page 2 ──────────────────────────────────────────────────────────────
    let mut page = PageBuilder::new();
    page.text(72.0, 800.0, 8.0, false, "Journal of Testing and Examples");
    page.paragraph(
        72.0,
        760.0,
        11.0,
        &[
            "pipeline in detail. We measured every widget twice and the",
            "results were consistent across the three runs we performed.",
        ],
    );
    page.text(72.0, 700.0, 12.0, true, "2 Methods");
    page.text(72.0, 675.0, 11.0, false, "Each widget was weighed and then measured again.");

    // A ruled table: 3 horizontal rules, 4 vertical rules, 2 rows.
    for y in [570.0f32, 545.0, 520.0] {
        page.rule(72.0, y, 444.0, 0.8);
    }
    for x in [72.0f32, 220.0, 368.0, 516.0] {
        page.rule(x, 520.0, 0.8, 50.0);
    }
    page.text(80.0, 555.0, 11.0, false, "Method");
    page.text(228.0, 555.0, 11.0, false, "Score");
    page.text(80.0, 530.0, 11.0, false, "ours");
    page.text(228.0, 530.0, 11.0, false, "97.5");
    page.text(72.0, 40.0, 9.0, false, "- 2 -");
    pages.push(page);

    // ── Page 3 ──────────────────────────────────────────────────────────────
    let mut page = PageBuilder::new();
    page.text(72.0, 800.0, 8.0, false, "Journal of Testing and Examples");
    page.text(72.0, 740.0, 12.0, true, "2.1 Measurement");
    page.text(72.0, 720.0, 11.0, false, "Measurement details are given below for completeness.");
    page.text(72.0, 660.0, 12.0, true, "3 Results");
    page.paragraph(72.0, 635.0, 11.0, &["Widgets work well."]);
    page.text(72.0, 600.0, 11.0, false, "• First widget");
    page.text(72.0, 576.0, 11.0, false, "• Second widget");
    page.centered(540.0, 11.0, false, "y[n] = h[n] * x[n] + w[n]");
    page.text(72.0, 460.0, 11.0, false, "where w[n] denotes additive white Gaussian noise.");
    page.text(72.0, 40.0, 9.0, false, "- 3 -");
    pages.push(page);

    let mut kids: Vec<Object> = Vec::new();
    for page in &pages {
        let content = Content { operations: page.operations.clone() };
        let content_id = pdf.add_object(Stream::new(dictionary! {}, content.encode().expect("encode content")));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Contents" => content_id,
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => regular, "F2" => bold },
            },
        });
        kids.push(page_id.into());
    }

    let count = kids.len() as i64;
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).expect("write the fixture PDF");
    bytes
}
