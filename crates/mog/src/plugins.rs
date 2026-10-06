//! Starting plugins from the config and the plugin folder, keeping them running and routing
//! their commands and events.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as _,
    future::Future,
    mem,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use futures::future;
use mog_config::{PluginConfig, config_dir};
use mog_core::KeyChord;
use mog_plugin::{
    Action, DEFAULT_TIMEOUT, Hello, Manifest, Plugin, PluginCommand, PluginEvent, Spec, Stats,
    discover,
};
use serde_json::Value;
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedReceiver, UnboundedSender};

use crate::sdk;

/// The prefix of every plugin command name, like `plugin.words.count`.
pub const PREFIX: &str = "plugin.";

/// How many times in a row a crashing plugin is restarted before mog gives up on it.
const MAX_RESTARTS: u32 = 5;

/// How long a plugin has to run before a crash no longer counts as crashing in a row.
const STABLE_AFTER: Duration = Duration::from_secs(60);

/// How many requests may time out within [`TIMEOUT_WINDOW`] before mog stops a plugin.
const MAX_TIMEOUTS: usize = 5;

/// The time [`MAX_TIMEOUTS`] are counted over.
const TIMEOUT_WINDOW: Duration = Duration::from_secs(300);

/// How often mog says a plugin is slow at most.
const SLOW_WARNING_EVERY: Duration = Duration::from_secs(60);

/// How many stderr lines are kept for each plugin.
const LOG_LINES: usize = 500;

/// How many events from plugins can wait for the editor.
const EVENT_QUEUE: usize = 1024;

/// Something plugins reported.
#[derive(Debug)]
pub enum PluginUpdate {
    /// A plugin did something on its own.
    Event(PluginEvent),
    /// A command finished, with what it asks for or why it failed.
    Ran(String, Result<Vec<Action>, String>),
    /// Plugins answered `before_save` for the document at `path` and `version`, each with the
    /// changes it wants or why it failed.
    BeforeSave {
        /// The file about to be saved.
        path: PathBuf,
        /// The document version they answered for.
        version: u64,
        /// Each plugin's answer as `(plugin, result)`.
        answers: Vec<(String, Result<Value, String>)>,
    },
    /// A plugin answered `provide/diagnostics` for the file at `path` and `version`.
    Diagnostics {
        /// The plugin.
        plugin: String,
        /// The file.
        path: PathBuf,
        /// The document version it answered for.
        version: u64,
        /// Its answer, or why there is none.
        answer: Result<Value, String>,
    },
    /// Plugins answered `provide/tasks`, as `(plugin, answer)`.
    Tasks(Vec<(String, Value)>),
    /// A plugin that takes keys answered what to do with one.
    Key {
        /// The plugin that took the key.
        plugin: String,
        /// The key.
        chord: KeyChord,
        /// Its answer, or why there is none.
        answer: Result<Value, String>,
    },
}

/// Whether a plugin process is running.
#[derive(Debug)]
enum Run {
    /// Not running, waiting for an activation or a restart.
    Stopped,
    /// Started, waiting for the handshake.
    Starting(Plugin),
    /// Running and introduced.
    Ready(Plugin, Hello),
}

/// One configured plugin.
#[derive(Debug)]
struct Entry {
    /// How to run it.
    spec: Spec,
    /// Its manifest, if it has one.
    manifest: Option<Manifest>,
    /// Whether it runs.
    run: Run,
    /// Commands asked for before it was ready, run once it is.
    queued: Vec<(String, Value, Value)>,
    /// How many times it crashed in a row.
    restarts: u32,
    /// When it last started.
    started: Instant,
    /// When to start it again after a crash.
    restart_at: Option<Instant>,
    /// What it printed to stderr, newest last.
    log: VecDeque<String>,
    /// Why mog stopped it, when it did, like timing out too often.
    disabled: Option<String>,
    /// When mog last said it is slow.
    warned: Option<Instant>,
}

impl Entry {
    /// Returns the plugin handle if it is running.
    fn plugin(&self) -> Option<&Plugin> {
        match &self.run {
            Run::Stopped => None,
            Run::Starting(plugin) | Run::Ready(plugin, _) => Some(plugin),
        }
    }

