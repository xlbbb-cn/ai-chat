//! Composite (CID) fonts: how a page of Chinese gets its characters back.
//!
//! Every fixture here is a real PDF written with `lopdf` on the fly, because the
//! point of the test is the *font dictionary*, not a file that happens to sit on
//! someone's disk. The shapes are the ones the field actually produces:
//!
//! * a `ToUnicode` CMap whose header a strict grammar rejects — the bug that
//!   turned a 174-page Chinese book into `ł W L ! ¥ Ø J T X`;
//! * no `ToUnicode` at all, with the code space stated by a predefined CMap name
//!   (`GBK-EUC-H`, `UniGB-UCS2-H`) or by a `UCS` CID ordering;
//! * no `ToUnicode` and nothing stated, where only the font name hints at CJK
//!   and a scoring gate decides whether the guess may be believed;
//! * and the two cases that must **not** be "rescued": glyph-id CIDs under a
//!   Latin name, and an ordinary WinAnsi run with one high byte in it.

use lopdf::{content::Operation, dictionary, Dictionary, Document, Object, Stream, StringFormat};

use pdf2md_rs::extract::pdfoxide::extract;
use pdf2md_rs::Options;

/// What the font dictionary of the fixture should carry.
struct Spec {
    /// `/BaseFont` of both the Type0 font and its descendant.
    base: &'static str,
    /// `/Encoding`: a name (`Identity-H`, `GBK-EUC-H`) or none at all.
    encoding: Option<&'static [u8]>,
    /// `/Ordering` of the descendant's `/CIDSystemInfo`.
    ordering: &'static str,
    /// The `ToUnicode` CMap program, when the font carries one.
    to_unicode: Option<&'static str>,
}

/// One line of the fixture: raw character codes, placed at its own y.
struct Run {
    codes: Vec<u8>,
    y: f32,
}

/// The CMap the calibration book writes: the `/CMapName` value has no leading
/// slash, which is invalid PostScript and enough to make a strict parser drop the
/// whole mapping.
const BROKEN_HEADER_CMAP: &str = r#"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapName SimSun-cmap def
/CMapType 2 def
/CIDSystemInfo <<
/Registry (Adobe)
/Ordering (UCS)
/Supplement 0
>> def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
4 beginbfchar
<2D1E> <76EE>
<1585> <5F55>
<0A57> <524D>
<041D> <8A00>
endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end
"#;

/// A well-formed CMap, for the case where `lopdf` itself would also succeed.
const GOOD_HEADER_CMAP: &str = r#"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
2 beginbfchar
<0041> <76EE>
<0042> <5F55>
endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end
"#;

/// Build a one-page PDF with a composite font, `runs` of codes shown on it.
fn composite_pdf(spec: &Spec, runs: &[Run]) -> Vec<u8> {
    let mut pdf = Document::with_version("1.5");
    let pages_id = pdf.new_object_id();

    let descriptor_id = pdf.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => spec.base,
        "Flags" => 4i64,             // symbolic
        "FontBBox" => vec![
            Object::Integer(0),
            Object::Integer(-200),
            Object::Integer(1000),
            Object::Integer(800),
        ],
        "ItalicAngle" => Object::Integer(0),
        "Ascent" => Object::Integer(800),
        "Descent" => Object::Integer(-200),
        "CapHeight" => Object::Integer(700),
        "StemV" => Object::Integer(90),
    });

    let mut descendant = dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => spec.base,
        "CIDSystemInfo" => dictionary! {
            "Registry" => "Adobe", "Ordering" => spec.ordering, "Supplement" => 0i64,
        },
        "FontDescriptor" => descriptor_id,
        "DW" => 1000i64,
    };
    // Widths by CID range, so the font looks complete even though the extractor
    // estimates advance for composite fonts anyway.
    descendant.set("W", Object::Array(vec![
        Object::Integer(0),
        Object::Array(vec![Object::Integer(500)]),
        Object::Integer(1),
        Object::Array(vec![Object::Integer(900)]),
    ]));
    let descendant_id = pdf.add_object(Object::Dictionary(descendant));

    let mut font = dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => spec.base,
        "DescendantFonts" => vec![descendant_id.into()],
    };
    if let Some(encoding) = spec.encoding {
        font.set("Encoding", Object::Name(encoding.to_vec()));
    }
    if let Some(cmap) = spec.to_unicode {
        let stream_id = pdf.add_object(Object::Stream(Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            // Storing it compressed is deliberate: reading the CMap must go
            // through the same decode path a real document uses.
            flate(cmap.as_bytes()),
        )));
        font.set("ToUnicode", stream_id);
    }
    let font_id = pdf.add_object(Object::Dictionary(font));

    let mut operations = Vec::new();
    for run in runs {
        operations.push(Operation::new("BT", vec![]));
        operations.push(Operation::new(
            "Tf",
            vec![Object::Name(b"F0".to_vec()), 12f32.into()],
        ));
        operations.push(Operation::new(
            "Tm",
            vec![
                1f32.into(),
                0f32.into(),
                0f32.into(),
                1f32.into(),
                72f32.into(),
                run.y.into(),
            ],
        ));
        operations.push(Operation::new(
            "Tj",
            vec![Object::String(run.codes.clone(), StringFormat::Hexadecimal)],
        ));
        operations.push(Operation::new("ET", vec![]));
    }
    let content = pdf.add_object(Object::Stream(Stream::new(
        Dictionary::new(),
        lopdf::content::Content {
            operations: operations.clone(),
        }
        .encode()
        .expect("encode content"),
    )));

    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Contents" => content,
        "Resources" => dictionary! { "Font" => dictionary! { "F0" => font_id } },
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1i64,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).expect("write fixture");
    bytes
}

