//! Answers typed into the one line prompt, like save as and go to line.

use std::{ffi::OsStr, fs, io, mem, path::Path};

use clap::Parser;
use mog_config::project;
use mog_core::{Command, Document};
use mog_tui::{Focus, Overlay, PromptKind};

use super::App;
use crate::cli::Args;

impl App {
    /// Asks where to save the focused document.
    pub(super) fn ask_save_as(&mut self) {
        let current = self
            .editor
            .document()
            .path()
            .map(|path| {
                path.strip_prefix(&self.ui.root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .unwrap_or_default();
        let hint = format!("relative to {}", self.ui.root.display());
        self.ui
            .ask(PromptKind::SaveAs, "\u{21e9} save as", current, hint);
    }

    /// Asks for a file to open or a folder to switch to.
    pub(super) fn ask_open_path(&mut self) {
        let hint = format!("a file or folder, relative to {}", self.ui.root.display());
        self.ui.ask(PromptKind::OpenPath, "\u{25b8} open", "", hint);
    }

    /// Opens `path` as a file, or restarts the app on it if it is a folder.
    fn open_path(&mut self, path: &Path) -> io::Result<()> {
        if !path.is_dir() {
            self.editor.open(path)?;
            self.ui.focus = Focus::Editor;
            return Ok(());
        }
        if self.editor.documents().iter().any(Document::is_modified) {
            self.editor
                .set_status("save or close your unsaved files before switching folders");
            return Ok(());
        }
        self.save_session();
        *self = Self::new(Args::parse_from([OsStr::new("mog"), path.as_os_str()]));
        Ok(())
    }

    /// Acts on an answered prompt.
    pub(super) fn submit_prompt(&mut self) {
        let Some(prompt) = self.ui.submitted.take() else {
            return;
        };
        if prompt.kind == PromptKind::CopilotSignIn {
            if let Some(code) = self.copilot_code.take() {
                self.editor
                    .set_status("finish signing in to copilot in your browser");
                self.assistant.copilot_finish_sign_in(code);
            }
            return;
        }
        let text = prompt.text.trim();
        if text.is_empty() {
            return;
        }
        let resolve = |base: &Path| {
            let path = Path::new(text);
            if path.is_absolute() {
                path.to_owned()
            } else {
                base.join(path)
            }
        };
        let result = match prompt.kind {
            PromptKind::GotoLine => {
                if let Ok(line) = text.parse() {
                    self.ui.focus = Focus::Editor;
                    self.execute_command(Command::GotoLine(line));
                }
                Ok(())
            }
            PromptKind::OpenPath => {
                let path = resolve(&self.ui.root);
                self.open_path(&path)
            }
            PromptKind::SaveAs => {
                let path = resolve(&self.ui.root);
                let result = self.editor.save_as(&path);
                if result.is_ok() {
                    self.saved();
                }
                result
            }
            PromptKind::NewFile(dir) => {
                let path = resolve(&dir);
                let result = if text.ends_with('/') || text.ends_with('\\') {
                    fs::create_dir_all(&path)
                } else {
                    path.parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| fs::OpenOptions::new().create(true).append(true).open(&path))
                        .and_then(|_| self.editor.open(&path))
                };
                if result.is_ok() {
                    self.ui.focus = Focus::Editor;
                }
                result
            }
            PromptKind::RenameFile(old) => {
                let new = resolve(old.parent().unwrap_or(&self.ui.root));
                let result = fs::rename(&old, &new);
                if result.is_ok() {
                    for document in self.editor.documents_mut() {
                        if document.path() == Some(old.as_path()) {
                            document.set_path(&new);
                        }
                    }
                    self.editor
                        .set_status(format!("renamed to {}", new.display()));
                }
                result
            }
            PromptKind::DeleteFile(path) => {
                if !matches!(text, "y" | "yes") {
                    return;
                }
                let result = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                if result.is_ok() {
                    // folders take every file open inside them along
                    let gone: Vec<usize> = (0..self.editor.documents().len())
                        .filter(|&index| {
                            self.editor.documents()[index]
                                .path()
                                .is_some_and(|open| open.starts_with(&path))
                        })
                        .collect();
                    for index in gone.into_iter().rev() {
                        self.editor.close(index);
                    }
                    self.editor
                        .set_status(format!("deleted {}, gone forever", path.display()));
                }
                result
            }
            PromptKind::RenameSymbol => {
                self.request_rename(text.to_owned());
                Ok(())
            }
            PromptKind::TrustProject => {
                if let Some(file) = self.untrusted_project.take()
                    && matches!(text, "y" | "yes")
                {
                    match project::trust(&file) {
                        Ok(()) => self.reload_config(),
                        Err(err) => self.editor.set_status(format!("could not trust it: {err}")),
                    }
                }
                return;
            }
            PromptKind::ReplaceAll => {
                if matches!(text, "y" | "yes") {
                    self.replace_in_project();
                }
                self.ui.open(Overlay::ProjectSearch);
                return;
            }
            PromptKind::SaveTheme => {
                self.save_theme(text);
                return;
            }
            PromptKind::Plugin => {
                self.plugin_prompted(text);
                return;
            }
            PromptKind::Commit => {
                let message = text.to_owned();
                self.git.run(move |repo| {
                    let id = repo.commit(&message)?;
                    Ok(format!("committed {id}: {message}"))
                });
                if self.ui.git_panel.loaded {
                    self.ui.open(Overlay::Git);
                }
                return;
            }
            PromptKind::RecoverSwaps => {
                if matches!(text, "y" | "yes") {
                    self.recover_swaps();
                } else {
                    for (file, _) in mem::take(&mut self.recoverable) {
                        let _ = fs::remove_file(file);
                    }
                }
                return;
            }
            PromptKind::InstallServer(command) => {
                if matches!(text, "y" | "yes") {
                    self.ui.terminal_open = true;
                    self.ui.focus = Focus::Terminal;
                    self.ui.terminal_input.extend_from_slice(command.as_bytes());
                    self.ui.terminal_input.push(b'\r');
                    self.editor.set_status(
                        "installing, run Code: Restart language servers once it is done",
                    );
                }
                return;
            }
            PromptKind::CopilotSignIn => Ok(()),
        };
        self.ui.refresh_explorer = true;
        if let Err(err) = result {
            self.editor.set_status(format!("that did not work: {err}"));
        }
    }
}
