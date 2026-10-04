//! Open files and their text.

use std::{
    fs::File,
    io::{self, BufWriter, ErrorKind},
    path::{self, Path, PathBuf},
};

use ropey::Rope;

use crate::{
    diagnostic::Diagnostic,
    history::{History, Step},
    range::Range,
    transaction::Transaction,
};

/// The name shown for a document that has no path yet.
const SCRATCH_NAME: &str = "[scratch]";

/// A line ending style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// Unix style `\n`.
    #[default]
    Lf,
    /// Windows style `\r\n`.
    CrLf,
}

impl LineEnding {
    /// Guesses the line ending of `text` from its first line break.
    pub fn detect(text: &Rope) -> Self {
        let mut prev = None;
        for ch in text.chars() {
            if ch == '\n' {
                return if prev == Some('\r') {
                    Self::CrLf
                } else {
                    Self::Lf
                };
            }
            prev = Some(ch);
        }
        Self::default()
    }

    /// Returns the line ending as a string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

/// A text buffer, usually backed by a file.
#[derive(Debug, Default)]
pub struct Document {
    /// The full text.
    text: Rope,
    /// The file the document is saved to, if any.
    path: Option<PathBuf>,
    /// The current selection.
    selection: Range,
    /// The undo and redo stacks.
    history: History,
    /// The line ending inserted on new lines.
    line_ending: LineEnding,
    /// A counter bumped on every change, used to track unsaved edits.
    version: u64,
    /// The value of `version` when the document was last saved or loaded.
    saved_version: u64,
    /// Problems reported about the text.
    diagnostics: Vec<Diagnostic>,
}

impl Document {
    /// Creates an empty document with no path.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a document holding `text` with no path.
    pub fn from_text(text: &str) -> Self {
        let text = Rope::from_str(text);
        Self {
            line_ending: LineEnding::detect(&text),
            text,
            ..Self::default()
        }
    }

    /// Opens the file at `path`, or an empty document that will be saved there if it does not
    /// exist yet.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        // language servers and plugins want absolute paths so resolve it once here
        let path = path::absolute(&path).unwrap_or(path);
        let text = match File::open(&path) {
            Ok(file) => Rope::from_reader(file)?,
            Err(err) if err.kind() == ErrorKind::NotFound => Rope::new(),
            Err(err) => return Err(err),
        };
        Ok(Self {
            line_ending: LineEnding::detect(&text),
            text,
            path: Some(path),
            ..Self::default()
        })
    }

    /// Writes the document to its path.
    ///
    /// # Errors
    ///
    /// Returns an error if the document has no path or the file cannot be written.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "document has no path"))?;
        self.text.write_to(BufWriter::new(File::create(path)?))?;
        self.saved_version = self.version;
        Ok(())
    }

    /// Sets the path the document saves to.
    pub fn set_path(&mut self, path: impl Into<PathBuf>) {
        self.path = Some(path.into());
    }

    /// Returns the text.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Returns the path the document saves to, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Returns a short name for display, the file name or a placeholder.
    pub fn name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(|| SCRATCH_NAME.into(), |name| name.to_string_lossy().into())
    }

    /// Returns the current selection.
    pub fn selection(&self) -> Range {
        self.selection
    }

    /// Replaces the selection, clamping it to the text.
    pub fn set_selection(&mut self, range: Range) {
        let max = self.text.len_chars();
        self.selection = Range::new(range.anchor.min(max), range.head.min(max));
    }

    /// Returns the line ending used for new lines.
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Returns `true` if there are changes that have not been saved.
    pub fn is_modified(&self) -> bool {
        self.version != self.saved_version
    }

    /// Returns a counter that changes whenever the text does.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Returns the current diagnostics, sorted by position.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Replaces the diagnostics.
    pub fn set_diagnostics(&mut self, mut diagnostics: Vec<Diagnostic>) {
        diagnostics.sort_by_key(|diagnostic| (diagnostic.from, diagnostic.to));
        self.diagnostics = diagnostics;
    }

    /// Applies `tx`, records it for undo and moves the selection to `after`.
    ///
    /// With `merge` set the edit joins the previous undo step.
    ///
    /// # Panics
    ///
    /// Panics if `tx` is out of bounds for the text.
    pub fn apply(&mut self, tx: Transaction, after: Range, merge: bool) {
        if tx.is_empty() {
            self.set_selection(after);
            return;
        }
        let before = self.selection;
        let inverse = tx.apply(&mut self.text);
        self.history.record(
            Step {
                forward: tx,
                inverse,
            },
            before,
            after,
            merge,
        );
        self.version += 1;
        self.set_selection(after);
    }

    /// Reverts the newest undo step. Returns `false` if there was nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(revision) = self.history.undo() else {
            return false;
        };
        for step in revision.steps.iter().rev() {
            step.inverse.apply(&mut self.text);
        }
        let before = revision.before;
        self.version += 1;
        self.set_selection(before);
        true
    }

    /// Reapplies the newest undone step. Returns `false` if there was nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(revision) = self.history.redo() else {
            return false;
        };
        for step in &revision.steps {
            step.forward.apply(&mut self.text);
        }
        let after = revision.after;
        self.version += 1;
        self.set_selection(after);
        true
    }
}

#[cfg(test)]
/// Tests for [`Document`].
mod tests {
    use std::{env, fs};

    use ropey::Rope;

    use super::{Document, LineEnding};
    use crate::{range::Range, transaction::Transaction};

    /// Undo and redo move both the text and the selection.
    #[test]
    fn undo_and_redo_restore_text_and_selection() {
        let mut doc = Document::from_text("ac");
        doc.set_selection(Range::point(1));
        doc.apply(Transaction::insert(1, "b"), Range::point(2), false);
        assert_eq!(doc.text().to_string(), "abc");
        assert!(doc.undo());
        assert_eq!(doc.text().to_string(), "ac");
        assert_eq!(doc.selection(), Range::point(1));
        assert!(doc.redo());
        assert_eq!(doc.text().to_string(), "abc");
        assert_eq!(doc.selection(), Range::point(2));
    }

    /// Edits mark the document modified and saving clears it.
    #[test]
    fn save_round_trips_and_clears_modified() {
        let path = env::temp_dir().join("mog-core-save-test.txt");
        let mut doc = Document::open(&path).expect("open missing file");
        doc.apply(Transaction::insert(0, "mog"), Range::point(3), false);
        assert!(doc.is_modified());
        doc.save().expect("save");
        assert!(!doc.is_modified());
        assert_eq!(fs::read_to_string(&path).expect("read back"), "mog");
        fs::remove_file(path).expect("clean up");
    }

    /// Line endings are detected from the first line break.
    #[test]
    fn detects_line_endings() {
        assert_eq!(
            LineEnding::detect(&Rope::from_str("a\r\nb")),
            LineEnding::CrLf
        );
        assert_eq!(LineEnding::detect(&Rope::from_str("a\nb")), LineEnding::Lf);
        assert_eq!(LineEnding::detect(&Rope::from_str("a")), LineEnding::Lf);
    }
}
