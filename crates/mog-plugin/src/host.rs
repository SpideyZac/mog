//! Running a plugin process and talking to it.
//!
//! Every request mog sends has a timeout and is cancelled with `$/cancelRequest` when it runs
//! out, queues are bounded and messages have a size limit, so a stuck or chatty plugin cannot
//! hang or flood the editor.

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use mog_lsp::transport::{self, Message};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader, BufWriter},
    process::Command,
    sync::{
        mpsc::{self, Receiver, Sender, error::TrySendError},
        oneshot,
    },
    task::JoinHandle,
    time::{self, Instant},
};

use crate::protocol::{Action, Hello, initialize_params, parse_actions, parse_hello};

/// How long a plugin command may take unless the plugin or config says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a plugin may take to answer `initialize`.
const INIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The biggest message a plugin may send.
pub const MAX_MESSAGE: usize = 64 << 20;

/// How many messages may wait in each direction before more are refused.
pub const QUEUE: usize = 1024;

/// The JSON-RPC error code for a method that does not exist.
const METHOD_NOT_FOUND: i64 = -32601;

/// The JSON-RPC error code mog answers failed requests with.
const REQUEST_FAILED: i64 = -32000;

/// The id of the `initialize` request, below every other.
const INIT_ID: u64 = 0;

/// Where plugin events go, each tagged with the instance of the plugin that sent it, so events
/// from a plugin that was restarted are not mistaken for the new one.
pub type Events = Sender<(u64, PluginEvent)>;

/// How to run a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The plugin name, which namespaces its commands.
    pub name: String,
    /// The program to run, like `python`.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The plugin folder, when it has a manifest.
    pub dir: Option<PathBuf>,
    /// The folder with the SDKs that ship with mog, put on the plugin's import paths.
    pub sdk: Option<PathBuf>,
    /// Settings from the config, sent in `initialize`.
    pub settings: Value,
    /// How long a command may take.
    pub timeout: Duration,
    /// Tells this run of the plugin apart from earlier ones, see [`Events`].
    pub instance: u64,
}

