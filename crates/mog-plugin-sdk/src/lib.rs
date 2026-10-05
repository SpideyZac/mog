//! Write mog plugins in Rust.
//!
//! A plugin is a program mog runs that talks JSON-RPC over stdio, see `docs/plugins.md`. This
//! crate does the framing and dispatch so a plugin is only its handlers:
//!
//! ```no_run
//! use mog_plugin_sdk::{Command, Plugin, actions};
//!
//! Plugin::new()
//!     .command(
//!         Command::new("count").title("Words: Count"),
//!         |_mog, context, _args| {
//!             let words = context["text"]
//!                 .as_str()
//!                 .unwrap_or_default()
//!                 .split_whitespace()
//!                 .count();
//!             Ok(vec![actions::status(format!("{words} words"))])
//!         },
//!     )
//!     .run()
//!     .expect("talking to mog");
//! ```

pub mod actions;

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    io::{self, BufRead, BufReader, ErrorKind, Write},
};

use serde_json::{Map, Value, json};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 2;

/// Events mog sends as requests and waits on, the rest are notifications.
const REQUEST_EVENTS: &[&str] = &["before_save"];

/// The JSON-RPC error code for a failed request.
const REQUEST_FAILED: i64 = -32000;

/// A command a plugin adds to the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The name inside the plugin.
    name: String,
    /// What the palette shows.
    title: Option<String>,
    /// Keys the plugin would like bound to it.
    keys: Vec<String>,
    /// Whether it shows in the right click menu.
    menu: bool,
}

impl Command {
    /// Creates a command called `name`, which shows as `plugin.<plugin>.<name>` in mog.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            keys: Vec::new(),
            menu: false,
        }
    }

    /// Sets what the palette shows.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Asks for a key to be bound to it when nothing else uses the key.
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.keys.push(key.into());
        self
    }

    /// Shows it in the editor's right click menu.
    #[must_use]
    pub fn in_menu(mut self) -> Self {
        self.menu = true;
        self
    }
}

/// What a command handler returns: actions for mog, or an error message for the user.
pub type Actions = Result<Vec<Value>, String>;

/// Handles a command with the editor context and the arguments.
type CommandHandler<R, W> = Box<dyn FnMut(&mut Mog<R, W>, &Value, &Value) -> Actions>;

/// Handles an event, returning a result for events mog waits on.
type EventHandler<R, W> = Box<dyn FnMut(&mut Mog<R, W>, &Value) -> Result<Value, String>>;

/// A provider handler with the languages it covers, `None` for every file.
type Provider<R, W> = (Option<Vec<String>>, EventHandler<R, W>);

/// The connection to mog, handed to handlers so they can ask and tell it things.
pub struct Mog<R, W> {
    /// Where messages from mog come from.
    reader: R,
    /// Where messages to mog go.
    writer: W,
    /// Messages that arrived while waiting for an answer, handled after.
    waiting: VecDeque<Value>,
    /// The id of the next question to mog.
    next_id: u64,
    /// Requests mog gave up on, so their answers are not sent.
    cancelled: HashSet<String>,
    /// The project folder, once mog said hello.
    pub root: Option<String>,
    /// The settings from the user's config, once mog said hello.
    pub settings: Value,
    /// What mog says it can do, once it said hello.
    pub capabilities: Value,
}

impl<R: BufRead, W: Write> Mog<R, W> {
    /// Reads one message, or `None` when mog closed the pipe.
    fn read(&mut self) -> io::Result<Option<Value>> {
        let mut length = None;
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            let header = line.trim();
            if header.is_empty() {
                if length.is_some() {
                    break;
                }
                continue;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let length = length.ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "no length"))?;
        let mut body = vec![0; length];
        self.reader.read_exact(&mut body)?;
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))
    }

    /// Writes one message.
    fn write(&mut self, mut message: Value) -> io::Result<()> {
        message["jsonrpc"] = json!("2.0");
        let body = message.to_string();
        write!(self.writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
        self.writer.flush()
    }

    /// Returns the next message, ones put aside while asking first.
    fn next_message(&mut self) -> io::Result<Option<Value>> {
        match self.waiting.pop_front() {
            Some(message) => Ok(Some(message)),
            None => self.read(),
        }
    }

    /// Asks mog something, like `editor/text` or `ui/pick`, and waits for the answer.
    ///
    /// # Errors
    ///
    /// Returns mog's error message, or why mog could not be reached.
    pub fn ask(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = format!("ask-{}", self.next_id);
        self.write(json!({ "id": id, "method": method, "params": params }))
            .map_err(|err| err.to_string())?;
        loop {
            let Some(message) = self.read().map_err(|err| err.to_string())? else {
                return Err("mog went away".into());
            };
            if message["id"] == json!(id) && message.get("method").is_none() {
                return match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("mog said no").into()),
                    None => Ok(message["result"].clone()),
                };
            }
            if message["method"] == "$/cancelRequest" {
                self.cancelled.insert(message["params"]["id"].to_string());
                continue;
            }
            self.waiting.push_back(message);
        }
    }

    /// Sends mog a notification, like `segment` or `diagnostics`.
    pub fn notify(&mut self, method: &str, params: Value) {
        let _ = self.write(json!({ "method": method, "params": params }));
    }

    /// Shows text in the status line.
    pub fn status(&mut self, text: &str) {
        self.notify("status", json!({ "text": text }));
    }

    /// Adds a line to this plugin's log.
    pub fn log(&mut self, text: &str) {
        self.notify("log", json!({ "text": text }));
    }

    /// Asks mog to do `actions` now.
    pub fn actions(&mut self, actions: Vec<Value>) {
        self.notify("actions", json!({ "actions": actions }));
    }
}

