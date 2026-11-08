//! Rendering: IR → output format.
//!
//! The renderer is the only module that knows output syntax. It never looks at
//! coordinates, fonts or PDF objects — if something in the output is wrong, the
//! bug is either in the IR or here, never in the extractor.
//!
//! [`markdown`] is the built-in renderer. Anything else is an implementation of
//! [`Renderer`]: the document pipeline stops at the IR, so an HTML, DOCX or
//! editor-view backend only has to walk [`crate::ir::Block`].
//!
//! ```no_run
//! use pdf2md_rs::{Document, render::{self, Renderer}};
//!
//! struct Skeleton;
//! impl Renderer for Skeleton {
//!     fn extension(&self) -> &'static str { "txt" }
//!     fn render(&self, document: &Document) -> String {
//!         document.blocks.iter().filter_map(|b| b.text()).collect::<Vec<_>>().join("\n")
//!     }
//! }
//!
//! fn to_text(document: &Document) -> String {
//!     render::with(document, &Skeleton)
//! }
//! ```

pub mod markdown;

pub use markdown::render;

use crate::ir::Document;

/// A consumer of the intermediate representation.
///
/// The trait is deliberately minimal: one method, no error type, no I/O. A
/// writer that needs a filesystem handle (images, attachments) receives the
/// paths already resolved by [`crate::assets`], because the IR carries them.
pub trait Renderer {
    /// Extension of the file this renderer produces (`"md"`, `"html"`, …).
    fn extension(&self) -> &'static str;

    /// Produce the document in this format.
    fn render(&self, document: &Document) -> String;
}

/// The built-in Markdown renderer, as a [`Renderer`].
///
/// ```no_run
/// use pdf2md_rs::{Markdown, Document, render::{self, Renderer, MarkdownRenderer}};
/// fn to_markdown(document: &Document, options: &Markdown) -> String {
///     render::with(document, &MarkdownRenderer::new(options))
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct MarkdownRenderer<'a> {
    pub options: &'a crate::options::Markdown,
}

impl<'a> MarkdownRenderer<'a> {
    pub fn new(options: &'a crate::options::Markdown) -> Self {
        MarkdownRenderer { options }
    }
}

impl Renderer for MarkdownRenderer<'_> {
    fn extension(&self) -> &'static str {
        "md"
    }

    fn render(&self, document: &Document) -> String {
        markdown::render(document, self.options)
    }
}

/// Render with any [`Renderer`].
pub fn with(document: &Document, renderer: &dyn Renderer) -> String {
    renderer.render(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Block;

    #[test]
    fn the_markdown_renderer_matches_the_function() {
        let labels = crate::ir::Metadata { title: Some("T".into()), author: vec![], page_count: 1 };
        let mut document = Document::new(labels);
        document.push_page(crate::ir::Page { page_number: 1, width: 595.0, height: 842.0 });
        document.push(Block::Paragraph { text: "hello".into() });
        let options = crate::options::Markdown::default();
        assert_eq!(
            with(&document, &MarkdownRenderer::new(&options)),
            markdown::render(&document, &options)
        );
    }
}
