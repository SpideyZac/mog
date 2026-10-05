//! Open files and their text.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    io::{self, BufWriter, ErrorKind},
    path::{self, Path, PathBuf},
};

use ropey::Rope;

use crate::{
    diagnostic::Diagnostic,
    history::{History, Step},
    marks::{InlayHint, LineMarks, SemanticToken},
    range::Range,
    transaction::Transaction,
};

/// The name shown for a document that has no path yet.
const SCRATCH_NAME: &str = "[scratch]";

/// Documents bigger than this many bytes skip the expensive extras like syntax colors, git and
/// language servers, so they stay quick to edit.
pub const LARGE_FILE: usize = 8 << 20;

/// How many recent edits are kept for [`Document::changes_since`].
const CHANGE_LOG: usize = 128;

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
    /// Problems reported about the text, from every source, sorted by position.
    diagnostics: Vec<Diagnostic>,
    /// Problems by who reported them, the language server being the empty name.
    diagnostic_sources: BTreeMap<String, Vec<Diagnostic>>,
    /// Extra cursors besides the main selection.
    cursors: Vec<Range>,
    /// Notes a language server wants shown after lines.
    inlay_hints: LineMarks<InlayHint>,
    /// What a language server says each run of text is.
    semantic_tokens: LineMarks<SemanticToken>,
    /// Recent edits with the version each one led to, oldest first.
    changes: VecDeque<(u64, Transaction)>,
    /// The lines with a breakpoint, counted from 0, moved along with edits.
    breakpoints: BTreeSet<usize>,
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
        // language servers want absolute paths so resolve it once here
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

    /// Replaces the selection, clamping it to the text, and drops any extra cursors.
    pub fn set_selection(&mut self, range: Range) {
        let max = self.text.len_chars();
        self.selection = Range::new(range.anchor.min(max), range.head.min(max));
        self.cursors.clear();
    }

    /// Returns the extra cursors besides the main selection.
    pub fn cursors(&self) -> &[Range] {
        &self.cursors
    }

    /// Replaces the extra cursors, clamping them and dropping ones on the main cursor.
    pub fn set_cursors(&mut self, cursors: Vec<Range>) {
        let max = self.text.len_chars();
        let main = self.selection.head;
        self.cursors = cursors
            .into_iter()
            .map(|range| Range::new(range.anchor.min(max), range.head.min(max)))
            .filter(|range| range.head != main)
            .collect();
    }

    /// Returns every cursor, the main selection first.
    pub fn all_ranges(&self) -> Vec<Range> {
        let mut ranges = vec![self.selection];
        ranges.extend_from_slice(&self.cursors);
        ranges
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

    /// Returns `true` if the document is big enough that expensive extras are skipped for it.
    pub fn is_large(&self) -> bool {
        self.text.len_bytes() > LARGE_FILE
    }

    /// Returns the edits that took the text from `version` to now, oldest first, or `None` if
    /// they are no longer all known.
    pub fn changes_since(&self, version: u64) -> Option<Vec<&Transaction>> {
        if version == self.version {
            return Some(Vec::new());
        }
        // the oldest kept edit started from the version before it, anything older is lost
        let oldest = self.changes.front()?.0 - 1;
        if version < oldest || version > self.version {
            return None;
        }
        Some(
            self.changes
                .iter()
                .filter(|(after, _)| *after > version)
                .map(|(_, tx)| tx)
                .collect(),
        )
    }

    /// Returns the lines with a breakpoint, counted from 0.
    pub fn breakpoints(&self) -> &BTreeSet<usize> {
        &self.breakpoints
    }

    /// Replaces the breakpoints, dropping lines past the end.
    pub fn set_breakpoints(&mut self, lines: impl IntoIterator<Item = usize>) {
        let count = self.text.len_lines();
        self.breakpoints = lines.into_iter().filter(|line| *line < count).collect();
    }

    /// Applies `tx` to the text, moving breakpoints along so they stay on the same code.
    fn edit_text(&mut self, tx: &Transaction) -> Transaction {
        // a breakpoint sticks to the start of its line, which edits move like any position
        let starts: Vec<usize> = self
            .breakpoints
            .iter()
            .map(|line| self.text.line_to_char(*line))
            .collect();
        let inverse = tx.apply(&mut self.text);
        self.breakpoints = starts
            .into_iter()
            .map(|start| {
                self.text
                    .char_to_line(tx.map_pos(start).min(self.text.len_chars()))
            })
            .collect();
        inverse
    }

    /// Remembers that `tx` led to the current version.
    fn log_change(&mut self, tx: Transaction) {
        if self.changes.len() == CHANGE_LOG {
            let dropped = self.changes.pop_front().map(|(after, _)| after);
            // an undo logs several edits under one version, drop them together
            while self.changes.front().map(|(after, _)| *after) == dropped {
                self.changes.pop_front();
            }
        }
        self.changes.push_back((self.version, tx));
    }

    /// Returns the current diagnostics, sorted by position.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Replaces the diagnostics from the language server.
    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.set_source_diagnostics("", diagnostics);
    }

    /// Replaces the diagnostics reported by `source`, like a plugin, keeping everyone else's.
    pub fn set_source_diagnostics(&mut self, source: &str, diagnostics: Vec<Diagnostic>) {
        if diagnostics.is_empty() {
            self.diagnostic_sources.remove(source);
        } else {
            self.diagnostic_sources
                .insert(source.to_owned(), diagnostics);
        }
        let mut merged: Vec<Diagnostic> = self
            .diagnostic_sources
            .values()
            .flatten()
            .cloned()
            .collect();
        merged.sort_by_key(|diagnostic| (diagnostic.from, diagnostic.to));
        self.diagnostics = merged;
    }

    /// Returns the inlay hints of `line`, or none if it changed since they were worked out.
    pub fn inlay_hints(&self, line: usize) -> &[InlayHint] {
        self.inlay_hints.on_line(&self.text, line)
    }

    /// Replaces the inlay hints, given as `(line, hint)` for the current text.
    pub fn set_inlay_hints(&mut self, hints: Vec<(usize, InlayHint)>) {
        self.inlay_hints = LineMarks::new(&self.text, hints);
    }

    /// Returns the semantic tokens of `line`, or none if it changed since they were worked out.
    pub fn semantic_tokens(&self, line: usize) -> &[SemanticToken] {
        self.semantic_tokens.on_line(&self.text, line)
    }

    /// Replaces the semantic tokens, given as `(line, token)` for the current text.
    pub fn set_semantic_tokens(&mut self, tokens: Vec<(usize, SemanticToken)>) {
        self.semantic_tokens = LineMarks::new(&self.text, tokens);
    }

    /// Returns the undo and redo history.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Puts back undo and redo history saved for exactly the current text.
    pub fn restore_history(&mut self, history: History) {
        self.history = history;
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
        let inverse = self.edit_text(&tx);
        self.version += 1;
        self.log_change(tx.clone());
        self.history.record(
            Step {
                forward: tx,
                inverse,
            },
            before,
            after,
            merge,
        );
        self.set_selection(after);
    }

    /// Reverts the newest undo step. Returns `false` if there was nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(revision) = self.history.undo() else {
            return false;
        };
        let steps: Vec<Transaction> = revision
            .steps
            .iter()
            .rev()
            .map(|step| step.inverse.clone())
            .collect();
        let before = revision.before;
        self.version += 1;
        for tx in steps {
            self.edit_text(&tx);
            self.log_change(tx);
        }
        self.set_selection(before);
        true
    }

    /// Reapplies the newest undone step. Returns `false` if there was nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(revision) = self.history.redo() else {
            return false;
        };
        let steps: Vec<Transaction> = revision
            .steps
            .iter()
            .map(|step| step.forward.clone())
            .collect();
        let after = revision.after;
        self.version += 1;
        for tx in steps {
            self.edit_text(&tx);
            self.log_change(tx);
        }
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

    /// The change log replays edits since an old version, undo and redo included.
    #[test]
    fn logs_changes_since_a_version() {
        let mut doc = Document::from_text("ac");
        doc.apply(Transaction::insert(1, "b"), Range::point(2), false);
        let after_b = doc.version();
        doc.apply(Transaction::insert(3, "d"), Range::point(4), false);
        assert!(doc.undo());
        let changes = doc.changes_since(after_b).expect("known");
        assert_eq!(changes.len(), 2);
        let mut pos = 3;
        for tx in &changes {
            pos = tx.map_pos(pos);
        }
        assert_eq!(pos, 3);
        assert_eq!(doc.changes_since(doc.version()).map(|c| c.len()), Some(0));
        assert!(doc.changes_since(doc.version() + 5).is_none());
    }

    /// Breakpoints follow their line through edits above them, undo included.
    #[test]
    fn breakpoints_move_with_edits() {
        let mut doc = Document::from_text("a\nb\nc\n");
        doc.set_breakpoints([1, 2, 9]);
        assert_eq!(
            doc.breakpoints().iter().copied().collect::<Vec<_>>(),
            [1, 2]
        );
        doc.apply(Transaction::insert(0, "new\n"), Range::point(4), false);
        assert_eq!(
            doc.breakpoints().iter().copied().collect::<Vec<_>>(),
            [2, 3]
        );
        // deleting the line with a breakpoint leaves it on the line that took its place
        doc.apply(Transaction::delete(6, 8), Range::point(6), false);
        assert_eq!(doc.breakpoints().iter().copied().collect::<Vec<_>>(), [2]);
        // undo puts the line back, the breakpoint stays with the code it ended up on
        assert!(doc.undo());
        assert_eq!(doc.breakpoints().iter().copied().collect::<Vec<_>>(), [3]);
        assert!(doc.undo());
        assert_eq!(doc.breakpoints().iter().copied().collect::<Vec<_>>(), [2]);
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
