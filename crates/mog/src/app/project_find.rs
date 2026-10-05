//! Finding and replacing across the project.

use std::{collections::HashMap, path::PathBuf, sync::atomic::Ordering};

use anyhow::Result;
use mog_core::{
    project_search::{self, ProjectResults},
    search::{self as core_search, Matcher},
};
use mog_tui::{Focus, Overlay, PromptKind};
use tokio::task;

use super::{App, PROJECT_LIMIT};

impl App {
    /// Opens project find and replace, starting from the selected text if there is some.
    pub(super) fn open_project_search(&mut self, replacing: bool) {
        if !self.ui.has_explorer {
            self.editor
                .set_status("open a folder to search the whole project: mog <folder>");
            return;
        }
        let document = self.editor.document();
        let selection = document.selection();
        let selected = document
            .text()
            .slice(selection.from()..selection.to())
            .to_string();
        let state = &mut self.ui.project_search;
        state.replacing = replacing;
        if !selected.is_empty() && !selected.contains('\n') && selected != state.query {
            state.query = selected;
            state.changed(&mut self.ui.requests);
        }
        self.ui.open(Overlay::ProjectSearch);
    }

    /// Starts searching the project for the current query, cancelling any older search.
    pub(super) fn run_project_search(&mut self) {
        let state = &self.ui.project_search;
        let generation = state.generation;
        self.project_generation.store(generation, Ordering::Relaxed);
        if state.query.is_empty() {
            return;
        }
        let open: HashMap<PathBuf, String> = self
            .editor
            .documents()
            .iter()
            .filter_map(|document| Some((document.path()?.to_owned(), document.text().to_string())))
            .collect();
        let (root, query, options) = (self.ui.root.clone(), state.query.clone(), state.options());
        let current = self.project_generation.clone();
        let sender = self.project_sender.clone();
        task::spawn_blocking(move || {
            let cancelled = || current.load(Ordering::Relaxed) != generation;
            let results = project_search::search_project(
                &root,
                &query,
                options,
                &open,
                PROJECT_LIMIT,
                &cancelled,
            );
            if !cancelled() {
                let _ = sender.send((generation, results));
            }
        });
    }

    /// Shows what a project search found, unless a newer search started since.
    pub(super) fn show_project_results(
        &mut self,
        generation: u64,
        results: Result<ProjectResults, String>,
    ) {
        let state = &mut self.ui.project_search;
        if generation != state.generation {
            return;
        }
        state.searching = false;
        match results {
            Ok(results) => {
                state.results = results;
                state.error = None;
            }
            Err(err) => {
                state.results = ProjectResults::default();
                state.error = Some(err);
            }
        }
    }

    /// Opens the match picked in project search.
    pub(super) fn pick_project_match(&mut self) {
        let state = &mut self.ui.project_search;
        let Some(found) = state
            .picked
            .take()
            .and_then(|index| state.results.matches.get(index).cloned())
        else {
            return;
        };
        self.ui.close();
        self.ui.focus = Focus::Editor;
        if let Err(err) = self.editor.open(&found.path) {
            self.editor
                .set_status(format!("could not open {}: {err}", found.path.display()));
            return;
        }
        let text = self.editor.document().text();
        let line = found.line.min(text.len_lines().saturating_sub(1));
        let from = (text.line_to_char(line) + found.column).min(text.len_chars());
        let to = (from + found.len).min(text.len_chars());
        self.editor.select(from, to);
    }

    /// Asks before replacing every match in the project.
    pub(super) fn ask_replace_in_project(&mut self) {
        let state = &self.ui.project_search;
        if state.query.is_empty() || state.results.matches.is_empty() {
            self.editor.set_status("nothing to replace");
            return;
        }
        let more = if state.results.truncated { "+" } else { "" };
        let title = format!(
            "replace {}{more} matches in {} files with \"{}\"?",
            state.results.matches.len(),
            state.results.files,
            state.replacement
        );
        self.ui.ask(
            PromptKind::ReplaceAll,
            title,
            "",
            "type y and enter. open files change in the editor, others are saved right away",
        );
    }

    /// Replaces every match of the project search, in open documents and on disk.
    pub(super) fn replace_in_project(&mut self) {
        let state = self.ui.project_search.clone();
        let focused = self.editor.active();
        let open: HashMap<PathBuf, String> = self
            .editor
            .documents()
            .iter()
            .filter_map(|document| Some((document.path()?.to_owned(), document.text().to_string())))
            .collect();
        let Ok(matcher) = Matcher::new(&state.query, state.options()) else {
            return;
        };
        let Ok(results) = project_search::search_project(
            &self.ui.root,
            &state.query,
            state.options(),
            &open,
            usize::MAX,
            &|| false,
        ) else {
            return;
        };
        let mut paths: Vec<PathBuf> = results
            .matches
            .iter()
            .map(|found| found.path.clone())
            .collect();
        paths.dedup();
        let (mut replaced, mut files, mut problems) = (0, 0, Vec::new());
        for path in paths {
            let index = self
                .editor
                .documents()
                .iter()
                .position(|document| document.path() == Some(path.as_path()));
            let count = match index {
                Some(index) => {
                    self.editor.focus(index);
                    let changes = core_search::replace_changes(
                        self.editor.document().text(),
                        &matcher,
                        &state.replacement,
                    );
                    let count = changes.len();
                    self.editor.apply_changes(changes);
                    count
                }
                None => {
                    match project_search::replace_in_file(&path, &matcher, &state.replacement) {
                        Ok(count) => count,
                        Err(err) => {
                            problems.push(format!("{}: {err}", path.display()));
                            0
                        }
                    }
                }
            };
            if count > 0 {
                replaced += count;
                files += 1;
            }
        }
        self.editor.focus(focused);
        self.ui.refresh_explorer = true;
        self.editor.set_status(if problems.is_empty() {
            format!("replaced {replaced} matches in {files} files")
        } else {
            format!(
                "replaced {replaced} matches, failed: {}",
                problems.join("; ")
            )
        });
        self.ui.project_search.changed(&mut self.ui.requests);
    }
}
