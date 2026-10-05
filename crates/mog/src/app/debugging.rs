//! Debug sessions and breakpoints.

use std::{collections::BTreeSet, path::PathBuf};

use mog_config::{DebugConfig, merged_debuggers};
use mog_core::movement;
use mog_dap::DapEvent;
use mog_tui::{Focus, debug_panel::FrameEntry};
use serde_json::json;

use super::App;
use crate::{
    debug::{self, DebugReply},
    tasks::{self},
};

impl App {
    /// Starts debugging the focused file's project, continuing instead if the program is paused.
    pub(super) fn start_debugging(&mut self) {
        if self.debugger.is_active() {
            if self.ui.debug.paused {
                self.debugger.step("continue");
                self.ui.debug.resume();
            } else {
                self.editor.set_status("already debugging, shift+f5 stops");
            }
            return;
        }
        let debuggers = merged_debuggers(&self.ui.config.debug);
        let path = self.editor.document().path().map(ToOwned::to_owned);
        let Some((name, config)) = debug::pick(&debuggers, path.as_deref()) else {
            self.editor.set_status(
                "no debugger for this file, add [debug.<name>] to the config, see the readme",
            );
            return;
        };
        if !config.before.is_empty() {
            let task = tasks::tasks(&self.ui.root, &self.ui.config.tasks)
                .into_iter()
                .find(|task| task.name == config.before);
            if let Some(task) = task {
                self.debug_after_task = Some((name, config));
                self.start_task(task);
                return;
            }
        }
        self.launch_debugger(&name, &config);
    }

    /// Starts the debug adapter of `config`, called `name`.
    pub(super) fn launch_debugger(&mut self, name: &str, config: &DebugConfig) {
        let path = self.editor.document().path().map(ToOwned::to_owned);
        let vars = debug::variables(&self.ui.root, path.as_deref());
        let arguments = debug::substitute(&config.arguments, &vars);
        let arguments = if arguments.is_object() {
            arguments
        } else {
            json!({})
        };
        if let Err(err) = self.debugger.start(name, config, arguments, &self.ui.root) {
            self.editor.set_status(err);
            return;
        }
        let state = &mut self.ui.debug;
        state.active = true;
        state.open = true;
        state.paused = false;
        state.console.clear();
        state.status = format!("starting {name}");
        self.debug_frames.clear();
        self.editor
            .set_status(format!("debugging with {name}, shift+f5 stops"));
    }

    /// Ends the debugging session and clears what it showed.
    pub(super) fn end_debugging(&mut self, message: impl Into<String>) {
        self.debugger.stop();
        self.debug_after_task = None;
        self.debug_frames.clear();
        let state = &mut self.ui.debug;
        state.active = false;
        state.paused = false;
        state.frames.clear();
        state.variables.clear();
        state.stopped_at = None;
        state.status = "finished".into();
        self.editor.set_status(message);
    }

