//! Starting language servers and keeping them in sync with open documents.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use lsp_types::{
    CompletionItem as LspItem, CompletionItemKind, CompletionTextEdit, Diagnostic as LspDiagnostic,
    DiagnosticSeverity, Position, PublishDiagnosticsParams, Range as LspRange, TextEdit,
};
use mog_config::{Install, ServerConfig, install_hint};
use mog_core::{
    Change, Diagnostic, Document, Editor, InlayHint, SemanticToken, Severity, TokenKind,
};
use mog_lsp::{
    Client, LspEvent, convert,
    features::{self, CodeAction, FileEdits},
    symbols::{self, Signature, Symbol},
};
use mog_tui::{
    SymbolEntry,
    completion::{CompletionItem, ItemKind},
};
use serde_json::{Value, json};
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
    /// Inlay hints and semantic tokens for a file at a document version, each missing if the
    /// server cannot give them.
    Marks {
        /// The file they are for.
        path: PathBuf,
        /// The document version they are for.
        version: u64,
        /// The inlay hints.
        hints: Option<Vec<symbols::InlayHint>>,
        /// The semantic tokens.
        tokens: Option<Vec<symbols::SemanticToken>>,
    },
    /// The signature of the call around the cursor, for the line the cursor was on.
    Signature(Option<Signature>, usize),
    /// Symbols of the focused file, or of the whole project for `query` when it is set.
    Symbols {
        /// What the project was searched for, `None` for the symbols of one file.
        query: Option<String>,
        /// What was found.
        symbols: Vec<Symbol>,
    },
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

/// Converts a symbol from the server to a picker entry.
pub fn to_symbol_entry(symbol: Symbol) -> SymbolEntry {
    SymbolEntry {
        kind: symbols::kind_name(symbol.kind).to_owned(),
        name: symbol.name,
        detail: symbol.detail.lines().next().unwrap_or_default().to_owned(),
        depth: symbol.depth,
        path: symbol.path,
        line: usize::try_from(symbol.position.line).unwrap_or(0),
        column: usize::try_from(symbol.position.character).unwrap_or(0),
    }
}

/// Returns the identifier that ends at char `pos` of `document`, like a variable name.
fn word_before(document: &Document, pos: usize) -> String {
    let text = document.text();
    let start = (0..pos)
        .rev()
        .take_while(|&at| {
            text.get_char(at)
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        })
        .last()
        .unwrap_or(pos);
    text.slice(start..pos).to_string()
}

/// Converts server inlay hints to hints on lines of `document`.
///
/// Hints are shown at the end of the line instead of next to what they are about, so type hints
/// like `: i32` get the name they belong to in front and parameter names are left out.
pub fn to_hints(document: &Document, hints: Vec<symbols::InlayHint>) -> Vec<(usize, InlayHint)> {
    let text = document.text();
    hints
        .into_iter()
        .filter(|hint| !hint.parameter)
        .map(|hint| {
            let pos = convert::position_to_char(text, hint.position);
            let line = text.char_to_line(pos);
            let trimmed = hint.label.trim();
            let label = if trimmed.starts_with(':') {
                format!("{}{trimmed}", word_before(document, pos))
            } else {
                trimmed.to_owned()
            };
            let col = pos - text.line_to_char(line);
            (line, InlayHint { col, label })
        })
        .collect()
}

/// Returns what a token type `name` with `modifiers` means to mog, `None` for ones it ignores.
pub fn token_kind(name: &str, modifiers: &[String]) -> Option<TokenKind> {
    let has = |modifier: &str| modifiers.iter().any(|other| other == modifier);
    Some(match name {
        "namespace" => TokenKind::Namespace,
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "builtinType"
        | "typeAlias" | "trait" => TokenKind::Type,
        "function" | "method" => TokenKind::Function,
        "macro" | "decorator" | "attribute" | "derive" => TokenKind::Macro,
        "property" => TokenKind::Property,
        "enumMember" => TokenKind::EnumMember,
        "variable" if has("readonly") && has("static") => TokenKind::Constant,
        "constant" | "static" => TokenKind::Constant,
        "variable" => TokenKind::Variable,
        "parameter" => TokenKind::Parameter,
        "keyword" | "modifier" | "selfKeyword" | "builtinAttribute" => TokenKind::Keyword,
        "string" | "regexp" => TokenKind::String,
        "number" => TokenKind::Number,
        "comment" => TokenKind::Comment,
        "operator" => TokenKind::Operator,
        _ => return None,
    })
}

