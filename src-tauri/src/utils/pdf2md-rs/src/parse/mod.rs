//! Parsing: page layout → IR.
//!
//! The modules here are pure (no I/O except [`blocks::build`] handing images to
//! the [`crate::assets::AssetWriter`]), which makes them easy to test with
//! synthetic [`crate::layout::PageLayout`] values — no PDF fixture needed for the
//! heading, table and list rules.
//!
//! * [`headings`] — infer levels from numbering, structural keywords and
//!   typography, with a strict no-level-skip guarantee.
//! * [`tables`] — ruled grids and aligned text.
//! * [`blocks`] — the assembly line that turns one page into `Block`s.

pub mod blocks;
pub mod headings;
pub mod tables;

pub use blocks::{build, BuildReport};
pub use headings::HeadingDetector;
pub use tables::TableCandidate;
