//! A connection to one debug adapter process.

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    process::Stdio,
};

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

use crate::transport;

/// Something the debug adapter reported.
#[derive(Debug, Clone, PartialEq)]
pub enum DapEvent {
    /// The adapter is ready for breakpoints and `configurationDone`.
    Initialized,
    /// The program stopped, like at a breakpoint or after a step.
    Stopped {
        /// The thread that stopped, if the adapter said.
        thread: Option<i64>,
        /// Why, like `breakpoint`, `step` or `exception`.
        reason: String,
        /// More about why, like the exception message.
        text: Option<String>,
    },
    /// The program runs again.
    Continued,
    /// The program or the adapter printed something.
    Output {
        /// Where it came from, like `stdout`, `stderr` or `console`.
        category: String,
        /// What was printed.
        text: String,
    },
    /// The debugging session ended.
    Terminated,
    /// The program exited.
    Exited {
        /// Its exit code.
        code: Option<i64>,
    },
    /// The adapter process went away, with the last thing it printed to stderr.
    Gone {
        /// Why, if it said.
        reason: Option<String>,
    },
}

/// One frame of the call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The id to ask for its scopes with.
    pub id: i64,
    /// The function name.
    pub name: String,
    /// The file it is in, if it has source.
    pub path: Option<PathBuf>,
    /// The line, from 0.
    pub line: usize,
    /// The column, from 0.
    pub column: usize,
}

/// A group of variables in a frame, like locals or globals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// The name, like `Locals`.
    pub name: String,
    /// The id to ask for its variables with.
    pub reference: i64,
    /// Whether the adapter warns that reading it is slow.
    pub expensive: bool,
}

/// A variable and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// The name.
    pub name: String,
    /// The value as text.
    pub value: String,
    /// The type, if the adapter says.
    pub kind: Option<String>,
    /// The id to ask for its children with, 0 when it has none.
    pub reference: i64,
}

/// A message waiting to be sent to the adapter.
enum Outgoing {
    /// A request whose response body or error message goes to `reply`.
    Request {
        /// The command, like `launch`.
        command: String,
        /// The command arguments.
        arguments: Value,
        /// Where the answer goes.
        reply: oneshot::Sender<Result<Value, String>>,
    },
}

/// A running debug adapter. Clones share the connection.
#[derive(Clone)]
pub struct DebugClient {
    /// The queue of messages for the adapter.
    outgoing: UnboundedSender<Outgoing>,
}

