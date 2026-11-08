//! Font metadata and text decoding for one page.
//!
//! The extractor needs three things from a font: how to turn the bytes a `Tj`
//! operator carries into Unicode, how wide each glyph is (so it can place the
//! next one), and whether the font is bold / italic / monospaced /
//! mathematical — the last four feed the heading and formula heuristics.
//!
//! Widths are *estimated* from a table calibrated on Helvetica: precise enough
//! for line breaking and column detection, and it avoids shipping an AFM
//! database.

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Encoding, ObjectId};

/// PDF font descriptor flags (ISO 32000-1, table 123) that we care about.
const FLAG_FIXED_PITCH: i64 = 1;
const FLAG_ITALIC: i64 = 1 << 6;
const FLAG_FORCE_BOLD: i64 = 1 << 18;

/// What the layout heuristics need to know about a font.
#[derive(Debug, Clone)]
pub struct FontInfo {
    /// Base font name with the `ABCDEF+` subset prefix removed.
    pub name: String,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    /// A math/symbol font (`CMSY`, `Math`, `Symbol`, `STIX`…).
    pub math: bool,
    /// Composite font with multi-byte codes (`/Subtype /Type0`).
    pub cid: bool,
    /// Ascent as a fraction of the em, used to derive a glyph's top edge.
    pub ascent: f32,
    /// Exact advances from the font's `/Widths` array, in em units, indexed by
    /// `code - first_char`. Empty when the PDF does not carry them (composite
    /// fonts, broken generators), in which case [`FontInfo::advance`] estimates.
    pub widths: Vec<f32>,
    /// `/FirstChar`: the code the first entry of `widths` belongs to.
    pub first_char: u16,
    /// `/MissingWidth`, in em units, for codes outside `widths`.
    pub missing_width: Option<f32>,
}

impl Default for FontInfo {
    fn default() -> Self {
        FontInfo {
            name: "unknown".to_string(),
            bold: false,
            italic: false,
            mono: false,
            math: false,
            cid: false,
            ascent: 0.75,
            widths: Vec::new(),
            first_char: 0,
            missing_width: None,
        }
    }
}

impl FontInfo {
    /// Classify a font from its `/BaseFont` name, descriptor flags and subtype.
    pub fn classify(base_font: &str, flags: i64, italic_angle: f32, cid: bool) -> Self {
        let name = clean_font_name(base_font);
        let lower = name.to_ascii_lowercase();
        let bold = lower.contains("bold")
            || lower.contains("black")
            || lower.contains("heavy")
            || lower.contains("semib")
            || lower.contains("demi")
            || lower.contains("cmbx")
            || flags & FLAG_FORCE_BOLD != 0;
        let italic = lower.contains("italic")
            || lower.contains("oblique")
            || lower.contains("slanted")
            || lower.contains("cmmi")
            || flags & FLAG_ITALIC != 0
            || italic_angle.abs() > 4.0;
        let mono = lower.contains("mono")
            || lower.contains("courier")
            || lower.contains("consol")
            || lower.contains("menlo")
            || lower.contains("monaco")
            || lower.contains("typewriter")
            || lower.contains("inconsolata")
            || lower.contains("cmtt")
            || flags & FLAG_FIXED_PITCH != 0;
        let math = lower.contains("math")
            || lower.contains("cmsy")
            || lower.contains("cmex")
            || lower.contains("cmmi")
            || lower.contains("msam")
            || lower.contains("msbm")
            || lower.contains("symbol")
            || lower.contains("mtmi")
            || lower.contains("mtsy")
            || lower.contains("stix");
        FontInfo {
            name,
            bold,
            italic,
            mono,
            math,
            cid,
            ascent: 0.75,
            widths: Vec::new(),
            first_char: 0,
            missing_width: None,
        }
    }

    /// Advance width of a single-byte code, from the font's own `/Widths`.
    ///
    /// `None` when the font carries no widths or the entry is unusable; the
    /// caller then falls back to [`FontInfo::advance`]. Using the real widths
    /// instead of an estimate is what keeps runs that resume from an explicit
    /// text matrix (`Tm`/`Td`) from overlapping the text before them.
    pub fn advance_code(&self, code: u8) -> Option<f32> {
        let index = i32::from(code) - i32::from(self.first_char);
        if index >= 0 {
            if let Some(width) = self.widths.get(index as usize) {
                if width.is_finite() {
                    return Some(*width);
                }
            }
        }
        self.missing_width.filter(|width| width.is_finite())
    }

    /// Estimated advance width of `c`, as a fraction of the font size.
    pub fn advance(&self, c: char) -> f32 {
        if self.mono {
            return 0.6;
        }
        if is_wide(c) {
            return 1.0;
        }
        match c {
            ' ' => 0.278,
            'i' | 'l' | 'j' | 't' | 'f' | 'r' | 'I' | '.' | ',' | ':' | ';' | '\'' | '|' | '!'
            | '[' | ']' | '(' | ')' => 0.3,
            'm' | 'M' | 'w' | 'W' => 0.83,
            'A'..='Z' => 0.68,
            '0'..='9' => 0.556,
            '-' | '+' | '=' | '~' | '<' | '>' | '/' | '\\' => 0.58,
            _ => 0.52,
        }
    }
}

