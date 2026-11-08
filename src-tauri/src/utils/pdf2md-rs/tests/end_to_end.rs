//! End-to-end conversion of a real (generated) three-page PDF.
//!
//! These tests are the contract of the whole pipeline: if the header removal,
//! the heading levels, the table, the list, the formula or the cross-page
//! paragraph merge break, one of them fails.

mod common;

use pdf2md_rs::ir::Block;
use pdf2md_rs::options::{Options, TableStyle};
use pdf2md_rs::pipeline;
use pdf2md_rs::router::DocumentClass;

fn convert(options: &Options) -> pipeline::Conversion {
    let bytes = common::sample_pdf();
    pipeline::convert_bytes(&bytes, options, &mut |_| {}).expect("conversion")
}

#[test]
fn the_sample_paper_becomes_structured_markdown() {
    let conversion = convert(&Options::paper());
    let markdown = &conversion.markdown;

    // Front matter carries the title and both authors.
    assert!(markdown.contains("title: \"Understanding Widgets: A Study\""), "{markdown}");
    assert!(markdown.contains("  - \"Ada Lovelace\""), "{markdown}");
    assert!(markdown.contains("  - \"Alan Turing\""), "{markdown}");

    // Headings: the structural keyword, the numbered sections, the subsection.
    assert!(markdown.contains("# Abstract"), "{markdown}");
    assert!(markdown.contains("# 1 Introduction"), "{markdown}");
    assert!(markdown.contains("# 2 Methods"), "{markdown}");
    assert!(markdown.contains("## 2.1 Measurement"), "{markdown}");
    assert!(markdown.contains("# 3 Results"), "{markdown}");

    // Running header and page numbers are gone.
    assert!(!markdown.contains("Journal of Testing and Examples"), "{markdown}");
    assert!(!markdown.contains("- 1 -"), "{markdown}");
    assert!(!markdown.contains("- 3 -"), "{markdown}");

    // The paragraph cut by the page break is one paragraph again.
    assert!(markdown.contains("describe the widget pipeline in detail."), "{markdown}");

    // The ruled table came out as a GFM table.
    assert!(markdown.contains("| Method | Score |"), "{markdown}");
    assert!(markdown.contains("| ours | 97.5 |"), "{markdown}");

    // Bullets.
    assert!(markdown.contains("- First widget\n- Second widget"), "{markdown}");

    // The centred text-layer formula became display math.
    assert!(markdown.contains("$$\n"), "{markdown}");
    assert!(markdown.contains("y[n] = h[n] * x[n] + w[n]"), "{markdown}");

    // Page breaks are marked, and the router saw a paper.
    assert!(markdown.contains("<!-- page 2 -->"), "{markdown}");
    assert_eq!(conversion.profile.class, DocumentClass::Paper);
    assert_eq!(conversion.document.metadata.page_count, 3);
    assert!(conversion.assets.is_empty());
}

#[test]
fn the_ir_keeps_the_blocks_the_renderer_saw() {
    let conversion = convert(&Options::paper());
    let blocks = &conversion.document.blocks;
    assert!(blocks.iter().any(|b| matches!(b, Block::Table { rows } if rows.len() >= 2)));
    assert!(blocks.iter().any(|b| matches!(b, Block::Formula { .. })));
    assert!(blocks.iter().filter(|b| matches!(b, Block::Heading { .. })).count() >= 5);
}

#[test]
fn headers_can_be_kept_on_request() {
    let mut options = Options::paper();
    options.clean.remove_headers_footers = false;
    let conversion = convert(&options);
    assert!(conversion.markdown.contains("Journal of Testing and Examples"));
}

#[test]
fn a_page_range_limits_the_output() {
    let options = Options::paper().with_page_range(2, 3);
    let conversion = convert(&options);
    assert!(!conversion.markdown.contains("1 Introduction"), "{}", conversion.markdown);
    assert!(conversion.markdown.contains("2 Methods"));
    assert_eq!(conversion.document.metadata.page_count, 2);
}

#[test]
fn the_page_count_is_readable_without_converting() {
    let bytes = common::sample_pdf();
    assert_eq!(pipeline::page_count(&bytes).expect("page count"), 3);
    // Junk is an error, not a panic.
    assert!(pipeline::page_count(b"not a pdf").is_err());
}

#[test]
fn table_styles_are_honoured() {
    let mut options = Options::paper();
    options.markdown.table_style = TableStyle::Html;
    let conversion = convert(&options);
    assert!(conversion.markdown.contains("<table>"), "{}", conversion.markdown);
    assert!(conversion.markdown.contains("<th>Method</th>"));

    options.markdown.table_style = TableStyle::Text;
    let conversion = convert(&options);
    assert!(conversion.markdown.contains("Method  Score"), "{}", conversion.markdown);
}

#[test]
fn progress_is_reported_from_start_to_finish() {
    let bytes = common::sample_pdf();
    let mut seen: Vec<(pipeline::Stage, u8)> = Vec::new();
    pipeline::convert_bytes(&bytes, &Options::paper(), &mut |event| {
        seen.push((event.stage, event.percent()));
    })
    .expect("conversion");
    assert_eq!(seen.first().map(|s| s.0), Some(pipeline::Stage::Extracting));
    assert_eq!(seen.last().map(|s| s.0), Some(pipeline::Stage::Done));
    assert_eq!(seen.last().map(|s| s.1), Some(100));
    // Percent must never go backwards.
    assert!(seen.windows(2).all(|pair| pair[1].1 >= pair[0].1), "{seen:?}");
}

#[test]
fn garbage_input_is_rejected_without_panicking() {
    let error = pipeline::convert_bytes(b"not a pdf at all", &Options::default(), &mut |_| {})
        .expect_err("garbage must fail");
    assert!(matches!(error, pdf2md_rs::Error::Pdf(_)), "{error}");
}

#[test]
fn json_round_trip_of_a_converted_document() {
    let conversion = convert(&Options::paper());
    let json = conversion.document.to_json().expect("serialise");
    let back = pdf2md_rs::Document::from_json(&json).expect("deserialise");
    assert_eq!(back.blocks.len(), conversion.document.blocks.len());
    assert_eq!(back.metadata.title, conversion.document.metadata.title);
}
