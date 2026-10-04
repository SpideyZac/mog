//! The editor state and command execution.

use std::{io, mem, path::PathBuf};

use ropey::Rope;

use crate::{
    clipboard::Clipboard,
    command::{Command, Motion},
    document::Document,
    movement,
    range::Range,
    transaction::{Change, Transaction},
    view::{self, View},
};

/// Settings that change how editing commands behave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The width of a tab stop in cells.
    pub tab_width: usize,
    /// Whether the tab key inserts spaces instead of a tab char.
    pub insert_spaces: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
        }
    }
}

/// What the caller should do after [`Editor::execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The command was handled.
    Done,
    /// The editor should exit.
    Quit,
    /// The command is not something the core handles, like opening the palette or asking the
    /// AI.
    Unhandled(Command),
}

/// The whole editing state: open documents, the view and the clipboard.
pub struct Editor {
    /// The open documents. Never empty.
    documents: Vec<Document>,
    /// The index of the focused document.
    active: usize,
    /// The view of each open document, in the same order as `documents`.
    views: Vec<View>,
    /// Editing settings.
    options: Options,
    /// Where copied text goes.
    clipboard: Box<dyn Clipboard>,
    /// A short message for the status line.
    status: Option<String>,
    /// Where the last typed char ended, so following chars join its undo step.
    typing_at: Option<usize>,
    /// Set after a quit was refused due to unsaved changes, so a second quit goes through.
    quit_armed: bool,
    /// Set after closing a document was refused due to unsaved changes.
    close_armed: bool,
}

impl Editor {
    /// Creates an editor with one empty document.
    pub fn new(clipboard: Box<dyn Clipboard>) -> Self {
        Self {
            documents: vec![Document::new()],
            active: 0,
            views: vec![View::default()],
            options: Options::default(),
            clipboard,
            status: None,
            typing_at: None,
            quit_armed: false,
            close_armed: false,
        }
    }

