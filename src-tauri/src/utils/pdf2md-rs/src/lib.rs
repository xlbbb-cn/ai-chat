//! # pdf2md-rs
//!
//! A PDF → Markdown **library** that is built as a *document* pipeline rather
//! than a text extractor: a host application decides what to read, where to
//! write and when to stop, and this crate turns bytes into an intermediate
//! representation ([`ir::Document`]) and Markdown. Every backend produces that
//! IR and every renderer consumes it, so the Markdown writer can be swapped for
//! an HTML/DOCX writer (or an editor view) without touching the parsing side.
//!
//! ```text
//! bytes ──▶ extract ──▶ layout ──▶ router ──▶ parse ──▶ clean ──▶ render ──▶ Markdown
//!           (PDFOxide)  (columns,   (plain /   (IR)      (headers,           + assets
//!                        paragraphs) paper /              paragraphs)
//!                                    scan)
//! ```
//!
//! ## Quick start
//!
//! ```no_run
//! use pdf2md_rs::{Options, pipeline};
//!
//! let options = Options::paper().with_assets("out/assets");
//! let conversion = pipeline::convert_to_path("paper.pdf", "paper.md", &options, &mut |_| {})?;
//! println!("{} blocks, {} images", conversion.document.content_block_count(), conversion.assets.len());
//! # Ok::<(), pdf2md_rs::Error>(())
//! ```
//!
//! ## What is where
//!
//! | module | responsibility |
//! |---|---|
//! | [`extract`] | PDF bytes → positioned glyphs, rules, images (PDFOxide, optional GROBID) |
//! | [`layout`] | glyphs → columns, paragraphs, reading order |
//! | [`router`] | decide plain-text / paper / scanned, with the evidence |
//! | [`parse`] | page layout → IR blocks (headings, tables, formulas, captions) |
//! | [`clean`] | running headers/footers, hyphenation, cross-page paragraphs |
//! | [`render`] | IR → Markdown, plus the [`render::Renderer`] trait |
//! | [`assets`] | image naming, deduplication, writing |
//! | [`pipeline`] | the orchestrator that runs every stage in order |
//! | [`jobs`] | background job manager (submit / progress / cancel) |
//! | [`batch`] | directories → one Markdown file per PDF, optionally parallel |
//!
//! ## Extending it
//!
//! The seams are traits, not configuration flags:
//!
//! | trait | lets you |
//! |---|---|
//! | [`formula::FormulaRecognizer`] | read image-only formulas with an OCR model |
//! | [`pipeline::Control`] | cancel a conversion between pages |
//! | [`render`]'s [`render::Renderer`] | write HTML/DOCX/JSON from the same IR |
//!
//! New *block kinds* go in [`ir::Block`]; every renderer has to handle them, and
//! `#[non_exhaustive]`-style guard rails are the compiler's job — this crate
//! keeps the enum closed on purpose so a new kind is a compile error in every
//! backend instead of a silently dropped block.
//!
//! ## Honest limits
//!
//! * A PDF has no structure, only glyphs. Headings, tables and formulas are
//!   *inferred*; the heuristics are conservative on purpose — a missed heading
//!   costs a `#`, a wrong one destroys the outline.
//! * Formulas are transcribed from the text layer. Image-only formulas need a
//!   [`formula::FormulaRecognizer`] plugin (OCR); without one they stay images.
//! * Scanned PDFs are refused with [`Error::NoTextLayer`] rather than silently
//!   producing an empty document.
//!
//! ## Features
//!
//! * `grobid` — optional GROBID HTTP backend for academic papers.
//! * `parallel` — convert a batch of files on a rayon worker pool.

#![deny(rustdoc::broken_intra_doc_links)]
#![warn(missing_debug_implementations)]

pub mod assets;
pub mod batch;
pub mod clean;
pub mod error;
pub mod extract;
pub mod formula;
pub mod geom;
pub mod ir;
pub mod jobs;
pub mod layout;
pub mod options;
pub mod parse;
pub mod pipeline;
pub mod render;
pub mod router;

pub use assets::{AssetOutput, AssetWriter};
pub use batch::{BatchOptions, FileReport};
pub use error::{Error, Result};
pub use ir::{Block, Document, Metadata, Page};
pub use options::{
    Assets, Clean, FormulaOptions, FormulaStyle, GrobidConfig, LayoutBackend, Markdown, Options,
    PageBreaks, ParseMode, TableStyle,
};
pub use pipeline::{Conversion, Progress, Stage};
pub use router::{DocumentClass, PdfProfile};

/// The crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Human-readable one-line summary of the pipeline, useful in logs and UIs.
pub fn about() -> String {
    format!("pdf2md-rs {VERSION} — PDF to Markdown (plain text and academic papers)")
}