/// Remove `ABCDEF+` subset prefixes and a leading slash.
pub fn clean_font_name(base_font: &str) -> String {
    let name = base_font.trim_start_matches('/');
    match name.split_once('+') {
        Some((prefix, rest))
            if prefix.len() == 6 && prefix.chars().all(|c| c.is_ascii_uppercase()) =>
        {
            rest.to_string()
        }
        _ => name.to_string(),
    }
}

/// `true` for characters that occupy a full em (CJK, kana, hangul, full-width).
pub fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
    )
}

/// One page's fonts, keyed by the resource name a `Tf` operator selects.
pub struct FontTable<'a> {
    fonts: HashMap<Vec<u8>, Entry<'a>>,
}

impl std::fmt::Debug for FontTable<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontTable")
            .field("fonts", &self.fonts.keys().map(|k| String::from_utf8_lossy(k)).collect::<Vec<_>>())
            .finish()
    }
}

struct Entry<'a> {
    info: FontInfo,
    encoding: Option<Encoding<'a>>,
}

impl<'a> FontTable<'a> {
    /// Resolve every `/Font` entry of the page's resource dictionary.
    pub fn build(doc: &'a Document, page_id: ObjectId) -> Self {
        let mut fonts = HashMap::new();
        let Ok(page_fonts) = doc.get_page_fonts(page_id) else {
            return FontTable { fonts };
        };
        for (name, dict) in page_fonts {
            let base_font = dict
                .get(b"BaseFont")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .unwrap_or_default();
            let subtype = dict
                .get(b"Subtype")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|(_, o)| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .unwrap_or_default();
            let cid = subtype == "Type0";
            let descriptor = descriptor_dict(doc, dict);
            let flags = descriptor
                .and_then(|d| d.get(b"Flags").ok())
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0);
            let italic_angle = descriptor
                .and_then(|d| d.get(b"ItalicAngle").ok())
                .and_then(|o| o.as_float().ok())
                .unwrap_or(0.0);
            let ascent = descriptor
                .and_then(|d| d.get(b"Ascent").ok())
                .and_then(|o| o.as_float().ok())
                .map(|a| (a / 1000.0).clamp(0.4, 1.2))
                .unwrap_or(0.75);
            let mut info = FontInfo::classify(&base_font, flags, italic_angle, cid);
            info.ascent = ascent;
            if !cid {
                let (widths, first_char, missing_width) = font_widths(doc, dict, descriptor);
                info.widths = widths;
                info.first_char = first_char;
                info.missing_width = missing_width;
            }
            let encoding = dict.get_font_encoding(doc).ok();
            fonts.insert(name, Entry { info, encoding });
        }
        FontTable { fonts }
    }

    /// Metadata for the font selected by `Tf`, if it is known.
    pub fn info(&self, name: &[u8]) -> Option<&FontInfo> {
        self.fonts.get(name).map(|e| &e.info)
    }

    /// Decode a string operand to Unicode.
    ///
    /// Fonts with a `/ToUnicode` CMap decode exactly; simple fonts fall back to
    /// their base encoding and finally to Latin-1, so no byte is silently
    /// dropped (a wrong-but-visible character beats a hole in the text).
    pub fn decode(&self, name: &[u8], bytes: &[u8]) -> String {
        let Some(entry) = self.fonts.get(name) else {
            return utf8_if_valid(bytes).map(str::to_string).unwrap_or_else(|| latin1(bytes));
        };
        // Some generators write UTF-8 into a literal string although the font
        // declares a single-byte encoding; a byte sequence that parses as
        // multi-byte UTF-8 is authoritative over the (wrong) table.
        if !entry.info.cid {
            if let Some(text) = utf8_if_valid(bytes) {
                return text.to_string();
            }
        }
        if let Some(encoding) = &entry.encoding {
            if let Ok(text) = encoding.bytes_to_string(bytes) {
                if !text.is_empty() {
                    return text;
                }
            }
        }
        latin1(bytes)
    }

    /// `true` when no font resource was found at all (broken page resources).
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }
}