    /// Returns what the plugin said about itself, once it is ready.
    fn hello(&self) -> Option<&Hello> {
        match &self.run {
            Run::Ready(_, hello) => Some(hello),
            _ => None,
        }
    }

    /// Returns its commands, from the handshake or else from the manifest.
    fn commands(&self) -> &[PluginCommand] {
        match (self.hello(), &self.manifest) {
            (Some(hello), _) => &hello.commands,
            (None, Some(manifest)) => &manifest.commands,
            (None, None) => &[],
        }
    }

    /// Returns whether it crashed or timed out too often and mog gave up on it.
    fn gave_up(&self) -> bool {
        matches!(self.run, Run::Stopped)
            && self.restart_at.is_none()
            && (self.restarts >= MAX_RESTARTS || self.disabled.is_some())
    }

    /// Returns whether it starts as soon as mog does.
    fn starts_at_once(&self) -> bool {
        self.manifest.as_ref().is_none_or(Manifest::starts_at_once)
    }

    /// Adds a line to the log, dropping the oldest past [`LOG_LINES`].
    fn log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }
}

/// The plugins and the commands they added.
pub struct Plugins {
    /// Every configured plugin by name.
    entries: BTreeMap<String, Entry>,
    /// The project the plugins run in.
    root: PathBuf,
    /// The folder plugin folders are found in.
    dir: Option<PathBuf>,
    /// The instance the next started plugin gets.
    next_instance: u64,
    /// Where plugins send events.
    sender: Sender<(u64, PluginEvent)>,
    /// Events from plugins.
    events: Receiver<(u64, PluginEvent)>,
    /// Where background work reports.
    done_tx: UnboundedSender<PluginUpdate>,
    /// Finished background work.
    done: UnboundedReceiver<PluginUpdate>,
}

/// Returns the full command name for `command` of `plugin`.
pub fn command_name(plugin: &str, command: &str) -> String {
    format!("{PREFIX}{plugin}.{command}")
}

/// Splits a full command name like `plugin.words.count:arg` into the plugin, the command and
/// the argument text after a colon, if any.
pub fn split_command(full: &str) -> Option<(&str, &str, Option<&str>)> {
    let rest = full.strip_prefix(PREFIX)?;
    let (plugin, command) = rest.split_once('.')?;
    Some(match command.split_once(':') {
        Some((command, arg)) => (plugin, command, Some(arg)),
        None => (plugin, command, None),
    })
}

/// Asks every plugin in `providers` for `method` with `params`, waiting at most `timeout` for
/// each, and returns the answers that came as `(plugin, result)`.
pub async fn ask_providers(
    providers: Vec<Plugin>,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Vec<(String, Value)> {
    let asks = providers.iter().map(|plugin| {
        let params = params.clone();
        async move {
            let answer = plugin.request(method, params, timeout).await;
            (plugin.name().to_owned(), answer)
        }
    });
    future::join_all(asks)
        .await
        .into_iter()
        .filter_map(|(plugin, answer)| Some((plugin, answer.ok()?)))
        .collect()
}

/// Returns `took` short, like `12ms` or `1.5s`.
pub fn short_duration(took: Duration) -> String {
    if took < Duration::from_secs(1) {
        format!("{}ms", took.as_millis())
    } else {
        format!("{:.1}s", took.as_secs_f32())
    }
}

/// Describes how fast a plugin answered and how much memory it uses, in one line.
fn describe_stats(stats: &Stats, memory: Option<&u64>) -> String {
    let mut line = format!("{} requests", stats.requests);
    if let (Some(mean), Some(p95)) = (stats.mean(), stats.percentile(0.95)) {
        let _ = write!(
            line,
            ", mean {}, p95 {}, slowest {}",
            short_duration(mean),
            short_duration(p95),
            short_duration(stats.slowest)
        );
    }
    let _ = write!(
        line,
        ", {} failed, {} timed out",
        stats.failed, stats.timeouts
    );
    if let Some(bytes) = memory {
        let _ = write!(line, ", {} MB", bytes / (1 << 20));
    }
    line
}

/// Returns the folder plugin folders are found in, `plugins` in the config directory.
pub fn plugin_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("plugins"))
}

