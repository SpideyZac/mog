//! Git status, the source control panel and staging hunks.

use std::{path::PathBuf, time::Instant};

use mog_core::Change;
use mog_git::{Hunk, apply_hunk, hunks, map_line, revert_hunk};
use mog_tui::Overlay;

use super::App;
use crate::git::{self, GitUpdate};

impl App {
    /// Asks git about open files and the cursor line.
    pub(super) fn sync_git(&mut self) {
        for document in self.editor.documents() {
            if let Some(path) = document.path().filter(|_| !document.is_large()) {
                self.git.ensure_base(path);
            }
        }
        if !self.ui.config.ui.git_blame || self.editor.document().is_large() {
            return;
        }
        let document = self.editor.document();
        if let Some(path) = document.path() {
            let line = document.text().char_to_line(document.selection().head);
            self.git.request_blame(path, line, document.version(), || {
                document.text().to_string()
            });
        }
    }

    /// Acts on a background git answer.
    pub(super) fn handle_git(&mut self, update: GitUpdate) {
        let GitUpdate::Done(result) = update else {
            let status = matches!(update, GitUpdate::Status(_) | GitUpdate::Branch(..));
            git::apply(&mut self.ui, update);
            if status {
                self.plugin_git_changed();
            }
            if self.ui.overlay == Some(Overlay::Git) && !self.ui.git_panel.diff_is_current() {
                self.request_git_diff();
            }
            return;
        };
        match result {
            Ok(message) => self.editor.set_status(message),
            Err(err) => self.editor.set_status(format!("git: {err}")),
        }
        // staging and committing change the gutter, the explorer colors and the panel
        self.git.refresh();
        self.git_refreshed = Instant::now();
        if self.ui.overlay == Some(Overlay::Git) {
            self.git.request_changes();
            self.ui.git_panel.diff_for = None;
        }
    }

    /// Asks for the diff of the file highlighted in the source control panel.
    pub(super) fn request_git_diff(&mut self) {
        if let Some(row) = self.ui.git_panel.current() {
            self.git.request_diff(row.path.clone(), row.staged);
        }
    }

    /// Opens the source control panel.
    pub(super) fn open_git_panel(&mut self) {
        if !self.git.is_repo() {
            self.editor
                .set_status("this folder is not in a git repository");
            return;
        }
        self.ui.git_panel.diff_for = None;
        self.ui.open(Overlay::Git);
        self.git.request_changes();
    }

    /// Returns the focused file, its text and the cursor line, for hunk commands.
    pub(super) fn hunk_target(&mut self) -> Option<(PathBuf, String, usize)> {
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            self.editor
                .set_status("save the file first so git can see it");
            return None;
        };
        if !self.git.is_repo() {
            self.editor
                .set_status("this folder is not in a git repository");
            return None;
        }
        let line = document.text().char_to_line(document.selection().head);
        Some((path, document.text().to_string(), line))
    }

    /// Stages the change under the cursor, leaving the rest of the file as it is in the index.
    pub(super) fn stage_hunk(&mut self) {
        let Some((path, text, line)) = self.hunk_target() else {
            return;
        };
        self.git.run(move |repo| {
            let index = repo.index_text(&path).unwrap_or_default();
            let hunk = hunks(&index, &text)
                .into_iter()
                .find(|hunk| hunk.touches(line))
                .ok_or("no change here to stage")?;
            repo.set_index_text(&path, &apply_hunk(&index, &text, &hunk))?;
            Ok(format!("staged lines {}", hunk_lines(&hunk)))
        });
    }

    /// Takes the staged change under the cursor back out of the index.
    pub(super) fn unstage_hunk(&mut self) {
        let Some((path, text, line)) = self.hunk_target() else {
            return;
        };
        self.git.run(move |repo| {
            let head = repo.head_text(&path).unwrap_or_default();
            let index = repo
                .index_text(&path)
                .ok_or("nothing is staged for this file")?;
            let at = map_line(&index, &text, line);
            let hunk = hunks(&head, &index)
                .into_iter()
                .find(|hunk| hunk.touches(at))
                .ok_or("no staged change here")?;
            repo.set_index_text(&path, &revert_hunk(&head, &index, &hunk))?;
            Ok(format!("unstaged lines {}", hunk_lines(&hunk)))
        });
    }

    /// Puts the change under the cursor back the way it is in the last commit.
    pub(super) fn revert_hunk(&mut self) {
        let Some((path, text, line)) = self.hunk_target() else {
            return;
        };
        let Some(base) = self.ui.git_base.get(&path) else {
            self.editor
                .set_status("this file is not committed, nothing to go back to");
            return;
        };
        let Some(hunk) = hunks(base, &text)
            .into_iter()
            .find(|hunk| hunk.touches(line))
        else {
            self.editor.set_status("no change here");
            return;
        };
        let old: String = base
            .split_inclusive('\n')
            .skip(hunk.old.start)
            .take(hunk.old.len())
            .collect();
        let rope = self.editor.document().text();
        let at = |line: usize| rope.line_to_char(line.min(rope.len_lines()));
        let change = Change {
            start: at(hunk.new.start),
            end: at(hunk.new.end),
            text: old,
        };
        self.editor.apply_changes(vec![change]);
        self.editor
            .set_status("reverted to the last commit, ctrl+z brings it back");
    }

    /// Stages or unstages the file highlighted in the source control panel.
    pub(super) fn toggle_git_row(&mut self) {
        let Some(row) = self.ui.git_panel.current().cloned() else {
            return;
        };
        self.git.run(move |repo| {
            let name = row
                .path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            if row.staged {
                repo.unstage(&row.path)?;
                Ok(format!("unstaged {name}"))
            } else {
                repo.stage(&row.path)?;
                Ok(format!("staged {name}"))
            }
        });
    }
}

/// Describes the lines of `hunk` in the new text for a status message, counted from 1.
fn hunk_lines(hunk: &Hunk) -> String {
    if hunk.new.len() <= 1 {
        format!("{}", hunk.new.start + 1)
    } else {
        format!("{} to {}", hunk.new.start + 1, hunk.new.end)
    }
}
