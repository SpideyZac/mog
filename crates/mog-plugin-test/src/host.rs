//! The fake editor: documents in memory and answers to everything a plugin asks, the way mog
//! gives them.

use std::{
    collections::VecDeque,
    fs,
    future::Future,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

use mog_core::{Change, Key, KeyChord, Rope, Transaction};
use mog_plugin::{
    Action, DEFAULT_TIMEOUT, Edit, FileEdit, Hello, Plugin, PluginEvent, Spec, parse_actions,
    protocol::{initialize_params, parse_action, parse_selections},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::{self, Receiver},
    time::{self, Instant},
};

/// How long a plugin gets to answer `initialize`.
const START_WAIT: Duration = Duration::from_secs(15);

/// How many events from the plugin can wait.
const QUEUE: usize = 1024;

/// The size of the screen the fake editor pretends to have, for `ui/layout`.
const SCREEN: (u16, u16) = (120, 40);

/// A file open in the fake editor.
#[derive(Debug, Clone, Default)]
pub struct FakeDocument {
    /// Where it is, `None` for an untitled file.
    pub path: Option<PathBuf>,
    /// The text.
    pub text: Rope,
    /// Goes up on every change.
    pub version: u64,
    /// The selections as `(anchor, head)`, the main one first.
    pub selections: Vec<(usize, usize)>,
    /// The version when it was last saved.
    pub saved_version: u64,
    /// What the plugin last sent with `diagnostics` for it.
    pub diagnostics: Vec<Value>,
}

impl FakeDocument {
    /// Returns the language the way plugins see it, the file extension.
    pub fn language(&self) -> Option<String> {
        self.path
            .as_deref()
            .and_then(Path::extension)
            .map(|ext| ext.to_string_lossy().into_owned())
    }

    /// Returns the main selection.
    pub fn selection(&self) -> (usize, usize) {
        self.selections.first().copied().unwrap_or_default()
    }

    /// Applies `changes` in offsets of the text as it is, as one new version.
    ///
    /// # Errors
    ///
    /// Returns why the changes do not fit, like overlapping or running past the end.
    pub fn apply(&mut self, changes: Vec<Change>) -> Result<(), String> {
        let tx = Transaction::try_new(changes).map_err(|err| err.to_string())?;
        tx.check_bounds(self.text.len_chars())
            .map_err(|err| err.to_string())?;
        tx.apply(&mut self.text);
        self.selections = self
            .selections
            .iter()
            .map(|&(anchor, head)| (tx.map_pos(anchor), tx.map_pos(head)))
            .collect();
        self.version += 1;
        Ok(())
    }
}

/// Something the plugin sent mog: a notification, or a request it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// The method, like `segment` or `editor/text`.
    pub method: String,
    /// The params.
    pub params: Value,
}

/// A fake mog that runs one plugin.
///
/// Requests the plugin sends are answered while the host waits for something, like
/// [`FakeHost::command`] or [`FakeHost::wait_for`], so a test reads top to bottom.
#[derive(Debug)]
pub struct FakeHost {
    /// The plugin.
    plugin: Plugin,
    /// What it does.
    events: Receiver<(u64, PluginEvent)>,
    /// What it said it offers.
    hello: Hello,
    /// The project folder.
    root: PathBuf,
    /// The open files, never empty.
    documents: Vec<FakeDocument>,
    /// The index of the focused file.
    active: usize,
    /// Answers for the next `ui/pick` requests.
    picks: VecDeque<Value>,
    /// Answers for the next `ui/prompt` requests.
    prompts: VecDeque<Value>,
    /// Every notification it sent, oldest first.
    notifications: Vec<Seen>,
    /// Every request it sent, oldest first.
    requests: Vec<Seen>,
    /// What it printed to stderr and sent with `log` and `notify`, and problems the host saw.
    log: Vec<String>,
    /// The last status line text.
    status: Option<String>,
    /// The last thing shown in the output panel, as `(title, text)`.
    output: Option<(String, String)>,
    /// The mog commands it asked to run, as `(name, args)`.
    commands: Vec<(String, Value)>,
    /// The files it saved.
    saved: Vec<PathBuf>,
    /// Why it stopped, once it did.
    exited: Option<Option<String>>,
}