/// Works out how to run the plugin called `name` from its config and manifest.
///
/// # Errors
///
/// Returns why the plugin cannot run, like a broken manifest or no command.
pub fn resolve(
    name: &str,
    config: Option<&PluginConfig>,
    manifest: Option<Manifest>,
) -> Result<(Spec, Option<Manifest>), String> {
    let defaults = PluginConfig::default();
    let config = config.unwrap_or(&defaults);
    let manifest = match (&config.path, manifest) {
        (Some(path), _) => Some(Manifest::read(path)?),
        (None, manifest) => manifest,
    };
    let (command, args) = match (&manifest, config.command.is_empty()) {
        (Some(manifest), true) => (manifest.command.clone(), manifest.args.clone()),
        // naming just the program, like python instead of python3, keeps the manifest args
        (Some(manifest), false) if config.args.is_empty() => {
            (config.command.clone(), manifest.args.clone())
        }
        (_, false) => (config.command.clone(), config.args.clone()),
        (None, true) => return Err(format!("plugin {name} needs a command or a path")),
    };
    let timeout = config
        .timeout
        .or(manifest.as_ref().and_then(|manifest| manifest.timeout))
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
    let spec = Spec {
        name: name.to_owned(),
        command,
        args,
        dir: manifest.as_ref().map(|manifest| manifest.dir.clone()),
        sdk: sdk::dir(),
        settings: config.settings.clone(),
        timeout,
        instance: 0,
    };
    Ok((spec, manifest))
}

impl Plugins {
    /// Creates the manager with nothing running, for plugins in `root` that are found in `dir`.
    pub fn new(root: &Path, dir: Option<PathBuf>) -> Self {
        let (sender, events) = mpsc::channel(EVENT_QUEUE);
        let (done_tx, done) = mpsc::unbounded_channel();
        Self {
            entries: BTreeMap::new(),
            root: root.to_owned(),
            dir,
            next_instance: 1,
            sender,
            events,
            done_tx,
            done,
        }
    }

