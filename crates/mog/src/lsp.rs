//! Starting language servers and keeping them in sync with open documents.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use lsp_types::{
    CompletionItem as LspItem, CompletionItemKind, CompletionTextEdit, Diagnostic as LspDiagnostic,
    DiagnosticSeverity, Position, PublishDiagnosticsParams, Range as LspRange, TextEdit,
};
use mog_config::ServerConfig;
use mog_core::{Change, Diagnostic, Document, Editor, Severity};
use mog_lsp::{
    Client, LspEvent, convert,
    features::{self, CodeAction, FileEdits},
};
use mog_tui::completion::{CompletionItem, ItemKind};
use tokio::sync::mpsc::UnboundedSender;

/// An answer to a feature request.
#[derive(Debug)]
pub enum LspReply {
    /// Completions for the word starting at `anchor`.
    Completion {
        /// Which request this answers, so stale ones can be dropped.
        request: u64,
        /// The document index the request was for.
        document: usize,
        /// Where the word being completed starts.
        anchor: usize,
        /// The completions.
        items: Vec<CompletionItem>,
    },
    /// What is under the cursor, as text, for the char offset `pos`.
    Hover(String, usize),
    /// Where a symbol is defined.
    Definition(PathBuf, Position),
    /// Edits that format a file at a given document version.
    Format(PathBuf, u64, Vec<TextEdit>),
    /// Edits across files that rename a symbol.
    Rename(FileEdits),
    /// Places a symbol is used.
    References(Vec<(PathBuf, Position)>),
    /// Code actions as titles with their edits.
    Actions(Vec<CodeAction>),
    /// A request found nothing or failed, with a message for the status line.
    Nothing(String),
}

/// Converts a server completion to a menu item.
pub fn to_item(item: LspItem) -> CompletionItem {
    let kind = match item.kind {
        Some(
            CompletionItemKind::FUNCTION
            | CompletionItemKind::METHOD
            | CompletionItemKind::CONSTRUCTOR,
        ) => ItemKind::Function,
        Some(CompletionItemKind::VARIABLE | CompletionItemKind::VALUE) => ItemKind::Variable,
        Some(CompletionItemKind::FIELD | CompletionItemKind::PROPERTY) => ItemKind::Field,
        Some(
            CompletionItemKind::CLASS
            | CompletionItemKind::STRUCT
            | CompletionItemKind::INTERFACE
            | CompletionItemKind::ENUM
            | CompletionItemKind::TYPE_PARAMETER,
        ) => ItemKind::Type,
        Some(CompletionItemKind::MODULE) => ItemKind::Module,
        Some(CompletionItemKind::KEYWORD) => ItemKind::Keyword,
        Some(CompletionItemKind::CONSTANT | CompletionItemKind::ENUM_MEMBER) => ItemKind::Constant,
        _ => ItemKind::Other,
    };
    let insert = match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => edit.new_text.clone(),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => edit.new_text.clone(),
        None => item
            .insert_text
            .clone()
            .unwrap_or_else(|| item.label.clone()),
    };
    let detail = item
        .detail
        .clone()
        .or_else(|| {
            item.documentation
                .as_ref()
                .map(|doc| features::documentation_text(doc).to_owned())
        })
        .unwrap_or_default();
    CompletionItem {
        filter: item.filter_text.unwrap_or_else(|| item.label.clone()),
        label: item.label,
        detail,
        kind,
        insert,
    }
}

/// Converts server text edits to changes on `document`.
pub fn to_changes(document: &Document, edits: &[TextEdit]) -> Vec<Change> {
    let text = document.text();
    edits
        .iter()
        .map(|edit| Change {
            start: convert::position_to_char(text, edit.range.start),
            end: convert::position_to_char(text, edit.range.end),
            text: edit.new_text.clone(),
        })
        .collect()
}