/// Converts server semantic tokens to tokens on lines of `document`.
pub fn to_tokens(
    document: &Document,
    tokens: Vec<symbols::SemanticToken>,
) -> Vec<(usize, SemanticToken)> {
    let text = document.text();
    tokens
        .into_iter()
        .filter_map(|token| {
            let kind = token_kind(&token.kind, &token.modifiers)?;
            let line = usize::try_from(token.line).ok()?;
            if line >= text.len_lines() {
                return None;
            }
            let start = text.line_to_char(line);
            let at = |units| convert::position_to_char(text, Position::new(token.line, units));
            let from = at(token.start) - start;
            let to = at(token.start + token.length) - start;
            (to > from).then_some((line, SemanticToken { from, to, kind }))
        })
        .collect()
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
    /// Programs that were not found, with how to install them, waiting to be offered.
    missing: Vec<(String, Install)>,
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
            missing: Vec::new(),
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

    /// Returns the server for `path` if it runs, or else any running server.
    pub fn client_for_or_any(&self, path: Option<&Path>) -> Option<Client> {
        path.and_then(|path| self.client_for(path)).or_else(|| {
            self.clients
                .values()
                .find(|client| client.is_running())
                .cloned()
        })
    }

    /// Takes a missing server program and how to install it, if one was found missing.
    pub fn take_missing(&mut self) -> Option<(String, Install)> {
        self.missing.pop()
    }

    /// Stops every server so each starts again for the next file that needs it, giving servers
    /// that failed another try.
    pub fn restart(&mut self) {
        self.clients.clear();
        self.failed.clear();
        self.synced.clear();
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
        let settings = server_settings(name, &config.settings, &self.root);
        match Client::start(
            name,
            &config.command,
            &config.args,
            &self.root,
            &settings,
            self.events.clone(),
        ) {
            Ok(client) => {
                self.clients.insert(name.to_owned(), client);
                Ok(())
            }
            Err(err) => {
                self.failed.insert(name.to_owned());
                let hint = install_hint(&config.command);
                let message = match hint {
                    Some(Install::Run(how) | Install::Manual(how))
                        if err.kind() == ErrorKind::NotFound =>
                    {
                        format!("{} is not installed: {how}", config.command)
                    }
                    _ => format!("could not start {}: {err}", config.command),
                };
                if let Some(install) = hint.filter(|_| err.kind() == ErrorKind::NotFound) {
                    self.missing.push((config.command.clone(), install));
                }
                Err(Some(message))
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
    ///
    /// Returns `false` when the exit was from an old instance that was already replaced.
    pub fn exited(&mut self, name: &str) -> bool {
        if self.clients.get(name).is_some_and(Client::is_running) {
            return false;
        }
        self.clients.remove(name);
        self.failed.insert(name.to_owned());
        if let Some(config) = self.configs.get(name).cloned() {
            self.forget_documents(&config);
        }
        true
    }

    /// Switches to `configs`, stopping servers whose settings changed so they start again with
    /// the new ones when a matching file is next synced.
    pub fn reconfigure(&mut self, configs: BTreeMap<String, ServerConfig>) {
        let names: HashSet<String> = self.configs.keys().chain(configs.keys()).cloned().collect();
        for name in names {
            let (old, new) = (self.configs.get(&name), configs.get(&name));
            if old == new {
                continue;
            }
            self.clients.remove(&name);
            self.failed.remove(&name);
            for config in [old, new]
                .into_iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
            {
                self.forget_documents(&config);
            }
        }
        self.configs = configs;
    }

    /// Forgets which documents a server with `config` was told about.
    fn forget_documents(&mut self, config: &ServerConfig) {
        self.synced.retain(|path, _| {
            let extension = path.extension().and_then(|ext| ext.to_str());
            !config
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

/// Returns the settings to start the server `name` with, filling in what it cannot run without.
///
/// The Vue server needs to know where TypeScript is, so the project's copy is used unless the
/// config already says.
fn server_settings(name: &str, settings: &Value, root: &Path) -> Value {
    if name != "vue" || !settings["typescript"]["tsdk"].is_null() {
        return settings.clone();
    }
    let tsdk = root.join("node_modules").join("typescript").join("lib");
    if !tsdk.is_dir() {
        return settings.clone();
    }
    let mut settings = if settings.is_object() {
        settings.clone()
    } else {
        json!({})
    };
    settings["typescript"] = json!({ "tsdk": tsdk.to_string_lossy() });
    settings
}

#[cfg(test)]
/// Tests for language server helpers.
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use mog_config::{Config, ServerConfig};
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    use super::{LanguageServers, exit_message, server_settings};

    /// The Vue server is pointed at the project's TypeScript, other servers are left alone.
    #[test]
    fn vue_gets_typescript() {
        let root = env::temp_dir().join(format!("mog-vue-{}", process::id()));
        let tsdk = root.join("node_modules").join("typescript").join("lib");
        fs::create_dir_all(&tsdk).expect("temp dir");
        let settings = server_settings("vue", &Value::Null, &root);
        assert_eq!(
            settings["typescript"]["tsdk"],
            json!(tsdk.to_string_lossy())
        );
        let given = json!({ "typescript": { "tsdk": "/mine" } });
        assert_eq!(server_settings("vue", &given, &root), given);
        assert_eq!(server_settings("rust", &Value::Null, &root), Value::Null);
        let _ = fs::remove_dir_all(&root);
    }

    /// Changing a server's settings makes it start over, other servers are left alone.
    #[test]
    fn reconfigure_restarts_changed_servers() {
        let (events, _events) = mpsc::unbounded_channel();
        let config = Config::default();
        let mut servers = LanguageServers::new(config.language_servers(), PathBuf::new(), events);
        servers.synced.insert(PathBuf::from("main.rs"), 1);
        servers.synced.insert(PathBuf::from("app.py"), 1);
        servers.failed.insert("rust".into());
        let mut changed = Config::default();
        changed.lsp.insert(
            "rust".into(),
            ServerConfig {
                settings: json!({ "check": { "command": "clippy" } }),
                ..Default::default()
            },
        );
        servers.reconfigure(changed.language_servers());
        assert!(!servers.synced.contains_key(&PathBuf::from("main.rs")));
        assert!(servers.synced.contains_key(&PathBuf::from("app.py")));
        assert!(!servers.failed.contains("rust"));
        assert_eq!(
            servers.configs["rust"].settings["check"]["command"],
            "clippy"
        );
    }

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
