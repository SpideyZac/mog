//! Plugins for mog: programs that talk JSON-RPC over stdio and add commands to the editor.
//!
//! The protocol is described in `docs/plugins.md`. Plugins run as their own processes, so they
//! can be written in any language and a broken one cannot take mog down with it.

use std::{
    collections::HashMap,
    env, io,
    path::{Path, PathBuf},
    process::Stdio,
};

use mog_lsp::transport::{self, Message};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader, BufWriter},
    process::Command,
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

/// The version of the plugin protocol mog speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// The JSON-RPC error code for a method that does not exist.
const METHOD_NOT_FOUND: i64 = -32601;

/// The JSON-RPC error code mog answers failed requests with.
const REQUEST_FAILED: i64 = -32000;

/// A command a plugin adds to the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// The name inside the plugin, like `count_words`.
    pub name: String,
    /// What the palette shows, like `Words: Count`.
    pub title: String,
    /// Keys the plugin would like bound to it, used when nothing else has them.
    pub keys: Vec<String>,
}

/// A change to a document, in char offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The first char replaced.
    pub start: usize,
    /// The char just past the last one replaced.
    pub end: usize,
    /// The new text.
    pub text: String,
}

/// Something a plugin asks mog to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Show a message in the status line.
    Status(String),
    /// Change a document, the focused one when `path` is not given.
    Edit {
        /// The open document to change.
        path: Option<PathBuf>,
        /// The changes, which must not overlap.
        changes: Vec<Edit>,
    },
    /// Replace the selection, or type at the cursor.
    Insert(String),
    /// Open a file, at a line from 0 if given.
    Open {
        /// The file.
        path: PathBuf,
        /// The line to put the cursor on.
        line: Option<usize>,
    },
    /// Run a mog command by name, like `save` or `terminal.toggle`.
    Command(String),
}

/// Something a plugin did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    /// The plugin started and said what commands it has.
    Ready {
        /// The plugin name.
        plugin: String,
        /// Its commands.
        commands: Vec<PluginCommand>,
    },
    /// The plugin asked for actions on its own, like after background work.
    Actions {
        /// The plugin name.
        plugin: String,
        /// What to do.
        actions: Vec<Action>,
    },
    /// The plugin wants `text` shown in the status line, or nothing when it is empty.
    Segment {
        /// The plugin name.
        plugin: String,
        /// The text.
        text: String,
    },
    /// The plugin asked mog something and waits for [`Plugin::respond`].
    Request {
        /// The plugin name.
        plugin: String,
        /// The request id to answer with.
        id: Value,
        /// What it asks, like `editor/context` or `ui/pick`.
        method: String,
        /// The arguments.
        params: Value,
    },
    /// The plugin process went away.
    Exited {
        /// The plugin name.
        plugin: String,
        /// The last thing it printed to stderr, which usually says why.
        reason: Option<String>,
    },
}