    /// Opens the file at `path` and focuses it.
    ///
    /// An untouched empty scratch document is replaced instead of kept around, and a file that is
    /// already open is focused instead of opened twice.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> io::Result<()> {
        let document = Document::open(path)?;
        let current = self.document();
        if let Some(index) = self
            .documents
            .iter()
            .position(|open| open.path().is_some() && open.path() == document.path())
        {
            self.active = index;
            return Ok(());
        }
        let view = View {
            width: self.view().width,
            height: self.view().height,
            ..View::default()
        };
        if current.path().is_none() && !current.is_modified() && current.text().len_chars() == 0 {
            self.documents[self.active] = document;
            self.views[self.active] = view;
        } else {
            self.documents.push(document);
            self.views.push(view);
            self.active = self.documents.len() - 1;
        }
        Ok(())
    }

    /// Returns the index of the focused document.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Focuses the document at `index`. Out of range indexes are ignored.
    pub fn focus(&mut self, index: usize) {
        if index < self.documents.len() {
            self.active = index;
            self.typing_at = None;
        }
    }

    /// Closes the document at `index`, leaving an empty scratch document if it was the last.
    ///
    /// Unsaved changes are thrown away, so callers should ask first.
    pub fn close(&mut self, index: usize) {
        if index >= self.documents.len() {
            return;
        }
        if self.documents.len() == 1 {
            let view = View {
                width: self.view().width,
                height: self.view().height,
                ..View::default()
            };
            self.documents[0] = Document::new();
            self.views[0] = view;
            return;
        }
        self.documents.remove(index);
        self.views.remove(index);
        if self.active > index || self.active == self.documents.len() {
            self.active -= 1;
        }
        self.typing_at = None;
    }

    /// Returns the focused document.
    pub fn document(&self) -> &Document {
        &self.documents[self.active]
    }

    /// Returns the focused document mutably.
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.documents[self.active]
    }

    /// Returns every open document.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// Returns every open document mutably.
    pub fn documents_mut(&mut self) -> &mut [Document] {
        &mut self.documents
    }

    /// Returns the view of the focused document.
    pub fn view(&self) -> &View {
        &self.views[self.active]
    }

    /// Returns the view of the focused document mutably.
    pub fn view_mut(&mut self) -> &mut View {
        &mut self.views[self.active]
    }

    /// Returns the editing settings.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Replaces the editing settings.
    pub fn set_options(&mut self, options: Options) {
        self.options = options;
    }

    /// Returns the status message, if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Shows `message` in the status line.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Runs `command` and reports what the caller should do next.
    pub fn execute(&mut self, command: Command) -> Outcome {
        let typing_at = self.typing_at.take();
        if command != Command::Quit {
            self.quit_armed = false;
        }
        let close_armed = mem::take(&mut self.close_armed);
        match command {
            Command::Move { motion, extend } => self.move_cursor(motion, extend),
            Command::InsertChar(ch) => {
                let head = self.document().selection().head;
                let merge = typing_at == Some(head) && !ch.is_whitespace();
                self.replace_selection(&ch.to_string(), merge);
                self.typing_at = Some(self.document().selection().head);
            }
            Command::InsertText(text) => self.insert_text(&text),
            Command::InsertNewline => self.insert_newline(),
            Command::InsertTab => self.insert_tab(),
            Command::DeleteBackward => self.delete_with(movement::left),
            Command::DeleteForward => self.delete_with(movement::right),
            Command::DeleteWordBackward => self.delete_with(movement::word_left),
            Command::SelectAll => {
                let len = self.document().text().len_chars();
                self.document_mut().set_selection(Range::new(0, len));
            }
            Command::Undo => {
                if !self.document_mut().undo() {
                    self.set_status("nothing to undo");
                }
                self.reveal_cursor();
            }
            Command::Redo => {
                if !self.document_mut().redo() {
                    self.set_status("nothing to redo");
                }
                self.reveal_cursor();
            }
            Command::Copy => {
                let (from, to) = self.copy_span();
                let text = self.document().text().slice(from..to).to_string();
                self.clipboard.set(text);
            }
            Command::Cut => {
                let (from, to) = self.copy_span();
                let text = self.document().text().slice(from..to).to_string();
                self.clipboard.set(text);
                self.document_mut()
                    .apply(Transaction::delete(from, to), Range::point(from), false);
                self.reveal_cursor();
            }
            Command::Paste => {
                if let Some(text) = self.clipboard.get() {
                    self.insert_text(&text);
                }
            }
            Command::Save => self.save(),
            Command::Quit => return self.quit(),
            Command::GotoLine(line) => {
                let text = self.document().text();
                let line = line.saturating_sub(1).min(text.len_lines() - 1);
                let pos = text.line_to_char(line);
                self.document_mut().set_selection(Range::point(pos));
                self.views[self.active].preferred_col = None;
                self.reveal_cursor();
            }
            Command::NextTab => self.focus((self.active + 1) % self.documents.len()),
            Command::PrevTab => {
                let count = self.documents.len();
                self.focus((self.active + count - 1) % count);
            }
            Command::CloseTab => {
                if self.document().is_modified() && !close_armed {
                    self.close_armed = true;
                    self.set_status(format!(
                        "{} has unsaved changes, close again to throw them away",
                        self.document().name()
                    ));
                } else {
                    self.close(self.active);
                }
            }
            Command::Scroll(lines) => {
                let text = self.documents[self.active].text();
                self.views[self.active].scroll_by(lines, text);
            }
            unhandled @ (Command::CommandPalette | Command::Custom(_)) => {
                return Outcome::Unhandled(unhandled);
            }
        }
        Outcome::Done
    }

    /// Selects `from..to` with the cursor at `to` and scrolls it into view.
    pub fn select(&mut self, from: usize, to: usize) {
        self.document_mut().set_selection(Range::new(from, to));
        self.views[self.active].preferred_col = None;
        self.typing_at = None;
        self.reveal_cursor();
    }

    /// Replaces every range in `ranges` with `text` as one undo step.
    ///
    /// Ranges must not overlap. The cursor ends after the first replacement.
    pub fn replace_ranges(&mut self, ranges: &[(usize, usize)], text: &str) {
        let Some(&(first, _)) = ranges.first() else {
            return;
        };
        let changes = ranges
            .iter()
            .map(|&(start, end)| Change {
                start,
                end,
                text: text.to_owned(),
            })
            .collect();
        let after = Range::point(first + text.chars().count());
        self.document_mut()
            .apply(Transaction::new(changes), after, false);
        self.typing_at = None;
        self.reveal_cursor();
    }

    /// Places the cursor at a cell of the view, extending the selection if `extend` is set.
    ///
    /// `row` and `col` are relative to the top left of the text area.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) {
        let pos = self.pos_at_cell(row, col);
        let range = self.document().selection().put_head(pos, extend);
        self.document_mut().set_selection(range);
        self.views[self.active].preferred_col = None;
        self.typing_at = None;
    }

    /// Selects the word under a cell of the view.
    pub fn select_word_at(&mut self, row: usize, col: usize) {
        let pos = self.pos_at_cell(row, col);
        let (from, to) = movement::word_at(self.document().text(), pos);
        self.document_mut().set_selection(Range::new(from, to));
    }

    /// Selects the whole line shown on a row of the view.
    pub fn select_line_at(&mut self, row: usize) {
        let text = self.document().text();
        let line = (self.views[self.active].scroll_line + row).min(text.len_lines() - 1);
        let (from, to) = movement::line_span(text, line);
        self.document_mut().set_selection(Range::new(from, to));
    }

    /// Returns the char offset shown at a cell of the view.
    fn pos_at_cell(&self, row: usize, col: usize) -> usize {
        self.view()
            .pos_at_cell(self.document().text(), row, col, self.options.tab_width)
    }

    /// Scrolls the view so the cursor is visible.
    fn reveal_cursor(&mut self) {
        let document = &self.documents[self.active];
        self.views[self.active].ensure_visible(
            document.text(),
            document.selection().head,
            self.options.tab_width,
        );
    }

    /// Applies a cursor motion.
    fn move_cursor(&mut self, motion: Motion, extend: bool) {
        let tab_width = self.options.tab_width;
        let page = isize::try_from(self.views[self.active].height.max(1)).unwrap_or(isize::MAX);
        let document = &self.documents[self.active];
        let text = document.text();
        let range = document.selection();
        let head = range.head;
        let mut preferred_col = None;
        let pos = match motion {
            Motion::Left if !extend && !range.is_empty() => range.from(),
            Motion::Right if !extend && !range.is_empty() => range.to(),
            Motion::Left => movement::left(text, head),
            Motion::Right => movement::right(text, head),
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let count = match motion {
                    Motion::Up => -1,
                    Motion::Down => 1,
                    Motion::PageUp => -page,
                    _ => page,
                };
                let col = self.views[self.active]
                    .preferred_col
                    .unwrap_or_else(|| view::visual_col(text, head, tab_width));
                preferred_col = Some(col);
                let line = text.char_to_line(head);
                let last = text.len_lines() - 1;
                match line.checked_add_signed(count) {
                    None => 0,
                    Some(target) if target > last => text.len_chars(),
                    Some(target) => view::pos_at_visual_col(text, target, col, tab_width),
                }
            }
            Motion::WordLeft => movement::word_left(text, head),
            Motion::WordRight => movement::word_right(text, head),
            Motion::LineStart => movement::smart_home(text, head),
            Motion::LineEnd => movement::line_end(text, head),
            Motion::DocStart => 0,
            Motion::DocEnd => text.len_chars(),
        };
        let range = range.put_head(pos, extend);
        self.document_mut().set_selection(range);
        self.views[self.active].preferred_col = preferred_col;
        self.reveal_cursor();
    }

    /// Replaces the selection with `text` and puts the cursor after it.
    fn replace_selection(&mut self, text: &str, merge: bool) {
        let range = self.document().selection();
        let after = Range::point(range.from() + text.chars().count());
        self.document_mut().apply(
            Transaction::replace(range.from(), range.to(), text),
            after,
            merge,
        );
        self.views[self.active].preferred_col = None;
        self.reveal_cursor();
    }

    /// Inserts `text` with its line breaks converted to the document's line ending.
    fn insert_text(&mut self, text: &str) {
        let ending = self.document().line_ending().as_str();
        let text = text.replace("\r\n", "\n").replace('\n', ending);
        self.replace_selection(&text, false);
    }

    /// Inserts a line break that keeps the indentation of the current line.
    fn insert_newline(&mut self) {
        let document = self.document();
        let text = document.text();
        let from = document.selection().from();
        let start = movement::line_start(text, from);
        let indent: String = text
            .slice(start..from)
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let insert = format!("{}{indent}", document.line_ending().as_str());
        self.replace_selection(&insert, false);
    }

    /// Inserts a tab char, or spaces up to the next tab stop.
    fn insert_tab(&mut self) {
        let insert = if self.options.insert_spaces {
            let document = self.document();
            let tab_width = self.options.tab_width.max(1);
            let col = view::visual_col(document.text(), document.selection().from(), tab_width);
            " ".repeat(tab_width - col % tab_width)
        } else {
            "\t".into()
        };
        self.replace_selection(&insert, false);
    }

    /// Deletes the selection, or the span from the cursor to where `motion` takes it.
    fn delete_with(&mut self, motion: fn(&Rope, usize) -> usize) {
        let document = self.document();
        let range = document.selection();
        let (from, to) = if range.is_empty() {
            let target = motion(document.text(), range.head);
            (target.min(range.head), target.max(range.head))
        } else {
            (range.from(), range.to())
        };
        self.document_mut()
            .apply(Transaction::delete(from, to), Range::point(from), false);
        self.views[self.active].preferred_col = None;
        self.reveal_cursor();
    }

    /// Returns the span copy and cut act on: the selection, or the whole line when nothing is
    /// selected.
    fn copy_span(&self) -> (usize, usize) {
        let document = self.document();
        let range = document.selection();
        if range.is_empty() {
            let text = document.text();
            movement::line_span(text, text.char_to_line(range.head))
        } else {
            (range.from(), range.to())
        }
    }

    /// Saves the focused document and reports the result in the status line.
    fn save(&mut self) {
        let document = self.document_mut();
        let message = match document.save() {
            Ok(()) => format!("saved {}", document.name()),
            Err(err) => format!("could not save {}: {err}", document.name()),
        };
        self.set_status(message);
    }

    /// Quits, unless there are unsaved changes and this is the first attempt.
    fn quit(&mut self) -> Outcome {
        let unsaved = self.documents.iter().filter(|d| d.is_modified()).count();
        if unsaved == 0 || self.quit_armed {
            return Outcome::Quit;
        }
        self.quit_armed = true;
        self.set_status(format!(
            "{unsaved} unsaved document(s), quit again to throw away changes"
        ));
        Outcome::Done
    }
}