/// Something a plugin did.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginEvent {
    /// The plugin started and said what it can do.
    Ready {
        /// The plugin name.
        plugin: String,
        /// What it said.
        hello: Hello,
    },
    /// The plugin sent a notification, like `actions` or `segment`.
    Notification {
        /// The plugin name.
        plugin: String,
        /// The method.
        method: String,
        /// The arguments.
        params: Value,
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
    /// The plugin printed a line to stderr.
    Log {
        /// The plugin name.
        plugin: String,
        /// The line.
        line: String,
    },
    /// The plugin process went away.
    Exited {
        /// The plugin name.
        plugin: String,
        /// Why, often the last thing it printed to stderr.
        reason: Option<String>,
    },
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

/// Returns the search path in the environment variable `name` with `dir` added at the end.
fn with_path(name: &str, dir: PathBuf) -> OsString {
    let mut paths: Vec<PathBuf> = env::var_os(name)
        .map(|paths| env::split_paths(&paths).collect())
        .unwrap_or_default();
    paths.push(dir);
    env::join_paths(&paths).unwrap_or_default()
}

/// A message waiting to be sent to the plugin.
#[derive(Debug)]
enum Outgoing {
    /// A request whose result goes to `reply`.
    Request {
        /// The request id.
        id: u64,
        /// The method.
        method: String,
        /// The arguments.
        params: Value,
        /// Where the result or error message goes.
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Gives up on the request with this id.
    Cancel(u64),
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
#[derive(Debug, Clone)]
pub struct Plugin {
    /// The plugin name.
    name: String,
    /// The queue of messages for the plugin.
    outgoing: Sender<Outgoing>,
    /// The id of the next request, shared by clones.
    next_id: Arc<AtomicU64>,
    /// How long a command may take.
    timeout: Duration,
}

/// Reads `stderr` line by line, sending each to `events`, and returns the last non empty one.
async fn read_stderr(
    name: String,
    instance: u64,
    stderr: impl AsyncRead + Unpin,
    events: Events,
) -> Option<String> {
    let mut lines = BufReader::new(stderr).lines();
    let mut last = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim_end().to_owned();
        if line.trim().is_empty() {
            continue;
        }
        // logs are nice to have, so a busy editor drops them instead of stalling the plugin
        let log = PluginEvent::Log {
            plugin: name.clone(),
            line: line.clone(),
        };
        let _ = events.try_send((instance, log));
        last = Some(line);
    }
    last
}

impl Plugin {
    /// Starts the plugin described by `spec` in the project folder `root`.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be started.
    pub fn start(spec: &Spec, root: &Path, events: Events) -> io::Result<Self> {
        let mut command = Command::new(program_path(&spec.command));
        command
            .args(&spec.args)
            .current_dir(root)
            .env("MOG_VERSION", env!("CARGO_PKG_VERSION"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &spec.dir {
            command.env("MOG_PLUGIN_DIR", dir);
        }
        // last on the path, so a copy of the sdk next to the plugin still wins
        if let Some(sdk) = &spec.sdk {
            command
                .env("MOG_SDK_DIR", sdk)
                .env("PYTHONPATH", with_path("PYTHONPATH", sdk.join("python")))
                .env("NODE_PATH", with_path("NODE_PATH", sdk.join("node")));
        }
        let mut child = command.spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(io::Error::other("plugin has no stdio"));
        };
        let (inner, mut forwarded) = mpsc::channel::<(u64, PluginEvent)>(QUEUE);
        let instance = spec.instance;
        let stderr = tokio::spawn(read_stderr(
            spec.name.clone(),
            instance,
            stderr,
            events.clone(),
        ));
        tokio::spawn(async move {
            let mut stderr = Some(stderr);
            while let Some(mut event) = forwarded.recv().await {
                if let (PluginEvent::Exited { reason, .. }, Some(lines)) =
                    (&mut event.1, stderr.take())
                {
                    let last = lines.await.ok().flatten();
                    if reason.is_none() {
                        *reason = last;
                    }
                }
                if events.send(event).await.is_err() {
                    break;
                }
            }
        });
        let params = initialize_params(&root.to_string_lossy(), &spec.settings);
        let (plugin, connection) = Self::connect(
            &spec.name,
            instance,
            stdout,
            stdin,
            params,
            spec.timeout,
            inner,
        );
        tokio::spawn(async move {
            let _ = connection.await;
            let _ = child.kill().await;
        });
        Ok(plugin)
    }

    /// Talks to run `instance` of a plugin that reads from `writer` and writes to `reader`,
    /// introducing mog with the `initialize` parameters `params`.
    pub fn connect<R, W>(
        name: &str,
        instance: u64,
        reader: R,
        writer: W,
        params: Value,
        timeout: Duration,
        events: Events,
    ) -> (Self, JoinHandle<()>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, queue) = mpsc::channel(QUEUE);
        let connection = Connection {
            name: name.to_owned(),
            instance,
            writer: BufWriter::new(writer),
            events,
            pending: HashMap::new(),
        };
        let handle = tokio::spawn(connection.run(reader, queue, params));
        (
            Self {
                name: name.to_owned(),
                outgoing,
                next_id: Arc::new(AtomicU64::new(INIT_ID + 1)),
                timeout,
            },
            handle,
        )
    }

    /// Returns the plugin name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns how long a command may take.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns whether the plugin is still connected.
    pub fn is_running(&self) -> bool {
        !self.outgoing.is_closed()
    }

    /// Queues `message`, saying why it could not be.
    fn queue(&self, message: Outgoing) -> Result<(), String> {
        self.outgoing.try_send(message).map_err(|err| match err {
            TrySendError::Full(_) => format!("{} is too busy to take more", self.name),
            TrySendError::Closed(_) => format!("{} is not running", self.name),
        })
    }

    /// Sends the request `method` and waits up to `timeout` for the answer, cancelling it if
    /// none comes.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error message, or why it did not answer.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.queue(Outgoing::Request {
            id,
            method: method.to_owned(),
            params,
            reply,
        })?;
        match time::timeout(timeout, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{} stopped", self.name)),
            Err(_) => {
                let _ = self.queue(Outgoing::Cancel(id));
                Err(format!(
                    "{} did not answer {method} in {}s",
                    self.name,
                    timeout.as_secs_f32()
                ))
            }
        }
    }

    /// Runs the plugin's `command` with `args` and what the editor looks like in `context`,
    /// and returns what it asks mog to do.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error message, why it cannot be reached, or what is wrong with its
    /// answer.
    pub async fn run(
        &self,
        command: &str,
        context: Value,
        args: Value,
    ) -> Result<Vec<Action>, String> {
        let params = json!({ "command": command, "context": context, "args": args });
        let result = self.request("command", params, self.timeout).await?;
        parse_actions(&result)
    }

    /// Answers the request `id` the plugin sent, with a result or an error message.
    pub fn respond(&self, id: Value, result: Result<Value, String>) {
        let _ = self.queue(Outgoing::Response { id, result });
    }

    /// Sends the notification `method`.
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.queue(Outgoing::Notification {
            method: method.to_owned(),
            params,
        });
    }

    /// Tells the plugin something happened, like `saved`, with details in `params`.
    pub fn event(&self, kind: &str, mut params: Value) {
        if !params.is_object() {
            params = json!({});
        }
        params["kind"] = json!(kind);
        self.notify("event", params);
    }
}

