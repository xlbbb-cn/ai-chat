//! PDFOxide: the built-in pure-Rust PDF reader.
//!
//! It is the only backend that touches `lopdf`; everything above it works on
//! [`ExtractedPage`] values, which is what makes the optional GROBID backend a
//! drop-in alternative rather than a second pipeline.
//!
//! Responsibilities, in order: page tree walk, page geometry (inherited
//! `/MediaBox`, `/Rotate`), content-stream interpretation (positioned glyphs,
//! rules, images), then line assembly.

pub mod content;
mod cmap;
mod fallback;
pub mod fonts;
pub mod images;
pub mod lines;

pub use content::{Glyph, ImagePlacement, PageContent, Rule};
pub use fonts::{FontInfo, FontTable};
pub use images::{ImageData, ImageFormat};
pub use lines::{Line, Word};

use lopdf::{Document, Object, ObjectId};

use crate::error::{Error, Result};
use crate::options::Options;

/// Metadata read from the PDF information dictionary.
#[derive(Debug, Clone, Default)]
pub struct PdfInfo {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub subject: Option<String>,
    pub keywords: Vec<String>,
    pub producer: Option<String>,
    pub creator: Option<String>,
    /// PDF version from the file header (`1.7`, `2.0`, …).
    pub version: String,
}

/// Everything one page yielded.
#[derive(Debug, Clone, Default)]
pub struct ExtractedPage {
    /// 1-based page number as it appears in the document.
    pub number: usize,
    pub width: f32,
    pub height: f32,
    /// `/Rotate` value normalised to 0/90/180/270.
    pub rotation: i64,
    pub lines: Vec<Line>,
    pub rules: Vec<Rule>,
    pub images: Vec<ImagePlacement>,
    pub glyph_count: usize,
}

impl ExtractedPage {
    /// Total number of characters on the page.
    pub fn char_count(&self) -> usize {
        self.lines.iter().map(|l| l.length).sum()
    }

    /// `true` when the page has a usable text layer.
    pub fn has_text(&self) -> bool {
        self.char_count() >= 8
    }
}

/// A whole PDF, reduced to lines, rules and images.
#[derive(Debug, Clone, Default)]
pub struct ExtractedDoc {
    pub pages: Vec<ExtractedPage>,
    pub info: PdfInfo,
}

impl ExtractedDoc {
    /// Sum of characters over all pages.
    pub fn char_count(&self) -> usize {
        self.pages.iter().map(|p| p.char_count()).sum()
    }

    /// `true` when no page carries a text layer (a scanned PDF).
    pub fn is_scanned(&self) -> bool {
        !self.pages.iter().any(|p| p.has_text())
    }
}

/// Read a PDF into [`ExtractedDoc`].
///
/// `on_page` is called with `(page_number, total_pages_to_convert)` before each
/// page, which is what the job manager turns into progress events.
pub fn extract(
    doc: &Document,
    options: &Options,
    on_page: &mut dyn FnMut(usize, usize),
) -> Result<ExtractedDoc> {
    if doc.is_encrypted() {
        return Err(Error::Encrypted);
    }
    let pages: Vec<(u32, ObjectId)> = doc
        .get_pages()
        .into_iter()
        .filter(|(number, _)| options.includes_page(*number as usize))
        .collect();
    if pages.is_empty() {
        return Err(Error::NoPages);
    }

    let mut extracted = ExtractedDoc { pages: Vec::with_capacity(pages.len()), info: read_info(doc) };
    let total = pages.len();
    for (number, page_id) in pages {
        on_page(number as usize, total);
        let (width, height) = page_size(doc, page_id).unwrap_or((595.0, 842.0));
        let rotation = page_rotation(doc, page_id);
        let font_table = FontTable::build(doc, page_id);
        let page_content = content::interpret_page(
            doc,
            page_id,
            height,
            &font_table,
            options.assets.dir.is_some(),
        )?;
        let lines = lines::build(&page_content.glyphs);
        extracted.pages.push(ExtractedPage {
            number: number as usize,
            width,
            height,
            rotation,
            lines,
            rules: page_content.rules.into_iter().filter(|r| !r.is_furniture()).collect(),
            images: page_content.images,
            glyph_count: page_content.glyphs.len(),
        });
    }
    Ok(extracted)
}

