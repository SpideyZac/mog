//! A connection to one language server process.

use std::{
    collections::HashMap,
    env,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    process::{self, Stdio},
    sync::{Arc, OnceLock},
    time::Duration,
};

use lsp_types::{
    ClientCapabilities, ClientInfo, CodeActionClientCapabilities, CodeActionKindLiteralSupport,
    CodeActionLiteralSupport, CompletionClientCapabilities, CompletionItemCapability,
    GotoCapability, HoverClientCapabilities, InitializeParams, MarkupKind,
    PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams, ShowDocumentClientCapabilities,
    TextDocumentClientCapabilities, TextDocumentSyncClientCapabilities, WindowClientCapabilities,
    WorkspaceClientCapabilities, WorkspaceFolder,
};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader, BufWriter},
    process::Command,
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
    time,
};

use crate::{
    convert,
    transport::{self, Message},
};

/// The id of the initialize request, always the first request sent.
const INITIALIZE_ID: u64 = 0;

/// How long a server gets to exit after shutdown before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How many of the last stderr lines are kept to explain why a server stopped.
const STDERR_LINES: usize = 3;

/// Something a language server did that the editor may care about.
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// The server finished starting up and is ready for documents.
    Ready {
        /// The name of the server.
        server: String,
    },
    /// The server published diagnostics for a document.
    Diagnostics {
        /// The name of the server.
        server: String,
        /// The published diagnostics.
        params: PublishDiagnosticsParams,
    },
    /// The server wants to show the user a message.
    Message {
        /// The name of the server.
        server: String,
        /// The message text.
        text: String,
    },
    /// The server asked to show a document, like a sign in page in the browser.
    ShowDocument {
        /// The name of the server.
        server: String,
        /// The address to show.
        uri: String,
        /// Whether the server wants it shown outside the editor.
        external: bool,
    },
    /// The server sent a notification mog has no special handling for.
    Notification {
        /// The name of the server.
        server: String,
        /// The notification method.
        method: String,
        /// The notification arguments.
        params: Value,
    },
    /// The server process went away.
    Exited {
        /// The name of the server.
        server: String,
        /// The last lines the server wrote to stderr, which usually say why.
        reason: Option<String>,
    },
}

/// An error from a request to a language server.
#[derive(Debug, Error)]
pub enum LspError {
    /// The server is no longer running.
    #[error("language server is not running")]
    Closed,
    /// The server answered with an error object.
    #[error("language server error: {0}")]
    Server(Value),
}

/// A message waiting to be sent to the server.
enum Outgoing {
    /// A request whose result goes to `reply`.
    Request {
        /// The method to call.
        method: String,
        /// The call arguments.
        params: Value,
        /// Where the response goes.
        reply: oneshot::Sender<Result<Value, Value>>,
    },
    /// A notification.
    Notification {
        /// The method to call.
        method: String,
        /// The call arguments.
        params: Value,
    },
}

/// A running language server.
///
/// Clones share the connection. Dropping the last one shuts the server down.
#[derive(Clone)]
pub struct Client {
    /// The name of the server, from the config.
    name: String,
    /// The queue of messages for the server.
    outgoing: UnboundedSender<Outgoing>,
    /// What the server said it can do, once the handshake is done.
    capabilities: Arc<OnceLock<Value>>,
}

