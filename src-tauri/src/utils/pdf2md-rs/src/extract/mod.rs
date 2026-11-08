//! The extraction layer: PDF bytes in, positioned text out.
//!
//! Two backends exist behind this module boundary:
//!
//! * [`pdfoxide`] — the built-in pure-Rust reader ([`crate::extract::pdfoxide`]);
//! * [`grobid`] — an optional HTTP client for a GROBID server (`feature =
//!   "grobid"`), used for academic papers when the built-in layout analysis is
//!   not good enough.
//!
//! Both are driven by [`crate::pipeline`]; the layout analysis and the IR never
//! learn which one produced the page.

pub mod pdfoxide;

#[cfg(feature = "grobid")]
pub mod grobid;

pub use pdfoxide::{
    ExtractedDoc, ExtractedPage, Glyph, ImageData, ImageFormat, ImagePlacement, Line, PdfInfo,
    Rule, Word,
};
