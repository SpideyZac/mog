//! The silly and cool extras drawn around the mog editor.
//!
//! A [`Flair`] is a small widget, like an animated badge or a critter that walks over the text.
//! Flairs never get input and never change the document, they just look good.

pub mod flair;

pub use flair::{Corner, Flair, FlairContext, Placement};