/// The language servers for the current project.
pub struct LanguageServers {
    /// The configured servers by name.
    configs: BTreeMap<String, ServerConfig>,
    /// The running servers by name.
    clients: HashMap<String, Client>,
    /// Servers that failed to start, so they are not retried on every file.
    failed: HashSet<String>,
    /// The last document version each server was told about, by path.
    synced: HashMap<PathBuf, u64>,
    /// Where server events go.
    events: UnboundedSender<LspEvent>,
    /// The project root servers are started in.
    root: PathBuf,
}

impl LanguageServers {
    /// Creates the manager. No server starts until a matching document is opened.
    pub fn new(
        configs: BTreeMap<String, ServerConfig>,
        root: PathBuf,
        events: UnboundedSender<LspEvent>,
    ) -> Self {
        Self {
            configs,
            clients: HashMap::new(),
            failed: HashSet::new(),
            synced: HashMap::new(),
            events,
            root,
        }
    }

    /// Returns the name of the server that handles `path`.
    fn server_for(&self, path: &Path) -> Option<&str> {
        let extension = path.extension()?.to_str()?;
        self.configs
            .iter()
            .find(|(_, config)| config.extensions.iter().any(|ext| ext == extension))
            .map(|(name, _)| name.as_str())
    }

    /// Returns the running server for `path`, if there is one.
    pub fn client_for(&self, path: &Path) -> Option<Client> {
        self.server_for(path)
            .and_then(|name| self.clients.get(name))
            .cloned()
    }

    /// Opens every document that is not known to its server yet and sends changes for the rest.
    ///
    /// Returns a message for each server that could not be started.
    pub fn sync(&mut self, documents: &[Document]) -> Vec<String> {
        let mut problems = Vec::new();
        let closed: Vec<PathBuf> = self
            .synced
            .keys()
            .filter(|path| !documents.iter().any(|d| d.path() == Some(path.as_path())))
            .cloned()
            .collect();
        for path in closed {
            self.synced.remove(&path);
            if let Some(client) = self.client_for(&path) {
                client.did_close(&path);
            }
        }
        for document in documents {
            let Some(path) = document.path() else {
                continue;
            };
            let Some(name) = self.server_for(path).map(str::to_owned) else {
                continue;
            };
            if self.synced.get(path) == Some(&document.version()) {
                continue;
            }
            if let Err(problem) = self.ensure_started(&name) {
                problems.extend(problem);
                continue;
            }
            let Some(client) = self.clients.get(&name) else {
                continue;
            };
            let version = i32::try_from(document.version()).unwrap_or(i32::MAX);
            let text = document.text().to_string();
            if self.synced.contains_key(path) {
                client.did_change(path, version, &text);
            } else {
                let language_id = self.configs[&name].language_id.as_deref().unwrap_or(&name);
                client.did_open(path, language_id, version, &text);
            }
            self.synced.insert(path.to_owned(), document.version());
        }
        problems
    }

    /// Starts the server called `name` unless it is already running.
    ///
    /// Fails with a message the first time a server cannot start and silently after that.
    fn ensure_started(&mut self, name: &str) -> Result<(), Option<String>> {
        if self.clients.contains_key(name) {
            return Ok(());
        }
        if self.failed.contains(name) {
            return Err(None);
        }
        let config = &self.configs[name];
        match Client::start(
            name,
            &config.command,
            &config.args,
            &self.root,
            self.events.clone(),
        ) {
            Ok(client) => {
                self.clients.insert(name.to_owned(), client);
                Ok(())
            }
            Err(err) => {
                self.failed.insert(name.to_owned());
                Err(Some(format!("could not start {}: {err}", config.command)))
            }
        }
    }

    /// Tells the server for `path` that it was saved, so checks that run on save start.
    pub fn saved(&self, path: &Path) {
        if self.synced.contains_key(path)
            && let Some(client) = self.client_for(path)
        {
            client.did_save(path);
        }
    }

    /// Forgets a server that exited so its documents are opened again if it is restarted.
    pub fn exited(&mut self, name: &str) {
        self.clients.remove(name);
        self.failed.insert(name.to_owned());
        let configs = &self.configs;
        self.synced.retain(|path, _| {
            let extension = path.extension().and_then(|ext| ext.to_str());
            !configs[name]
                .extensions
                .iter()
                .any(|ext| Some(ext.as_str()) == extension)
        });
    }
}

