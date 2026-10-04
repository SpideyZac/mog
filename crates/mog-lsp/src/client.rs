//! A connection to one language server process.

use std::{
    collections::HashMap,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    process::{self, Stdio},
    time::Duration,
};

use lsp_types::{
    ClientCapabilities, ClientInfo, CodeActionClientCapabilities, CodeActionKindLiteralSupport,
    CodeActionLiteralSupport, CompletionClientCapabilities, CompletionItemCapability,
    GotoCapability, HoverClientCapabilities, InitializeParams, MarkupKind,
    PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams, TextDocumentClientCapabilities,
    TextDocumentSyncClientCapabilities, WorkspaceFolder,
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
}

impl Client {
    /// Starts `command` as a language server for the project at `root`.
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
        events: UnboundedSender<LspEvent>,
    ) -> io::Result<Self> {
        let name = name.into();
        // windows only finds programs given with its own separators, so rebuild the path
        let program: PathBuf = Path::new(command).components().collect();
        let mut child = Command::new(program)
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
        let (client, connection) = Self::connect(name, stdout, stdin, root, inner);
        tokio::spawn(async move {
            let _ = connection.await;
            // a server that ignores shutdown gets killed when the child is dropped
            let _ = time::timeout(EXIT_TIMEOUT, child.wait()).await;
        });
        Ok(client)
    }

    /// Connects to a server that reads from `writer` and writes to `reader`.
    ///
    /// Returns the client and the handle of the background task driving the connection.
    pub fn connect<R, W>(
        name: impl Into<String>,
        reader: R,
        writer: W,
        root: &Path,
        events: UnboundedSender<LspEvent>,
    ) -> (Self, JoinHandle<()>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let name = name.into();
        let (outgoing, queue) = mpsc::unbounded_channel();
        let connection = Connection {
            name: name.clone(),
            writer: BufWriter::new(writer),
            events,
            pending: HashMap::new(),
            next_id: INITIALIZE_ID + 1,
        };
        let params = initialize_params(root);
        let handle = tokio::spawn(connection.run(reader, queue, params));
        (Self { name, outgoing }, handle)
    }

    /// Returns the name of the server.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sends a request and waits for the result.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or answers with an error.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, LspError> {
        let (reply, response) = oneshot::channel();
        self.outgoing
            .send(Outgoing::Request {
                method: method.to_owned(),
                params,
                reply,
            })
            .map_err(|_| LspError::Closed)?;
        response
            .await
            .map_err(|_| LspError::Closed)?
            .map_err(LspError::Server)
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

/// Builds the initialize request arguments for a project at `root`.
fn initialize_params(root: &Path) -> Value {
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
        client_info: Some(ClientInfo {
            name: "mog".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        capabilities: ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                synchronization: Some(TextDocumentSyncClientCapabilities::default()),
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
            ..ClientCapabilities::default()
        },
        ..InitializeParams::default()
    };
    serde_json::to_value(params).unwrap_or(Value::Null)
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
                Some(Message::Response { id, .. }) if id == json!(INITIALIZE_ID) => break,
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
            Message::Notification { method, params } => self.notification(&method, params),
            Message::Request { id, method, params } => {
                // answering with nulls keeps servers that wait on us from hanging
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let count = params["items"].as_array().map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; count])
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
            _ => return,
        };
        let _ = self.events.send(event);
    }

    /// Writes a message to the server.
    async fn write(&mut self, message: Message) -> io::Result<()> {
        transport::write_message(&mut self.writer, &message).await
    }
}