/// Reads the commands from an `initialize` result.
fn parse_commands(result: &Value) -> Vec<PluginCommand> {
    result["commands"]
        .as_array()
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    let name = command["name"].as_str()?.to_owned();
                    let title = command["title"].as_str().unwrap_or(&name).to_owned();
                    let keys = command["keys"]
                        .as_array()
                        .map(|keys| {
                            keys.iter()
                                .filter_map(|key| key.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(PluginCommand { name, title, keys })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads one action, or `None` for ones mog does not know.
fn parse_action(action: &Value) -> Option<Action> {
    let offset = |value: &Value| value.as_u64().and_then(|n| usize::try_from(n).ok());
    Some(match action["type"].as_str()? {
        "status" => Action::Status(action["text"].as_str()?.to_owned()),
        "insert" => Action::Insert(action["text"].as_str()?.to_owned()),
        "command" => Action::Command(action["name"].as_str()?.to_owned()),
        "open" => Action::Open {
            path: PathBuf::from(action["path"].as_str()?),
            line: offset(&action["line"]),
        },
        "edit" => Action::Edit {
            path: action["path"].as_str().map(PathBuf::from),
            changes: action["changes"]
                .as_array()?
                .iter()
                .filter_map(|change| {
                    Some(Edit {
                        start: offset(&change["start"])?,
                        end: offset(&change["end"])?,
                        text: change["text"].as_str().unwrap_or_default().to_owned(),
                    })
                })
                .collect(),
        },
        _ => return None,
    })
}

/// Reads the actions from a command result or an `actions` notification.
pub fn parse_actions(value: &Value) -> Vec<Action> {
    value["actions"]
        .as_array()
        .map(|actions| actions.iter().filter_map(parse_action).collect())
        .unwrap_or_default()
}

/// Returns the path to run for `command`, finding npm style `.cmd` shims on Windows.
fn program_path(command: &str) -> PathBuf {
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

/// A message waiting to be sent to the plugin.
enum Outgoing {
    /// A request whose result goes to `reply`.
    Request {
        /// The method.
        method: String,
        /// The arguments.
        params: Value,
        /// Where the result or error message goes.
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// A notification.
    Notification {
        /// The method.
        method: String,
        /// The arguments.
        params: Value,
    },
    /// The answer to a request the plugin sent.
    Response {
        /// The id of the request.
        id: Value,
        /// The result, or an error message.
        result: Result<Value, String>,
    },
}

/// A running plugin. Clones share the connection, and dropping the last one stops it.
#[derive(Clone)]
pub struct Plugin {
    /// The plugin name from the config.
    name: String,
    /// The queue of messages for the plugin.
    outgoing: UnboundedSender<Outgoing>,
}

/// Reads `stderr` to the end and returns its last non empty line.
async fn last_line(stderr: impl AsyncRead + Unpin) -> Option<String> {
    let mut lines = BufReader::new(stderr).lines();
    let mut last = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            last = Some(line.trim().to_owned());
        }
    }
    last
}

impl Plugin {
    /// Starts the plugin called `name` by running `command` with `args` in `root`.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be started.
    pub fn start(
        name: &str,
        command: &str,
        args: &[String],
        root: &Path,
        events: UnboundedSender<PluginEvent>,
    ) -> io::Result<Self> {
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
            return Err(io::Error::other("plugin has no stdio"));
        };
        let (inner, mut forwarded) = mpsc::unbounded_channel();
        let stderr = tokio::spawn(last_line(stderr));
        tokio::spawn(async move {
            let mut stderr = Some(stderr);
            while let Some(mut event) = forwarded.recv().await {
                if let (PluginEvent::Exited { reason, .. }, Some(lines)) =
                    (&mut event, stderr.take())
                {
                    *reason = lines.await.ok().flatten();
                }
                if events.send(event).is_err() {
                    break;
                }
            }
        });
        let (plugin, connection) = Self::connect(name, stdout, stdin, root, inner);
        tokio::spawn(async move {
            let _ = connection.await;
            let _ = child.kill().await;
        });
        Ok(plugin)
    }

    /// Talks to a plugin that reads from `writer` and writes to `reader`, introducing mog with
    /// the project `root`.
    pub fn connect<R, W>(
        name: &str,
        reader: R,
        writer: W,
        root: &Path,
        events: UnboundedSender<PluginEvent>,
    ) -> (Self, JoinHandle<()>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, queue) = mpsc::unbounded_channel();
        let connection = Connection {
            name: name.to_owned(),
            writer: BufWriter::new(writer),
            events,
            pending: HashMap::new(),
            next_id: 1,
        };
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "mogVersion": env!("CARGO_PKG_VERSION"),
            "root": root.to_string_lossy(),
        });
        let handle = tokio::spawn(connection.run(reader, queue, params));
        (
            Self {
                name: name.to_owned(),
                outgoing,
            },
            handle,
        )
    }

    /// Returns the plugin name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runs the plugin's `command` with what the editor looks like in `context`, and returns
    /// what it asks mog to do.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error message, or why it cannot be reached.
    pub async fn run(&self, command: &str, context: Value) -> Result<Vec<Action>, String> {
        let (reply, answer) = oneshot::channel();
        self.outgoing
            .send(Outgoing::Request {
                method: "command".into(),
                params: json!({ "command": command, "context": context }),
                reply,
            })
            .map_err(|_| format!("{} is not running", self.name))?;
        let result = answer
            .await
            .map_err(|_| format!("{} stopped", self.name))??;
        Ok(parse_actions(&result))
    }

    /// Answers the request `id` the plugin sent, with a result or an error message.
    pub fn respond(&self, id: Value, result: Result<Value, String>) {
        let _ = self.outgoing.send(Outgoing::Response { id, result });
    }

    /// Tells the plugin something happened, like `opened` or `saved` with the file.
    pub fn event(&self, kind: &str, path: Option<&Path>) {
        let _ = self.outgoing.send(Outgoing::Notification {
            method: "event".into(),
            params: json!({ "kind": kind, "path": path.map(|path| path.to_string_lossy()) }),
        });
    }
}