impl Client {
    /// Starts `command` as a language server for the project at `root`, configured with
    /// `settings`.
    ///
    /// The handshake happens in the background. Messages sent before it finishes are queued, and
    /// [`LspEvent::Ready`] is sent to `events` once it is done.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be started.
    pub fn start(
        name: impl Into<String>,
        command: &str,
        args: &[String],
        root: &Path,
        settings: &Value,
        events: UnboundedSender<LspEvent>,
    ) -> io::Result<Self> {
        let name = name.into();
        let mut child = Command::new(program_path(command))
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(io::Error::other("language server has no stdio"));
        };
        let stderr = tokio::spawn(last_lines(stderr));
        let (inner, mut forwarded) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut stderr = Some(stderr);
            while let Some(mut event) = forwarded.recv().await {
                if let (LspEvent::Exited { reason, .. }, Some(lines)) = (&mut event, stderr.take())
                {
                    // the pipe closes when the process dies so this is quick
                    *reason = time::timeout(EXIT_TIMEOUT, lines)
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .filter(|text| !text.is_empty());
                }
                if events.send(event).is_err() {
                    break;
                }
            }
        });
        let (client, connection) = Self::connect(name, stdout, stdin, root, settings, inner);
        tokio::spawn(async move {
            let _ = connection.await;
            // a server that ignores shutdown gets killed when the child is dropped
            let _ = time::timeout(EXIT_TIMEOUT, child.wait()).await;
        });
        Ok(client)
    }

    /// Connects to a server that reads from `writer` and writes to `reader`, configured with
    /// `settings`.
    ///
    /// Returns the client and the handle of the background task driving the connection.
    pub fn connect<R, W>(
        name: impl Into<String>,
        reader: R,
        writer: W,
        root: &Path,
        settings: &Value,
        events: UnboundedSender<LspEvent>,
    ) -> (Self, JoinHandle<()>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let name = name.into();
        let (outgoing, queue) = mpsc::unbounded_channel();
        let capabilities = Arc::new(OnceLock::new());
        let connection = Connection {
            name: name.clone(),
            capabilities: Arc::clone(&capabilities),
            writer: BufWriter::new(writer),
            events,
            pending: HashMap::new(),
            next_id: INITIALIZE_ID + 1,
            settings: settings.clone(),
        };
        let params = initialize_params(root, settings);
        let handle = tokio::spawn(connection.run(reader, queue, params));
        (
            Self {
                name,
                outgoing,
                capabilities,
            },
            handle,
        )
    }

    /// Returns the name of the server.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the capabilities the server answered the handshake with, once it has.
    pub fn capabilities(&self) -> Option<&Value> {
        self.capabilities.get()
    }

    /// Returns `true` while the connection to the server is open.
    pub fn is_running(&self) -> bool {
        !self.outgoing.is_closed()
    }

    /// Sends a request and waits for the result.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or answers with an error.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, LspError> {
        self.send_request(method, params)?.await
    }

    /// Queues a request right now and returns what waits for the result.
    ///
    /// Unlike [`Client::request`] the request is ordered against other messages when this is
    /// called, not when the result is first polled.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone.
    pub fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<impl Future<Output = Result<Value, LspError>> + use<>, LspError> {
        let (reply, response) = oneshot::channel();
        self.outgoing
            .send(Outgoing::Request {
                method: method.to_owned(),
                params,
                reply,
            })
            .map_err(|_| LspError::Closed)?;
        Ok(async move {
            response
                .await
                .map_err(|_| LspError::Closed)?
                .map_err(LspError::Server)
        })
    }

    /// Sends a notification. Notifications to a dead server are dropped.
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.outgoing.send(Outgoing::Notification {
            method: method.to_owned(),
            params,
        });
    }

    /// Tells the server a document was opened.
    pub fn did_open(&self, path: &Path, language_id: &str, version: i32, text: &str) {
        let Some(uri) = convert::path_to_uri(path) else {
            return;
        };
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": language_id, "version": version, "text": text
            }}),
        );
    }

    /// Tells the server a document was saved to disk.
    pub fn did_save(&self, path: &Path) {
        if let Some(params) = document_params(path) {
            self.notify("textDocument/didSave", params);
        }
    }

    /// Tells the server a document was closed.
    pub fn did_close(&self, path: &Path) {
        if let Some(params) = document_params(path) {
            self.notify("textDocument/didClose", params);
        }
    }

    /// Tells the server a document changed, sending the full new text.
    pub fn did_change(&self, path: &Path, version: i32, text: &str) {
        let Some(uri) = convert::path_to_uri(path) else {
            return;
        };
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }),
        );
    }
}

/// Reads `stderr` to the end and returns its last few non empty lines joined with spaces.
async fn last_lines(stderr: impl AsyncRead + Unpin) -> String {
    let mut lines = BufReader::new(stderr).lines();
    let mut kept = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if kept.len() == STDERR_LINES {
            kept.remove(0);
        }
        kept.push(line.to_owned());
    }
    kept.join(" ")
}