/// Stores published diagnostics on the matching open document.
pub fn apply_diagnostics(editor: &mut Editor, params: PublishDiagnosticsParams) {
    let Some(path) = convert::uri_to_path(&params.uri) else {
        return;
    };
    let Some(document) = editor
        .documents_mut()
        .iter_mut()
        .find(|document| document.path().is_some_and(|other| same_path(other, &path)))
    else {
        return;
    };
    let text = document.text();
    let diagnostics = params
        .diagnostics
        .into_iter()
        .map(|diagnostic| Diagnostic {
            from: convert::position_to_char(text, diagnostic.range.start),
            to: convert::position_to_char(text, diagnostic.range.end),
            severity: match diagnostic.severity {
                Some(DiagnosticSeverity::WARNING) => Severity::Warning,
                Some(DiagnosticSeverity::INFORMATION) => Severity::Info,
                Some(DiagnosticSeverity::HINT) => Severity::Hint,
                _ => Severity::Error,
            },
            message: diagnostic.message,
        })
        .collect();
    document.set_diagnostics(diagnostics);
}

/// Compares two paths the way the file system does, ignoring case on Windows.
fn same_path(a: &Path, b: &Path) -> bool {
    if !cfg!(windows) {
        return a == b;
    }
    let parts = |path: &Path| -> Vec<String> {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    parts(a) == parts(b)
}

/// Returns the diagnostics of `document` on `line` in protocol form, for code action requests.
pub fn diagnostics_on_line(document: &Document, line: usize) -> Vec<LspDiagnostic> {
    let text = document.text();
    document
        .diagnostics()
        .iter()
        .filter(|d| text.char_to_line(d.from.min(text.len_chars())) == line)
        .map(|d| LspDiagnostic {
            range: LspRange::new(
                convert::char_to_position(text, d.from),
                convert::char_to_position(text, d.to),
            ),
            severity: Some(match d.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
                Severity::Info => DiagnosticSeverity::INFORMATION,
                Severity::Hint => DiagnosticSeverity::HINT,
            }),
            message: d.message.clone(),
            ..LspDiagnostic::default()
        })
        .collect()
}

/// Returns line `line` of `path` for previews, from the open document if there is one.
pub fn line_preview(editor: &Editor, path: &Path, line: usize) -> String {
    let open = editor
        .documents()
        .iter()
        .find(|document| document.path().is_some_and(|other| same_path(other, path)));
    if let Some(document) = open {
        let text = document.text();
        if line < text.len_lines() {
            return text.line(line).to_string().trim_end().to_owned();
        }
        return String::new();
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().nth(line).map(str::to_owned))
        .unwrap_or_default()
}

/// Returns the status message for a server called `server` that stopped, saying why if it can.
pub fn exit_message(server: &str, reason: Option<&str>) -> String {
    let Some(reason) = reason else {
        return format!("{server} language server stopped");
    };
    // rustup installs a proxy for rust-analyzer even when the component is missing
    if reason.contains("Unknown binary") && reason.contains("rust-analyzer") {
        return "rust-analyzer is not installed, run: rustup component add rust-analyzer".into();
    }
    format!("{server} language server stopped: {reason}")
}

#[cfg(test)]
/// Tests for language server helpers.
mod tests {
    use super::exit_message;

    /// A missing rustup component gets a hint on how to install it.
    #[test]
    fn exit_message_hints_at_rustup() {
        assert_eq!(exit_message("rust", None), "rust language server stopped");
        let reason = "error: Unknown binary 'rust-analyzer.exe' in official toolchain";
        assert!(exit_message("rust", Some(reason)).ends_with("rustup component add rust-analyzer"));
        assert_eq!(
            exit_message("go", Some("boom")),
            "go language server stopped: boom"
        );
    }
}