/// A mog plugin. Add handlers, then call [`Plugin::run`].
pub struct Plugin<R = BufReader<io::Stdin>, W = io::Stdout> {
    /// The commands, in the order they were added.
    commands: Vec<(Command, CommandHandler<R, W>)>,
    /// The event handlers by event name.
    events: BTreeMap<String, EventHandler<R, W>>,
    /// The providers by name, with the languages they cover, `None` for every file.
    providers: BTreeMap<String, Provider<R, W>>,
}

impl<R: BufRead, W: Write> Default for Plugin<R, W> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            events: BTreeMap::new(),
            providers: BTreeMap::new(),
        }
    }
}

impl Plugin {
    /// Creates a plugin that talks to mog over stdio.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers mog over stdio until it says `shutdown` or goes away.
    ///
    /// # Errors
    ///
    /// Returns an error if reading or writing stdio fails.
    pub fn run(self) -> io::Result<()> {
        self.run_with(BufReader::new(io::stdin()), io::stdout())
    }
}

impl<R: BufRead, W: Write> Plugin<R, W> {
    /// Adds `command`, run by `handler` with the editor context and the arguments.
    #[must_use]
    pub fn command(
        mut self,
        command: Command,
        handler: impl FnMut(&mut Mog<R, W>, &Value, &Value) -> Actions + 'static,
    ) -> Self {
        self.commands.push((command, Box::new(handler)));
        self
    }

    /// Handles `event`, like `saved`. For `before_save`, return `{ "changes": [...] }`.
    #[must_use]
    pub fn on(
        mut self,
        event: &str,
        handler: impl FnMut(&mut Mog<R, W>, &Value) -> Result<Value, String> + 'static,
    ) -> Self {
        self.events.insert(event.to_owned(), Box::new(handler));
        self
    }

    /// Provides `provider`, like `completion` or `hover`, for files with one of `languages` as
    /// extension, or every file when it is `None`.
    #[must_use]
    pub fn provide(
        mut self,
        provider: &str,
        languages: Option<Vec<String>>,
        handler: impl FnMut(&mut Mog<R, W>, &Value) -> Result<Value, String> + 'static,
    ) -> Self {
        self.providers
            .insert(provider.to_owned(), (languages, Box::new(handler)));
        self
    }

