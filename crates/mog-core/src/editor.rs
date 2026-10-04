//! The editor state and command execution.

use std::{fs, io, mem, path::PathBuf};

use ropey::Rope;

use crate::{
    clipboard::Clipboard,
    command::{Command, Motion},
    cursors,
    document::Document,
    lines::{self, LineEdit},
    movement,
    range::Range,
    search,
    transaction::{Change, Transaction},
    view::{self, View},
};

/// Chars that auto close, as `(open, close)`. Brackets come first.
const PAIRS: [(char, char); 6] = [
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('"', '"'),
    ('\'', '\''),
    ('`', '`'),
];

/// Settings that change how editing commands behave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The width of a tab stop in cells.
    pub tab_width: usize,
    /// Whether the tab key inserts spaces instead of a tab char.
    pub insert_spaces: bool,
    /// Whether typing an opening bracket or quote also types the closing one.
    pub auto_close: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
            auto_close: true,
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

    /// Opens a new empty document and focuses it.
    pub fn new_document(&mut self) {
        let view = View {
            width: self.view().width,
            height: self.view().height,
            ..View::default()
        };
        self.documents.push(Document::new());
        self.views.push(view);
        self.active = self.documents.len() - 1;
    }

    /// Saves the focused document to `path` and keeps saving there from now on.
    ///
    /// Missing folders are created.
    ///
    /// # Errors
    ///
    /// Returns an error if the folders or the file cannot be written.
    pub fn save_as(&mut self, path: impl Into<PathBuf>) -> io::Result<()> {
        let path = path.into();
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let document = self.document_mut();
        document.set_path(path);
        document.save()?;
        let name = document.name();
        self.set_status(format!("saved {name}"));
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

    /// Puts `text` on the clipboard.
    pub fn copy_text(&mut self, text: String) {
        self.clipboard.set(text);
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
        if !self.document().cursors().is_empty() {
            if let Some(outcome) = self.execute_multi(&command) {
                return outcome;
            }
            // everything else works on the main cursor only
            let main = self.document().selection();
            self.document_mut().set_selection(main);
        }
        match command {
            Command::Move { motion, extend } => self.move_cursor(motion, extend),
            Command::SelectNextOccurrence => self.select_next_occurrence(),
            Command::SelectAllOccurrences => self.select_all_occurrences(),
            Command::AddCursorAbove | Command::AddCursorBelow => {
                let up = command == Command::AddCursorAbove;
                self.add_vertical_cursor(up);
            }
            Command::InsertChar(ch) => {
                let head = self.document().selection().head;
                let merge = typing_at == Some(head) && !ch.is_whitespace();
                if !(self.options.auto_close && self.auto_pair(ch)) {
                    self.replace_selection(&ch.to_string(), merge);
                }
                self.typing_at = Some(self.document().selection().head);
            }
            Command::InsertText(text) => self.insert_text(&text),
            Command::InsertNewline => self.insert_newline(),
            Command::InsertTab => {
                let document = self.document();
                let (first, last) = lines::selected_lines(document.text(), document.selection());
                if first == last {
                    self.insert_tab();
                } else {
                    let unit = self.indent_unit();
                    let edit = lines::indent_lines(document.text(), document.selection(), &unit);
                    self.apply_edit(edit);
                }
            }
            Command::Outdent => {
                let document = self.document();
                let edit = lines::outdent_lines(
                    document.text(),
                    document.selection(),
                    self.options.tab_width,
                );
                self.apply_edit(edit);
            }
            Command::ToggleComment => {
                let token = self
                    .document()
                    .path()
                    .and_then(|path| path.extension())
                    .and_then(|ext| lines::comment_token(&ext.to_string_lossy().to_lowercase()));
                match token {
                    Some(token) => {
                        let document = self.document();
                        let edit =
                            lines::toggle_comment(document.text(), document.selection(), token);
                        self.apply_edit(edit);
                    }
                    None => self.set_status("mog does not know how to comment this file"),
                }
            }
            Command::DuplicateLine => {
                let document = self.document();
                let ending = document.line_ending().as_str();
                let edit = lines::duplicate_lines(document.text(), document.selection(), ending);
                self.apply_edit(edit);
            }
            Command::DeleteLine => {
                let document = self.document();
                let edit = lines::delete_lines(document.text(), document.selection());
                self.apply_edit(edit);
            }
            Command::MoveLineUp | Command::MoveLineDown => {
                let document = self.document();
                let up = command == Command::MoveLineUp;
                if let Some(edit) = lines::move_lines(document.text(), document.selection(), up) {
                    self.apply_edit(edit);
                }
            }
            Command::DeleteBackward if self.delete_pair() => {}
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

    /// Applies `changes`, like edits from a formatter, as one undo step keeping the cursor put.
    ///
    /// Overlapping changes are dropped since they cannot be applied together.
    pub fn apply_changes(&mut self, mut changes: Vec<Change>) {
        changes.sort_by_key(|change| change.start);
        let mut kept: Vec<Change> = Vec::with_capacity(changes.len());
        for change in changes {
            if kept.last().is_none_or(|last| last.end <= change.start) {
                kept.push(change);
            }
        }
        if kept.is_empty() {
            return;
        }
        let tx = Transaction::new(kept);
        let selection = self.document().selection();
        let after = Range::new(tx.map_pos(selection.anchor), tx.map_pos(selection.head));
        self.document_mut().apply(tx, after, false);
        self.reveal_cursor();
    }

    /// Applies a planned line edit as one undo step.
    fn apply_edit(&mut self, edit: LineEdit) {
        if edit.tx.is_empty() {
            return;
        }
        self.document_mut().apply(edit.tx, edit.selection, false);
        self.views[self.active].preferred_col = None;
        self.reveal_cursor();
    }

    /// Returns one level of indentation as text.
    fn indent_unit(&self) -> String {
        if self.options.insert_spaces {
            " ".repeat(self.options.tab_width.max(1))
        } else {
            "\t".into()
        }
    }

    /// Handles typing `ch` when it opens or closes a pair. Returns `false` to type it normally.
    fn auto_pair(&mut self, ch: char) -> bool {
        let document = self.document();
        let text = document.text();
        let selection = document.selection();
        let next = text.get_char(selection.head);
        let prev = selection
            .head
            .checked_sub(1)
            .and_then(|at| text.get_char(at));
        let is_quote = matches!(ch, '"' | '\'' | '`');
        if selection.is_empty()
            && next == Some(ch)
            && (is_quote || PAIRS.iter().any(|(_, close)| *close == ch))
        {
            let pos = selection.head + 1;
            self.document_mut().set_selection(Range::point(pos));
            self.reveal_cursor();
            return true;
        }
        let Some(close) = PAIRS
            .iter()
            .find(|(open, _)| *open == ch)
            .map(|(_, close)| *close)
        else {
            return false;
        };
        if !selection.is_empty() {
            let (from, to) = (selection.from(), selection.to());
            let inner = text.slice(from..to).to_string();
            let tx = Transaction::replace(from, to, format!("{ch}{inner}{close}"));
            let after = Range::new(from + 1, to + 1);
            self.document_mut().apply(tx, after, false);
            return true;
        }
        let word_around = prev.is_some_and(|c| c.is_alphanumeric() || c == '_')
            || next.is_some_and(|c| c.is_alphanumeric() || c == '_');
        if is_quote && word_around {
            return false;
        }
        let free_after = next.is_none_or(|c| {
            c.is_whitespace()
                || PAIRS.iter().any(|(_, close)| *close == c)
                || matches!(c, ',' | ';' | ':')
        });
        if !free_after {
            return false;
        }
        let pos = selection.head;
        let tx = Transaction::insert(pos, format!("{ch}{close}"));
        self.document_mut().apply(tx, Range::point(pos + 1), false);
        self.reveal_cursor();
        true
    }

    /// Deletes an empty pair like `()` around the cursor. Returns whether it did.
    fn delete_pair(&mut self) -> bool {
        if !self.options.auto_close {
            return false;
        }
        let document = self.document();
        let selection = document.selection();
        let text = document.text();
        let Some(prev_at) = selection
            .head
            .checked_sub(1)
            .filter(|_| selection.is_empty())
        else {
            return false;
        };
        let (prev, next) = (text.get_char(prev_at), text.get_char(selection.head));
        let is_pair = PAIRS
            .iter()
            .any(|(open, close)| Some(*open) == prev && Some(*close) == next);
        if !is_pair {
            return false;
        }
        let tx = Transaction::delete(prev_at, selection.head + 1);
        self.document_mut().apply(tx, Range::point(prev_at), false);
        self.reveal_cursor();
        true
    }

    /// Moves the cursor to the next diagnostic after it, or the previous one before it, wrapping.
    ///
    /// Returns the message of the diagnostic, or `None` if there are none.
    pub fn goto_problem(&mut self, forward: bool) -> Option<String> {
        let document = self.document();
        let head = document.selection().head;
        let mut starts: Vec<(usize, &str)> = document
            .diagnostics()
            .iter()
            .map(|d| (d.from, d.message.as_str()))
            .collect();
        starts.sort_by_key(|(from, _)| *from);
        let found = if forward {
            starts
                .iter()
                .find(|(from, _)| *from > head)
                .or_else(|| starts.first())
        } else {
            starts
                .iter()
                .rev()
                .find(|(from, _)| *from < head)
                .or_else(|| starts.last())
        };
        let (pos, message) = found.map(|(from, message)| (*from, (*message).to_owned()))?;
        self.select(pos, pos);
        Some(message)
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
        let changes = ranges
            .iter()
            .map(|&(start, end)| Change {
                start,
                end,
                text: text.to_owned(),
            })
            .collect();
        self.replace_with(changes);
    }

    /// Applies `changes`, sorted and not overlapping, as one undo step with the cursor after the
    /// first one.
    pub fn replace_with(&mut self, changes: Vec<Change>) {
        let Some(first) = changes.first() else {
            return;
        };
        let after = Range::point(first.start + first.text.chars().count());
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
        let range = self.document().selection();
        let preferred = self.views[self.active].preferred_col;
        let (pos, preferred_col) = self.motion_target(range, motion, extend, preferred);
        let range = range.put_head(pos, extend);
        self.document_mut().set_selection(range);
        self.views[self.active].preferred_col = preferred_col;
        self.reveal_cursor();
    }

    /// Returns where `motion` takes `range` and the column vertical motion should keep.
    fn motion_target(
        &self,
        range: Range,
        motion: Motion,
        extend: bool,
        preferred: Option<usize>,
    ) -> (usize, Option<usize>) {
        let tab_width = self.options.tab_width;
        let page = isize::try_from(self.views[self.active].height.max(1)).unwrap_or(isize::MAX);
        let text = self.document().text();
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
                let col = preferred.unwrap_or_else(|| view::visual_col(text, head, tab_width));
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
        (pos, preferred_col)
    }

    /// Runs `command` at every cursor if it is something that makes sense there.
    ///
    /// Returns `None` for commands that only act on the main cursor.
    fn execute_multi(&mut self, command: &Command) -> Option<Outcome> {
        let ranges = self.document().all_ranges();
        let text = self.document().text();
        let ending = self.document().line_ending().as_str();
        let replace = |range: Range, insert: String| Change {
            start: range.from(),
            end: range.to(),
            text: insert,
        };
        let changes: Vec<Change> = match command {
            Command::InsertChar(ch) => ranges.iter().map(|&r| replace(r, ch.to_string())).collect(),
            Command::InsertText(insert) => {
                let insert = insert.replace("\r\n", "\n").replace('\n', ending);
                ranges.iter().map(|&r| replace(r, insert.clone())).collect()
            }
            Command::Paste => {
                let pasted = self.clipboard.get()?;
                let lines: Vec<&str> = pasted.lines().collect();
                // one line per cursor when the counts match, like most editors
                if lines.len() == ranges.len() {
                    ranges
                        .iter()
                        .zip(lines)
                        .map(|(&r, line)| replace(r, line.to_owned()))
                        .collect()
                } else {
                    ranges.iter().map(|&r| replace(r, pasted.clone())).collect()
                }
            }
            Command::InsertNewline => ranges
                .iter()
                .map(|&r| {
                    let start = movement::line_start(text, r.from());
                    let indent: String = text
                        .slice(start..r.from())
                        .chars()
                        .take_while(|ch| *ch == ' ' || *ch == '\t')
                        .collect();
                    replace(r, format!("{ending}{indent}"))
                })
                .collect(),
            Command::InsertTab => {
                let unit = self.indent_unit();
                ranges.iter().map(|&r| replace(r, unit.clone())).collect()
            }
            Command::DeleteBackward | Command::DeleteForward | Command::DeleteWordBackward => {
                let motion: fn(&Rope, usize) -> usize = match command {
                    Command::DeleteBackward => movement::left,
                    Command::DeleteForward => movement::right,
                    _ => movement::word_left,
                };
                ranges
                    .iter()
                    .map(|&r| {
                        let (start, end) = if r.is_empty() {
                            let target = motion(text, r.head);
                            (target.min(r.head), target.max(r.head))
                        } else {
                            (r.from(), r.to())
                        };
                        Change {
                            start,
                            end,
                            text: String::new(),
                        }
                    })
                    .collect()
            }
            Command::Copy | Command::Cut => {
                let copied: Vec<String> = ranges
                    .iter()
                    .map(|r| text.slice(r.from()..r.to()).to_string())
                    .collect();
                self.clipboard.set(copied.join("\n"));
                if *command == Command::Copy {
                    return Some(Outcome::Done);
                }
                ranges.iter().map(|&r| replace(r, String::new())).collect()
            }
            Command::Move { motion, extend } => {
                let moved: Vec<Range> = ranges
                    .iter()
                    .map(|&r| {
                        let (pos, _) = self.motion_target(r, *motion, *extend, None);
                        r.put_head(pos, *extend)
                    })
                    .collect();
                self.set_all_ranges(moved);
                return Some(Outcome::Done);
            }
            Command::SelectNextOccurrence => {
                self.select_next_occurrence();
                return Some(Outcome::Done);
            }
            Command::AddCursorAbove | Command::AddCursorBelow => {
                self.add_vertical_cursor(*command == Command::AddCursorAbove);
                return Some(Outcome::Done);
            }
            _ => return None,
        };
        let (tx, after) = cursors::edit_all(changes);
        let main = after.first().copied().unwrap_or_default();
        self.document_mut().apply(tx, main, false);
        self.document_mut()
            .set_cursors(after.into_iter().skip(1).collect());
        self.views[self.active].preferred_col = None;
        self.reveal_cursor();
        Some(Outcome::Done)
    }

    /// Sets every cursor, main first, dropping duplicates.
    fn set_all_ranges(&mut self, ranges: Vec<Range>) {
        let mut ranges = cursors::dedup(ranges).into_iter();
        let Some(main) = ranges.next() else {
            return;
        };
        self.document_mut().set_selection(main);
        self.document_mut().set_cursors(ranges.collect());
        self.reveal_cursor();
    }

    /// Selects the word at the cursor, or adds a cursor on the next occurrence of the selection.
    fn select_next_occurrence(&mut self) {
        let document = self.document();
        let main = document.selection();
        if main.is_empty() {
            let (from, to) = movement::word_at(document.text(), main.head);
            if from < to {
                self.document_mut().set_selection(Range::new(from, to));
            }
            return;
        }
        let mut ranges = document.all_ranges();
        match cursors::next_occurrence(document.text(), &ranges) {
            Some(next) => {
                ranges.push(next);
                self.set_all_ranges(ranges);
            }
            None => self.set_status("no more matches"),
        }
    }

    /// Puts a cursor on every occurrence of the selection, or of the word at the cursor.
    fn select_all_occurrences(&mut self) {
        let document = self.document();
        let main = document.selection();
        let (from, to) = if main.is_empty() {
            movement::word_at(document.text(), main.head)
        } else {
            (main.from(), main.to())
        };
        if from >= to {
            return;
        }
        let needle = document.text().slice(from..to).to_string();
        let mut ranges: Vec<Range> = search::find_all(document.text(), &needle, true)
            .into_iter()
            .map(|(a, b)| Range::new(a, b))
            .collect();
        // keep the one the cursor was on as the main cursor
        if let Some(at) = ranges.iter().position(|range| range.from() == from) {
            ranges.swap(0, at);
        }
        let count = ranges.len();
        self.set_all_ranges(ranges);
        self.set_status(format!("{count} cursors, go wild"));
    }

    /// Adds a cursor on the line above or below the outermost cursor.
    fn add_vertical_cursor(&mut self, up: bool) {
        let document = self.document();
        let mut ranges = document.all_ranges();
        if let Some(added) =
            cursors::add_vertical(document.text(), &ranges, up, self.options.tab_width)
        {
            ranges.push(added);
            self.set_all_ranges(ranges);
        }
    }

    /// Adds a cursor at a cell of the view, or removes one that is already there.
    pub fn toggle_cursor_at(&mut self, row: usize, col: usize) {
        let pos = self.pos_at_cell(row, col);
        let mut ranges = self.document().all_ranges();
        if let Some(at) = ranges.iter().position(|range| range.head == pos) {
            if ranges.len() > 1 {
                ranges.remove(at);
            }
        } else {
            ranges.push(Range::point(pos));
        }
        self.set_all_ranges(ranges);
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
        let ending = document.line_ending().as_str();
        let prev = from.checked_sub(1).and_then(|at| text.get_char(at));
        let next = text.get_char(document.selection().to());
        let between = PAIRS
            .iter()
            .take(3)
            .any(|(open, close)| Some(*open) == prev && Some(*close) == next);
        if between && document.selection().is_empty() {
            let unit = self.indent_unit();
            let inner = format!("{ending}{indent}{unit}");
            let insert = format!("{inner}{ending}{indent}");
            let after = Range::point(from + inner.chars().count());
            self.document_mut()
                .apply(Transaction::insert(from, insert), after, false);
            self.views[self.active].preferred_col = None;
            self.reveal_cursor();
            return;
        }
        let insert = format!("{ending}{indent}");
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
    use std::{env, fs, process};

    use super::{Editor, Outcome};
    use crate::{
        clipboard::MemoryClipboard,
        command::{Command, Motion},
        document::Document,
        range::Range,
        transaction::Change,
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

    /// Formatter style changes apply together and overlapping ones are skipped.
    #[test]
    fn applies_changes() {
        let mut editor = editor_with("a  b", 4);
        editor.apply_changes(vec![
            Change {
                start: 1,
                end: 3,
                text: " ".into(),
            },
            Change {
                start: 2,
                end: 4,
                text: "x".into(),
            },
        ]);
        assert_eq!(text(&editor), "a b");
        assert_eq!(editor.document().selection().head, 3);
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

    /// Brackets close themselves, typing the closer steps over it and backspace removes both.
    #[test]
    fn auto_closes_pairs() {
        let mut editor = editor_with("", 0);
        editor.execute(Command::InsertChar('('));
        assert_eq!(text(&editor), "()");
        editor.execute(Command::InsertChar('x'));
        editor.execute(Command::InsertChar(')'));
        assert_eq!(text(&editor), "(x)");
        assert_eq!(editor.document().selection().head, 3);
        let mut editor = editor_with("", 0);
        editor.execute(Command::InsertChar('['));
        editor.execute(Command::DeleteBackward);
        assert_eq!(text(&editor), "");
        let mut editor = editor_with("don", 3);
        editor.execute(Command::InsertChar('\''));
        assert_eq!(text(&editor), "don'");
    }

    /// Enter between braces opens an indented block.
    #[test]
    fn enter_between_braces() {
        let mut editor = editor_with("{}", 1);
        editor.execute(Command::InsertNewline);
        assert_eq!(text(&editor), "{\n    \n}");
        assert_eq!(editor.document().selection().head, 6);
    }

    /// Typing and deleting happen at every cursor, and moving moves them all.
    #[test]
    fn multiple_cursors_edit_together() {
        let mut editor = editor_with("foo bar foo baz foo", 0);
        editor.execute(Command::SelectNextOccurrence);
        editor.execute(Command::SelectNextOccurrence);
        editor.execute(Command::SelectNextOccurrence);
        assert_eq!(editor.document().cursors().len(), 2);
        editor.execute(Command::InsertChar('x'));
        assert_eq!(text(&editor), "x bar x baz x");
        editor.execute(Command::DeleteBackward);
        editor.execute(Command::InsertText("mog".into()));
        assert_eq!(text(&editor), "mog bar mog baz mog");
        editor.execute(Command::Undo);
        assert!(editor.document().cursors().is_empty());
        let mut editor = editor_with("ab\nab\nab", 1);
        editor.execute(Command::AddCursorBelow);
        editor.execute(Command::AddCursorBelow);
        editor.execute(Command::Move {
            motion: Motion::LineEnd,
            extend: false,
        });
        editor.execute(Command::InsertChar('!'));
        assert_eq!(text(&editor), "ab!\nab!\nab!");
    }

    /// Selecting all occurrences puts a cursor on each.
    #[test]
    fn select_all_occurrences() {
        let mut editor = editor_with("a.b a.b", 1);
        editor.execute(Command::SelectAllOccurrences);
        assert_eq!(editor.document().cursors().len(), 1);
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

    /// Save as writes to the new path and new documents start empty.
    #[test]
    fn save_as_and_new() {
        let dir = env::temp_dir().join(format!("mog-save-as-{}", process::id()));
        let mut editor = editor_with("hi", 0);
        let path = dir.join("nested").join("out.txt");
        editor.save_as(&path).expect("save");
        assert_eq!(fs::read_to_string(&path).expect("read"), "hi");
        assert!(!editor.document().is_modified());
        editor.new_document();
        assert_eq!(editor.documents().len(), 2);
        assert_eq!(text(&editor), "");
        let _ = fs::remove_dir_all(dir);
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