/// Returns the path to run for `command`, finding npm style `.cmd` shims on Windows.
pub fn program_path(command: &str) -> PathBuf {
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

impl DebugClient {
    /// Starts `command` with `args` in `cwd` as a debug adapter talking over stdio, with `env`
    /// added to its environment.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be started.
    pub fn start(
        command: &str,
        args: &[String],
        cwd: &Path,
        env: &[(OsString, OsString)],
        events: UnboundedSender<DapEvent>,
    ) -> io::Result<Self> {
        let mut child = Command::new(program_path(command))
            .args(args)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(io::Error::other("debug adapter has no stdio"));
        };
        let (inner, mut forwarded) = mpsc::unbounded_channel();
        let stderr = tokio::spawn(last_line(stderr));
        tokio::spawn(async move {
            let mut stderr = Some(stderr);
            while let Some(mut event) = forwarded.recv().await {
                if let (DapEvent::Gone { reason }, Some(lines)) = (&mut event, stderr.take()) {
                    *reason = lines.await.ok().flatten();
                }
                if events.send(event).is_err() {
                    break;
                }
            }
        });
        let (client, connection) = Self::connect(stdout, stdin, inner);
        tokio::spawn(async move {
            let _ = connection.await;
            let _ = child.kill().await;
        });
        Ok(client)
    }

    /// Talks to an adapter that reads from `writer` and writes to `reader`.
    ///
    /// Returns the client and the background task driving the connection.
    pub fn connect<R, W>(
        reader: R,
        writer: W,
        events: UnboundedSender<DapEvent>,
    ) -> (Self, JoinHandle<()>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, queue) = mpsc::unbounded_channel();
        let connection = Connection {
            writer: BufWriter::new(writer),
            events,
            pending: HashMap::new(),
            next_seq: 1,
        };
        let handle = tokio::spawn(connection.run(reader, queue));
        (Self { outgoing }, handle)
    }

    /// Returns `true` while the connection to the adapter is open.
    pub fn is_running(&self) -> bool {
        !self.outgoing.is_closed()
    }

    /// Sends `command` with `arguments` and waits for the response body.
    ///
    /// # Errors
    ///
    /// Returns the adapter's error message, or why the adapter cannot be reached.
    pub async fn request(&self, command: &str, arguments: Value) -> Result<Value, String> {
        let (reply, answer) = oneshot::channel();
        self.outgoing
            .send(Outgoing::Request {
                command: command.to_owned(),
                arguments,
                reply,
            })
            .map_err(|_| "the debugger is not running".to_owned())?;
        answer
            .await
            .map_err(|_| "the debugger stopped".to_owned())?
    }

    /// Introduces mog to the adapter called `adapter`.
    ///
    /// # Errors
    ///
    /// Returns why the adapter refused.
    pub async fn initialize(&self, adapter: &str) -> Result<Value, String> {
        let arguments = json!({
            "clientID": "mog",
            "clientName": "mog",
            "adapterID": adapter,
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
            "supportsRunInTerminalRequest": false,
            "supportsVariableType": true,
        });
        self.request("initialize", arguments).await
    }

    /// Sets the breakpoints of `path` to `lines`, counted from 0.
    ///
    /// # Errors
    ///
    /// Returns why the adapter refused.
    pub async fn set_breakpoints(&self, path: &Path, lines: &[usize]) -> Result<Value, String> {
        let breakpoints: Vec<Value> = lines
            .iter()
            .map(|line| json!({ "line": line + 1 }))
            .collect();
        let arguments = json!({
            "source": { "path": path.to_string_lossy() },
            "breakpoints": breakpoints,
            "sourceModified": false,
        });
        self.request("setBreakpoints", arguments).await
    }

    /// Returns the threads of the program as `(id, name)`.
    ///
    /// # Errors
    ///
    /// Returns why the adapter refused.
    pub async fn threads(&self) -> Result<Vec<(i64, String)>, String> {
        let body = self.request("threads", json!({})).await?;
        Ok(body["threads"]
            .as_array()
            .map(|threads| {
                threads
                    .iter()
                    .filter_map(|thread| {
                        Some((
                            thread["id"].as_i64()?,
                            thread["name"].as_str().unwrap_or_default().to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Returns the call stack of `thread`, innermost first.
    ///
    /// # Errors
    ///
    /// Returns why the adapter refused.
    pub async fn stack_trace(&self, thread: i64) -> Result<Vec<Frame>, String> {
        let body = self
            .request("stackTrace", json!({ "threadId": thread, "levels": 50 }))
            .await?;
        Ok(body["stackFrames"]
            .as_array()
            .map(|frames| frames.iter().filter_map(parse_frame).collect())
            .unwrap_or_default())
    }

    /// Returns the scopes of the frame with `frame` id.
    ///
    /// # Errors
    ///
    /// Returns why the adapter refused.
    pub async fn scopes(&self, frame: i64) -> Result<Vec<Scope>, String> {
        let body = self.request("scopes", json!({ "frameId": frame })).await?;
        Ok(body["scopes"]
            .as_array()
            .map(|scopes| {
                scopes
                    .iter()
                    .filter_map(|scope| {
                        Some(Scope {
                            name: scope["name"].as_str()?.to_owned(),
                            reference: scope["variablesReference"].as_i64()?,
                            expensive: scope["expensive"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Returns the variables behind `reference`.
    ///
    /// # Errors
    ///
    /// Returns why the adapter refused.
    pub async fn variables(&self, reference: i64) -> Result<Vec<Variable>, String> {
        let body = self
            .request("variables", json!({ "variablesReference": reference }))
            .await?;
        Ok(body["variables"]
            .as_array()
            .map(|variables| {
                variables
                    .iter()
                    .filter_map(|variable| {
                        Some(Variable {
                            name: variable["name"].as_str()?.to_owned(),
                            value: variable["value"].as_str().unwrap_or_default().to_owned(),
                            kind: variable["type"].as_str().map(str::to_owned),
                            reference: variable["variablesReference"].as_i64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Reads a stack frame from its JSON form.
fn parse_frame(frame: &Value) -> Option<Frame> {
    let index = |key: &str| {
        frame[key]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(1)
            .saturating_sub(1)
    };
    Some(Frame {
        id: frame["id"].as_i64()?,
        name: frame["name"].as_str().unwrap_or("?").to_owned(),
        path: frame["source"]["path"].as_str().map(PathBuf::from),
        line: index("line"),
        column: index("column"),
    })
}

/// Turns an adapter event into a [`DapEvent`], or `None` for ones mog ignores.
fn parse_event(event: &str, body: &Value) -> Option<DapEvent> {
    Some(match event {
        "initialized" => DapEvent::Initialized,
        "stopped" => DapEvent::Stopped {
            thread: body["threadId"].as_i64(),
            reason: body["reason"].as_str().unwrap_or("paused").to_owned(),
            text: body["text"]
                .as_str()
                .or_else(|| body["description"].as_str())
                .map(str::to_owned),
        },
        "continued" => DapEvent::Continued,
        "output" => DapEvent::Output {
            category: body["category"].as_str().unwrap_or("console").to_owned(),
            text: body["output"].as_str().unwrap_or_default().to_owned(),
        },
        "terminated" => DapEvent::Terminated,
        "exited" => DapEvent::Exited {
            code: body["exitCode"].as_i64(),
        },
        _ => return None,
    })
}

/// The background half of a [`DebugClient`] that owns the pipes.
struct Connection<W> {
    /// The adapter stdin.
    writer: BufWriter<W>,
    /// Where events go.
    events: UnboundedSender<DapEvent>,
    /// Requests waiting for a response, by sequence number.
    pending: HashMap<i64, oneshot::Sender<Result<Value, String>>>,
    /// The sequence number of the next message.
    next_seq: i64,
}

impl<W: AsyncWrite + Unpin> Connection<W> {
    /// Runs until the adapter exits or every client is dropped.
    async fn run(
        mut self,
        reader: impl AsyncRead + Unpin + Send + 'static,
        mut queue: UnboundedReceiver<Outgoing>,
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
        loop {
            let result = tokio::select! {
                message = queue.recv() => match message {
                    Some(message) => self.send(message).await,
                    None => {
                        // nobody is debugging any more, so end the session politely
                        let _ = self.write("disconnect", json!({ "terminateDebuggee": true })).await;
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
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err("the debugger stopped".into()));
        }
        let _ = self.events.send(DapEvent::Gone { reason: None });
    }

    /// Writes a request for `command` and returns its sequence number.
    async fn write(&mut self, command: &str, arguments: Value) -> io::Result<i64> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let message = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        transport::write_message(&mut self.writer, &message).await?;
        Ok(seq)
    }

    /// Sends a queued message.
    async fn send(&mut self, message: Outgoing) -> io::Result<()> {
        let Outgoing::Request {
            command,
            arguments,
            reply,
        } = message;
        let seq = self.write(&command, arguments).await?;
        self.pending.insert(seq, reply);
        Ok(())
    }

    /// Handles a message from the adapter.
    async fn receive(&mut self, message: Value) -> io::Result<()> {
        match message["type"].as_str() {
            Some("response") => {
                let Some(reply) = message["request_seq"]
                    .as_i64()
                    .and_then(|seq| self.pending.remove(&seq))
                else {
                    return Ok(());
                };
                let answer = if message["success"].as_bool().unwrap_or(false) {
                    Ok(message["body"].clone())
                } else {
                    let error = message["body"]["error"]["format"]
                        .as_str()
                        .or_else(|| message["message"].as_str())
                        .unwrap_or("the debugger refused");
                    Err(error.to_owned())
                };
                let _ = reply.send(answer);
            }
            Some("event") => {
                let event = message["event"].as_str().unwrap_or_default();
                if let Some(event) = parse_event(event, &message["body"]) {
                    let _ = self.events.send(event);
                }
            }
            Some("request") => {
                // reverse requests like runInTerminal are not supported, saying so keeps the
                // adapter from waiting forever
                let seq = self.next_seq;
                self.next_seq += 1;
                let response = json!({
                    "seq": seq,
                    "type": "response",
                    "request_seq": message["seq"],
                    "command": message["command"],
                    "success": false,
                    "message": "mog does not support this",
                });
                transport::write_message(&mut self.writer, &response).await?;
            }
            _ => return Err(ErrorKind::InvalidData.into()),
        }
        Ok(())
    }
}

#[cfg(test)]
/// Tests for the debug adapter client.
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::{
        io::{self, BufReader},
        sync::mpsc,
        time,
    };

    use super::{DapEvent, DebugClient, parse_event, parse_frame};
    use crate::transport;

    /// Frames count lines from 0 and keep their source path.
    #[test]
    fn reads_frames() {
        let frame = json!({ "id": 7, "name": "main", "line": 12, "column": 5,
            "source": { "path": "/src/main.c" } });
        let frame = parse_frame(&frame).expect("frame");
        assert_eq!((frame.id, frame.line, frame.column), (7, 11, 4));
        assert_eq!(frame.path.as_deref(), Some("/src/main.c".as_ref()));
    }

    /// Stopped events carry the thread and why.
    #[test]
    fn reads_events() {
        let stopped = parse_event("stopped", &json!({ "threadId": 3, "reason": "breakpoint" }));
        assert_eq!(
            stopped,
            Some(DapEvent::Stopped {
                thread: Some(3),
                reason: "breakpoint".into(),
                text: None
            })
        );
        assert_eq!(parse_event("loadedSource", &Value::Null), None);
    }

    /// Requests get their response body back and events come through.
    #[tokio::test]
    async fn talks_to_an_adapter() {
        let (client_side, adapter_side) = io::duplex(1 << 16);
        let (client_read, client_write) = io::split(client_side);
        let (adapter_read, mut adapter_write) = io::split(adapter_side);
        let (events, mut events_rx) = mpsc::unbounded_channel();
        let (client, _task) = DebugClient::connect(client_read, client_write, events);
        let adapter = tokio::spawn(async move {
            let mut reader = BufReader::new(adapter_read);
            let request = transport::read_message(&mut reader)
                .await
                .expect("read")
                .expect("request");
            assert_eq!(request["command"], "threads");
            let response = json!({ "seq": 1, "type": "response", "request_seq": request["seq"],
                "success": true, "command": "threads",
                "body": { "threads": [{ "id": 1, "name": "main" }] } });
            transport::write_message(&mut adapter_write, &response)
                .await
                .expect("write");
            let event = json!({ "seq": 2, "type": "event", "event": "stopped",
                "body": { "threadId": 1, "reason": "step" } });
            transport::write_message(&mut adapter_write, &event)
                .await
                .expect("write");
        });
        let threads = time::timeout(Duration::from_secs(5), client.threads())
            .await
            .expect("in time")
            .expect("threads");
        assert_eq!(threads, [(1, "main".to_owned())]);
        let event = time::timeout(Duration::from_secs(5), events_rx.recv())
            .await
            .expect("in time")
            .expect("event");
        assert!(matches!(
            event,
            DapEvent::Stopped {
                thread: Some(1),
                ..
            }
        ));
        adapter.await.expect("adapter");
    }
}
