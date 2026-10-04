//! The editing model of mog.
//!
//! This crate knows nothing about terminals. It holds documents, selections and edits, and turns
//! commands into changes.

pub mod document;
pub mod history;
pub mod range;
pub mod transaction;

pub use document::{Document, LineEnding};
pub use history::History;
pub use range::Range;
pub use transaction::{Change, Transaction};
