//! Error type shared by the whole pipeline.

use std::path::PathBuf;

/// Everything that can go wrong while turning a PDF into Markdown.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("i/o error: {0}")]
    PlainIo(#[from] std::io::Error),

    #[error("cannot read PDF structure: {0}")]
    Pdf(#[from] lopdf::Error),

    #[error("the PDF is encrypted and no password was given")]
    Encrypted,

    #[error("the PDF has no pages")]
    NoPages,

    #[error("no text layer found on any page (scanned PDF — OCR is not built in)")]
    NoTextLayer,

    #[error("unsupported colour space for image extraction: {0}")]
    UnsupportedImage(String),

    #[error("could not encode extracted image: {0}")]
    ImageEncode(String),

    #[error("asset output failed: {0}")]
    Asset(String),

    #[error("layout backend unavailable: {0}")]
    Backend(String),

    #[error("job {job} was cancelled")]
    Cancelled { job: String },

    #[error("job {0} not found")]
    UnknownJob(String),

    #[error("{0}")]
    Other(String),
}

/// Convenient alias for the crate's fallible APIs.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Wrap an i/o error with the path it happened on.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }

    /// `true` when the failure is the user's (bad file) rather than ours.
    pub fn is_input_error(&self) -> bool {
        matches!(self, Error::Encrypted | Error::NoPages | Error::NoTextLayer)
    }
}