/// Returns the path to run for `command`.
///
/// Windows only finds `.exe` files on its own, so bare names are also looked up as the `.cmd`
/// and `.bat` shims npm installs.
fn program_path(command: &str) -> PathBuf {
    // windows only finds programs given with its own separators, so rebuild the path
    let program: PathBuf = Path::new(command).components().collect();
    if !cfg!(windows) || program.extension().is_some() || program.components().count() > 1 {
        return program;
    }
    let Some(dirs) = env::var_os("PATH") else {
        return program;
    };
    env::split_paths(&dirs)
        .flat_map(|dir| ["exe", "cmd", "bat"].map(|ext| dir.join(&program).with_extension(ext)))
        .find(|candidate| candidate.is_file())
        .unwrap_or(program)
}

/// Returns the arguments of a notification that is only about the document at `path`.
fn document_params(path: &Path) -> Option<Value> {
    let uri = convert::path_to_uri(path)?;
    Some(json!({ "textDocument": { "uri": uri } }))
}

/// Returns the part of `settings` a server asks for with `section`, like `python.analysis`.
///
/// Servers like rust-analyzer ask for their own name, which users leave out, so a section that
/// is not found gives all of `settings`.
pub fn settings_section<'a>(settings: &'a Value, section: Option<&str>) -> &'a Value {
    let Some(section) = section.filter(|section| !section.is_empty()) else {
        return settings;
    };
    section
        .split('.')
        .try_fold(settings, |value, key| value.get(key))
        .unwrap_or(settings)
}

/// Builds the initialize request arguments for a project at `root` with server `settings`.
fn initialize_params(root: &Path, settings: &Value) -> Value {
    let folders = convert::path_to_uri(root).map(|uri| {
        vec![WorkspaceFolder {
            uri,
            name: root
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into()),
        }]
    });
    let params = InitializeParams {
        process_id: Some(process::id()),
        workspace_folders: folders,
        initialization_options: (!settings.is_null()).then(|| settings.clone()),
        client_info: Some(ClientInfo {
            name: "mog".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        capabilities: ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                synchronization: Some(TextDocumentSyncClientCapabilities {
                    // rust-analyzer only runs cargo check when told about saves
                    did_save: Some(true),
                    ..TextDocumentSyncClientCapabilities::default()
                }),
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities::default()),
                completion: Some(CompletionClientCapabilities {
                    completion_item: Some(CompletionItemCapability {
                        // snippets need a placeholder engine mog does not have
                        snippet_support: Some(false),
                        documentation_format: Some(vec![MarkupKind::PlainText]),
                        ..CompletionItemCapability::default()
                    }),
                    ..CompletionClientCapabilities::default()
                }),
                hover: Some(HoverClientCapabilities {
                    content_format: Some(vec![MarkupKind::PlainText, MarkupKind::Markdown]),
                    ..HoverClientCapabilities::default()
                }),
                definition: Some(GotoCapability::default()),
                formatting: Some(Default::default()),
                rename: Some(Default::default()),
                references: Some(Default::default()),
                code_action: Some(CodeActionClientCapabilities {
                    // without literal support servers may only send bare commands
                    code_action_literal_support: Some(CodeActionLiteralSupport {
                        code_action_kind: CodeActionKindLiteralSupport {
                            value_set: [
                                "",
                                "quickfix",
                                "refactor",
                                "refactor.extract",
                                "refactor.inline",
                                "refactor.rewrite",
                                "source",
                            ]
                            .map(String::from)
                            .to_vec(),
                        },
                    }),
                    ..CodeActionClientCapabilities::default()
                }),
                ..TextDocumentClientCapabilities::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                configuration: Some(true),
                ..WorkspaceClientCapabilities::default()
            }),
            window: Some(WindowClientCapabilities {
                show_document: Some(ShowDocumentClientCapabilities { support: true }),
                ..WindowClientCapabilities::default()
            }),
            ..ClientCapabilities::default()
        },
        ..InitializeParams::default()
    };
    let mut params = serde_json::to_value(params).unwrap_or(Value::Null);
    // these are plain json since the typed versions take far more lines to say the same
    let text_document = &mut params["capabilities"]["textDocument"];
    text_document["inlayHint"] = json!({ "dynamicRegistration": false });
    // rust-analyzer only offers items that need an import to clients that can resolve them
    text_document["completion"]["completionItem"]["resolveSupport"] =
        json!({ "properties": ["additionalTextEdits"] });
    text_document["signatureHelp"] = json!({
        "signatureInformation": {
            "documentationFormat": ["plaintext", "markdown"],
            "parameterInformation": { "labelOffsetSupport": true },
            "activeParameterSupport": true,
        },
    });
    text_document["documentSymbol"] = json!({ "hierarchicalDocumentSymbolSupport": true });
    text_document["semanticTokens"] = json!({
        "requests": { "full": true },
        "tokenTypes": [
            "namespace", "type", "class", "enum", "interface", "struct", "typeParameter",
            "parameter", "variable", "property", "enumMember", "event", "function", "method",
            "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator",
            "decorator",
        ],
        "tokenModifiers": [
            "declaration", "definition", "readonly", "static", "deprecated", "abstract",
            "async", "modification", "documentation", "defaultLibrary",
        ],
        "formats": ["relative"],
    });
    let workspace = &mut params["capabilities"]["workspace"];
    workspace["symbol"] = json!({ "dynamicRegistration": false });
    workspace["semanticTokens"] = json!({ "refreshSupport": true });
    workspace["inlayHint"] = json!({ "refreshSupport": true });
    params
}

