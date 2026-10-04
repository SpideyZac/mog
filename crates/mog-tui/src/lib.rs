//! The terminal user interface of mog.
//!
//! Everything that draws to the screen or reads terminal input lives here.

pub mod compositor;
pub mod input;
pub mod theme;

pub use compositor::{Compositor, Context, EventResult, Layer};
pub use theme::Theme;
