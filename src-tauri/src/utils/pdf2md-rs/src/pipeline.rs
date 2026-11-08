//! The orchestrator: one PDF in, Markdown plus assets out.
//!
//! ```text
//! load ──▶ extract ──▶ analyse ──▶ route ──▶ strip furniture ──▶ parse ──▶ clean ──▶ render
//! ```
//!
//! Every stage is a pure function of the previous one, so a failure has exactly
//! one place to come from and the IR can be inspected between any two stages.
//! Progress is reported through a callback (a UI turns it into events, a test
//! counts the stages) and cancellation is polled at every page boundary.
//!
//! The entry points are deliberately small:
//!
//! | function | use |
//! |---|---|
//! | [`convert_bytes`] | in-memory conversion, for uploads |
//! | [`convert_path`] | a file, without writing anything |
//! | [`convert_to_path`] | a file, writing `.md` and `assets/` next to it |
//! | `*_with` | the same, with a [`Control`] for cancellation |
//!
//! The `Conversion` value always carries the IR, so a caller can render again
//! with different options without re-parsing the PDF.

use std::path::{Path, PathBuf};

use crate::assets::{AssetOutput, AssetWriter};
use crate::clean::{self, CleanReport};
use crate::error::{Error, Result};
use crate::extract::pdfoxide;
use crate::ir::Document;
use crate::layout::PageLayout;
use crate::options::{LayoutBackend, Options, ParseMode};
use crate::parse::{self, BuildReport};
use crate::render;
use crate::router::{self, DocumentClass, PdfProfile};

/// Which stage a [`Progress`] event comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Reading,
    Extracting,
    Analysing,
    Parsing,
    Cleaning,
    Rendering,
    Done,
}

impl Stage {
    /// Lowercase identifier, for logs and UI labels.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Reading => "reading",
            Stage::Extracting => "extracting",
            Stage::Analysing => "analysing",
            Stage::Parsing => "parsing",
            Stage::Cleaning => "cleaning",
            Stage::Rendering => "rendering",
            Stage::Done => "done",
        }
    }
}

/// One progress event. `page` is 1-based; `total` is the number of pages that
/// will be converted (after the page range and page limit were applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub page: usize,
    pub total: usize,
}