    /// Returns the answer to `initialize`.
    fn hello(&self) -> Value {
        let commands: Vec<Value> = self
            .commands
            .iter()
            .map(|(command, _)| {
                json!({
                    "name": command.name,
                    "title": command.title.as_deref().unwrap_or(&command.name),
                    "keys": command.keys,
                    "menu": command.menu,
                })
            })
            .collect();
        let providers: Map<String, Value> = self
            .providers
            .iter()
            .map(|(name, (languages, _))| {
                let languages = languages.as_ref().map_or(json!(true), |list| json!(list));
                (name.clone(), languages)
            })
            .collect();
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "commands": commands,
            "events": self.events.keys().collect::<Vec<_>>(),
            "providers": providers,
        })
    }

    /// Answers the request `method`.
    fn answer(
        &mut self,
        mog: &mut Mog<R, W>,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        match method {
            "initialize" => {
                mog.root = params["root"].as_str().map(str::to_owned);
                mog.settings = params["settings"].clone();
                mog.capabilities = params["capabilities"].clone();
                Ok(self.hello())
            }
            "command" => {
                let name = params["command"].as_str().unwrap_or_default();
                let (_, handler) = self
                    .commands
                    .iter_mut()
                    .find(|(command, _)| command.name == name)
                    .ok_or_else(|| format!("no command called {name}"))?;
                let actions = handler(mog, &params["context"], &params["args"])?;
                Ok(json!({ "actions": actions }))
            }
            event if REQUEST_EVENTS.contains(&event) => match self.events.get_mut(event) {
                Some(handler) => handler(mog, params),
                None => Ok(json!({ "changes": [] })),
            },
            provider if provider.starts_with("provide/") => {
                let (_, handler) = self
                    .providers
                    .get_mut(&provider["provide/".len()..])
                    .ok_or_else(|| format!("no provider for {provider}"))?;
                handler(mog, params)
            }
            other => Err(format!("unknown request {other}")),
        }
    }

    /// Answers mog through `reader` and `writer` until it says `shutdown` or goes away.
    ///
    /// # Errors
    ///
    /// Returns an error if reading or writing fails.
    pub fn run_with(mut self, reader: R, writer: W) -> io::Result<()> {
        let mut mog = Mog {
            reader,
            writer,
            waiting: VecDeque::new(),
            next_id: 0,
            cancelled: HashSet::new(),
            root: None,
            settings: Value::Null,
            capabilities: Value::Null,
        };
        while let Some(message) = mog.next_message()? {
            let Some(method) = message["method"].as_str().map(str::to_owned) else {
                continue;
            };
            let params = &message["params"];
            if let Some(id) = message.get("id") {
                let reply = match self.answer(&mut mog, &method, params) {
                    Ok(result) => json!({ "id": id, "result": result }),
                    Err(error) => {
                        json!({ "id": id, "error": { "code": REQUEST_FAILED, "message": error } })
                    }
                };
                if !mog.cancelled.remove(&id.to_string()) {
                    mog.write(reply)?;
                }
                continue;
            }
            match method.as_str() {
                "event" => {
                    let kind = params["kind"].as_str().unwrap_or_default();
                    if let Some(handler) = self.events.get_mut(kind)
                        && let Err(err) = handler(&mut mog, params)
                    {
                        mog.log(&err);
                    }
                }
                "$/cancelRequest" => {
                    mog.cancelled.insert(params["id"].to_string());
                }
                "shutdown" => break,
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
/// Tests for the SDK against a scripted mog.
mod tests {
    use std::io::Cursor;

    use serde_json::{Value, json};

    use super::{Command, Plugin, actions};

    /// Frames `messages` the way mog sends them.
    fn script(messages: &[Value]) -> Cursor<Vec<u8>> {
        let mut bytes = Vec::new();
        for message in messages {
            let body = message.to_string();
            bytes.extend(format!("Content-Length: {}\r\n\r\n{body}", body.len()).bytes());
        }
        Cursor::new(bytes)
    }

    /// Reads back the framed answers.
    fn answers(bytes: &[u8]) -> Vec<Value> {
        let text = String::from_utf8_lossy(bytes);
        text.split("Content-Length: ")
            .filter_map(|part| part.split_once("\r\n\r\n"))
            .filter_map(|(_, body)| serde_json::from_str(body).ok())
            .collect()
    }

    /// The handshake, a command that asks mog something, events, providers and errors all
    /// work, and a cancelled request is not answered.
    #[test]
    fn talks_to_mog() {
        let input = script(&[
            json!({ "id": 0, "method": "initialize", "params": { "root": "/p", "settings": { "a": 1 } } }),
            json!({ "id": 1, "method": "command", "params": { "command": "count", "context": {} } }),
            json!({ "id": "ask-1", "result": { "text": "one two" } }),
            json!({ "id": 2, "method": "before_save", "params": {} }),
            json!({ "id": 3, "method": "provide/hover", "params": {} }),
            json!({ "id": 4, "method": "command", "params": { "command": "nope" } }),
            json!({ "method": "$/cancelRequest", "params": { "id": 5 } }),
            json!({ "id": 5, "method": "provide/hover", "params": {} }),
            json!({ "method": "shutdown" }),
        ]);
        let mut output = Vec::new();
        Plugin::default()
            .command(
                Command::new("count").title("Count").key("alt+c"),
                |mog, _, _| {
                    let text = mog.ask("editor/text", json!({}))?;
                    let words = text["text"].as_str().unwrap_or_default().split(' ').count();
                    Ok(vec![actions::status(format!("{words} words"))])
                },
            )
            .on("before_save", |_, _| {
                Ok(json!({ "changes": [actions::change(0, 1, "")] }))
            })
            .provide("hover", Some(vec!["md".into()]), |_, _| {
                Ok(json!({ "text": "hi" }))
            })
            .run_with(input, &mut output)
            .expect("ran");
        let answers = answers(&output);
        let by_id = |id: i64| {
            answers
                .iter()
                .find(|answer| answer["id"] == json!(id) && answer.get("method").is_none())
                .cloned()
        };
        let hello = &by_id(0).expect("hello")["result"];
        assert_eq!(hello["protocolVersion"], 2);
        assert_eq!(hello["commands"][0]["keys"], json!(["alt+c"]));
        assert_eq!(hello["events"], json!(["before_save"]));
        assert_eq!(hello["providers"]["hover"], json!(["md"]));
        assert_eq!(
            by_id(1).expect("count")["result"]["actions"][0]["text"],
            "2 words"
        );
        assert_eq!(by_id(2).expect("save")["result"]["changes"][0]["end"], 1);
        assert_eq!(by_id(3).expect("hover")["result"]["text"], "hi");
        assert!(
            by_id(4).expect("error")["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("no command"))
        );
        assert!(by_id(5).is_none());
    }
}