/// Something read from the plugin.
type Incoming = Result<Message, String>;

/// The background half of a [`Plugin`] that owns the pipes.
struct Connection<W> {
    /// The plugin name.
    name: String,
    /// Which run of the plugin this is.
    instance: u64,
    /// The plugin stdin.
    writer: BufWriter<W>,
    /// Where events go.
    events: Events,
    /// Requests waiting for a response, by id.
    pending: HashMap<u64, oneshot::Sender<Result<Value, String>>>,
}

/// What the main loop of a [`Connection`] should do next.
enum Step {
    /// Keep going.
    Continue,
    /// Stop, with a reason if something went wrong.
    Stop(Option<String>),
}

impl<W: AsyncWrite + Unpin> Connection<W> {
    /// Says hello, then runs until the plugin exits or every handle is dropped.
    async fn run(
        mut self,
        reader: impl AsyncRead + Unpin + Send + 'static,
        mut queue: Receiver<Outgoing>,
        params: Value,
    ) {
        let (incoming_tx, mut incoming) = mpsc::channel::<Incoming>(QUEUE);
        // reading is not cancel safe so it gets its own task instead of a select branch
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            loop {
                let message = match transport::read_message_limited(&mut reader, MAX_MESSAGE).await
                {
                    Ok(Some(message)) => Ok(message),
                    Ok(None) => break,
                    Err(err) => Err(format!("sent a broken message: {err}")),
                };
                let broken = message.is_err();
                if incoming_tx.send(message).await.is_err() || broken {
                    break;
                }
            }
        });
        let hello = Message::Request {
            id: json!(INIT_ID),
            method: "initialize".into(),
            params,
        };
        let mut reason = None;
        if self.write(hello).await.is_ok() {
            let deadline = time::sleep_until(Instant::now() + INIT_TIMEOUT);
            tokio::pin!(deadline);
            let mut greeted = false;
            loop {
                let step = tokio::select! {
                    message = queue.recv() => match message {
                        Some(message) => self.send(message).await,
                        None => {
                            let _ = self.write(Message::Notification {
                                method: "shutdown".into(),
                                params: Value::Null,
                            }).await;
                            Step::Stop(None)
                        }
                    },
                    message = incoming.recv() => match message {
                        Some(Ok(message)) => self.receive(message, &mut greeted).await,
                        Some(Err(err)) => Step::Stop(Some(err)),
                        None => Step::Stop(None),
                    },
                    () = &mut deadline, if !greeted => Step::Stop(Some(format!(
                        "did not answer initialize in {}s",
                        INIT_TIMEOUT.as_secs()
                    ))),
                };
                if let Step::Stop(why) = step {
                    reason = why;
                    break;
                }
            }
        }
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err(format!("{} stopped", self.name)));
        }
        let exited = PluginEvent::Exited {
            plugin: self.name.clone(),
            reason,
        };
        let _ = self.events.send((self.instance, exited)).await;
    }

    /// Writes `message` to the plugin.
    async fn write(&mut self, message: Message) -> io::Result<()> {
        transport::write_message(&mut self.writer, &message).await
    }

    /// Turns a write result into the next step.
    fn written(result: io::Result<()>) -> Step {
        match result {
            Ok(()) => Step::Continue,
            Err(_) => Step::Stop(None),
        }
    }

    /// Sends a queued message.
    async fn send(&mut self, message: Outgoing) -> Step {
        let written = match message {
            Outgoing::Request {
                id,
                method,
                params,
                reply,
            } => {
                self.pending.insert(id, reply);
                self.write(Message::Request {
                    id: json!(id),
                    method,
                    params,
                })
                .await
            }
            Outgoing::Cancel(id) => {
                if self.pending.remove(&id).is_none() {
                    return Step::Continue;
                }
                self.write(Message::Notification {
                    method: "$/cancelRequest".into(),
                    params: json!({ "id": id }),
                })
                .await
            }
            Outgoing::Notification { method, params } => {
                self.write(Message::Notification { method, params }).await
            }
            Outgoing::Response { id, result } => {
                let result =
                    result.map_err(|message| json!({ "code": REQUEST_FAILED, "message": message }));
                self.write(Message::Response { id, result }).await
            }
        };
        Self::written(written)
    }

    /// Sends `event` to mog, stopping if mog is gone.
    async fn emit(&mut self, event: PluginEvent) -> Step {
        match self.events.send((self.instance, event)).await {
            Ok(()) => Step::Continue,
            Err(_) => Step::Stop(None),
        }
    }

    /// Handles a message from the plugin, `greeted` saying whether `initialize` was answered.
    async fn receive(&mut self, message: Message, greeted: &mut bool) -> Step {
        match message {
            Message::Response { id, result } if id.as_u64() == Some(INIT_ID) && !*greeted => {
                *greeted = true;
                let hello = result
                    .map_err(|error| {
                        error["message"]
                            .as_str()
                            .unwrap_or("initialize failed")
                            .to_owned()
                    })
                    .and_then(|result| parse_hello(&result));
                match hello {
                    Ok(hello) => {
                        self.emit(PluginEvent::Ready {
                            plugin: self.name.clone(),
                            hello,
                        })
                        .await
                    }
                    Err(err) => {
                        let _ = self
                            .write(Message::Notification {
                                method: "shutdown".into(),
                                params: Value::Null,
                            })
                            .await;
                        Step::Stop(Some(err))
                    }
                }
            }
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
                Step::Continue
            }
            Message::Notification { method, params } => {
                self.emit(PluginEvent::Notification {
                    plugin: self.name.clone(),
                    method,
                    params,
                })
                .await
            }
            Message::Request { id, method, params } => {
                let event = PluginEvent::Request {
                    plugin: self.name.clone(),
                    id: id.clone(),
                    method,
                    params,
                };
                if self.events.send((self.instance, event)).await.is_ok() {
                    return Step::Continue;
                }
                // without mog listening nobody will answer, so say so instead of hanging
                let error = json!({ "code": METHOD_NOT_FOUND, "message": "mog is closing" });
                let written = self
                    .write(Message::Response {
                        id,
                        result: Err(error),
                    })
                    .await;
                Self::written(written)
            }
        }
    }
}
