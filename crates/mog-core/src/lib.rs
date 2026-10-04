//! The editing model of mog.
//!
//! This crate knows nothing about terminals. It holds documents, selections and edits, and turns
//! [`Command`]s into changes through the [`Editor`].

pub mod clipboard;
pub mod command;
pub mod diagnostic;
pub mod document;
pub mod editor;
pub mod file_tree;
pub mod fuzzy;
pub mod history;
pub mod keymap;
pub mod movement;
pub mod range;
pub mod transaction;
pub mod view;

pub use clipboard::{Clipboard, MemoryClipboard};
pub use command::{Command, Motion};
pub use diagnostic::{Diagnostic, Severity};
pub use document::{Document, LineEnding};
pub use editor::{Editor, Options, Outcome};
pub use file_tree::{Entry, FileTree};
pub use fuzzy::{Match, fuzzy_match};
pub use history::History;
pub use keymap::{Key, KeyChord, Keymap, Modifiers};
pub use range::Range;
pub use transaction::{Change, Transaction};
pub use view::View;