fn flate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    use flate2::write::ZlibEncoder;
    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).expect("compress");
    encoder.finish().expect("finish")
}

/// The text the pipeline read back, one `String` per extracted line.
fn lines_of(bytes: &[u8]) -> Vec<String> {
    let doc = Document::load_mem(bytes).expect("load fixture");
    let extracted = extract(&doc, &Options::default(), &mut |_, _| {}).expect("extract");
    let mut out: Vec<String> = extracted
        .pages
        .iter()
        .flat_map(|page| page.lines.iter())
        .map(|line| line.text.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn broken_cmap_header_still_decodes() {
    let spec = Spec {
        base: "AAAAAA+SimSun",
        encoding: Some(b"Identity-H"),
        ordering: "UCS",
        to_unicode: Some(BROKEN_HEADER_CMAP),
    };
    let bytes = composite_pdf(
        &spec,
        &[
            Run { codes: vec![0x2D, 0x1E, 0x15, 0x85], y: 700.0 },
            Run { codes: vec![0x0A, 0x57, 0x04, 0x1D], y: 660.0 },
        ],
    );
    let lines = lines_of(&bytes);
    assert!(lines.contains(&"目录".to_string()), "{lines:?}");
    assert!(lines.contains(&"前言".to_string()), "{lines:?}");
    // The mojibake this used to produce must be gone entirely.
    assert!(!lines.iter().any(|l| l.contains('ł') || l.contains('Ø')), "{lines:?}");
}

#[test]
fn good_cmap_header_decodes_too() {
    let spec = Spec {
        base: "AAAAAA+SimSun",
        encoding: Some(b"Identity-H"),
        ordering: "UCS",
        to_unicode: Some(GOOD_HEADER_CMAP),
    };
    let bytes = composite_pdf(
        &spec,
        &[Run { codes: vec![0x00, 0x41, 0x00, 0x42], y: 700.0 }],
    );
    assert_eq!(lines_of(&bytes), vec!["目录".to_string()]);
}

#[test]
fn predefined_cmap_names_state_their_own_code_space() {
    // GBK-EUC-H: the codes are GBK byte sequences, and there is no CMap anywhere.
    let gbk = Spec {
        base: "AAAAAA+SimSun",
        encoding: Some(b"GBK-EUC-H"),
        ordering: "GB1",
        to_unicode: None,
    };
    let bytes = composite_pdf(
        &gbk,
        &[Run { codes: vec![0xC4, 0xBF, 0xC2, 0xBC], y: 700.0 }],
    );
    assert_eq!(lines_of(&bytes), vec!["目录".to_string()]);

    // UniGB-UCS2-H: the codes are UTF-16BE values, written straight into the
    // string, which is why this CMap needs no destination table.
    let ucs2 = Spec {
        base: "AAAAAA+SimSun",
        encoding: Some(b"UniGB-UCS2-H"),
        ordering: "GB1",
        to_unicode: None,
    };
    let bytes = composite_pdf(
        &ucs2,
        &[Run { codes: vec![0x76, 0xEE, 0x5F, 0x55], y: 700.0 }],
    );
    assert_eq!(lines_of(&bytes), vec!["目录".to_string()]);
}

#[test]
fn ucs_ordering_under_identity_h_is_unicode() {
    // Identity-H says nothing on its own; the descendant's Ordering (UCS) does.
    let spec = Spec {
        base: "AAAAAA+SimSun",
        encoding: Some(b"Identity-H"),
        ordering: "UCS",
        to_unicode: None,
    };
    let bytes = composite_pdf(
        &spec,
        &[Run { codes: vec![0x76, 0xEE, 0x5F, 0x55], y: 700.0 }],
    );
    assert_eq!(lines_of(&bytes), vec!["目录".to_string()]);
}

#[test]
fn font_name_hint_lets_the_byte_encoding_be_recovered() {
    // Nothing states the code space: Identity-H over an Adobe-GB1 ordering, no
    // CMap. Only `SimSun` in `/BaseFont` suggests GBK, and the codes are valid
    // GBK end to end, so the guess clears its gate.
    let spec = Spec {
        base: "AAAAAA+SimSun",
        encoding: Some(b"Identity-H"),
        ordering: "GB1",
        to_unicode: None,
    };
    let bytes = composite_pdf(
        &spec,
        &[Run { codes: vec![0xC4, 0xBF, 0xC2, 0xBC, 0xD6, 0xD0], y: 700.0 }],
    );
    assert_eq!(lines_of(&bytes), vec!["目录中".to_string()]);
}

#[test]
fn glyph_id_cids_under_a_latin_name_are_not_turned_into_chinese() {
    // The dangerous shape: a subset font whose CIDs are glyph ids (low numbers),
    // a Latin name, no CMap. Reading the pairs as UTF-16 would hand back
    // convincing ideographs, and the byte encodings would hand back junk, so the
    // answer must stay what it always was: no CJK at all.
    let spec = Spec {
        base: "AAAAAE+TimesNewRomanPSMT",
        encoding: Some(b"Identity-H"),
        ordering: "Identity",
        to_unicode: None,
    };
    let bytes = composite_pdf(
        &spec,
        &[Run { codes: vec![0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04], y: 700.0 }],
    );
    let lines = lines_of(&bytes);
    assert!(
        !lines.iter().any(|line| line.chars().any(|c| matches!(c as u32, 0x4E00..=0x9FFF))),
        "invented Chinese: {lines:?}"
    );
}

#[test]
fn simple_font_keeps_its_winansi_bytes() {
    // A one-byte font is not a composite one: `café` with the é as WinAnsi 0xE9
    // must not be "rescued" into anything else.
    let mut pdf = Document::with_version("1.5");
    let pages_id = pdf.new_object_id();
    let font_id = pdf.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "AAAAAA+SimSun",
        // `/Encoding` matters here: with none at all the code space is
        // StandardEncoding, which has no `eacute` at all (Ghostscript decodes
        // 0xE9 as `Oslash`), so the run would legitimately come back `cafØ`.
        "Encoding" => "WinAnsiEncoding",
        "FirstChar" => 32i64, "LastChar" => 255i64, "Widths" => vec![500.into(); 224],
        "FontDescriptor" => dictionary! {
            "Type" => "FontDescriptor", "FontName" => "AAAAAA+SimSun", "Flags" => 32i64,
            "FontBBox" => vec![
            Object::Integer(0),
            Object::Integer(-200),
            Object::Integer(1000),
            Object::Integer(800),
        ],
            "ItalicAngle" => Object::Integer(0), "Ascent" => Object::Integer(800), "Descent" => Object::Integer(-200),
            "CapHeight" => Object::Integer(700), "StemV" => Object::Integer(90),
        },
    });
    let mut operations = Vec::new();
    for (codes, y) in [
        (vec![0x63u8, 0x61, 0x66, 0xE9], 700.0f32),
        (vec![0xE2, 0x80, 0xA2, 0x20, 0x41], 660.0),
    ] {
        operations.push(Operation::new("BT", vec![]));
        operations.push(Operation::new(
            "Tf",
            vec![Object::Name(b"F1".to_vec()), 12f32.into()],
        ));
        operations.push(Operation::new(
            "Tm",
            vec![1f32.into(), 0f32.into(), 0f32.into(), 1f32.into(), 72f32.into(), y.into()],
        ));
        operations.push(Operation::new("Tj", vec![Object::String(codes, StringFormat::Hexadecimal)]));
        operations.push(Operation::new("ET", vec![]));
    }
    let content = pdf.add_object(Object::Stream(Stream::new(
        Dictionary::new(),
        lopdf::content::Content { operations: operations.clone() }
            .encode()
            .expect("encode"),
    )));
    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Contents" => content,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
    });
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1i64,
        }),
    );
    let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    pdf.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).expect("write");

    let lines = lines_of(&bytes);
    assert!(lines.contains(&"café".to_string()), "{lines:?}");
    // The UTF-8 bullet stays a bullet rather than three Latin-1 characters.
    assert!(lines.contains(&"• A".to_string()), "{lines:?}");
}
