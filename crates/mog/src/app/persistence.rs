//! Bringing back open files, unsaved work and undo history between runs.

use std::{
    collections::{HashMap, HashSet},
    fs, mem,
};

use mog_core::Change;
use mog_tui::{Pane, PromptKind, SplitState};

use super::App;
use crate::session::{Session, SessionFile};

impl App {
    /// Opens the files from the last time this folder was open and offers to bring back unsaved
    /// work from a mog that crashed.
    ///
    /// Not part of [`App::new`] so snapshots and tests start clean.
    pub fn restore_session(&mut self) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let root = self.ui.root.clone();
        self.ui.recent_commands = state.load_recent_commands();
        self.saved_recent.clone_from(&self.ui.recent_commands);
        let wanted = self.ui.config.editor.restore_session && self.ui.has_explorer;
        // a file given on the command line is a one off, not the project session
        let opened_file = self.editor.document().path().is_some();
        if wanted && !opened_file {
            self.session_enabled = true;
            if let Some(session) = state.load_session(&root) {
                self.apply_session(&session);
                self.last_session = Some(session);
            }
        }
        self.recoverable = state
            .orphaned_swaps(&root)
            .into_iter()
            .filter(|(_, swap)| {
                // a swap that matches the file on disk has nothing to recover
                swap.path.as_ref().is_none_or(|path| {
                    fs::read_to_string(path).ok().as_deref() != Some(swap.text.as_str())
                })
            })
            .collect();
        if self.recoverable.is_empty() {
            return;
        }
        let names: Vec<String> = self
            .recoverable
            .iter()
            .map(|(_, swap)| {
                swap.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map_or_else(|| "untitled".into(), |name| name.to_string_lossy().into())
            })
            .collect();
        self.ui.ask(
            PromptKind::RecoverSwaps,
            "\u{26a0} mog did not close cleanly, recover unsaved work?",
            "",
            format!("y brings back {}, n throws it away", names.join(", ")),
        );
    }

    /// Opens the files in `session` and puts the cursors, scroll, split and breakpoints back.
    pub(super) fn apply_session(&mut self, session: &Session) {
        self.ui.breakpoints = session
            .breakpoints
            .iter()
            .map(|(path, lines)| (path.clone(), lines.iter().copied().collect()))
            .collect();
        let mut opened = Vec::new();
        for file in &session.files {
            if !file.path.is_file() || self.editor.open(&file.path).is_err() {
                opened.push(None);
                continue;
            }
            let len = self.editor.document().text().len_chars();
            self.editor.select(file.anchor.min(len), file.head.min(len));
            let lines = self.editor.document().text().len_lines();
            self.editor.view_mut().scroll_line = file.scroll_line.min(lines.saturating_sub(1));
            opened.push(Some(self.editor.active()));
        }
        let index = |at: usize| opened.get(at).copied().flatten();
        if let Some(split) = session.split.and_then(index) {
            self.ui.split = Some(SplitState {
                focused: Pane::Main,
                other_document: split,
                other_view: self.editor.view().clone(),
            });
        }
        if let Some(active) = index(session.active) {
            self.editor.focus(active);
        }
        self.ui.explorer_open = session.explorer_open && self.ui.has_explorer;
    }

    /// Returns what is open now as a session.
    pub(super) fn current_session(&self) -> Session {
        let mut files = Vec::new();
        let mut indexes = HashMap::new();
        for (index, document) in self.editor.documents().iter().enumerate() {
            let Some(path) = document.path() else {
                continue;
            };
            let view = if index == self.editor.active() {
                self.editor.view().scroll_line
            } else {
                0
            };
            let selection = document.selection();
            indexes.insert(index, files.len());
            files.push(SessionFile {
                path: path.to_owned(),
                anchor: selection.anchor,
                head: selection.head,
                scroll_line: view,
            });
        }
        Session {
            active: indexes.get(&self.editor.active()).copied().unwrap_or(0),
            split: self
                .ui
                .split
                .as_ref()
                .and_then(|split| indexes.get(&split.other_document).copied()),
            explorer_open: self.ui.explorer_open,
            breakpoints: self
                .ui
                .breakpoints
                .iter()
                .map(|(path, lines)| (path.clone(), lines.iter().copied().collect()))
                .collect(),
            files,
        }
    }

    /// Saves the open files for next time if they changed.
    pub(super) fn save_session(&mut self) {
        let Some(state) = self.state.as_ref().filter(|_| self.session_enabled) else {
            return;
        };
        let session = self.current_session();
        if self.last_session.as_ref() != Some(&session) {
            state.save_session(&self.ui.root, &session);
            self.last_session = Some(session);
        }
    }

    /// Saves the commands run from the palette if they changed.
    pub(super) fn save_recent_commands(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self.saved_recent != self.ui.recent_commands {
            state.save_recent_commands(&self.ui.recent_commands);
            self.saved_recent.clone_from(&self.ui.recent_commands);
        }
    }

    /// Writes unsaved documents to swap files and removes the swaps of ones that were saved or
    /// closed.
    pub(super) fn write_swaps(&mut self) {
        let Some(state) = &self.state else {
            return;
        };
        let mut wanted = HashSet::new();
        for (index, document) in self.editor.documents().iter().enumerate() {
            // writing a huge file every second would stall typing
            if !document.is_modified() || document.is_large() {
                continue;
            }
            let key = document.path().map_or_else(
                || format!("untitled-{index}"),
                |path| path.to_string_lossy().into_owned(),
            );
            if self.swaps.get(&key).map(|(version, _)| *version) != Some(document.version()) {
                let text = document.text().clone();
                let file = state.write_swap(&key, document.path(), &self.ui.root, text);
                self.swaps.insert(key.clone(), (document.version(), file));
            }
            wanted.insert(key);
        }
        self.swaps.retain(|key, (_, file)| {
            let keep = wanted.contains(key);
            if !keep {
                state.remove_swap(file.clone());
            }
            keep
        });
        if let Some(err) = state.take_error() {
            self.editor
                .set_status(format!("could not save recovery data: {err}"));
        }
    }

    /// Puts back the undo history saved for files that were just opened.
    pub(super) fn restore_undo(&mut self) {
        let Some(state) = self
            .state
            .as_ref()
            .filter(|_| self.ui.config.editor.persistent_undo)
        else {
            return;
        };
        for document in self.editor.documents_mut() {
            let Some(path) = document.path().map(ToOwned::to_owned) else {
                continue;
            };
            if document.version() != 0 || !self.undo_checked.insert(path.clone()) {
                continue;
            }
            if let Some(history) = state.load_undo(&path, &document.text().to_string()) {
                document.restore_history(history);
            }
        }
    }

    /// Saves the undo history of the focused file, which was just saved.
    pub(super) fn save_undo(&self) {
        let Some(state) = self
            .state
            .as_ref()
            .filter(|_| self.ui.config.editor.persistent_undo)
        else {
            return;
        };
        let document = self.editor.document();
        if let Some(path) = document.path() {
            state.save_undo(path, document.text(), document.history());
        }
    }

    /// Brings back the unsaved work the user chose to recover.
    pub(super) fn recover_swaps(&mut self) {
        for (file, swap) in mem::take(&mut self.recoverable) {
            match &swap.path {
                Some(path) => {
                    if let Err(err) = self.editor.open(path) {
                        self.editor
                            .set_status(format!("could not open {}: {err}", path.display()));
                        continue;
                    }
                }
                None => self.editor.new_document(),
            }
            let end = self.editor.document().text().len_chars();
            self.editor.apply_changes(vec![Change {
                start: 0,
                end,
                text: swap.text,
            }]);
            let _ = fs::remove_file(file);
        }
        self.editor
            .set_status("recovered, save to keep it. ctrl+z goes back to the file on disk");
    }

    /// Saves what should outlive this run and cleans up swap files, right before quitting.
    pub(super) fn shut_down(&mut self) {
        self.save_session();
        self.save_recent_commands();
        if let Some(state) = self
            .state
            .as_ref()
            .filter(|_| self.ui.config.editor.persistent_undo)
        {
            for document in self.editor.documents() {
                if let Some(path) = document.path().filter(|_| !document.is_modified()) {
                    state.save_undo(path, document.text(), document.history());
                }
            }
        }
        // quitting with unsaved changes was confirmed, so they are meant to go
        if let Some(state) = &self.state {
            for (_, file) in self.swaps.values() {
                state.remove_swap(file.clone());
            }
            state.flush();
        }
        self.swaps.clear();
    }
}