/// The background half of a [`Client`] that owns the pipes.
struct Connection<W> {
    /// The name of the server.
    name: String,
    /// The server stdin.
    writer: BufWriter<W>,
    /// Where events for the editor go.
    events: UnboundedSender<LspEvent>,
    /// Requests waiting for a response, by id.
    pending: HashMap<u64, oneshot::Sender<Result<Value, Value>>>,
    /// The id the next request gets.
    next_id: u64,
    /// The settings handed out when the server asks for its configuration.
    settings: Value,
    /// Where the capabilities from the handshake are kept for the client.
    capabilities: Arc<OnceLock<Value>>,
}

impl<W: AsyncWrite + Unpin> Connection<W> {
    /// Runs the connection until the server exits or the client is dropped.
    async fn run(
        mut self,
        reader: impl AsyncRead + Unpin + Send + 'static,
        mut queue: UnboundedReceiver<Outgoing>,
        params: Value,
    ) {
        let (incoming_tx, mut incoming) = mpsc::unbounded_channel();
        // reading is not cancel safe so it gets its own task instead of a select branch
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            while let Ok(Some(message)) = transport::read_message(&mut reader).await {
                if incoming_tx.send(message).is_err() {
                    break;
                }
            }
        });

        if self.handshake(&mut incoming, params).await.is_ok() {
            let _ = self.events.send(LspEvent::Ready {
                server: self.name.clone(),
            });
            loop {
                let result = tokio::select! {
                    message = queue.recv() => match message {
                        Some(message) => self.send(message).await,
                        None => {
                            let _ = self.shutdown().await;
                            break;
                        }
                    },
                    message = incoming.recv() => match message {
                        Some(message) => self.receive(message).await,
                        None => break,
                    },
                };
                if result.is_err() {
                    break;
                }
            }
        }
        let _ = self.events.send(LspEvent::Exited {
            server: self.name.clone(),
            reason: None,
        });
    }

    /// Sends initialize, waits for its response and sends initialized.
    async fn handshake(
        &mut self,
        incoming: &mut UnboundedReceiver<Message>,
        params: Value,
    ) -> io::Result<()> {
        self.write(Message::Request {
            id: json!(INITIALIZE_ID),
            method: "initialize".into(),
            params,
        })
        .await?;
        loop {
            match incoming.recv().await {
                Some(Message::Response { id, result }) if id == json!(INITIALIZE_ID) => {
                    let capabilities = result.ok().map(|mut result| result["capabilities"].take());
                    let _ = self.capabilities.set(capabilities.unwrap_or(Value::Null));
                    break;
                }
                Some(message) => self.receive(message).await?,
                None => return Err(ErrorKind::UnexpectedEof.into()),
            }
        }
        self.write(Message::Notification {
            method: "initialized".into(),
            params: json!({}),
        })
        .await
    }

    /// Asks the server to shut down and exit.
    async fn shutdown(&mut self) -> io::Result<()> {
        let id = self.next_id;
        self.write(Message::Request {
            id: json!(id),
            method: "shutdown".into(),
            params: Value::Null,
        })
        .await?;
        self.write(Message::Notification {
            method: "exit".into(),
            params: Value::Null,
        })
        .await
    }

    /// Sends a queued message.
    async fn send(&mut self, message: Outgoing) -> io::Result<()> {
        match message {
            Outgoing::Request {
                method,
                params,
                reply,
            } => {
                let id = self.next_id;
                self.next_id += 1;
                self.pending.insert(id, reply);
                self.write(Message::Request {
                    id: json!(id),
                    method,
                    params,
                })
                .await
            }
            Outgoing::Notification { method, params } => {
                self.write(Message::Notification { method, params }).await
            }
        }
    }

    /// Handles a message from the server.
    async fn receive(&mut self, message: Message) -> io::Result<()> {
        match message {
            Message::Response { id, result } => {
                if let Some(reply) = id.as_u64().and_then(|id| self.pending.remove(&id)) {
                    let _ = reply.send(result);
                }
            }
            Message::Notification { method, params } if method == "tsserver/request" => {
                // the vue server forwards typescript questions to the editor and waits, and mog
                // has no tsserver, so it is told there is no answer
                let id = params[0][0].clone();
                self.write(Message::Notification {
                    method: "tsserver/response".into(),
                    params: json!([[id, Value::Null]]),
                })
                .await?;
            }
            Message::Notification { method, params } => self.notification(&method, params),
            Message::Request { id, method, params } => {
                // answering with nulls keeps servers that wait on us from hanging
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let items = params["items"].as_array().cloned().unwrap_or_default();
                        items
                            .iter()
                            .map(|item| {
                                settings_section(&self.settings, item["section"].as_str()).clone()
                            })
                            .collect()
                    }
                    "workspace/semanticTokens/refresh" | "workspace/inlayHint/refresh" => {
                        let _ = self.events.send(LspEvent::Notification {
                            server: self.name.clone(),
                            method: method.clone(),
                            params,
                        });
                        Value::Null
                    }
                    "window/showDocument" => {
                        let _ = self.events.send(LspEvent::ShowDocument {
                            server: self.name.clone(),
                            uri: params["uri"].as_str().unwrap_or_default().to_owned(),
                            external: params["external"].as_bool().unwrap_or(false),
                        });
                        json!({ "success": true })
                    }
                    _ => Value::Null,
                };
                self.write(Message::Response {
                    id,
                    result: Ok(result),
                })
                .await?;
            }
        }
        Ok(())
    }

    /// Turns a server notification into an editor event.
    fn notification(&self, method: &str, params: Value) {
        let server = self.name.clone();
        let event = match method {
            "textDocument/publishDiagnostics" => match serde_json::from_value(params) {
                Ok(params) => LspEvent::Diagnostics { server, params },
                Err(_) => return,
            },
            "window/showMessage" => LspEvent::Message {
                server,
                text: params["message"].as_str().unwrap_or_default().to_owned(),
            },
            _ => LspEvent::Notification {
                server,
                method: method.to_owned(),
                params,
            },
        };
        let _ = self.events.send(event);
    }

    /// Writes a message to the server.
    async fn write(&mut self, message: Message) -> io::Result<()> {
        transport::write_message(&mut self.writer, &message).await
    }
}