impl Progress {
    /// Overall completion, 0..=100. Coarse but monotonic: the page work is 80 %
    /// of it, the remaining stages share the rest.
    pub fn percent(&self) -> u8 {
        let stage_base = match self.stage {
            Stage::Reading => 0.0,
            Stage::Extracting => 5.0,
            Stage::Analysing => 55.0,
            Stage::Parsing => 65.0,
            Stage::Cleaning => 85.0,
            Stage::Rendering => 92.0,
            Stage::Done => 100.0,
        };
        let stage_span = match self.stage {
            Stage::Extracting => 50.0,
            Stage::Analysing => 10.0,
            _ => 0.0,
        };
        let fraction = if self.total > 0 {
            (self.page as f32 / self.total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (stage_base + stage_span * fraction).clamp(0.0, 100.0) as u8
    }
}

/// Cancellation hook.
///
/// The pipeline polls it between pages and before the expensive stages. The job
/// manager implements it with an `AtomicBool`, so a cancel request from the UI
/// stops the work at the next boundary instead of at the end.
pub trait Control: Send + Sync {
    /// `true` once the conversion should stop.
    fn cancelled(&self) -> bool;

    /// Name used in [`Error::Cancelled`] (a job id, or `"conversion"`).
    fn job(&self) -> &str {
        "conversion"
    }
}

/// A control that never cancels — used by the convenience entry points.
#[derive(Debug, Clone, Copy, Default)]
pub struct Uncontrolled;

impl Control for Uncontrolled {
    fn cancelled(&self) -> bool {
        false
    }
}

impl Control for () {
    fn cancelled(&self) -> bool {
        false
    }
}

/// Everything a conversion produced.
#[derive(Debug, Clone)]
pub struct Conversion {
    /// The intermediate representation, reusable for another renderer.
    pub document: Document,
    /// The rendered Markdown.
    pub markdown: String,
    /// What the router decided.
    pub profile: PdfProfile,
    /// What the parser found.
    pub report: BuildReport,
    /// What the clean-up changed.
    pub clean: CleanReport,
    /// Files written for the images referenced by the Markdown.
    pub assets: Vec<AssetOutput>,
}

impl Conversion {
    /// Write the Markdown to `path` (creating parent directories).
    pub fn write_markdown(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
        }
        std::fs::write(path, &self.markdown).map_err(|e| Error::io(path, e))
    }

    /// `true` when no page carried a text layer.
    pub fn is_empty(&self) -> bool {
        self.document.content_block_count() == 0
    }
}

/// Convert a PDF that is already in memory.
pub fn convert_bytes(
    bytes: &[u8],
    options: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Conversion> {
    convert_bytes_with(bytes, options, &Uncontrolled, progress)
}

/// Convert in-memory PDF bytes with a cancellation hook.
pub fn convert_bytes_with(
    bytes: &[u8],
    options: &Options,
    control: &dyn Control,
    progress: &mut dyn FnMut(Progress),
) -> Result<Conversion> {
    run(bytes, options, control, progress, None)
}

/// Convert a file without writing any output.
pub fn convert_path(
    path: impl AsRef<Path>,
    options: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Conversion> {
    convert_path_with(path, options, &Uncontrolled, progress)
}

/// Convert a file with a cancellation hook.
pub fn convert_path_with(
    path: impl AsRef<Path>,
    options: &Options,
    control: &dyn Control,
    progress: &mut dyn FnMut(Progress),
) -> Result<Conversion> {
    let path = path.as_ref();
    progress(Progress { stage: Stage::Reading, page: 0, total: 0 });
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    run(&bytes, options, control, progress, None)
}

/// Convert `input` and write `output` (`.md`) plus its `assets/` folder.
///
/// When [`Options::assets`] has no directory, the images are written next to the
/// Markdown file — that is what makes the result portable.
pub fn convert_to_path(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Conversion> {
    convert_to_path_with(input, output, options, &Uncontrolled, progress)
}

/// [`convert_to_path`] with a cancellation hook.
pub fn convert_to_path_with(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &Options,
    control: &dyn Control,
    progress: &mut dyn FnMut(Progress),
) -> Result<Conversion> {
    let input = input.as_ref();
    let output = output.as_ref();
    let mut options = options.clone();
    if options.assets.dir.is_none() {
        let parent = output.parent().filter(|p| !p.as_os_str().is_empty());
        options.assets.dir = Some(parent.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")));
    }

    progress(Progress { stage: Stage::Reading, page: 0, total: 0 });
    let bytes = std::fs::read(input).map_err(|e| Error::io(input, e))?;
    let conversion = run(&bytes, &options, control, progress, None)?;
    conversion.write_markdown(output)?;
    Ok(conversion)
}

fn run(
    bytes: &[u8],
    options: &Options,
    control: &dyn Control,
    progress: &mut dyn FnMut(Progress),
    _output: Option<&Path>,
) -> Result<Conversion> {
    checkpoint(control)?;

    // ── Optional GROBID path ────────────────────────────────────────────────
    if options.layout_backend == LayoutBackend::Grobid || options.mode == ParseMode::ForceGrobid {
        let document = grobid_document(bytes, options, control)?;
        progress(Progress { stage: Stage::Rendering, page: 0, total: 0 });
        let markdown = render::render(&document, &options.markdown);
        progress(Progress { stage: Stage::Done, page: 0, total: 0 });
        return Ok(Conversion {
            profile: PdfProfile::grobid(document.metadata.page_count),
            document,
            markdown,
            report: BuildReport::default(),
            clean: CleanReport::default(),
            assets: Vec::new(),
        });
    }

    // ── PDFOxide path ───────────────────────────────────────────────────────
    progress(Progress { stage: Stage::Extracting, page: 0, total: 0 });
    let mut pdf = lopdf::Document::load_mem(bytes)?;
    if pdf.is_encrypted() {
        match &options.password {
            Some(password) => {
                pdf.decrypt(password).map_err(|_| Error::Encrypted)?;
            }
            None => return Err(Error::Encrypted),
        }
    }
    checkpoint(control)?;

    let extracted = pdfoxide::extract(&pdf, options, &mut |page, total| {
        progress(Progress { stage: Stage::Extracting, page, total });
    })?;
    if extracted.is_scanned() {
        return Err(Error::NoTextLayer);
    }
    checkpoint(control)?;

    let total = extracted.pages.len();
    progress(Progress { stage: Stage::Analysing, page: 0, total });
    let mut layouts: Vec<PageLayout> = Vec::with_capacity(total);
    for (index, page) in extracted.pages.iter().enumerate() {
        checkpoint(control)?;
        layouts.push(PageLayout::analyse(page));
        progress(Progress { stage: Stage::Analysing, page: index + 1, total });
    }

    let profile = router::detect(&layouts, &extracted.info, options.mode);
    if profile.class == DocumentClass::Scanned {
        return Err(Error::NoTextLayer);
    }

    // Running headers/footers must go before paragraphs are grouped.
    if options.clean.remove_headers_footers {
        let furniture = clean::furniture::detect(&layouts, &options.clean);
        clean::furniture::strip(&mut layouts, &furniture);
    }
    checkpoint(control)?;

    progress(Progress { stage: Stage::Parsing, page: 0, total });
    let mut assets = AssetWriter::new(&options.assets)?;
    let (mut document, report) = parse::build(&layouts, &profile, &extracted.info, options, &mut assets)?;
    checkpoint(control)?;

    progress(Progress { stage: Stage::Cleaning, page: 0, total });
    let clean_report = clean::document(&mut document, &options.clean);

    document.metadata.page_count = document.pages.len();
    progress(Progress { stage: Stage::Rendering, page: 0, total });
    let markdown = render::render(&document, &options.markdown);
    progress(Progress { stage: Stage::Done, page: total, total });

    Ok(Conversion {
        document,
        markdown,
        profile,
        report,
        clean: clean_report,
        assets: assets.take_written(),
    })
}

fn checkpoint(control: &dyn Control) -> Result<()> {
    if control.cancelled() {
        Err(Error::Cancelled { job: control.job().to_string() })
    } else {
        Ok(())
    }
}

#[cfg(feature = "grobid")]
fn grobid_document(bytes: &[u8], options: &Options, control: &dyn Control) -> Result<Document> {
    checkpoint(control)?;
    crate::extract::grobid::convert(bytes, options)
}

#[cfg(not(feature = "grobid"))]
fn grobid_document(_bytes: &[u8], _options: &Options, _control: &dyn Control) -> Result<Document> {
    Err(Error::Backend(
        "this build has no GROBID support (rebuild with `--features grobid`)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;

    /// Build a one-page PDF with two lines of Helvetica text.
    fn sample_pdf(lines: &[(&str, f32)]) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document as Pdf, Object, Stream};

        let mut pdf = Pdf::with_version("1.5");
        let pages_id = pdf.new_object_id();
        let font_id = pdf.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });

        let mut operations = vec![Operation::new("BT", vec![])];
        for (index, (text, _size)) in lines.iter().enumerate() {
            operations.push(Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 11.0.into()]));
            if index == 0 {
                operations.push(Operation::new("Td", vec![72.0.into(), 760.0.into()]));
            } else {
                // `Td` is relative to the line matrix: return to the left margin
                // and drop one line.
                operations.push(Operation::new("Td", vec![0.0.into(), (-16.0).into()]));
            }
            operations.push(Operation::new("Tj", vec![Object::string_literal(text.to_string())]));
        }
        operations.push(Operation::new("ET", vec![]));

        let content = Content { operations };
        let content_id = pdf.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).expect("write PDF");
        bytes
    }

    #[test]
    fn a_simple_pdf_becomes_markdown() {
        let bytes = sample_pdf(&[
            ("A Short Report", 760.0),
            ("This is the first paragraph of the report.", 700.0),
        ]);
        let conversion = convert_bytes(&bytes, &Options::plain_text(), &mut |_| {}).unwrap();
        assert!(conversion.markdown.contains("first paragraph"));
        assert!(conversion.document.pages.len() == 1);
    }

    #[test]
    fn a_cancelled_conversion_stops_before_parsing() {
        struct Always;
        impl Control for Always {
            fn cancelled(&self) -> bool {
                true
            }
            fn job(&self) -> &str {
                "test-job"
            }
        }
        let bytes = sample_pdf(&[("Hello", 700.0)]);
        let error = convert_bytes_with(&bytes, &Options::plain_text(), &Always, &mut |_| {}).unwrap_err();
        assert!(matches!(error, Error::Cancelled { ref job } if job == "test-job"));
    }
}
