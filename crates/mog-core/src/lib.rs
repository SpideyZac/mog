//! The editing model of mog.
//!
//! This crate knows nothing about terminals. It holds documents, selections and edits, and turns
//! commands into changes.

pub mod clipboard;
pub mod command;
pub mod document;
pub mod history;
pub mod keymap;
pub mod movement;
pub mod range;
pub mod transaction;

pub use clipboard::{Clipboard, MemoryClipboard};
pub use command::{Command, Motion};
pub use document::{Document, LineEnding};
pub use history::History;
pub use keymap::{Key, KeyChord, Keymap, Modifiers};
pub use range::Range;
pub use transaction::{Change, Transaction};