#[cfg(test)]
/// Tests for [`Editor`].
mod tests {
    use std::env;

    use super::{Editor, Outcome};
    use crate::{
        clipboard::MemoryClipboard,
        command::{Command, Motion},
        document::Document,
        range::Range,
    };

    /// Creates an editor holding `text` with the cursor at `pos`.
    fn editor_with(text: &str, pos: usize) -> Editor {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        editor.documents[0] = Document::from_text(text);
        editor.document_mut().set_selection(Range::point(pos));
        editor.view_mut().resize(80, 24);
        editor
    }

    /// Returns the focused document text.
    fn text(editor: &Editor) -> String {
        editor.document().text().to_string()
    }

    /// Consecutive typing is undone as one step.
    #[test]
    fn typing_merges_into_one_undo() {
        let mut editor = editor_with("", 0);
        for ch in "mog".chars() {
            editor.execute(Command::InsertChar(ch));
        }
        assert_eq!(text(&editor), "mog");
        editor.execute(Command::Undo);
        assert_eq!(text(&editor), "");
    }

    /// New lines copy the indentation of the current line.
    #[test]
    fn newline_keeps_indent() {
        let mut editor = editor_with("    foo", 7);
        editor.execute(Command::InsertNewline);
        assert_eq!(text(&editor), "    foo\n    ");
    }