/// The background half of a [`Plugin`] that owns the pipes.
struct Connection<W> {
    /// The plugin name.
    name: String,
    /// The plugin stdin.
    writer: BufWriter<W>,
    /// Where events go.
    events: UnboundedSender<PluginEvent>,
    /// Requests waiting for a response, by id.
    pending: HashMap<u64, oneshot::Sender<Result<Value, String>>>,
    /// The id of the next request.
    next_id: u64,
}

impl<W: AsyncWrite + Unpin> Connection<W> {
    /// Says hello, then runs until the plugin exits or every handle is dropped.
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
        let (ready, hello) = oneshot::channel();
        if self.request("initialize", params, ready).await.is_ok() {
            let mut hello = Some(hello);
            loop {
                let result = tokio::select! {
                    message = queue.recv() => match message {
                        Some(message) => self.send(message).await,
                        None => {
                            let _ = self.write(Message::Notification {
                                method: "shutdown".into(),
                                params: Value::Null,
                            }).await;
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
                // the handshake answer arrives like any other response
                if let Some(answer) = hello.as_mut().and_then(|hello| hello.try_recv().ok()) {
                    hello = None;
                    let commands = answer
                        .map(|result| parse_commands(&result))
                        .unwrap_or_default();
                    let _ = self.events.send(PluginEvent::Ready {
                        plugin: self.name.clone(),
                        commands,
                    });
                }
            }
        }
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err(format!("{} stopped", self.name)));
        }
        let _ = self.events.send(PluginEvent::Exited {
            plugin: self.name.clone(),
            reason: None,
        });
    }

    /// Writes `message` to the plugin.
    async fn write(&mut self, message: Message) -> io::Result<()> {
        transport::write_message(&mut self.writer, &message).await
    }

