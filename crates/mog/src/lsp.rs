//! Starting language servers and keeping them in sync with open documents.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use lsp_types::{DiagnosticSeverity, PublishDiagnosticsParams};
use mog_config::ServerConfig;
use mog_core::{Diagnostic, Document, Editor, Severity};
use mog_lsp::{Client, LspEvent, convert};
use tokio::sync::mpsc::UnboundedSender;

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

    /// Opens every document that is not known to its server yet and sends changes for the rest.
    ///
    /// Returns a message for each server that could not be started.
    pub fn sync(&mut self, documents: &[Document]) -> Vec<String> {
        let mut problems = Vec::new();
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