    /// Makes the plugins match `configs` and the plugin folder, keeping ones that did not change
    /// running and starting the ones that start at once.
    ///
    /// Returns a message for each plugin that could not be set up or started.
    pub fn configure(&mut self, configs: &BTreeMap<String, PluginConfig>) -> Vec<String> {
        let mut problems = Vec::new();
        let mut found: BTreeMap<String, Manifest> = BTreeMap::new();
        for manifest in self.dir.as_deref().map(discover).unwrap_or_default() {
            match manifest {
                Ok(manifest) => {
                    found.insert(manifest.name.clone(), manifest);
                }
                Err(err) => problems.push(format!("plugin: {err}")),
            }
        }
        let names: BTreeSet<String> = configs.keys().chain(found.keys()).cloned().collect();
        let mut wanted = BTreeMap::new();
        for name in names {
            let config = configs.get(&name);
            if config.is_some_and(|config| !config.enabled) {
                continue;
            }
            match resolve(&name, config, found.remove(&name)) {
                Ok(resolved) => {
                    wanted.insert(name, resolved);
                }
                Err(err) => problems.push(err),
            }
        }
        self.entries.retain(|name, entry| {
            wanted.get(name).is_some_and(|(spec, manifest)| {
                let mut old = entry.spec.clone();
                old.instance = 0;
                old == *spec && entry.manifest == *manifest
            })
        });
        for (name, (spec, manifest)) in wanted {
            self.entries.entry(name).or_insert_with(|| Entry {
                spec,
                manifest,
                run: Run::Stopped,
                queued: Vec::new(),
                restarts: 0,
                started: Instant::now(),
                restart_at: None,
                log: VecDeque::new(),
                disabled: None,
                warned: None,
            });
        }
        let at_once: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.run, Run::Stopped) && entry.starts_at_once())
            .map(|(name, _)| name.clone())
            .collect();
        for name in at_once {
            problems.extend(self.start(&name));
        }
        problems
    }

    /// Starts the plugin called `name`, returning why it could not start.
    fn start(&mut self, name: &str) -> Option<String> {
        let instance = self.next_instance;
        self.next_instance += 1;
        let entry = self.entries.get_mut(name)?;
        entry.spec.instance = instance;
        entry.restart_at = None;
        entry.started = Instant::now();
        match Plugin::start(&entry.spec, &self.root, self.sender.clone()) {
            Ok(plugin) => {
                entry.run = Run::Starting(plugin);
                None
            }
            Err(err) => {
                let message = format!("plugin {name} could not start: {err}");
                entry.log(message.clone());
                Some(message)
            }
        }
    }

    /// Stops and starts every plugin again, forgetting earlier crashes.
    pub fn restart_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self.entries.keys().cloned().collect();
        let mut problems = Vec::new();
        for name in names {
            let Some(entry) = self.entries.get_mut(&name) else {
                continue;
            };
            let was_running = entry.plugin().is_some() || entry.disabled.is_some();
            entry.run = Run::Stopped;
            entry.restarts = 0;
            entry.restart_at = None;
            entry.disabled = None;
            if was_running || entry.starts_at_once() {
                problems.extend(self.start(&name));
            }
        }
        problems
    }

    /// Starts plugins whose restart after a crash is due.
    pub fn restart_due(&mut self) -> Vec<String> {
        let now = Instant::now();
        let due: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.restart_at.is_some_and(|at| at <= now))
            .map(|(name, _)| name.clone())
            .collect();
        due.iter().filter_map(|name| self.start(name)).collect()
    }

    /// Starts plugins that wait for a file with extension `language` to get focus.
    pub fn activate_language(&mut self, language: &str) -> Vec<String> {
        let waiting: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                matches!(entry.run, Run::Stopped)
                    && entry.restart_at.is_none()
                    && !entry.gave_up()
                    && entry
                        .manifest
                        .as_ref()
                        .is_some_and(|manifest| manifest.starts_for_language(language))
            })
            .map(|(name, _)| name.clone())
            .collect();
        waiting.iter().filter_map(|name| self.start(name)).collect()
    }

    /// Returns whether an event came from the current run of its plugin.
    pub fn is_current(&self, instance: u64, event: &PluginEvent) -> bool {
        let plugin = match event {
            PluginEvent::Ready { plugin, .. }
            | PluginEvent::Notification { plugin, .. }
            | PluginEvent::Request { plugin, .. }
            | PluginEvent::Log { plugin, .. }
            | PluginEvent::Exited { plugin, .. } => plugin,
        };
        self.entries
            .get(plugin)
            .is_some_and(|entry| entry.spec.instance == instance)
    }

    /// Remembers what a plugin said when it started and runs commands that waited for it.
    pub fn ready(&mut self, plugin: &str, hello: Hello) {
        let Some(entry) = self.entries.get_mut(plugin) else {
            return;
        };
        let Run::Starting(handle) = &entry.run else {
            return;
        };
        let handle = handle.clone();
        entry.run = Run::Ready(handle.clone(), hello);
        let queued = mem::take(&mut entry.queued);
        for (command, context, args) in queued {
            self.spawn_run(plugin, handle.clone(), command, context, args);
        }
    }

    /// Notes that a plugin stopped and schedules a restart if it crashed.
    ///
    /// Returns a message about it for the status line.
    pub fn exited(&mut self, plugin: &str, reason: Option<String>) -> String {
        let reason = reason
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default();
        let Some(entry) = self.entries.get_mut(plugin) else {
            return format!("plugin {plugin} stopped{reason}");
        };
        entry.run = Run::Stopped;
        entry.restart_at = None;
        entry.log(format!("stopped{reason}"));
        for (command, _, _) in entry.queued.drain(..) {
            let _ = self.done_tx.send(PluginUpdate::Ran(
                plugin.to_owned(),
                Err(format!("{command} could not run, the plugin stopped")),
            ));
        }
        if let Some(why) = &entry.disabled {
            return format!("plugin {plugin} stopped, {why}. run plugins.restart to try again");
        }
        if entry.started.elapsed() >= STABLE_AFTER {
            entry.restarts = 0;
        }
        if entry.restarts >= MAX_RESTARTS {
            return format!(
                "plugin {plugin} stopped{reason}. it crashed {MAX_RESTARTS} times, run \
                 plugins.restart to try again"
            );
        }
        let wait = Duration::from_secs(1 << entry.restarts);
        entry.restarts += 1;
        entry.restart_at = Some(Instant::now() + wait);
        format!(
            "plugin {plugin} stopped{reason}, restarting in {}s",
            wait.as_secs()
        )
    }

    /// Warns about plugins that answered slowly and stops ones that keep timing out.
    ///
    /// Returns messages for the status line.
    pub fn check_health(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        for (name, entry) in &mut self.entries {
            let Some(plugin) = entry.plugin().cloned() else {
                continue;
            };
            let slow = plugin.take_slow();
            for (method, took) in &slow {
                entry.log(format!("slow: {method} took {}ms", took.as_millis()));
            }
            if let Some((method, took)) = slow.iter().max_by_key(|(_, took)| *took)
                && entry
                    .warned
                    .is_none_or(|at| at.elapsed() >= SLOW_WARNING_EVERY)
            {
                entry.warned = Some(Instant::now());
                messages.push(format!(
                    "plugin {name} is slow, {method} took {:.1}s. see plugins.log",
                    took.as_secs_f32()
                ));
            }
            let timeouts = plugin.stats().timeouts_within(TIMEOUT_WINDOW);
            if timeouts >= MAX_TIMEOUTS && entry.disabled.is_none() {
                let why = format!(
                    "it timed out {timeouts} times in {} minutes",
                    TIMEOUT_WINDOW.as_secs() / 60
                );
                entry.log(format!("stopped by mog: {why}"));
                entry.disabled = Some(why);
                plugin.stop();
                messages.push(format!(
                    "plugin {name} keeps timing out, mog stopped it. run plugins.restart to try \
                     again"
                ));
            }
        }
        messages
    }

    /// Returns each running plugin's name, process id and request stats.
    pub fn health(&self) -> Vec<(String, Option<u32>, Stats)> {
        self.entries
            .iter()
            .filter_map(|(name, entry)| {
                let plugin = entry.plugin()?;
                Some((name.clone(), plugin.pid(), plugin.stats()))
            })
            .collect()
    }

    /// Adds a line a plugin printed to its log.
    pub fn log(&mut self, plugin: &str, line: String) {
        if let Some(entry) = self.entries.get_mut(plugin) {
            entry.log(line);
        }
    }

    /// Returns every plugin's state, request stats and log as text for the output panel, with
    /// the memory each process uses from `memory`.
    pub fn report(&self, memory: &BTreeMap<String, u64>) -> String {
        let mut text = String::from(
            "plugins are not sandboxed: each runs as you and can read and change any file, read \
             environment variables and use the network\n\n",
        );
        if self.entries.is_empty() {
            text.push_str("no plugins are configured, see docs/plugins.md\n");
        }
        for (name, entry) in &self.entries {
            let state = match (&entry.run, entry.restart_at) {
                (Run::Ready(_, hello), _) => format!("running, protocol {}", hello.protocol),
                (Run::Starting(_), _) => "starting".to_owned(),
                (Run::Stopped, Some(_)) => "crashed, restarting soon".to_owned(),
                (Run::Stopped, None) if entry.disabled.is_some() => format!(
                    "stopped by mog, {}",
                    entry.disabled.as_deref().unwrap_or_default()
                ),
                (Run::Stopped, None) if entry.restarts >= MAX_RESTARTS => {
                    "crashed too often, stopped".to_owned()
                }
                (Run::Stopped, None) => "waiting to be needed".to_owned(),
            };
            let version = entry
                .manifest
                .as_ref()
                .filter(|manifest| !manifest.version.is_empty())
                .map(|manifest| format!(" {}", manifest.version))
                .unwrap_or_default();
            let _ = writeln!(text, "== {name}{version}: {state}");
            let _ = writeln!(
                text,
                "   {} {}",
                entry.spec.command,
                entry.spec.args.join(" ")
            );
            if let Some(plugin) = entry.plugin() {
                let _ = writeln!(
                    text,
                    "   {}",
                    describe_stats(&plugin.stats(), memory.get(name))
                );
            }
            for line in &entry.log {
                let _ = writeln!(text, "   {line}");
            }
        }
        text
    }

    /// Returns every plugin command as `(full name, title, suggested keys)`.
    pub fn palette(&self) -> Vec<(String, String, Vec<String>)> {
        self.entries
            .iter()
            .flat_map(|(plugin, entry)| {
                entry.commands().iter().map(move |command| {
                    (
                        command_name(plugin, &command.name),
                        command.title.clone(),
                        command.keys.clone(),
                    )
                })
            })
            .collect()
    }

    /// Returns the commands that go in the right click menu as `(title, full name)`.
    pub fn menu(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .flat_map(|(plugin, entry)| {
                entry
                    .commands()
                    .iter()
                    .filter(|command| command.menu)
                    .map(move |command| {
                        (command.title.clone(), command_name(plugin, &command.name))
                    })
            })
            .collect()
    }

    /// Runs `command` of `handle` in the background, reporting to [`Plugins::update`].
    fn spawn_run(
        &self,
        plugin: &str,
        handle: Plugin,
        command: String,
        context: Value,
        args: Value,
    ) {
        let plugin = plugin.to_owned();
        self.spawn(async move {
            let result = handle.run(&command, context, args).await;
            PluginUpdate::Ran(plugin, result)
        });
    }

    /// Runs `work` in the background, reporting what it gives to [`Plugins::update`].
    pub fn spawn(&self, work: impl Future<Output = PluginUpdate> + Send + 'static) {
        let done = self.done_tx.clone();
        tokio::spawn(async move {
            let _ = done.send(work.await);
        });
    }

    /// Runs the plugin command called `full`, like `plugin.words.count`, with `context` and
    /// `args`, starting the plugin first if it waits to be needed.
    ///
    /// Returns `false` if no plugin has that command.
    pub fn run(&mut self, full: &str, context: Value, args: Value) -> bool {
        let Some((plugin, command, _)) = split_command(full) else {
            return false;
        };
        let (plugin, command) = (plugin.to_owned(), command.to_owned());
        let Some(entry) = self.entries.get(&plugin) else {
            return false;
        };
        if !entry.commands().iter().any(|known| known.name == command) {
            return false;
        }
        match &entry.run {
            Run::Ready(handle, _) => {
                let handle = handle.clone();
                self.spawn_run(&plugin, handle, command, context, args);
            }
            // a plugin that is starting, or waiting out its backoff after a crash, runs it once up
            run if matches!(run, Run::Starting(_)) || entry.restart_at.is_some() => {
                if let Some(entry) = self.entries.get_mut(&plugin) {
                    entry.queued.push((command, context, args));
                }
            }
            _ => {
                if let Some(entry) = self.entries.get_mut(&plugin) {
                    entry.queued.push((command, context, args));
                    // asking for a command of a plugin mog gave up on is a deliberate retry
                    if entry.gave_up() {
                        entry.restarts = 0;
                        entry.disabled = None;
                    }
                }
                if let Some(problem) = self.start(&plugin) {
                    let _ = self
                        .done_tx
                        .send(PluginUpdate::Ran(plugin.clone(), Err(problem)));
                    if let Some(entry) = self.entries.get_mut(&plugin) {
                        entry.queued.clear();
                    }
                }
            }
        }
        true
    }

    /// Returns the plugin called `name`, if it is running.
    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.entries.get(name).and_then(Entry::plugin)
    }

    /// Answers the request `id` that `plugin` sent.
    pub fn respond(&self, plugin: &str, id: Value, result: Result<Value, String>) {
        if let Some(running) = self.get(plugin) {
            running.respond(id, result);
        }
    }

    /// Returns whether any ready plugin listens for `event`.
    pub fn wants(&self, event: &str) -> bool {
        self.entries
            .values()
            .filter_map(Entry::hello)
            .any(|hello| hello.events.contains(event))
    }

    /// Returns the ready plugins that listen for `event`.
    pub fn listeners(&self, event: &str) -> Vec<Plugin> {
        self.entries
            .values()
            .filter_map(|entry| match &entry.run {
                Run::Ready(plugin, hello) if hello.events.contains(event) => Some(plugin.clone()),
                _ => None,
            })
            .collect()
    }

    /// Tells every plugin that listens for `kind` that it happened, with details in `params`.
    pub fn event(&self, kind: &str, params: &Value) {
        for plugin in self.listeners(kind) {
            plugin.event(kind, params.clone());
        }
    }

    /// Returns the ready plugins that provide `provider` for files with extension `language`.
    pub fn providers(&self, provider: &str, language: Option<&str>) -> Vec<Plugin> {
        self.entries
            .values()
            .filter_map(|entry| match &entry.run {
                Run::Ready(plugin, hello) if hello.provides(provider, language) => {
                    Some(plugin.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Returns the ready plugins that provide `provider` for any file.
    pub fn providers_any(&self, provider: &str) -> Vec<Plugin> {
        self.entries
            .values()
            .filter_map(|entry| match &entry.run {
                Run::Ready(plugin, hello)
                    if hello.providers.iter().any(|(name, _)| name == provider) =>
                {
                    Some(plugin.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Waits for the next event from a current plugin or a finished command.
    pub async fn update(&mut self) -> Option<PluginUpdate> {
        loop {
            let update = tokio::select! {
                Some((instance, event)) = self.events.recv() => {
                    if !self.is_current(instance, &event) {
                        continue;
                    }
                    PluginUpdate::Event(event)
                }
                Some(update) = self.done.recv() => update,
                else => return None,
            };
            return Some(update);
        }
    }
}

#[cfg(test)]
/// Tests for the plugin manager.
mod tests {
    use std::{collections::BTreeMap, env, fs, path::Path, process, time::Duration};

    use mog_config::PluginConfig;
    use mog_plugin::{Manifest, Plugin, protocol::parse_hello};
    use serde_json::{Value, json};
    use tokio::{io, sync::mpsc};

    use super::{Plugins, Run, command_name, split_command};

    /// Commands get namespaced by plugin, carry arguments after a colon, and unknown ones do not
    /// run.
    #[test]
    fn names_commands() {
        assert_eq!(command_name("a", "b"), "plugin.a.b");
        assert_eq!(
            split_command("plugin.words.count"),
            Some(("words", "count", None))
        );
        assert_eq!(
            split_command("plugin.snip.insert:greeting"),
            Some(("snip", "insert", Some("greeting")))
        );
        assert_eq!(split_command("save"), None);
    }

    /// A plugin with a manifest lists its commands before it runs and starts lazily.
    #[tokio::test]
    async fn lists_manifest_commands_without_starting() {
        let dir = env::temp_dir().join(format!("mog-plugins-test-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        let folder = dir.join("lazy");
        fs::create_dir_all(&folder).expect("dir");
        fs::write(
            folder.join("plugin.toml"),
            "name = \"lazy\"\ncommand = \"definitely-not-a-program\"\nactivation = [\"command\"]\n\
             [[commands]]\nname = \"go\"\ntitle = \"Lazy: Go\"\nmenu = true\n",
        )
        .expect("manifest");
        let mut plugins = Plugins::new(Path::new("."), Some(dir.clone()));
        let problems = plugins.configure(&BTreeMap::new());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(plugins.palette()[0].0, "plugin.lazy.go");
        assert_eq!(plugins.menu()[0].0, "Lazy: Go");
        assert!(plugins.get("lazy").is_none());
        assert!(!plugins.run("plugin.lazy.nope", Value::Null, Value::Null));
        let mut off = BTreeMap::new();
        off.insert(
            "lazy".to_owned(),
            PluginConfig {
                enabled: false,
                ..PluginConfig::default()
            },
        );
        plugins.configure(&off);
        assert!(plugins.palette().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A crashing plugin is restarted with growing waits, then given up on.
    #[test]
    fn backs_off_crashes() {
        let mut plugins = Plugins::new(Path::new("."), None);
        let mut configs = BTreeMap::new();
        configs.insert(
            "crashy".to_owned(),
            PluginConfig {
                command: "definitely-not-a-program".into(),
                ..PluginConfig::default()
            },
        );
        // starting fails here, which is fine, the entry still exists
        let _ = plugins.configure(&configs);
        for wait in [1, 2, 4, 8, 16] {
            let message = plugins.exited("crashy", Some("boom".into()));
            assert!(
                message.contains(&format!("restarting in {wait}s")),
                "{message}"
            );
        }
        assert!(plugins.exited("crashy", None).contains("crashed 5 times"));
        assert!(
            plugins
                .report(&BTreeMap::new())
                .contains("crashed too often")
        );
    }

    /// A plugin that keeps timing out is warned about, then stopped until it is asked for again.
    #[tokio::test]
    async fn stops_plugins_that_keep_timing_out() {
        let mut plugins = Plugins::new(Path::new("."), None);
        let mut configs = BTreeMap::new();
        configs.insert(
            "stuck".to_owned(),
            PluginConfig {
                command: "definitely-not-a-program".into(),
                ..PluginConfig::default()
            },
        );
        let _ = plugins.configure(&configs);
        // a plugin over pipes that never reads, so every request times out
        let (mog_side, _plugin_side) = io::duplex(1 << 16);
        let (reader, writer) = io::split(mog_side);
        let (events, _events) = mpsc::channel(8);
        let (plugin, _task) = Plugin::connect(
            "stuck",
            0,
            reader,
            writer,
            Value::Null,
            Duration::from_secs(1),
            events,
        );
        let entry = plugins.entries.get_mut("stuck").expect("entry");
        let hello = parse_hello(&json!({ "protocolVersion": 2 })).expect("hello");
        entry.run = Run::Ready(plugin.clone(), hello);
        for _ in 0..5 {
            let _ = plugin
                .request("provide/hover", Value::Null, Duration::from_millis(1))
                .await;
        }
        let messages = plugins.check_health();
        assert!(messages[0].contains("is slow"), "{messages:?}");
        assert!(messages[1].contains("keeps timing out"), "{messages:?}");
        assert!(plugins.check_health().is_empty(), "said once");
        assert!(plugins.exited("stuck", None).contains("timed out 5 times"));
        let entry = &plugins.entries["stuck"];
        assert!(entry.restart_at.is_none(), "not restarted on its own");
        assert!(entry.gave_up());
        assert!(plugins.report(&BTreeMap::new()).contains("stopped by mog"));
        plugins.restart_all();
        assert!(plugins.entries["stuck"].disabled.is_none());
    }

    /// Neither a command nor a file getting focus skips the wait after a crash.
    #[tokio::test]
    async fn commands_wait_out_the_backoff() {
        let mut plugins = Plugins::new(Path::new("."), None);
        let mut configs = BTreeMap::new();
        configs.insert(
            "crashy".to_owned(),
            PluginConfig {
                command: "definitely-not-a-program".into(),
                ..PluginConfig::default()
            },
        );
        let _ = plugins.configure(&configs);
        let entry = plugins.entries.get_mut("crashy").expect("entry");
        let manifest = "name = \"crashy\"
command = \"x\"
activation = [\"language:txt\"]
                        [[commands]]
name = \"go\"
title = \"Go\"
";
        entry.manifest = Some(Manifest::parse(manifest, Path::new(".")).expect("manifest"));
        plugins.exited("crashy", None);
        plugins.exited("crashy", None);
        assert!(plugins.run("plugin.crashy.go", Value::Null, Value::Null));
        let entry = &plugins.entries["crashy"];
        assert_eq!(entry.restarts, 2, "the crash count is kept");
        assert!(entry.restart_at.is_some(), "the restart still waits");
        assert_eq!(entry.queued.len(), 1, "the command waits for it");
        for _ in 0..3 {
            plugins.exited("crashy", None);
        }
        assert!(plugins.exited("crashy", None).contains("crashed 5 times"));
        assert!(plugins.activate_language("txt").is_empty());
        assert!(
            plugins.entries["crashy"].gave_up(),
            "focus does not retry it"
        );
    }
}
