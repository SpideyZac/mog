//! The editing model of mog.
//!
//! This crate knows nothing about terminals. It holds documents, selections and edits, and turns
//! commands into changes.

pub mod range;

pub use range::Range;