    /// Tab inserts spaces to the next stop.
    #[test]
    fn tab_inserts_spaces_to_stop() {
        let mut editor = editor_with("ab", 2);
        editor.execute(Command::InsertTab);
        assert_eq!(text(&editor), "ab  ");
    }

    /// Vertical motion keeps the preferred column across short lines.
    #[test]
    fn vertical_motion_keeps_column() {
        let mut editor = editor_with("abcdef\nab\nabcdef", 5);
        let down = Command::Move {
            motion: Motion::Down,
            extend: false,
        };
        editor.execute(down.clone());
        assert_eq!(editor.document().selection().head, 9);
        editor.execute(down);
        assert_eq!(editor.document().selection().head, 15);
    }

    /// Cutting with no selection cuts the line and paste puts it back.
    #[test]
    fn cut_line_and_paste() {
        let mut editor = editor_with("one\ntwo\n", 5);
        editor.execute(Command::Cut);
        assert_eq!(text(&editor), "one\n");
        editor.execute(Command::Paste);
        assert_eq!(text(&editor), "one\ntwo\n");
    }

    /// Backspace deletes the selection or one char.
    #[test]
    fn delete_backward() {
        let mut editor = editor_with("hello", 5);
        editor.execute(Command::DeleteBackward);
        assert_eq!(text(&editor), "hell");
        editor.document_mut().set_selection(Range::new(0, 2));
        editor.execute(Command::DeleteBackward);
        assert_eq!(text(&editor), "ll");
    }

