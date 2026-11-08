//! Optional GROBID backend (`feature = "grobid"`).
//!
//! GROBID is the reference implementation for academic-paper structure: it
//! returns TEI XML with the title, authors, abstract, section tree, figures,
//! formulas and bibliography already separated. When it is available it beats
//! any geometric heuristic, so the pipeline lets it produce the IR directly and
//! skips PDFOxide entirely for that document.
//!
//! This module is a small, dependency-light client:
//!
//! * one HTTP POST of the raw PDF to `/api/processFulltextDocument`
//!   (`Content-Type: application/pdf`, which GROBID accepts as an alternative to
//!   a multipart upload);
//! * a streaming TEI reader that maps the elements we care about to [`Block`]s
//!   and ignores the rest.
//!
//! Failure policy: an unreachable server is an error ([`Error::Backend`]), not a
//! silent fallback. The caller decides — that is the difference between
//! "I want GROBID" and "try GROBID if you like", and [`Options`] exposes both.
//!
//! [`Options`]: crate::options::Options
//!
//! What GROBID cannot give us:
//!
//! * **Images** — the TEI references figures by coordinate, not by bytes, so
//!   figure blocks carry a caption and an empty path (the renderer writes a
//!   placeholder comment).
//! * **Page geometry** — the TEI has page breaks but no media box, so the IR
//!   keeps `pages` empty and only records the page count.

use std::time::Duration;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{Error, Result};
use crate::ir::{Block, Document, Metadata};
use crate::options::Options;

/// Send a PDF to GROBID and parse the TEI it returns.
pub fn convert(pdf: &[u8], options: &Options) -> Result<Document> {
    let xml = request(pdf, options)?;
    parse_tei(&xml)
}

/// The GROBID endpoint, honouring a base URL with or without a trailing slash.
pub fn endpoint(base_url: &str) -> String {
    format!(
        "{}/api/processFulltextDocument",
        base_url.trim_end_matches('/')
    )
}

fn request(pdf: &[u8], options: &Options) -> Result<String> {
    let mut url = endpoint(&options.grobid.base_url);
    let consolidate = if options.grobid.consolidate_header { "1" } else { "0" };
    url.push_str(&format!("?consolidateHeader={consolidate}&consolidateCitations=0"));

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(options.grobid.timeout_secs.max(1))))
        .build()
        .into();

    let mut response = agent
        .post(&url)
        .header("Content-Type", "application/pdf")
        .header("Accept", "application/xml")
        .send(pdf)
        .map_err(|error| Error::Backend(format!("GROBID request failed ({url}): {error}")))?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|error| Error::Backend(format!("cannot read the GROBID response: {error}")))
}

/// What an open TEI element contributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Title,
    Forename,
    Surname,
    Head,
    Paragraph,
    Formula { inline: bool },
    Caption,
    ListItem,
    Other,
}

struct Frame {
    kind: Kind,
    text: String,
}