    /// Sends a request whose answer goes to `reply`.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    ) -> io::Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, reply);
        self.write(Message::Request {
            id: json!(id),
            method: method.to_owned(),
            params,
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
            } => self.request(&method, params, reply).await,
            Outgoing::Notification { method, params } => {
                self.write(Message::Notification { method, params }).await
            }
            Outgoing::Response { id, result } => {
                let result =
                    result.map_err(|message| json!({ "code": REQUEST_FAILED, "message": message }));
                self.write(Message::Response { id, result }).await
            }
        }
    }

    /// Handles a message from the plugin.
    async fn receive(&mut self, message: Message) -> io::Result<()> {
        match message {
            Message::Response { id, result } => {
                if let Some(reply) = id.as_u64().and_then(|id| self.pending.remove(&id)) {
                    let result = result.map_err(|error| {
                        error["message"]
                            .as_str()
                            .unwrap_or("the plugin failed")
                            .to_owned()
                    });
                    let _ = reply.send(result);
                }
            }
            Message::Notification { method, params } => {
                let plugin = self.name.clone();
                let event = match method.as_str() {
                    "actions" => PluginEvent::Actions {
                        plugin,
                        actions: parse_actions(&params),
                    },
                    "status" => PluginEvent::Actions {
                        plugin,
                        actions: vec![Action::Status(
                            params["text"].as_str().unwrap_or_default().to_owned(),
                        )],
                    },
                    "segment" => PluginEvent::Segment {
                        plugin,
                        text: params["text"].as_str().unwrap_or_default().to_owned(),
                    },
                    _ => return Ok(()),
                };
                let _ = self.events.send(event);
            }
            Message::Request { id, method, params } => {
                let event = PluginEvent::Request {
                    plugin: self.name.clone(),
                    id: id.clone(),
                    method,
                    params,
                };
                // without mog listening nobody will answer, so say so instead of hanging
                if self.events.send(event).is_err() {
                    let error = json!({ "code": METHOD_NOT_FOUND, "message": "mog is closing" });
                    self.write(Message::Response {
                        id,
                        result: Err(error),
                    })
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
/// Tests for plugins.
mod tests {
    use std::{env, path::PathBuf, time::Duration};

    use mog_lsp::transport::{self, Message};
    use serde_json::json;
    use tokio::{
        io::{self, BufReader},
        sync::mpsc,
        time,
    };

    use super::{Action, Edit, Plugin, PluginEvent, parse_actions};

    /// Every action shape is read and unknown ones are skipped.
    #[test]
    fn reads_actions() {
        let value = json!({ "actions": [
            { "type": "status", "text": "hi" },
            { "type": "edit", "changes": [{ "start": 0, "end": 2, "text": "x" }] },
            { "type": "open", "path": "/a.txt", "line": 3 },
            { "type": "command", "name": "save" },
            { "type": "dance" },
        ]});
        assert_eq!(
            parse_actions(&value),
            [
                Action::Status("hi".into()),
                Action::Edit {
                    path: None,
                    changes: vec![Edit {
                        start: 0,
                        end: 2,
                        text: "x".into()
                    }]
                },
                Action::Open {
                    path: PathBuf::from("/a.txt"),
                    line: Some(3)
                },
                Action::Command("save".into()),
            ]
        );
    }

    /// A request from the plugin reaches mog and the answer gets back to the plugin.
    #[tokio::test]
    async fn answers_plugin_requests() {
        let (mog_side, plugin_side) = io::duplex(1 << 16);
        let (mog_read, mog_write) = io::split(mog_side);
        let (plugin_read, mut plugin_write) = io::split(plugin_side);
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let (plugin, _task) = Plugin::connect("ask", mog_read, mog_write, &env::temp_dir(), events);
        let answer = tokio::spawn(async move {
            let mut reader = BufReader::new(plugin_read);
            let Ok(Some(Message::Request { id, .. })) = transport::read_message(&mut reader).await
            else {
                panic!("expected initialize");
            };
            let hello = Message::Response {
                id,
                result: Ok(json!({ "commands": [] })),
            };
            transport::write_message(&mut plugin_write, &hello)
                .await
                .expect("write");
            let ask = Message::Request {
                id: json!("q"),
                method: "editor/context".into(),
                params: json!({}),
            };
            transport::write_message(&mut plugin_write, &ask)
                .await
                .expect("write");
            loop {
                if let Ok(Some(Message::Response { id, result })) =
                    transport::read_message(&mut reader).await
                {
                    assert_eq!(id, json!("q"));
                    return result.expect("answered");
                }
            }
        });
        loop {
            let event = time::timeout(Duration::from_secs(5), events_rx.recv())
                .await
                .expect("in time")
                .expect("event");
            if let PluginEvent::Request { id, method, .. } = event {
                assert_eq!(method, "editor/context");
                plugin.respond(id, Ok(json!({ "line": 4 })));
                break;
            }
        }
        let result = time::timeout(Duration::from_secs(5), answer)
            .await
            .expect("in time")
            .expect("plugin side");
        assert_eq!(result["line"], 4);
    }

    /// The handshake reports the commands and a command run comes back with its actions.
    #[tokio::test]
    async fn runs_a_command() {
        let (mog_side, plugin_side) = io::duplex(1 << 16);
        let (mog_read, mog_write) = io::split(mog_side);
        let (plugin_read, mut plugin_write) = io::split(plugin_side);
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let (plugin, _task) =
            Plugin::connect("words", mog_read, mog_write, &env::temp_dir(), events);
        tokio::spawn(async move {
            let mut reader = BufReader::new(plugin_read);
            while let Ok(Some(message)) = transport::read_message(&mut reader).await {
                let Message::Request { id, method, params } = message else {
                    continue;
                };
                let result = if method == "initialize" {
                    json!({ "commands": [{ "name": "count", "title": "Words: Count", "keys": ["alt+w"] }] })
                } else {
                    let words = params["context"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .split_whitespace()
                        .count();
                    json!({ "actions": [{ "type": "status", "text": format!("{words} words") }] })
                };
                let reply = Message::Response {
                    id,
                    result: Ok(result),
                };
                transport::write_message(&mut plugin_write, &reply)
                    .await
                    .expect("write");
            }
        });
        let ready = time::timeout(Duration::from_secs(5), events_rx.recv())
            .await
            .expect("in time")
            .expect("event");
        let PluginEvent::Ready { commands, .. } = ready else {
            panic!("expected ready, got {ready:?}");
        };
        assert_eq!(commands[0].title, "Words: Count");
        assert_eq!(commands[0].keys, ["alt+w"]);
        let actions = plugin
            .run("count", json!({ "text": "one two three" }))
            .await
            .expect("ran");
        assert_eq!(actions, [Action::Status("3 words".into())]);
    }
}
