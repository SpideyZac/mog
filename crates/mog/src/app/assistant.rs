//! The AI chat panel, ghost text and Copilot.

use std::mem;

use mog_ai::{CompletionFile, CompletionRequest, CopilotEvent, CopilotStatus};
use mog_tui::{CopilotState, Ghost, PromptKind, ghost};

use super::{App, GHOST_CONTEXT_AFTER, GHOST_CONTEXT_BEFORE, NO_AI};
use crate::ai::AiReply;

impl App {
    /// Asks the AI for a ghost suggestion at the cursor once typing pauses, or right away with
    /// several suggestions if the user `invoked` it.
    pub(super) fn suggest_ghost(&self, invoked: bool) {
        if !self.ui.config.ai.ghost_text
            || !self.assistant.can_suggest()
            || self.editor.document().is_large()
            || self
                .editor
                .document()
                .path()
                .is_some_and(|path| self.assistant.excludes(path))
        {
            return;
        }
        let document = self.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let prefix_start = head.saturating_sub(GHOST_CONTEXT_BEFORE);
        let suffix_end = (head + GHOST_CONTEXT_AFTER).min(text.len_chars());
        let request = CompletionRequest {
            prefix: text.slice(prefix_start..head).to_string(),
            suffix: text.slice(head..suffix_end).to_string(),
            language: document
                .path()
                .and_then(|path| path.extension())
                .map(|ext| ext.to_string_lossy().into_owned()),
            file: Some(CompletionFile {
                path: document.path().map(ToOwned::to_owned),
                index: self.editor.active(),
                text: text.clone(),
                version: document.version(),
                cursor: head,
                tab_size: self.editor.options().tab_width,
                insert_spaces: self.editor.options().insert_spaces,
            }),
            invoked,
        };
        self.assistant
            .suggest(request, self.editor.active(), document.version(), head);
    }

    /// Sends what was typed in the chat panel.
    pub(super) fn send_chat(&mut self) {
        let text = mem::take(&mut self.ui.chat.input);
        if text.trim().is_empty() || self.ui.chat.waiting {
            self.ui.chat.input = text;
            return;
        }
        self.ui.chat.messages.push((true, text));
        self.ui.chat.scroll = 0;
        let tools = self.plugin_chat_tools();
        if self.assistant.chat(&self.ui.chat.messages, tools) {
            self.ui.chat.waiting = true;
        } else {
            self.ui.chat.messages.push((false, NO_AI.to_owned()));
        }
    }

    /// Acts on a finished AI request.
    pub(super) fn handle_ai_reply(&mut self, reply: AiReply) {
        match reply {
            AiReply::ChatText(text) => {
                if mem::replace(&mut self.chat_streaming, true) {
                    if let Some((_, reply)) = self.ui.chat.messages.last_mut() {
                        reply.push_str(&text);
                    }
                } else {
                    self.ui.chat.messages.push((false, text));
                }
            }
            AiReply::Chat(text) => {
                self.ui.chat.waiting = false;
                if mem::take(&mut self.chat_streaming) {
                    self.ui.chat.messages.pop();
                }
                self.ui.chat.messages.push((false, text));
            }
            AiReply::ChatFailed(reason) => {
                self.ui.chat.waiting = false;
                let failed = format!("that failed: {reason}");
                match self.ui.chat.messages.last_mut() {
                    Some((_, reply)) if mem::take(&mut self.chat_streaming) => {
                        reply.push_str(&format!("\n\n({failed})"));
                    }
                    _ => self.ui.chat.messages.push((false, failed)),
                }
            }
            AiReply::Ghost {
                document,
                version,
                pos,
                items,
                more: true,
            } => {
                let shown = self.ui.ghost.as_mut().filter(|ghost| {
                    ghost.document == document && ghost.version == version && ghost.pos == pos
                });
                if let Some(ghost) = shown
                    && !ghost::add_more(ghost, items)
                {
                    self.editor.set_status("no other suggestions");
                }
            }
            AiReply::Ghost {
                document,
                version,
                pos,
                items,
                more: false,
            } => {
                let ghost = Ghost::new(document, version, pos, items)
                    .filter(|ghost| ghost.is_fresh(&self.editor));
                if ghost.is_some() {
                    self.ui.ghost = ghost;
                }
            }
            AiReply::Copilot(event) => self.handle_copilot_event(event),
            AiReply::CopilotCode(code) => {
                self.editor.copy_text(code.user_code.clone());
                self.ui.ask(
                    PromptKind::CopilotSignIn,
                    "sign in to copilot",
                    code.user_code.clone(),
                    format!(
                        "copied, enter opens {} to paste it",
                        code.uri.trim_start_matches("https://")
                    ),
                );
                self.copilot_code = Some(code);
            }
            AiReply::CopilotDone(message, status) => {
                if let Some(status) = status {
                    self.set_copilot_status(&status);
                }
                self.editor.set_status(message);
            }
        }
    }

    /// Acts on something the Copilot server reported.
    pub(super) fn handle_copilot_event(&mut self, event: CopilotEvent) {
        match event {
            CopilotEvent::Status(status) => self.set_copilot_status(&status),
            CopilotEvent::ShowDocument { uri, external } => self.show_document(&uri, external),
            CopilotEvent::Message(text) => self.editor.set_status(format!("copilot: {text}")),
            CopilotEvent::Exited(reason) => {
                let reason = reason.unwrap_or_else(|| "no reason given".into());
                self.editor.set_status(format!("copilot stopped: {reason}"));
                self.ui.copilot = Some(CopilotState::Problem);
            }
        }
    }

    /// Shows the Copilot `status` in the status line, explaining it when it gets worse.
    pub(super) fn set_copilot_status(&mut self, status: &CopilotStatus) {
        let state = match status {
            CopilotStatus::Starting => CopilotState::Starting,
            CopilotStatus::Ready => CopilotState::Ready,
            CopilotStatus::SignedOut => CopilotState::SignedOut,
            CopilotStatus::Problem(_) => CopilotState::Problem,
        };
        if self.ui.copilot != Some(state) {
            match status {
                CopilotStatus::SignedOut => self
                    .editor
                    .set_status("copilot is signed out, run Copilot: Sign in from the palette"),
                CopilotStatus::Problem(problem) => {
                    self.editor.set_status(format!("copilot: {problem}"));
                }
                CopilotStatus::Starting | CopilotStatus::Ready => {}
            }
        }
        self.ui.copilot = Some(state);
    }
}
