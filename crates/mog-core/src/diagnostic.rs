//! Problems reported about a document, usually by a language server.

/// How serious a [`Diagnostic`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Something that is definitely wrong.
    Error,
    /// Something that is probably wrong.
    Warning,
    /// Something worth knowing.
    Info,
    /// A suggestion.
    Hint,
}

/// A problem in a span of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The char offset where the span starts.
    pub from: usize,
    /// The char offset where the span ends.
    pub to: usize,
    /// How serious the problem is.
    pub severity: Severity,
    /// What is wrong.
    pub message: String,
}
