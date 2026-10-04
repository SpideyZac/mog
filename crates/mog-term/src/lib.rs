//! The built in terminal of mog.
//!
//! A [`Shell`] runs in a pseudo terminal and [`TerminalPanel`] draws it under the editor.

pub mod keys;
pub mod panel;
pub mod shell;

pub use panel::TerminalPanel;
pub use shell::Shell;
