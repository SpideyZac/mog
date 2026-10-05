//! Running app level commands like ai.chat or git.panel that the editor cannot handle.

use mog_core::movement;
use mog_tui::{
    Focus, Overlay, Pane, PromptKind, SplitState,
    debug_panel::{self},
    ghost, git_panel,
    popups::{PLUGIN_PICKED_COMMAND, SYMBOL_SEARCH_COMMAND},
    project_search as project_search_panel,
    release_notes::{self},
    search::{self, Toggle},
    settings::SettingKey,
    theme_editor::{self, ThemeDraft},
};
use serde_json::Value;

use super::App;
use crate::{
    plugins::{self},
    update::{self},
};

impl App {
    /// Runs an app level command like `explorer.toggle`.
    pub(super) fn execute_custom(&mut self, name: &str) {
        match name {
            "ai.explain" => {
                let document = self.editor.document();
                let selection = document.selection();
                if selection.is_empty() {
                    self.editor.set_status("select some code to explain first");
                    return;
                }
                if document
                    .path()
                    .is_some_and(|path| self.assistant.excludes(path))
                {
                    self.editor
                        .set_status("this file is in ai.exclude, so it is not sent to the ai");
                    return;
                }
                let code = document
                    .text()
                    .slice(selection.from()..selection.to())
                    .to_string();
                let name = document.name();
                self.ui.chat.open = true;
                self.ui.chat.input = format!("explain this code from {name}:\n\n{code}");
                self.send_chat();
            }
            "ai.chat" => {
                self.ui.chat.open = !self.ui.chat.open || self.ui.focus != Focus::Chat;
                self.ui.focus = if self.ui.chat.open {
                    Focus::Chat
                } else {
                    Focus::Editor
                };
            }
            "ai.send" => self.send_chat(),
            ghost::MORE_COMMAND => self.suggest_ghost(true),
            "copilot.sign_in" => self.assistant.copilot_sign_in(),
            "copilot.sign_out" => self.assistant.copilot_sign_out(),
            "copilot.status" => self.assistant.copilot_check(),
            "explorer.toggle" => {
                if !self.ui.has_explorer {
                    self.editor
                        .set_status("open a folder to get a file explorer: mog <folder>");
                    return;
                }
                self.ui.explorer_open = !self.ui.explorer_open;
                self.ui.focus = if self.ui.explorer_open {
                    Focus::Explorer
                } else {
                    Focus::Editor
                };
            }
            "finder.files" => self.ui.open(Overlay::Finder),
            "search.find" => search::open(&mut self.ui, &self.editor, false),
            "search.replace" => search::open(&mut self.ui, &self.editor, true),
            "search.next" | "search.prev" => {
                if self.ui.search.query.is_empty() {
                    search::open(&mut self.ui, &self.editor, false);
                    return;
                }
                let was_open = self.ui.search.open;
                self.ui.search.open = true;
                search::step(&mut self.ui, &mut self.editor, name == "search.next");
                if !was_open {
                    search::close(&mut self.ui);
                }
            }
            "search.toggle_case" | "search.toggle_word" | "search.toggle_regex" => {
                let toggle = match name {
                    "search.toggle_case" => Toggle::Case,
                    "search.toggle_word" => Toggle::Word,
                    _ => Toggle::Regex,
                };
                if self.ui.overlay == Some(Overlay::ProjectSearch) {
                    let state = &mut self.ui.project_search;
                    state.flip(toggle);
                    state.changed(&mut self.ui.requests);
                } else {
                    self.ui.search.flip(toggle);
                    search::refresh(&mut self.ui, &self.editor);
                }
            }
            "ui.escape" => {
                if self.ui.search.open {
                    search::close(&mut self.ui);
                } else {
                    let head = self.editor.document().selection().head;
                    self.editor.select(head, head);
                }
            }
            "help.keys" => self.ui.open(Overlay::Keys),
            "keys.rebind" => self.apply_rebind(),
            "goto.prompt" => {
                let lines = self.editor.document().text().len_lines();
                self.ui.ask(
                    PromptKind::GotoLine,
                    "\u{21b3} go to line",
                    "",
                    format!("a line from 1 to {lines}"),
                );
            }
            "prompt.submit" => self.submit_prompt(),
            "problems.list" => self.ui.open(Overlay::Problems),
            "problems.next" | "problems.prev" => {
                let message = self.editor.goto_problem(name == "problems.next");
                let message = message.map_or_else(
                    || "no problems, mog is proud of you".to_owned(),
                    |message| message.lines().next().unwrap_or_default().to_owned(),
                );
                self.editor.set_status(message);
            }
            "file.save_as" => self.ask_save_as(),
            "file.new" => self.editor.new_document(),
            "file.create" => {
                let root = self.ui.root.clone();
                self.ui.ask(
                    PromptKind::NewFile(root),
                    "\u{271a} new file",
                    "",
                    "a path in the project folder, end with / to make a folder",
                );
            }
            "settings.open" => self.ui.open(Overlay::Settings),
            "config.open" => self.open_config(),
            "config.reload" => self.reload_config(),
            "config.open_project" => self.open_project_config(),
            "split.toggle" => {
                self.ui.split = match self.ui.split.take() {
                    Some(_) => None,
                    None => Some(SplitState {
                        focused: Pane::Main,
                        other_document: self.editor.active(),
                        other_view: self.editor.view().clone(),
                    }),
                };
            }
            "split.focus" => {
                let Some(split) = self.ui.split.as_mut() else {
                    return;
                };
                // swap what the panes show so the focused one is always the active document
                let document = self.editor.active();
                let view = self.editor.view().clone();
                self.editor.focus(split.other_document);
                *self.editor.view_mut() = split.other_view.clone();
                split.other_document = document;
                split.other_view = view;
                split.focused = match split.focused {
                    Pane::Main => Pane::Side,
                    Pane::Side => Pane::Main,
                };
                self.ui.focus = Focus::Editor;
            }
            "lsp.complete" => self.request_completion(true),
            "lsp.hover" => self.request_feature("hover"),
            "lsp.definition" => self.request_feature("definition"),
            "lsp.format" => self.request_feature("format"),
            "lsp.references" => self.request_feature("references"),
            "lsp.actions" => self.request_feature("actions"),
            action if action.starts_with("plugins.action.") => {
                let index = action["plugins.action.".len()..].parse::<usize>().ok();
                if let Some((plugin, actions)) =
                    index.and_then(|i| self.plugin_state.code_actions.get(i).cloned())
                    && let Err(err) = self.apply_actions(actions)
                {
                    self.editor.set_status(format!("{plugin}: {err}"));
                }
            }
            action if action.starts_with("lsp.action.") => {
                let index = action["lsp.action.".len()..].parse::<usize>().ok();
                if let Some((title, edits)) = index.and_then(|i| self.code_actions.get(i).cloned())
                {
                    self.apply_rename(edits);
                    self.editor.set_status(title);
                }
            }
            "debug.start" => self.start_debugging(),
            "debug.stop" => {
                if self.debugger.is_active() {
                    self.end_debugging("stopped debugging");
                } else {
                    self.editor.set_status("not debugging");
                }
            }
            "debug.toggle_breakpoint" => self.toggle_breakpoint(),
            "debug.step_over" | "debug.step_into" | "debug.step_out" | "debug.pause" => {
                if !self.debugger.is_active() {
                    self.editor.set_status("not debugging, f5 starts");
                    return;
                }
                let command = match name {
                    "debug.step_over" => "next",
                    "debug.step_into" => "stepIn",
                    "debug.step_out" => "stepOut",
                    _ => "pause",
                };
                if command != "pause" {
                    self.ui.debug.resume();
                }
                self.debugger.step(command);
            }
            "debug.panel" => self.ui.debug.open = !self.ui.debug.open,
            debug_panel::FRAME_COMMAND => self.show_frame(self.ui.debug.frame),
            PLUGIN_PICKED_COMMAND => self.plugin_picked(),
            plugin if plugin.starts_with(plugins::PREFIX) => {
                self.run_plugin_command(plugin, Value::Null);
            }
            "plugins.restart" => self.restart_plugins(),
            "plugins.log" => self.show_plugin_log(),
            "task.run" => self.pick_task(),
            "task.start" => {
                let task = self
                    .ui
                    .picked_task
                    .take()
                    .and_then(|index| self.task_list.get(index).cloned());
                if let Some(task) = task {
                    self.start_task(task);
                }
            }
            "task.rerun" => match self.last_task.clone() {
                Some(task) => self.start_task(task),
                None => self.pick_task(),
            },
            "task.stop" => {
                if self.runner.is_running() {
                    self.runner.stop();
                    self.editor.set_status("stopping the task");
                } else {
                    self.editor.set_status("no task is running");
                }
            }
            "task.output" => self.ui.open(Overlay::Output),
            "git.panel" => self.open_git_panel(),
            "git.stage_hunk" => self.stage_hunk(),
            "git.unstage_hunk" => self.unstage_hunk(),
            "git.revert_hunk" => self.revert_hunk(),
            "git.stage_file" => {
                if let Some((path, _, _)) = self.hunk_target() {
                    self.git.run(move |repo| {
                        repo.stage(&path)?;
                        Ok("staged the whole file".into())
                    });
                }
            }
            "git.commit" => {
                if !self.git.is_repo() {
                    self.editor
                        .set_status("this folder is not in a git repository");
                    return;
                }
                self.ui.ask(
                    PromptKind::Commit,
                    "\u{2714} commit what is staged",
                    "",
                    "a message for the commit, enter commits",
                );
            }
            git_panel::DIFF_COMMAND => self.request_git_diff(),
            git_panel::TOGGLE_COMMAND => self.toggle_git_row(),
            git_panel::STAGE_ALL_COMMAND => self.git.run(|repo| {
                repo.stage_all()?;
                Ok("staged everything".into())
            }),
            git_panel::REFRESH_COMMAND => {
                self.git.request_changes();
                self.ui.git_panel.diff_for = None;
            }
            git_panel::OPEN_COMMAND => {
                let Some(row) = self.ui.git_panel.current().cloned() else {
                    return;
                };
                self.ui.close();
                self.ui.focus = Focus::Editor;
                if let Err(err) = self.editor.open(&row.path) {
                    self.editor
                        .set_status(format!("could not open {}: {err}", row.path.display()));
                }
            }
            "lsp.signature" => self.request_signature(),
            "lsp.symbols" => self.request_symbols(None),
            "lsp.workspace_symbols" => {
                self.ui.symbol_query.clear();
                self.ui.symbols.clear();
                self.ui.symbols_version += 1;
                self.ui.open(Overlay::WorkspaceSymbols);
                self.request_symbols(Some(String::new()));
            }
            SYMBOL_SEARCH_COMMAND => self.request_symbols(Some(self.ui.symbol_query.clone())),
            "lsp.restart" => {
                self.lsp.restart();
                self.marks_requested = None;
                self.editor
                    .set_status("language servers restart with the next file that needs one");
            }
            "lsp.rename" => {
                let document = self.editor.document();
                let head = document.selection().head;
                // at the end of a word the word is behind the cursor
                let text = document.text();
                let is_word = |pos: usize| {
                    text.get_char(pos)
                        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
                };
                let at = if !is_word(head) && head > 0 && is_word(head - 1) {
                    head - 1
                } else {
                    head
                };
                let (from, to) = movement::word_at(text, at);
                let word = text.slice(from..to).to_string();
                self.ui.ask(
                    PromptKind::RenameSymbol,
                    "\u{270e} rename symbol",
                    word,
                    "enter to rename everywhere",
                );
            }
            "audio.toggle_music" => self.change_setting(SettingKey::Audio("music")),
            "audio.toggle_effects" => self.change_setting(SettingKey::Audio("sound_effects")),
            "flair.toggle" => self.change_setting(SettingKey::FlairEnabled),
            "theme.edit" => {
                self.ui.theme_draft = Some(ThemeDraft::new(&self.theme));
                self.ui.open(Overlay::ThemeEditor);
            }
            theme_editor::CANCEL_COMMAND => {
                self.ui.theme_draft = None;
                self.ui.close();
                self.apply_config();
            }
            theme_editor::SAVE_COMMAND => {
                let Some(draft) = &self.ui.theme_draft else {
                    return;
                };
                let name = if self.ui.config.themes.contains_key(&draft.name) {
                    draft.name.clone()
                } else {
                    format!("my-{}", draft.name)
                };
                self.ui.ask(
                    PromptKind::SaveTheme,
                    "\u{25d0} save theme as",
                    name,
                    "saved as [themes.<name>] in your config",
                );
            }
            "theme.next" => {
                self.change_setting(SettingKey::Theme);
                self.editor
                    .set_status(format!("theme: {}", self.ui.config.ui.theme));
            }
            "graph.toggle" => {
                if self.ui.overlay == Some(Overlay::Graph) {
                    self.ui.close();
                } else {
                    self.ui.open(Overlay::Graph);
                }
            }
            "help.release_notes" => match self.installed.clone().or_else(|| self.update.clone()) {
                Some(release) => self.show_release_notes(&release),
                None => {
                    self.editor
                        .set_status("getting the release notes from github");
                    self.updater.notes(None);
                }
            },
            "update.check" => {
                self.editor.set_status("checking for updates");
                self.updater.check(true, self.ui.config.updates.install);
            }
            release_notes::UPDATE_COMMAND => match self.update.clone() {
                Some(release) => {
                    let message = match update::cannot_install() {
                        Some(reason) => format!("can not update: {reason}"),
                        None if self.updater.install(release) => {
                            "downloading the update in the background".to_owned()
                        }
                        None => "already updating, hang on".to_owned(),
                    };
                    self.editor.set_status(message);
                }
                None => {
                    self.editor.set_status("checking for updates");
                    self.updater.check(true, true);
                }
            },
            release_notes::OPEN_COMMAND => {
                let url = self
                    .ui
                    .release_notes
                    .as_ref()
                    .map_or("https://github.com/SpideyZac/mog/releases", |notes| {
                        notes.url.as_str()
                    });
                if let Err(err) = open::that_detached(url) {
                    self.editor
                        .set_status(format!("could not open {url}: {err}"));
                }
            }
            "annotate.toggle" => {
                self.ui.annotate.toggle();
                if self.ui.annotate.active {
                    self.editor.set_status(
                        "drawing: left drag draws, right drag erases, 1 to 8 pick a color, esc \
                         when done",
                    );
                }
            }
            "annotate.clear" => self.ui.annotate.clear(),
            "terminal.toggle" => {
                let focused = self.ui.terminal_open && self.ui.focus == Focus::Terminal;
                self.ui.terminal_open = !focused;
                self.ui.focus = if focused {
                    Focus::Editor
                } else {
                    Focus::Terminal
                };
            }
            "project_search.open" => self.open_project_search(false),
            "project_search.replace" => self.open_project_search(true),
            project_search_panel::RUN_COMMAND => self.run_project_search(),
            project_search_panel::PICK_COMMAND => self.pick_project_match(),
            project_search_panel::REPLACE_ALL_COMMAND => self.ask_replace_in_project(),
            "terminal.restart" => {
                self.ui.terminal_restart = true;
                self.ui.terminal_open = true;
                self.ui.focus = Focus::Terminal;
            }
            "explorer.focus" => {
                if self.ui.has_explorer {
                    self.ui.explorer_open = true;
                    self.ui.focus = Focus::Explorer;
                }
            }
            _ => self
                .editor
                .set_status(format!("{name} is not available yet")),
        }
    }
}