/// Read the information dictionary.
fn read_info(doc: &Document) -> PdfInfo {
    let mut info = PdfInfo { version: doc.version.clone(), ..PdfInfo::default() };
    let Ok(info_ref) = doc.trailer.get(b"Info") else {
        return info;
    };
    let Ok((_, Object::Dictionary(dict))) = doc.dereference(info_ref) else {
        return info;
    };
    info.title = text_field(doc, dict, b"Title");
    info.subject = text_field(doc, dict, b"Subject");
    info.producer = text_field(doc, dict, b"Producer");
    info.creator = text_field(doc, dict, b"Creator");
    info.authors = text_field(doc, dict, b"Author")
        .map(|raw| split_authors(&raw))
        .unwrap_or_default();
    info.keywords = text_field(doc, dict, b"Keywords")
        .map(|raw| {
            raw.split([',', ';'])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    info
}

fn text_field(doc: &Document, dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
    let value = dict.get_deref(key, doc).ok()?;
    let decoded = lopdf::decode_text_string(value)
        .ok()
        .or_else(|| value.as_str().ok().map(fonts::latin1))?;
    let trimmed = decoded.trim().trim_matches('\u{0}').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// `"Ada Lovelace; Alan Turing"` and `"A. Lovelace and A. Turing"` → two names.
pub fn split_authors(raw: &str) -> Vec<String> {
    let mut parts: Vec<String> = raw
        .split([';', '|'])
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if parts.len() <= 1 {
        parts = raw
            .split(" and ")
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
    }
    if parts.len() <= 1 {
        // `"Ada Lovelace, Alan Turing"` — only split when every part looks like
        // a name (no single-letter fragments, no trailing comma lists).
        let by_comma: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).collect();
        if by_comma.len() > 1 && by_comma.iter().all(|p| p.split_whitespace().count() >= 2) {
            parts = by_comma;
        }
    }
    parts
        .into_iter()
        .map(|s| s.trim().trim_matches(',').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Page size from `/CropBox` or `/MediaBox`, inherited through the page tree.
fn page_size(doc: &Document, page_id: ObjectId) -> Option<(f32, f32)> {
    for key in [b"CropBox".as_slice(), b"MediaBox".as_slice()] {
        if let Some(rect) = inherited_box(doc, page_id, key) {
            return Some(rect);
        }
    }
    None
}

fn inherited_box(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<(f32, f32)> {
    let mut current = page_id;
    for _ in 0..16 {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(box_value) = dict.get_deref(key, doc) {
            if let Ok(items) = box_value.as_array() {
                let values: Vec<f32> = items.iter().filter_map(|o| o.as_float().ok()).collect();
                if values.len() == 4 {
                    let width = (values[2] - values[0]).abs();
                    let height = (values[3] - values[1]).abs();
                    if width > 1.0 && height > 1.0 {
                        return Some((width, height));
                    }
                }
            }
        }
        let Ok(parent) = dict.get(b"Parent") else {
            return None;
        };
        current = parent.as_reference().ok()?;
    }
    None
}

fn page_rotation(doc: &Document, page_id: ObjectId) -> i64 {
    let mut current = page_id;
    for _ in 0..16 {
        let Ok(dict) = doc.get_dictionary(current) else {
            return 0;
        };
        if let Ok(value) = dict.get(b"Rotate") {
            if let Ok(angle) = value.as_i64() {
                return ((angle % 360) + 360) % 360;
            }
        }
        let Ok(parent) = dict.get(b"Parent") else {
            return 0;
        };
        match parent.as_reference() {
            Ok(id) => current = id,
            Err(_) => return 0,
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authors_split_on_semicolons_and_and() {
        assert_eq!(split_authors("Ada Lovelace; Alan Turing"), vec!["Ada Lovelace", "Alan Turing"]);
        assert_eq!(split_authors("Ada Lovelace and Alan Turing"), vec!["Ada Lovelace", "Alan Turing"]);
        assert_eq!(split_authors("Solo Author"), vec!["Solo Author"]);
        assert_eq!(
            split_authors("Ada Lovelace, Alan Turing"),
            vec!["Ada Lovelace", "Alan Turing"]
        );
        // A comma list that is not made of names stays one entry.
        assert_eq!(split_authors("Some Corp, Inc."), vec!["Some Corp, Inc."]);
    }

    #[test]
    fn page_without_text_is_not_counted() {
        let mut page = ExtractedPage { number: 1, width: 10.0, height: 10.0, ..Default::default() };
        assert!(!page.has_text());
        page.lines.push(Line {
            text: "hello world".into(),
            rect: crate::geom::Rect::new(0.0, 0.0, 10.0, 10.0),
            size: 10.0,
            bold: false,
            italic: false,
            mono: false,
            math_ratio: 0.0,
            words: Vec::new(),
            length: 11,
        });
        assert!(page.has_text());
        let doc = ExtractedDoc { pages: vec![page], info: PdfInfo::default() };
        assert!(!doc.is_scanned());
        assert_eq!(doc.char_count(), 11);
    }
}