/// Returns the char `chord` types, if it types one.
fn typed_char(chord: &KeyChord) -> Option<char> {
    if chord.mods.ctrl || chord.mods.alt {
        return None;
    }
    match chord.key {
        Key::Char(' ') => Some(' '),
        Key::Char(ch) if chord.mods.shift => ch.to_uppercase().next(),
        Key::Char(ch) => Some(ch),
        _ => None,
    }
}

impl FakeHost {
    /// Starts the plugin described by `spec` in the project folder `root` and waits for it to
    /// answer `initialize`.
    ///
    /// # Errors
    ///
    /// Returns why it did not start, with what it printed.
    pub async fn start(spec: &Spec, root: &Path) -> Result<Self, String> {
        let (sender, events) = mpsc::channel(QUEUE);
        let plugin = Plugin::start(spec, root, sender)
            .map_err(|err| format!("could not start {}: {err}", spec.name))?;
        Self::greet(plugin, events, root).await
    }

    /// Talks to a plugin over `reader` and `writer`, like in-memory pipes to a plugin running in
    /// the same test, sending `settings` in `initialize`.
    ///
    /// # Errors
    ///
    /// Returns why it did not answer `initialize`.
    pub async fn connect<R, W>(
        name: &str,
        reader: R,
        writer: W,
        root: &Path,
        settings: &Value,
    ) -> Result<Self, String>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (sender, events) = mpsc::channel(QUEUE);
        let params = initialize_params(&root.to_string_lossy(), settings);
        let (plugin, _task) =
            Plugin::connect(name, 0, reader, writer, params, DEFAULT_TIMEOUT, sender);
        Self::greet(plugin, events, root).await
    }

    /// Waits for `plugin` to answer `initialize`.
    async fn greet(
        plugin: Plugin,
        mut events: Receiver<(u64, PluginEvent)>,
        root: &Path,
    ) -> Result<Self, String> {
        let deadline = Instant::now() + START_WAIT;
        let mut log = Vec::new();
        let name = plugin.name().to_owned();
        let failed = |why: String, log: &[String]| {
            let printed = log
                .iter()
                .filter(|line| !why.contains(line.as_str()))
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n  ");
            if printed.is_empty() {
                why
            } else {
                format!("{why}, it printed:\n  {printed}")
            }
        };
        let hello = loop {
            match time::timeout_at(deadline, events.recv()).await {
                Err(_) => {
                    let why = format!(
                        "{name} did not answer initialize in {}s",
                        START_WAIT.as_secs()
                    );
                    return Err(failed(why, &log));
                }
                Ok(None) => return Err(failed(format!("{name} went away"), &log)),
                Ok(Some((_, PluginEvent::Ready { hello, .. }))) => break hello,
                Ok(Some((_, PluginEvent::Exited { reason, .. }))) => {
                    let why = format!("{name} stopped: {}", reason.unwrap_or_default());
                    return Err(failed(why, &log));
                }
                Ok(Some((_, PluginEvent::Log { line, .. }))) => log.push(line),
                Ok(Some(_)) => {}
            }
        };
        Ok(Self {
            plugin,
            events,
            hello,
            root: root.to_owned(),
            documents: vec![FakeDocument {
                selections: vec![(0, 0)],
                ..FakeDocument::default()
            }],
            active: 0,
            picks: VecDeque::new(),
            prompts: VecDeque::new(),
            notifications: Vec::new(),
            requests: Vec::new(),
            log,
            status: None,
            output: None,
            commands: Vec::new(),
            saved: Vec::new(),
            exited: None,
        })
    }

    /// Returns what the plugin said it offers.
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Returns the plugin, to send it things the host has no helper for.
    pub fn plugin(&self) -> &Plugin {
        &self.plugin
    }

    /// Returns the project folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns `path` made absolute against the project.
    fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        }
    }

    /// Returns the index of the file at `path`, or the focused one for `None`.
    fn find(&self, path: Option<&Path>) -> Option<usize> {
        match path {
            None => Some(self.active),
            Some(path) => {
                let path = self.resolve(path);
                self.documents
                    .iter()
                    .position(|document| document.path.as_deref() == Some(path.as_path()))
            }
        }
    }

    /// Returns the index of the file at `path`, reading it from disk if it is not open yet.
    fn find_or_load(&mut self, path: Option<&Path>) -> Result<usize, String> {
        if let Some(index) = self.find(path) {
            return Ok(index);
        }
        let path = self.resolve(path.ok_or("no file is open")?);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
            Err(err) => return Err(format!("could not open {}: {err}", path.display())),
        };
        self.documents.push(FakeDocument {
            path: Some(path),
            text: Rope::from_str(&text),
            selections: vec![(0, 0)],
            ..FakeDocument::default()
        });
        Ok(self.documents.len() - 1)
    }

    /// Opens `text` as the file at `path`, relative to the project, and focuses it, telling the
    /// plugin like mog does. Replaces a file already open there.
    pub fn open(&mut self, path: impl AsRef<Path>, text: &str) {
        let path = self.resolve(path.as_ref());
        let document = FakeDocument {
            path: Some(path.clone()),
            text: Rope::from_str(text),
            selections: vec![(0, 0)],
            ..FakeDocument::default()
        };
        let untouched = |document: &FakeDocument| {
            document.path.is_none() && document.version == 0 && document.text.len_chars() == 0
        };
        match self.find(Some(&path)) {
            Some(index) => self.documents[index] = document,
            None if self.documents.len() == 1 && untouched(&self.documents[0]) => {
                self.documents[0] = document;
            }
            None => self.documents.push(document),
        }
        self.active = self.find(Some(&path)).unwrap_or(0);
        self.focused();
    }

    /// Focuses the open file at `path`, telling the plugin like mog does.
    ///
    /// # Errors
    ///
    /// Returns an error if the file is not open.
    pub fn focus(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        self.active = self
            .find(Some(path))
            .ok_or_else(|| format!("{} is not open", path.display()))?;
        self.focused();
        Ok(())
    }

    /// Sends `opened` for the focused file, if the plugin listens.
    fn focused(&self) {
        let document = &self.documents[self.active];
        self.send_event(
            "opened",
            json!({
                "path": document.path.as_ref().map(|path| path.to_string_lossy()),
                "language": document.language(),
            }),
        );
    }

    /// Replaces the text of the focused file as if the user typed it, sending `changed` with the
    /// whole text if the plugin listens.
    pub fn set_text(&mut self, text: &str) {
        let document = &mut self.documents[self.active];
        let len = document.text.len_chars();
        document.text = Rope::from_str(text);
        document.version += 1;
        let end = document.text.len_chars();
        document.selections = vec![(end.min(len), end.min(len))];
        let params = json!({
            "path": document.path.as_ref().map(|path| path.to_string_lossy()),
            "version": document.version,
            "text": text,
        });
        self.send_event("changed", params);
    }

    /// Selects `anchor..head` in the focused file.
    pub fn select(&mut self, anchor: usize, head: usize) {
        let document = &mut self.documents[self.active];
        let len = document.text.len_chars();
        document.selections = vec![(anchor.min(len), head.min(len))];
    }

    /// Returns the focused file.
    pub fn active(&self) -> &FakeDocument {
        &self.documents[self.active]
    }

    /// Returns the open file at `path`, relative to the project.
    pub fn document(&self, path: impl AsRef<Path>) -> Option<&FakeDocument> {
        self.find(Some(path.as_ref()))
            .map(|index| &self.documents[index])
    }

    /// Returns the text of the focused file.
    pub fn text(&self) -> String {
        self.active().text.to_string()
    }

    /// Returns the last status line text, from `status` and `notify`.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Returns the last output panel text as `(title, text)`.
    pub fn output(&self) -> Option<(&str, &str)> {
        self.output
            .as_ref()
            .map(|(title, text)| (title.as_str(), text.as_str()))
    }

    /// Returns every notification the plugin sent, oldest first.
    pub fn notifications(&self) -> &[Seen] {
        &self.notifications
    }

    /// Returns every request the plugin sent, oldest first.
    pub fn requests(&self) -> &[Seen] {
        &self.requests
    }

    /// Returns what the plugin printed and logged, and problems the host saw.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// Returns the mog commands the plugin asked to run, as `(name, args)`.
    pub fn commands(&self) -> &[(String, Value)] {
        &self.commands
    }

    /// Returns the files the plugin saved.
    pub fn saved(&self) -> &[PathBuf] {
        &self.saved
    }

    /// Returns why the plugin stopped, `Some(None)` when it gave no reason, or `None` while it
    /// runs.
    pub fn exited(&self) -> Option<Option<&str>> {
        self.exited.as_ref().map(Option::as_deref)
    }

    /// Answers the next `ui/pick` with the item at `index`, or as if the user closed the list
    /// for `None`.
    pub fn queue_pick(&mut self, index: Option<usize>) {
        self.picks.push_back(json!(index));
    }

    /// Answers the next `ui/prompt` with `text`, or as if the user closed it for `None`.
    pub fn queue_prompt(&mut self, text: Option<&str>) {
        self.prompts.push_back(json!(text));
    }

    /// Returns where the cursor is and which file it is in, like the `command` context.
    pub fn context(&self) -> Value {
        let mut context = self.cursor();
        context["text"] = json!(self.text());
        context
    }

    /// Returns the context without the text, like the `key` request gets.
    fn cursor(&self) -> Value {
        let document = self.active();
        let text = &document.text;
        let (anchor, head) = document.selection();
        let line = text.char_to_line(head.min(text.len_chars()));
        let selections: Vec<Value> = document
            .selections
            .iter()
            .map(|(anchor, head)| json!({ "anchor": anchor, "head": head }))
            .collect();
        json!({
            "root": self.root.to_string_lossy(),
            "path": document.path.as_ref().map(|path| path.to_string_lossy()),
            "language": document.language(),
            "length": text.len_chars(),
            "lines": text.len_lines(),
            "version": document.version,
            "selection": { "anchor": anchor, "head": head },
            "selections": selections,
            "line": line,
            "column": head - text.line_to_char(line),
            "modified": document.version != document.saved_version,
        })
    }

    /// Tells the plugin `kind` happened, if it listens for it.
    pub fn send_event(&self, kind: &str, params: Value) {
        if self.hello.events.contains(kind) {
            self.plugin.event(kind, params);
        }
    }

    /// Tells the plugin `kind` happened whether it listens or not, like `click` and `timer`.
    pub fn force_event(&self, kind: &str, params: Value) {
        self.plugin.event(kind, params);
    }

    /// Runs the plugin's command `name` with `args` and does the actions it answers with.
    ///
    /// Returns the raw answer.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error, why it did not answer, or what is wrong with its actions.
    pub async fn command(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let params = json!({ "command": name, "context": self.context(), "args": args });
        let timeout = self.plugin.timeout();
        let answer = self.request("command", params, timeout).await?;
        self.apply(&answer)?;
        Ok(answer)
    }

    /// Asks the plugin for the feature `provider`, like `completion` or `hover`, at the cursor
    /// of the focused file, with `extra` params added to the ones mog always sends.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error or why it did not answer.
    pub async fn provide(&mut self, provider: &str, extra: Value) -> Result<Value, String> {
        let mut params = self.cursor();
        let document = self.active();
        let (anchor, head) = document.selection();
        params["offset"] = json!(head);
        params["selection"] = json!({ "anchor": anchor, "head": head });
        if let (Value::Object(params), Value::Object(extra)) = (&mut params, extra) {
            params.extend(extra);
        }
        self.request(&format!("provide/{provider}"), params, DEFAULT_TIMEOUT)
            .await
    }

    /// Sends the key `chord`, like `j` or `ctrl+r`, as the `key` request a plugin that takes
    /// keys gets, and does the actions it answers with.
    ///
    /// Returns the raw answer.
    ///
    /// # Errors
    ///
    /// Returns an error if the key does not parse, or the plugin's error.
    pub async fn key(&mut self, chord: &str) -> Result<Value, String> {
        let parsed: KeyChord = chord.parse().map_err(|err| format!("{chord}: {err}"))?;
        let mut params = self.cursor();
        params["key"] = json!(chord);
        params["char"] = json!(typed_char(&parsed).map(String::from));
        let answer = self.request("key", params, DEFAULT_TIMEOUT).await?;
        self.apply(&answer)?;
        Ok(answer)
    }

    /// Asks the plugin to change the focused file before it is saved, applies its changes and
    /// saves.
    ///
    /// Returns the raw answer.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error or why its changes do not fit.
    pub async fn before_save(&mut self) -> Result<Value, String> {
        let document = self.active();
        let params = json!({
            "path": document.path.as_ref().map(|path| path.to_string_lossy()),
            "language": document.language(),
            "version": document.version,
            "text": document.text.to_string(),
        });
        let answer = self
            .request("before_save", params, Duration::from_secs(2))
            .await?;
        let edit = parse_action(&json!({
            "type": "edit",
            "changes": answer.get("changes").cloned().unwrap_or(json!([])),
        }))?;
        if let Action::Edit(edit) = edit {
            self.edit_file(edit)?;
        }
        self.save(None)?;
        Ok(answer)
    }

    /// Sends the request `method` and waits up to `timeout` for the answer, answering what the
    /// plugin asks meanwhile.
    ///
    /// # Errors
    ///
    /// Returns the plugin's error or why it did not answer.
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let plugin = self.plugin.clone();
        let method = method.to_owned();
        self.pump(async move { plugin.request(&method, params, timeout).await })
            .await
    }

    /// Answers the plugin until `done` says so or `timeout` runs out.
    ///
    /// Returns whether `done` said so.
    pub async fn wait_for(&mut self, timeout: Duration, done: impl Fn(&Self) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while !done(self) {
            match time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some((_, event))) => self.handle(event),
                Ok(None) | Err(_) => return done(self),
            }
        }
        true
    }

    /// Answers the plugin for `duration`.
    pub async fn settle(&mut self, duration: Duration) {
        self.wait_for(duration, |_| false).await;
    }

    /// Asks the plugin to stop, like mog does when it quits, and waits a moment for it.
    pub async fn shutdown(self) {
        let Self {
            plugin, mut events, ..
        } = self;
        drop(plugin);
        let _ = time::timeout(Duration::from_secs(1), async {
            while let Some((_, event)) = events.recv().await {
                if matches!(event, PluginEvent::Exited { .. }) {
                    break;
                }
            }
        })
        .await;
    }

    /// Runs `work`, answering the plugin meanwhile, then handles what it sent before `work`
    /// finished.
    async fn pump<T>(&mut self, work: impl Future<Output = T>) -> T {
        tokio::pin!(work);
        let out = loop {
            tokio::select! {
                out = &mut work => break out,
                event = self.events.recv() => match event {
                    Some((_, event)) => self.handle(event),
                    None => break work.await,
                },
            }
        };
        while let Ok((_, event)) = self.events.try_recv() {
            self.handle(event);
        }
        out
    }

    /// Handles something the plugin did.
    fn handle(&mut self, event: PluginEvent) {
        match event {
            PluginEvent::Ready { .. } => {}
            PluginEvent::Log { line, .. } => self.log.push(line),
            PluginEvent::Exited { reason, .. } => self.exited = Some(reason),
            PluginEvent::Notification { method, params, .. } => {
                self.notified(&method, &params);
                self.notifications.push(Seen { method, params });
            }
            PluginEvent::Request {
                id, method, params, ..
            } => {
                let result = self.answer(&method, &params);
                self.requests.push(Seen { method, params });
                self.plugin.respond(id, result);
            }
        }
    }

    /// Acts on a notification, like mog would.
    fn notified(&mut self, method: &str, params: &Value) {
        let text = || params["text"].as_str().unwrap_or_default().to_owned();
        match method {
            "actions" => {
                if let Err(err) = self.apply(params) {
                    self.log.push(format!("actions: {err}"));
                }
            }
            "status" => self.status = Some(text()),
            "notify" => {
                self.log.push(text());
                self.status = Some(text());
            }
            "log" => self.log.push(text()),
            "output" => {
                let title = params["title"].as_str().unwrap_or("plugin").to_owned();
                self.output = Some((title, text()));
            }
            "diagnostics" => {
                let path = params["path"].as_str().map(Path::new);
                match self.find(path) {
                    Some(index) => {
                        self.documents[index].diagnostics = params["diagnostics"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                    }
                    None => self.log.push("diagnostics: the file is not open".into()),
                }
            }
            _ => {}
        }
    }

    /// Answers a request from the plugin.
    fn answer(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "editor/context" => Ok(self.context()),
            "editor/text" => self.read_text(params),
            "editor/documents" => {
                let documents: Vec<Value> = self
                    .documents
                    .iter()
                    .enumerate()
                    .map(|(index, document)| {
                        json!({
                            "path": document.path.as_ref().map(|path| path.to_string_lossy()),
                            "name": document.path.as_deref().and_then(Path::file_name)
                                .map_or_else(|| "untitled".into(), |name| name.to_string_lossy()),
                            "language": document.language(),
                            "version": document.version,
                            "modified": document.version != document.saved_version,
                            "active": index == self.active,
                        })
                    })
                    .collect();
                Ok(json!({ "documents": documents }))
            }
            "editor/diagnostics" => {
                let index = self
                    .find(params["path"].as_str().map(Path::new))
                    .ok_or("the file is not open")?;
                Ok(json!({ "diagnostics": self.documents[index].diagnostics }))
            }
            "editor/select" => {
                let path = params["path"].as_str().map(PathBuf::from);
                let selections = parse_selections(&params["selections"])?;
                self.select_in(path.as_deref(), selections)
                    .map(|()| json!({}))
            }
            "editor/save" => {
                let path = params["path"].as_str().map(PathBuf::from);
                self.save(path.as_deref()).map(|()| json!({}))
            }
            "actions" => self.apply(params).map(|()| json!({})),
            "ui/pick" => {
                let Some(answer) = self.picks.pop_front() else {
                    self.log
                        .push("ui/pick was not answered, queue a pick in the test".into());
                    return Ok(Value::Null);
                };
                let Some(index) = answer.as_u64() else {
                    return Ok(Value::Null);
                };
                let item = &params["items"][usize::try_from(index).unwrap_or(usize::MAX)];
                let label = item
                    .as_str()
                    .or_else(|| item["label"].as_str())
                    .ok_or_else(|| format!("the test picked {index} but there is no such item"))?;
                Ok(json!({ "index": index, "item": label }))
            }
            "ui/prompt" => {
                let Some(answer) = self.prompts.pop_front() else {
                    self.log
                        .push("ui/prompt was not answered, queue a prompt in the test".into());
                    return Ok(Value::Null);
                };
                Ok(answer
                    .as_str()
                    .map_or(Value::Null, |text| json!({ "text": text })))
            }
            "ui/layout" => {
                let (width, height) = SCREEN;
                Ok(json!({
                    "screen": { "x": 0, "y": 0, "width": width, "height": height },
                    "editor": { "x": 0, "y": 1, "width": width, "height": height - 2 },
                    "split": null,
                    "status": { "x": 0, "y": height - 1, "width": width, "height": 1 },
                    "cursor": { "x": 0, "y": 1 },
                    "flair": true,
                    "reduced_motion": false,
                    "theme": "mog",
                }))
            }
            other => Err(format!("mog does not know {other}")),
        }
    }

    /// Answers `editor/text`.
    fn read_text(&mut self, params: &Value) -> Result<Value, String> {
        let path = params["path"].as_str().map(PathBuf::from);
        let index = self.find_or_load(path.as_deref())?;
        let document = &self.documents[index];
        let len = document.text.len_chars();
        let offset = |value: &Value, default: usize| match value.as_u64() {
            None => Ok(default),
            Some(value) => usize::try_from(value)
                .ok()
                .filter(|value| *value <= len)
                .ok_or_else(|| format!("{value} is past the end, the text has {len} chars")),
        };
        let start = offset(&params["start"], 0)?;
        let end = offset(&params["end"], len)?;
        if start > end {
            return Err(format!("start {start} is after end {end}"));
        }
        Ok(json!({
            "text": document.text.slice(start..end).to_string(),
            "version": document.version,
            "length": len,
        }))
    }

    /// Sets the selections of the file at `path`, opening and focusing it.
    fn select_in(
        &mut self,
        path: Option<&Path>,
        selections: Vec<(usize, usize)>,
    ) -> Result<(), String> {
        let index = self.find_or_load(path)?;
        let document = &mut self.documents[index];
        let len = document.text.len_chars();
        if let Some((anchor, head)) = selections
            .iter()
            .find(|(anchor, head)| *anchor > len || *head > len)
        {
            return Err(format!(
                "selection {anchor}..{head} is past the end, the text has {len} chars"
            ));
        }
        document.selections = selections;
        self.active = index;
        Ok(())
    }

    /// Saves the file at `path`, or the focused one.
    fn save(&mut self, path: Option<&Path>) -> Result<(), String> {
        let index = self.find(path).ok_or("the file is not open")?;
        let document = &mut self.documents[index];
        let path = document.path.clone().ok_or("the file has no name yet")?;
        document.saved_version = document.version;
        self.saved.push(path);
        Ok(())
    }

    /// Changes one file, checking its version first.
    fn edit_file(&mut self, edit: FileEdit) -> Result<(), String> {
        self.edit_files(vec![edit])
    }

    /// Changes files, merging parts for the same file, all of them or none like mog.
    fn edit_files(&mut self, edits: Vec<FileEdit>) -> Result<(), String> {
        let mut groups: Vec<(usize, Vec<Change>)> = Vec::new();
        for edit in edits {
            let index = self.find_or_load(edit.path.as_deref())?;
            let document = &self.documents[index];
            if let Some(version) = edit.version
                && version != document.version
            {
                return Err(format!(
                    "the file changed since version {version}, it is at {} now",
                    document.version
                ));
            }
            let changes = edit
                .changes
                .into_iter()
                .map(|Edit { start, end, text }| Change { start, end, text });
            match groups.iter_mut().find(|(other, _)| *other == index) {
                Some((_, group)) => group.extend(changes),
                None => groups.push((index, changes.collect())),
            }
        }
        for (index, changes) in &groups {
            let tx = Transaction::try_new(changes.clone()).map_err(|err| err.to_string())?;
            tx.check_bounds(self.documents[*index].text.len_chars())
                .map_err(|err| err.to_string())?;
        }
        for (index, changes) in groups {
            self.documents[index].apply(changes)?;
        }
        Ok(())
    }

    /// Does the actions in a command answer or an `actions` message, stopping at the first that
    /// fails.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with the actions or why one failed.
    pub fn apply(&mut self, value: &Value) -> Result<(), String> {
        for action in parse_actions(value)? {
            match action {
                Action::Status(text) | Action::Notify { text, .. } => self.status = Some(text),
                Action::Insert(text) => {
                    let (anchor, head) = self.active().selection();
                    let (start, end) = (anchor.min(head), anchor.max(head));
                    let after = start + text.chars().count();
                    let document = &mut self.documents[self.active];
                    document.apply(vec![Change { start, end, text }])?;
                    document.selections = vec![(after, after)];
                }
                Action::Edit(edit) => self.edit_file(edit)?,
                Action::WorkspaceEdit(edits) => self.edit_files(edits)?,
                Action::Open { path, line, column } => {
                    let index = self.find_or_load(Some(&path))?;
                    self.active = index;
                    if let Some(line) = line {
                        let text = &self.documents[index].text;
                        let line = line.min(text.len_lines().saturating_sub(1));
                        let start = text.line_to_char(line);
                        let pos = start + column.unwrap_or(0).min(text.line(line).len_chars());
                        self.documents[index].selections = vec![(pos, pos)];
                    }
                    self.focused();
                }
                Action::Select { path, selections } => {
                    self.select_in(path.as_deref(), selections)?;
                }
                Action::Save(path) => self.save(path.as_deref())?,
                Action::Command { name, args } => self.commands.push((name, args)),
                Action::Output { title, text } => self.output = Some((title, text)),
            }
        }
        Ok(())
    }
}