#[cfg(test)]
/// Tests for [`Client`].
mod tests {
    use std::{env, time::Duration};

    use serde_json::{Value, json};
    use tokio::{
        io::{self, BufReader},
        sync::mpsc,
        time,
    };

    use super::{Client, LspEvent, settings_section};
    use crate::transport::{self, Message};

    /// Sections are looked up by dotted path and fall back to everything.
    #[test]
    fn finds_sections() {
        let settings = json!({ "python": { "analysis": { "strict": true } } });
        assert_eq!(
            settings_section(&settings, Some("python.analysis")),
            &json!({ "strict": true })
        );
        assert_eq!(
            settings_section(&settings, Some("rust-analyzer")),
            &settings
        );
        assert_eq!(settings_section(&settings, None), &settings);
    }

    /// Saving and closing a document reach the server after the handshake.
    #[tokio::test]
    async fn sends_save_and_close() {
        let (client_side, server_side) = io::duplex(1 << 16);
        let (client_read, client_write) = io::split(client_side);
        let (server_read, mut server_write) = io::split(server_side);
        let (events, _events) = mpsc::unbounded_channel();
        let root = env::temp_dir();
        let (client, _task) = Client::connect(
            "test",
            client_read,
            client_write,
            &root,
            &Value::Null,
            events,
        );
        let file = root.join("main.rs");
        client.did_save(&file);
        client.did_close(&file);
        let mut server_read = BufReader::new(server_read);
        let mut methods = Vec::new();
        while methods.len() < 4 {
            let message = time::timeout(
                Duration::from_secs(5),
                transport::read_message(&mut server_read),
            )
            .await
            .expect("server got every message")
            .expect("read")
            .expect("message");
            match message {
                Message::Request { id, method, .. } => {
                    let reply = Message::Response {
                        id,
                        result: Ok(json!({ "capabilities": {} })),
                    };
                    transport::write_message(&mut server_write, &reply)
                        .await
                        .expect("write");
                    methods.push(method);
                }
                Message::Notification { method, params } => {
                    if method != "initialized" {
                        assert_ne!(params["textDocument"]["uri"], Value::Null);
                    }
                    methods.push(method);
                }
                Message::Response { .. } => {}
            }
        }
        assert_eq!(
            methods,
            [
                "initialize",
                "initialized",
                "textDocument/didSave",
                "textDocument/didClose"
            ]
        );
    }