    /// Toggles a breakpoint on the cursor line.
    pub(super) fn toggle_breakpoint(&mut self) {
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            self.editor
                .set_status("save the file first so the debugger can find it");
            return;
        };
        let line = document.text().char_to_line(document.selection().head);
        let mut lines = document.breakpoints().clone();
        if !lines.remove(&line) {
            lines.insert(line);
        }
        self.editor
            .document_mut()
            .set_breakpoints(lines.iter().copied());
        self.publish_breakpoints(path, lines);
    }

    /// Records the breakpoints of `path` for the gutter and the session, and tells the debugger.
    pub(super) fn publish_breakpoints(&mut self, path: PathBuf, lines: BTreeSet<usize>) {
        let list: Vec<usize> = lines.iter().copied().collect();
        if lines.is_empty() {
            self.ui.breakpoints.remove(&path);
        } else {
            self.ui.breakpoints.insert(path.clone(), lines);
        }
        self.debugger.set_breakpoints(path, list);
    }

    /// Keeps breakpoints and open documents in step: newly opened files get theirs back, and
    /// breakpoints that edits moved are passed on.
    pub(super) fn sync_breakpoints(&mut self) {
        let mut moved = Vec::new();
        for document in self.editor.documents_mut() {
            let Some(path) = document.path().map(ToOwned::to_owned) else {
                continue;
            };
            let known = self.ui.breakpoints.get(&path);
            if document.version() == 0 && document.breakpoints().is_empty() {
                // a file that was just opened takes the breakpoints it had before
                if let Some(lines) = known {
                    document.set_breakpoints(lines.iter().copied());
                }
            } else if known.map_or(!document.breakpoints().is_empty(), |known| {
                known != document.breakpoints()
            }) {
                moved.push((path, document.breakpoints().clone()));
            }
        }
        for (path, lines) in moved {
            self.publish_breakpoints(path, lines);
        }
    }

    /// Shows the frame picked in the call stack and loads its variables.
    pub(super) fn show_frame(&mut self, index: usize) {
        let Some(frame) = self.debug_frames.get(index).cloned() else {
            return;
        };
        self.ui.debug.frame = index;
        self.ui.debug.variables.clear();
        self.debugger.load_variables(frame.id);
        let Some(path) = frame.path else {
            return;
        };
        if let Err(err) = self.editor.open(&path) {
            self.editor
                .set_status(format!("could not open {}: {err}", path.display()));
            return;
        }
        let text = self.editor.document().text();
        let line = frame.line.min(text.len_lines().saturating_sub(1));
        let pos = text.line_to_char(line) + frame.column.min(movement::line_len(text, line));
        self.editor.select(pos, pos);
        self.ui.debug.stopped_at = Some((
            self.editor
                .document()
                .path()
                .map_or(path, ToOwned::to_owned),
            line,
        ));
        self.ui.focus = Focus::Editor;
    }

    /// Acts on something the debug adapter reported.
    pub(super) fn handle_debug_event(&mut self, event: DapEvent) {
        match event {
            DapEvent::Initialized => self.debugger.configure(self.ui.breakpoints.clone()),
            DapEvent::Stopped {
                thread,
                reason,
                text,
            } => {
                let state = &mut self.ui.debug;
                state.paused = true;
                state.status = match text {
                    Some(text) if !text.is_empty() => format!("paused, {reason}: {text}"),
                    _ => format!("paused, {reason}"),
                };
                self.debugger.stopped(thread);
            }
            DapEvent::Continued => self.ui.debug.resume(),
            DapEvent::Output { category, text } => {
                if category != "telemetry" {
                    self.ui.debug.print(&text);
                }
            }
            DapEvent::Exited { code } => {
                let code = code.map_or_else(|| "?".to_owned(), |code| code.to_string());
                self.ui.debug.print(&format!("exited with code {code}"));
            }
            DapEvent::Terminated => self.end_debugging("the program finished"),
            DapEvent::Gone { reason } => {
                if self.debugger.is_active() {
                    let message = match reason {
                        Some(reason) => format!("the debugger stopped: {reason}"),
                        None => "the debugger stopped".to_owned(),
                    };
                    self.end_debugging(message);
                }
            }
        }
    }

    /// Acts on an answer to a background debugger request.
    pub(super) fn handle_debug_reply(&mut self, reply: DebugReply) {
        match reply {
            DebugReply::Stack(frames) => {
                self.ui.debug.frames = frames
                    .iter()
                    .map(|frame| FrameEntry {
                        name: frame.name.clone(),
                        path: frame.path.clone(),
                        line: frame.line,
                    })
                    .collect();
                self.debug_frames = frames;
                // the innermost frame with source is where the user wants to look
                let first = self
                    .debug_frames
                    .iter()
                    .position(|frame| frame.path.as_ref().is_some_and(|path| path.is_file()))
                    .unwrap_or(0);
                self.show_frame(first);
            }
            DebugReply::Variables(variables) => self.ui.debug.variables = variables,
            DebugReply::Failed(message) => {
                self.ui.debug.print(&message);
                if self.ui.debug.status.starts_with("starting") {
                    self.end_debugging(message);
                } else {
                    self.editor.set_status(message);
                }
            }
        }
    }
}