    /// Quitting with unsaved changes needs a second quit.
    #[test]
    fn quit_with_unsaved_changes_needs_confirmation() {
        let mut editor = editor_with("", 0);
        editor.execute(Command::InsertChar('x'));
        assert_eq!(editor.execute(Command::Quit), Outcome::Done);
        assert_eq!(editor.execute(Command::Quit), Outcome::Quit);
    }

    /// Replacing several ranges is one undo step.
    #[test]
    fn replace_ranges_is_one_step() {
        let mut editor = editor_with("a b a", 0);
        editor.replace_ranges(&[(0, 1), (4, 5)], "xy");
        assert_eq!(text(&editor), "xy b xy");
        editor.execute(Command::Undo);
        assert_eq!(text(&editor), "a b a");
    }

    /// Mouse helpers select words and lines.
    #[test]
    fn mouse_selection() {
        let mut editor = editor_with("let foo = 1;\nbar", 0);
        editor.select_word_at(0, 5);
        assert_eq!(editor.document().selection(), Range::new(4, 7));
        editor.select_line_at(0);
        assert_eq!(editor.document().selection(), Range::new(0, 13));
        editor.click(1, 1, false);
        editor.click(0, 0, true);
        assert_eq!(editor.document().selection(), Range::new(14, 0));
    }

    /// Each document keeps its own scroll position and closing moves focus to a neighbour.
    #[test]
    fn views_follow_documents() {
        let dir = env::temp_dir();
        let mut editor = editor_with("", 0);
        editor.open(dir.join("mog-view-a.txt")).expect("open a");
        editor.view_mut().scroll_line = 7;
        editor.open(dir.join("mog-view-b.txt")).expect("open b");
        assert_eq!(editor.view().scroll_line, 0);
        editor.focus(0);
        assert_eq!(editor.view().scroll_line, 7);
        editor.close(0);
        assert_eq!(editor.document().name(), "mog-view-b.txt");
        editor.close(0);
        assert_eq!(editor.documents().len(), 1);
        assert!(editor.document().path().is_none());
    }

    /// Opening a file that is already open focuses it instead of adding a copy.
    #[test]
    fn open_twice_focuses_existing() {
        let dir = env::temp_dir();
        let mut editor = editor_with("", 0);
        editor.open(dir.join("mog-open-a.txt")).expect("open a");
        editor.open(dir.join("mog-open-b.txt")).expect("open b");
        editor
            .open(dir.join("mog-open-a.txt"))
            .expect("open a again");
        assert_eq!(editor.documents().len(), 2);
        assert_eq!(editor.document().name(), "mog-open-a.txt");
    }
}