    /// Questions for a tsserver are answered with nothing so the vue server does not wait.
    #[tokio::test]
    async fn answers_tsserver_requests() {
        let (client_side, server_side) = io::duplex(1 << 16);
        let (client_read, client_write) = io::split(client_side);
        let (server_read, mut server_write) = io::split(server_side);
        let (events, _events) = mpsc::unbounded_channel();
        let (_client, _task) = Client::connect(
            "test",
            client_read,
            client_write,
            &env::temp_dir(),
            &Value::Null,
            events,
        );
        let mut server_read = BufReader::new(server_read);
        let mut read = async || {
            time::timeout(
                Duration::from_secs(5),
                transport::read_message(&mut server_read),
            )
            .await
            .expect("in time")
            .expect("read")
            .expect("message")
        };
        let Message::Request { id, .. } = read().await else {
            panic!("expected initialize");
        };
        let reply = Message::Response {
            id,
            result: Ok(json!({ "capabilities": {} })),
        };
        transport::write_message(&mut server_write, &reply)
            .await
            .expect("write");
        let ask = Message::Notification {
            method: "tsserver/request".into(),
            params: json!([[7, "_vue:projectInfo", { "file": "a.vue" }]]),
        };
        transport::write_message(&mut server_write, &ask)
            .await
            .expect("write");
        loop {
            if let Message::Notification { method, params } = read().await
                && method == "tsserver/response"
            {
                assert_eq!(params, json!([[7, null]]));
                break;
            }
        }
    }

    /// Asking to show a document becomes an event and is answered with success.
    #[tokio::test]
    async fn shows_documents() {
        let (client_side, server_side) = io::duplex(1 << 16);
        let (client_read, client_write) = io::split(client_side);
        let (server_read, mut server_write) = io::split(server_side);
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let (_client, _task) = Client::connect(
            "test",
            client_read,
            client_write,
            &env::temp_dir(),
            &Value::Null,
            events,
        );
        let mut server_read = BufReader::new(server_read);
        let mut read = async || {
            time::timeout(
                Duration::from_secs(5),
                transport::read_message(&mut server_read),
            )
            .await
            .expect("in time")
            .expect("read")
            .expect("message")
        };
        let Message::Request { id, .. } = read().await else {
            panic!("expected initialize");
        };
        let reply = Message::Response {
            id,
            result: Ok(json!({ "capabilities": {} })),
        };
        transport::write_message(&mut server_write, &reply)
            .await
            .expect("write");
        let ask = Message::Request {
            id: json!("show"),
            method: "window/showDocument".into(),
            params: json!({ "uri": "https://github.com/login/device", "external": true }),
        };
        transport::write_message(&mut server_write, &ask)
            .await
            .expect("write");
        loop {
            if let Message::Response { id, result } = read().await {
                assert_eq!(id, json!("show"));
                assert_eq!(result.expect("success")["success"], true);
                break;
            }
        }
        loop {
            let event = events_rx.recv().await.expect("event");
            if let LspEvent::ShowDocument { uri, external, .. } = event {
                assert_eq!(uri, "https://github.com/login/device");
                assert!(external);
                break;
            }
        }
    }
}