/// Read `/FirstChar`, `/Widths` and `/MissingWidth` from a simple font.
///
/// The values are 1/1000 em; the `/FontMatrix` of a Type 3 font is ignored
/// (those glyph widths are useless for text layout anyway), and entries that do
/// not parse become `NaN` so [`FontInfo::advance_code`] falls back per character
/// instead of shifting every code after it.
fn font_widths(
    doc: &Document,
    dict: &Dictionary,
    descriptor: Option<&Dictionary>,
) -> (Vec<f32>, u16, Option<f32>) {
    let first_char = dict
        .get(b"FirstChar")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0)
        .clamp(0, 255) as u16;
    let mut widths: Vec<f32> = Vec::new();
    if let Ok(object) = dict.get_deref(b"Widths", doc) {
        if let Ok(array) = object.as_array() {
            widths = array
                .iter()
                .map(|item| {
                    let (_, value) = doc.dereference(item).unwrap_or((None, item));
                    value
                        .as_float()
                        .or_else(|_| value.as_i64().map(|v| v as f32))
                        .map(|v| v / 1000.0)
                        .unwrap_or(f32::NAN)
                })
                .collect();
        }
    }
    let missing_width = descriptor
        .and_then(|d| d.get(b"MissingWidth").ok())
        .and_then(|o| o.as_float().ok().or_else(|| o.as_i64().ok().map(|v| v as f32)))
        .map(|v| v / 1000.0);
    (widths, first_char, missing_width)
}

/// Follow `/FontDescriptor`, and for composite fonts the descendant's one.
fn descriptor_dict<'a>(doc: &'a Document, dict: &'a Dictionary) -> Option<&'a Dictionary> {
    if let Ok(desc) = dict.get_deref(b"FontDescriptor", doc) {
        if let Ok(d) = desc.as_dict() {
            return Some(d);
        }
    }
    if let Ok(descendants) = dict.get_deref(b"DescendantFonts", doc) {
        if let Ok(array) = descendants.as_array() {
            if let Some(first) = array.first() {
                if let Ok((_, descendant)) = doc.dereference(first) {
                    if let Ok(d) = descendant.as_dict() {
                        if let Ok(desc) = d.get_deref(b"FontDescriptor", doc) {
                            if let Ok(descriptor) = desc.as_dict() {
                                return Some(descriptor);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Latin-1 decoding, the last-resort fallback.
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

/// Read the bytes as UTF-8 when they really are UTF-8.
///
/// Generators that write no `/ToUnicode` CMap very often put **UTF-8** in the
/// string operand anyway (reportlab, some LaTeX pipelines, hand-written PDFs),
/// and reading those bytes as Latin-1 turns every bullet into `â€¢`. A byte
/// string that is valid UTF-8 *and* contains a multi-byte sequence is taken as
/// UTF-8; anything else is not, and the caller keeps its own decoding (Latin-1
/// never loses a byte). Composite (`Type0`) fonts are excluded at the call site:
/// their codes are 2-byte CIDs, not a character stream.
fn utf8_if_valid(bytes: &[u8]) -> Option<&str> {
    if bytes.iter().any(|b| *b >= 0xC2) {
        std::str::from_utf8(bytes).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_operands_are_not_read_as_latin1() {
        // `•` is E2 80 A2 in UTF-8 and `¢` in Latin-1.
        assert_eq!(utf8_if_valid("• First".as_bytes()), Some("• First"));
        assert_eq!(utf8_if_valid("café".as_bytes()), Some("café"));
        // Latin-1 bytes are not valid UTF-8, so they keep their own reading.
        assert_eq!(utf8_if_valid(b"caf\xe9"), None);
        assert_eq!(latin1(b"caf\xe9"), "café");
        // Pure ASCII is left to the font encoding.
        assert_eq!(utf8_if_valid(b"plain"), None);
    }

    #[test]
    fn subset_prefix_is_stripped() {
        assert_eq!(clean_font_name("/ABCDEF+Helvetica-Bold"), "Helvetica-Bold");
        assert_eq!(clean_font_name("Times New Roman"), "Times New Roman");
    }

    #[test]
    fn classification_reads_the_name() {
        let bold = FontInfo::classify("ABCDEF+Times-Bold", 0, 0.0, false);
        assert!(bold.bold && !bold.italic && !bold.mono);
        assert!(FontInfo::classify("Courier", 0, 0.0, false).mono);
        assert!(!FontInfo::classify("NimbusRomNo9L-Medi", 0, 0.0, false).math);
        assert!(FontInfo::classify("CMMI10", 0, 0.0, false).math);
        assert!(FontInfo::classify("FooBar", FLAG_FORCE_BOLD, 0.0, false).bold);
        assert!(FontInfo::classify("FooBar", FLAG_ITALIC, 0.0, false).italic);
    }

    #[test]
    fn advance_is_monotonic_for_typical_glyphs() {
        let f = FontInfo::classify("Helvetica", 0, 0.0, false);
        assert!(f.advance('i') < f.advance('A'));
        assert!(f.advance('A') < f.advance('m'));
        assert_eq!(f.advance('中'), 1.0);
        assert_eq!(FontInfo::classify("Courier", 0, 0.0, false).advance('i'), 0.6);
    }

    #[test]
    fn latin1_fallback_never_drops_bytes() {
        assert_eq!(latin1(b"caf\xe9"), "café");
    }
}