/// Parse GROBID TEI into the IR.
///
/// Unknown elements are transparent: their text bubbles up to the nearest
/// paragraph/heading, so `<p>see <ref>Smith 2020</ref></p>` keeps "Smith 2020".
pub fn parse_tei(xml: &str) -> Result<Document> {
    let mut reader = Reader::from_str(xml);
    // Text nodes are *not* trimmed: the space in `We study <ref>things</ref>`
    // lives at the edge of a text node, and trimming it would glue the words
    // together. `clean()` collapses the pretty-printer indentation afterwards.

    let mut document = Document::new(Metadata::default());
    let mut blocks: Vec<Block> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut depth: u8 = 0;
    let mut page_breaks = 0usize;
    let mut authors: Vec<String> = Vec::new();
    let mut forename = String::new();
    let mut surname = String::new();
    let mut in_header = false;
    let mut in_title_stmt = false;
    let mut in_figure = false;
    let mut in_references = false;
    let mut figure_caption: Option<String> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|error| Error::Backend(format!("invalid GROBID TEI: {error}")))?;
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                let name = local_name(start.name().as_ref());
                let attributes = attributes(&start);
                match name.as_str() {
                    "teiHeader" => in_header = true,
                    "titleStmt" => in_title_stmt = true,
                    "div" => {
                        depth = depth.saturating_add(1);
                        if attributes.contains("references") || attributes.contains("bibliography")
                        {
                            in_references = true;
                        }
                    }
                    "author" => {
                        forename.clear();
                        surname.clear();
                    }
                    "figure" => {
                        in_figure = true;
                        figure_caption = None;
                    }
                    _ => {}
                }
                let kind = classify(&name, in_header, in_title_stmt, in_figure, &attributes);
                stack.push(Frame { kind, text: String::new() });
            }
            Event::Empty(empty) if local_name(empty.name().as_ref()) == "pb" => {
                page_breaks += 1;
            }
            Event::Empty(_) => {}
            Event::Text(text) => {
                let decoded = text.decode().unwrap_or_default();
                let unescaped = quick_xml::escape::unescape(&decoded)
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| decoded.to_string());
                if let Some(frame) = stack.last_mut() {
                    frame.text.push_str(&unescaped);
                }
            }
            Event::End(end) => {
                let name = local_name(end.name().as_ref());
                let Some(frame) = stack.pop() else { continue };
                let text = clean(&frame.text);

                match frame.kind {
                    Kind::Title
                        if document.metadata.title.is_none() && !text.is_empty() =>
                    {
                        document.metadata.title = Some(text);
                    }
                    Kind::Forename => forename = text,
                    Kind::Surname => surname = text,
                    // The "References" heading itself is useful; the entries
                    // below it are bibliographic noise and are skipped.
                    Kind::Head if !text.is_empty() => {
                        let level = depth.clamp(1, 6);
                        blocks.push(Block::Heading { level, text });
                    }
                    Kind::Paragraph if !in_references && !text.is_empty() => {
                        blocks.push(Block::Paragraph { text });
                    }
                    Kind::Formula { inline } if !text.is_empty() && !in_references => {
                        if inline {
                            // Inline formulas belong to the surrounding
                            // paragraph; append them as math.
                            if let Some(Block::Paragraph { text: previous }) = blocks.last_mut() {
                                previous.push_str(&format!(" ${text}$"));
                            }
                        } else {
                            blocks.push(Block::Formula { latex: text, display: true });
                        }
                    }
                    Kind::Caption if !text.is_empty() => {
                        figure_caption = Some(text);
                    }
                    Kind::ListItem if !in_references && !text.is_empty() => {
                        blocks.push(Block::ListItem { text, ordered: false, level: 0 });
                    }
                    Kind::Other => {
                        // Bubble the text up so wrappers stay transparent.
                        if let Some(parent) = stack.last_mut() {
                            parent.text.push_str(&frame.text);
                        }
                    }
                    _ => {}
                }

                match name.as_str() {
                    "author" => {
                        let name = match (forename.is_empty(), surname.is_empty()) {
                            (false, false) => format!("{forename} {surname}"),
                            (false, true) => forename.clone(),
                            (true, false) => surname.clone(),
                            (true, true) => String::new(),
                        };
                        if !name.is_empty() {
                            authors.push(name);
                        }
                    }
                    "teiHeader" => in_header = false,
                    "titleStmt" => in_title_stmt = false,
                    "figure" => {
                        in_figure = false;
                        blocks.push(Block::Image {
                            path: String::new(),
                            caption: figure_caption.take(),
                        });
                    }
                    "div" => {
                        depth = depth.saturating_sub(1);
                        if in_references && depth == 0 {
                            in_references = false;
                        }
                        // A div at level 0 means the body ended.
                        if name == "div" && depth == 0 && in_references {
                            in_references = false;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    document.metadata.author = authors;
    document.metadata.page_count = page_breaks.max(1);
    document.blocks = blocks;
    Ok(document)
}

fn classify(
    name: &str,
    in_header: bool,
    in_title_stmt: bool,
    in_figure: bool,
    attributes: &str,
) -> Kind {
    match name {
        "title" if in_title_stmt && attributes.contains("main") => Kind::Title,
        "forename" if in_header => Kind::Forename,
        "surname" if in_header => Kind::Surname,
        "head" if !in_figure => Kind::Head,
        "p" if !in_figure => Kind::Paragraph,
        "figDesc" => Kind::Caption,
        "formula" => Kind::Formula { inline: attributes.contains("inline") },
        "item" => Kind::ListItem,
        _ => Kind::Other,
    }
}

/// All attribute values of an element, space-separated, for cheap matching.
fn attributes(start: &quick_xml::events::BytesStart<'_>) -> String {
    let mut out = String::new();
    for attribute in start.attributes().flatten() {
        if let Ok(value) = attribute.unescape_value() {
            out.push_str(&value);
            out.push(' ');
        }
    }
    out
}

fn local_name(name: &[u8]) -> String {
    let name = match name.iter().rposition(|byte| *byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    };
    String::from_utf8_lossy(name).to_string()
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEI: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0">
  <teiHeader>
    <fileDesc>
      <titleStmt>
        <title type="main">A Study of Things</title>
        <author>
          <persName><forename>Ada</forename><surname>Lovelace</surname></persName>
        </author>
        <author>
          <persName><forename>Alan</forename><surname>Turing</surname></persName>
        </author>
      </titleStmt>
    </fileDesc>
  </teiHeader>
  <text>
    <body>
      <div>
        <head>1 Introduction</head>
        <p>We study <ref>things</ref> in detail.</p>
        <formula type="inline">x = 1</formula>
        <p>Second paragraph.</p>
        <figure>
          <head>Figure 1</head>
          <figDesc>A diagram of things.</figDesc>
          <graphic coords="1,1,2,2"/>
        </figure>
        <div>
          <head>1.1 Background</head>
          <p>Background text.</p>
        </div>
      </div>
      <div type="references">
        <head>References</head>
        <listBibl><biblStruct><analytic><title>Ignored</title></analytic></biblStruct></listBibl>
      </div>
    </body>
  </text>
</TEI>"#;

    #[test]
    fn metadata_and_structure_are_mapped() {
        let document = parse_tei(TEI).unwrap();
        assert_eq!(document.metadata.title.as_deref(), Some("A Study of Things"));
        assert_eq!(document.metadata.author, vec!["Ada Lovelace", "Alan Turing"]);
        assert_eq!(document.metadata.page_count, 1);

        assert!(document
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Heading { level: 1, text } if text == "1 Introduction")));
        assert!(document
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Heading { level: 2, text } if text == "1.1 Background")));
        assert!(document
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Paragraph { text } if text.contains("things") && text.contains("detail"))));
    }

    #[test]
    fn nested_markup_is_transparent() {
        let document = parse_tei(TEI).unwrap();
        assert!(
            document.blocks.iter().any(|b| b.text() == Some("We study things in detail. $x = 1$")),
            "{:#?}",
            document.blocks
        );
    }

    #[test]
    fn figures_become_images_with_a_caption() {
        let document = parse_tei(TEI).unwrap();
        let image = document
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Image { caption, .. } => Some(caption.clone()),
                _ => None,
            })
            .flatten();
        assert_eq!(image.as_deref(), Some("A diagram of things."));
    }

    #[test]
    fn references_are_not_parsed_as_body_text() {
        let document = parse_tei(TEI).unwrap();
        assert!(document.blocks.iter().any(|b| b.text() == Some("References")));
        assert!(!document.blocks.iter().any(|b| b.text() == Some("Ignored")));
    }

    #[test]
    fn endpoint_does_not_double_the_slash() {
        assert_eq!(
            endpoint("http://localhost:8070/"),
            "http://localhost:8070/api/processFulltextDocument"
        );
    }
}
