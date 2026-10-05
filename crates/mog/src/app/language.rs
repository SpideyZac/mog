//! Language server features: completion, hover, code actions, symbols and the rest.

use std::{io, time::Instant};

use futures::future;
use lsp_types::{Position as LspPosition, Range as LspRange};
use mog_config::Install;
use mog_core::{Command, Document, Range, Transaction};
use mog_lsp::{LspEvent, convert, features::FileEdits};
use mog_tui::{
    Overlay, PromptKind, SignatureHint,
    completion::{self, CompletionState},
    menu::{self, MenuAction, MenuItem},
};
use ratatui::layout::Position;
use serde_json::json;

use super::{App, PROVIDER_TIMEOUT, plugin_host};
use crate::lsp::{self, LspReply};

impl App {
    /// Opens or closes the completion menu after `ch` was typed.
    pub(super) fn auto_complete(&mut self, ch: char) {
        if !self.ui.config.editor.auto_complete {
            return;
        }
        let word = ch.is_alphanumeric() || ch == '_';
        if word {
            if self.ui.completion.is_none() {
                self.request_completion(false);
            }
        } else if matches!(ch, '.' | ':') {
            self.ui.completion = None;
            self.request_completion(false);
        } else {
            self.ui.completion = None;
        }
    }

    /// Asks the language server and plugins for completions at the cursor.
    pub(super) fn request_completion(&mut self, manual: bool) {
        self.sync_language_servers();
        let document = self.editor.document();
        let path = document.path().map(ToOwned::to_owned);
        let client = path.as_deref().and_then(|path| self.lsp.client_for(path));
        let head = document.selection().head;
        let anchor = completion::word_start(&self.editor, head);
        let text = document.text();
        let line = text.char_to_line(head);
        let params = json!({
            "path": path.as_ref().map(|path| path.to_string_lossy()),
            "language": plugin_host::language(path.as_deref()),
            "version": document.version(),
            "offset": head,
            "line": line,
            "column": head - text.line_to_char(line),
            "prefix": text.slice(anchor..head).to_string(),
            "manual": manual,
        });
        let plugins = self.ask_plugins("completion", params, PROVIDER_TIMEOUT);
        if client.is_none() && plugins.is_none() {
            if manual {
                self.editor.set_status("no language server for this file");
            }
            return;
        }
        let position = convert::char_to_position(text, head);
        self.completion_request += 1;
        let request = self.completion_request;
        let index = self.editor.active();
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let from_server = async {
                match (client, path) {
                    (Some(client), Some(path)) => client
                        .completion(&path, position)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(lsp::to_item)
                        .collect(),
                    _ => Vec::new(),
                }
            };
            let from_plugins = async {
                match plugins {
                    Some(plugins) => plugins.await,
                    None => Vec::new(),
                }
            };
            let (mut items, answers): (Vec<_>, _) = future::join(from_server, from_plugins).await;
            for (_, answer) in answers {
                items.extend(plugin_host::completion_items(&answer));
            }
            let _ = sender.send(LspReply::Completion {
                request,
                document: index,
                anchor,
                items,
            });
        });
    }

    /// Sends a hover, definition, references, code action or format request for the focused
    /// file, to its language server and to plugins that provide the feature.
    pub(super) fn request_feature(&mut self, feature: &str) {
        self.sync_language_servers();
        let document = self.editor.document();
        let path = document.path().map(ToOwned::to_owned);
        let client = path.as_deref().and_then(|path| self.lsp.client_for(path));
        let head = document.selection().head;
        let version = document.version();
        let text = document.text();
        let line = text.char_to_line(head);
        let options = self.editor.options();
        let (tab_size, spaces) = (
            u32::try_from(options.tab_width).unwrap_or(4),
            options.insert_spaces,
        );
        let provider = match feature {
            "hover" => Some("hover"),
            "actions" => Some("code_actions"),
            "format" => Some("formatting"),
            _ => None,
        };
        let plugins = provider.and_then(|provider| {
            let selection = document.selection();
            let whole = provider == "formatting" && !document.is_large();
            let params = json!({
                "path": path.as_ref().map(|path| path.to_string_lossy()),
                "language": plugin_host::language(path.as_deref()),
                "version": version,
                "offset": head,
                "line": line,
                "column": head - text.line_to_char(line),
                "selection": { "anchor": selection.anchor, "head": selection.head },
                "text": whole.then(|| text.to_string()),
                "tab_size": tab_size,
                "insert_spaces": spaces,
            });
            self.ask_plugins(provider, params, PROVIDER_TIMEOUT)
        });
        if client.is_none() && plugins.is_none() {
            self.editor.set_status(if path.is_none() {
                "save the file first so a language server can see it"
            } else {
                "no language server for this file"
            });
            return;
        }
        let position = convert::char_to_position(text, head);
        let diagnostics = lsp::diagnostics_on_line(document, line);
        let sender = self.lsp_sender.clone();
        let feature = feature.to_owned();
        tokio::spawn(async move {
            let answers = match plugins {
                Some(plugins) => plugins.await,
                None => Vec::new(),
            };
            let reply = match (feature.as_str(), client, path) {
                ("hover", client, path) => {
                    let mut texts: Vec<String> = Vec::new();
                    if let (Some(client), Some(path)) = (client, path)
                        && let Ok(Some(text)) = client.hover(&path, position).await
                    {
                        texts.push(text);
                    }
                    texts.extend(
                        answers
                            .iter()
                            .filter_map(|(_, answer)| answer["text"].as_str().map(str::to_owned))
                            .filter(|text| !text.is_empty()),
                    );
                    if texts.is_empty() {
                        LspReply::Nothing("nothing to say about that".into())
                    } else {
                        LspReply::Hover(texts.join("\n\n"), head)
                    }
                }
                ("definition", Some(client), Some(path)) => {
                    match client.definition(&path, position).await {
                        Ok(Some((target, at))) => LspReply::Definition(target, at),
                        _ => LspReply::Nothing("no definition found".into()),
                    }
                }
                ("references", Some(client), Some(path)) => {
                    match client.references(&path, position).await {
                        Ok(found) if !found.is_empty() => LspReply::References(found),
                        _ => LspReply::Nothing("no references found".into()),
                    }
                }
                ("actions", client, path) => {
                    let mut actions = Vec::new();
                    if let (Some(client), Some(path)) = (client, path) {
                        let range = LspRange::new(position, position);
                        actions = client
                            .code_actions(&path, range, diagnostics)
                            .await
                            .unwrap_or_default();
                    }
                    let offered = plugin_host::code_actions(answers);
                    if actions.is_empty() && offered.is_empty() {
                        LspReply::Nothing("no code actions here".into())
                    } else {
                        LspReply::Actions(actions, offered)
                    }
                }
                ("format", client, path) => {
                    // a formatter plugin was installed on purpose, so it goes first
                    if let Some((plugin, changes)) = plugin_host::format_changes(&answers) {
                        LspReply::Changes {
                            version,
                            changes,
                            what: format!("formatted by {plugin}"),
                        }
                    } else if let (Some(client), Some(path)) = (client, path) {
                        match client.formatting(&path, tab_size, spaces).await {
                            Ok(edits) => LspReply::Format(path, version, edits),
                            Err(err) => LspReply::Nothing(format!("could not format: {err}")),
                        }
                    } else {
                        LspReply::Nothing("no formatter answered".into())
                    }
                }
                _ => LspReply::Nothing("no language server for this file".into()),
            };
            let _ = sender.send(reply);
        });
    }

    /// Asks for inlay hints and semantic tokens of the focused file, once per version.
    pub(super) fn request_marks(&mut self) {
        let settings = &self.ui.config.ui;
        let want_hints = settings.inlay_hints;
        let want_tokens = settings.semantic_highlighting && settings.syntax_highlighting;
        if !want_hints && !want_tokens {
            return;
        }
        let document = self.editor.document();
        let Some(path) = document
            .path()
            .filter(|_| !document.is_large())
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let key = (path.clone(), document.version());
        if self.marks_requested.as_ref() == Some(&key) {
            return;
        }
        // until the handshake is done the server has not said what it can do
        let Some(client) = self
            .lsp
            .client_for(&path)
            .filter(|client| client.capabilities().is_some())
        else {
            return;
        };
        self.marks_requested = Some(key);
        let hints = want_hints && client.supports("inlayHintProvider");
        let tokens = want_tokens && client.supports("semanticTokensProvider");
        if !hints && !tokens {
            return;
        }
        let text = document.text();
        let end = convert::char_to_position(text, text.len_chars());
        let range = LspRange::new(LspPosition::new(0, 0), end);
        let version = document.version();
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let hints = if hints {
                client.inlay_hints(&path, range).await.ok()
            } else {
                None
            };
            let tokens = if tokens {
                client.semantic_tokens(&path).await.ok()
            } else {
                None
            };
            let _ = sender.send(LspReply::Marks {
                path,
                version,
                hints,
                tokens,
            });
        });
    }

    /// Asks for the signature of the call around the cursor.
    pub(super) fn request_signature(&mut self) {
        self.sync_language_servers();
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            return;
        };
        let Some(client) = self.lsp.client_for(&path) else {
            return;
        };
        let head = document.selection().head;
        let line = document.text().char_to_line(head);
        let position = convert::char_to_position(document.text(), head);
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let signature = client.signature_help(&path, position).await.ok().flatten();
            let _ = sender.send(LspReply::Signature(signature, line));
        });
    }

    /// Hides the signature once the cursor leaves the line of the call.
    pub(super) fn sync_signature(&mut self) {
        let document = self.editor.document();
        let line = document.text().char_to_line(document.selection().head);
        if self
            .ui
            .signature
            .as_ref()
            .is_some_and(|signature| signature.line != line)
        {
            self.ui.signature = None;
        }
    }

    /// Asks for the symbols of the focused file, or of the whole project matching `query`.
    pub(super) fn request_symbols(&mut self, query: Option<String>) {
        self.sync_language_servers();
        let path = self.editor.document().path().map(ToOwned::to_owned);
        if query.is_none() && path.is_none() {
            self.editor
                .set_status("save the file first so a language server can see it");
            return;
        }
        let Some(client) = self.lsp.client_for_or_any(path.as_deref()) else {
            self.editor.set_status("no language server is running");
            return;
        };
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let symbols = match (&query, &path) {
                (Some(query), _) => client.workspace_symbols(query).await,
                (None, Some(path)) => client.document_symbols(path).await,
                (None, None) => Ok(Vec::new()),
            };
            let reply = match symbols {
                Ok(symbols) => LspReply::Symbols { query, symbols },
                Err(err) => LspReply::Nothing(format!("could not get symbols: {err}")),
            };
            let _ = sender.send(reply);
        });
    }

    /// Offers to install a language server that was not found, if one was and nothing else is
    /// asking.
    pub(super) fn offer_install(&mut self) {
        if self.ui.overlay.is_some() {
            return;
        }
        let Some((program, install)) = self.lsp.take_missing() else {
            return;
        };
        match install {
            Install::Run(command) => self.ui.ask(
                PromptKind::InstallServer(command.to_owned()),
                format!("install {program}?"),
                "",
                format!("y runs `{command}` in the terminal"),
            ),
            Install::Manual(how) => self
                .editor
                .set_status(format!("{program} is not installed: {how}")),
        }
    }

    /// Asks the language server to rename the symbol at the cursor to `new_name`.
    pub(super) fn request_rename(&mut self, new_name: String) {
        self.sync_language_servers();
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            return;
        };
        let Some(client) = self.lsp.client_for(&path) else {
            self.editor.set_status("no language server for this file");
            return;
        };
        let position = convert::char_to_position(document.text(), document.selection().head);
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let reply = match client.rename(&path, position, &new_name).await {
                Ok(files) if !files.is_empty() => LspReply::Rename(files),
                Ok(_) => LspReply::Nothing("nothing to rename here".into()),
                Err(err) => LspReply::Nothing(format!("could not rename: {err}")),
            };
            let _ = sender.send(reply);
        });
    }

    /// Applies rename edits to open documents and writes the rest straight to disk.
    pub(super) fn apply_rename(&mut self, files: FileEdits) {
        let focused = self.editor.active();
        let mut count = 0;
        for (path, edits) in files {
            count += edits.len();
            let open = self
                .editor
                .documents()
                .iter()
                .position(|document| document.path() == Some(path.as_path()));
            if let Some(index) = open {
                self.editor.focus(index);
                let changes = lsp::to_changes(self.editor.document(), &edits);
                self.editor.apply_changes(changes);
                continue;
            }
            let result = Document::open(&path).and_then(|mut document| {
                let changes = lsp::to_changes(&document, &edits);
                // a server can send edits that overlap, which must not take mog down
                let tx = Transaction::try_new(changes)
                    .and_then(|tx| tx.check_bounds(document.text().len_chars()).map(|()| tx))
                    .map_err(io::Error::other)?;
                document.apply(tx, Range::point(0), false);
                document.save()
            });
            if let Err(err) = result {
                self.editor
                    .set_status(format!("could not edit {}: {err}", path.display()));
            }
        }
        self.editor.focus(focused);
        self.editor.set_status(format!("renamed in {count} places"));
    }

    /// Acts on an answer to a feature request.
    pub(super) fn handle_lsp_reply(&mut self, reply: LspReply) {
        match reply {
            LspReply::Completion {
                request,
                document,
                anchor,
                items,
            } => {
                if request != self.completion_request || document != self.editor.active() {
                    return;
                }
                self.ui.completion =
                    (!items.is_empty()).then(|| CompletionState::new(items, anchor, document));
            }
            LspReply::Hover(text, pos) => self.ui.hover = Some((text, pos)),
            LspReply::Definition(path, position) => {
                if let Err(err) = self.editor.open(&path) {
                    self.editor
                        .set_status(format!("could not open {}: {err}", path.display()));
                    return;
                }
                let pos = convert::position_to_char(self.editor.document().text(), position);
                self.editor.select(pos, pos);
            }
            LspReply::Format(path, version, edits) => {
                let document = self.editor.document();
                if document.path() != Some(path.as_path()) || document.version() != version {
                    return;
                }
                let changes = lsp::to_changes(document, &edits);
                let count = changes.len();
                self.editor.apply_changes(changes);
                self.editor.set_status(if count == 0 {
                    "already formatted, mog approves".to_owned()
                } else {
                    format!("formatted ({count} edits)")
                });
            }
            LspReply::Rename(files) => self.apply_rename(files),
            LspReply::References(found) => {
                self.ui.references = found
                    .into_iter()
                    .map(|(path, position)| {
                        let line = usize::try_from(position.line).unwrap_or(0);
                        let column = usize::try_from(position.character).unwrap_or(0);
                        let preview = lsp::line_preview(&self.editor, &path, line);
                        (path, line, column, preview)
                    })
                    .collect();
                self.ui.open(Overlay::References);
            }
            LspReply::Actions(actions, offered) => {
                let from_server = actions.iter().enumerate().map(|(i, (title, _))| MenuItem {
                    label: title.clone(),
                    keys: String::new(),
                    action: Some(MenuAction::Run(Command::Custom(format!("lsp.action.{i}")))),
                });
                let from_plugins = offered
                    .iter()
                    .enumerate()
                    .map(|(i, (title, _, _))| MenuItem {
                        label: title.clone(),
                        keys: String::new(),
                        action: Some(MenuAction::Run(Command::Custom(format!(
                            "plugins.action.{i}"
                        )))),
                    });
                let items = from_server.chain(from_plugins).collect();
                self.code_actions = actions;
                self.plugin_state.code_actions = offered
                    .into_iter()
                    .map(|(_, plugin, actions)| (plugin, actions))
                    .collect();
                let at = self.ui.cursor_screen.unwrap_or_default();
                menu::open_menu(&mut self.ui, Position::new(at.x, at.y + 1), items);
            }
            LspReply::Marks {
                path,
                version,
                hints,
                tokens,
            } => {
                let Some(document) = self
                    .editor
                    .documents_mut()
                    .iter_mut()
                    .find(|document| document.path() == Some(path.as_path()))
                else {
                    return;
                };
                // marks for an older version would land on the wrong text
                if document.version() != version {
                    return;
                }
                if let Some(hints) = hints {
                    let hints = lsp::to_hints(document, hints);
                    document.set_inlay_hints(hints);
                }
                if let Some(tokens) = tokens {
                    let tokens = lsp::to_tokens(document, tokens);
                    document.set_semantic_tokens(tokens);
                }
            }
            LspReply::Signature(signature, line) => {
                let cursor = self.editor.document();
                let cursor_line = cursor.text().char_to_line(cursor.selection().head);
                self.ui.signature =
                    signature
                        .filter(|_| line == cursor_line)
                        .map(|found| SignatureHint {
                            label: found.label,
                            active: found.active,
                            documentation: found.documentation,
                            line,
                        });
            }
            LspReply::Symbols { query, symbols } => {
                let workspace = query.is_some();
                // answers to an older query are stale once more was typed
                if query.is_some_and(|query| query != self.ui.symbol_query) {
                    return;
                }
                if !workspace && symbols.is_empty() {
                    self.editor.set_status("no symbols in this file");
                    return;
                }
                self.ui.symbols = symbols.into_iter().map(lsp::to_symbol_entry).collect();
                self.ui.symbols_version += 1;
                let overlay = if workspace {
                    Overlay::WorkspaceSymbols
                } else {
                    Overlay::Symbols
                };
                if self.ui.overlay != Some(overlay) {
                    self.ui.open(overlay);
                }
            }
            LspReply::Changes {
                version,
                changes,
                what,
            } => {
                if self.editor.document().version() != version {
                    return;
                }
                match self.editor.try_apply_changes(changes) {
                    Ok(()) => self.editor.set_status(what),
                    Err(err) => self.editor.set_status(format!("{what} failed: {err}")),
                }
            }
            LspReply::Nothing(message) => self.editor.set_status(message),
        }
    }

    /// Tells language servers about opened and changed documents.
    pub(super) fn sync_language_servers(&mut self) {
        let problems = self.lsp.sync(self.editor.documents());
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
    }

    /// Reacts to an event from a language server.
    pub(super) fn handle_lsp_event(&mut self, event: LspEvent) {
        match event {
            LspEvent::Ready { .. } => {}
            LspEvent::Diagnostics { params, .. } => {
                if Instant::now() < self.idle_at() {
                    // keep only the newest set for each file
                    self.pending_diagnostics
                        .retain(|pending| pending.uri != params.uri);
                    self.pending_diagnostics.push(params);
                } else {
                    lsp::apply_diagnostics(&mut self.editor, params);
                }
            }
            LspEvent::Message { server, text } => {
                self.editor.set_status(format!("{server}: {text}"));
            }
            LspEvent::ShowDocument { uri, external, .. } => self.show_document(&uri, external),
            LspEvent::Notification { method, .. }
                if matches!(
                    method.as_str(),
                    "workspace/semanticTokens/refresh" | "workspace/inlayHint/refresh"
                ) =>
            {
                self.marks_requested = None;
            }
            LspEvent::Notification { .. } => {}
            LspEvent::Exited { server, reason } => {
                if self.lsp.exited(&server) {
                    self.editor
                        .set_status(lsp::exit_message(&server, reason.as_deref()));
                }
            }
        }
    }

    /// Shows a document a server asked for, in the browser if it is not a local file.
    pub(super) fn show_document(&mut self, uri: &str, external: bool) {
        let path = uri.parse().ok().and_then(|uri| convert::uri_to_path(&uri));
        match path {
            Some(path) if !external => {
                if let Err(err) = self.editor.open(&path) {
                    self.editor
                        .set_status(format!("could not open {}: {err}", path.display()));
                }
            }
            _ => {
                if let Err(err) = open::that_detached(uri) {
                    self.editor
                        .set_status(format!("could not open {uri}: {err}"));
                }
            }
        }
    }
}
