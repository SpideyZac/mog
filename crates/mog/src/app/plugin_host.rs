//! Plugins inside the app: starting them, doing what they ask, answering their questions and
//! telling them what happens in the editor.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use futures::future;
use mog_config::ThemeConfig;
use mog_core::{
    Change, Command, Diagnostic, Range, Severity, Transaction, command::UnknownCommand,
};
use mog_plugin::{
    Action, Edit, Level, Plugin, PluginEvent, parse_actions, parse_segment,
    protocol::{parse_action, parse_selections},
};
use mog_tui::{
    CommandInfo, CursorStyle, Overlay, PluginHealth, PluginPick, PluginSegment, PromptKind, Side,
    SymbolEntry,
    completion::{CompletionItem, ItemKind},
    picker::PickerItem,
};
use serde_json::{Value, json};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::App;
use crate::{
    commands,
    plugins::{PluginUpdate, ask_providers, split_command},
    tasks::Task,
};

mod contributions;
mod edits;
mod screen;

use screen::ScreenState;

/// How long plugins get to change a file before it is saved.
const BEFORE_SAVE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often the memory and speed of plugins is read again.
const HEALTH_EVERY: Duration = Duration::from_secs(2);

/// How long a plugin gets to find the problems in a file.
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the cursor has to rest before plugins hear where it is.
const SELECTION_DELAY: Duration = Duration::from_millis(150);

/// Selections sent to plugins, the file and `(anchor, head)` pairs.
type SentSelections = (Option<PathBuf>, Vec<(usize, usize)>);

/// Text shown after lines of one file, by plugin and line, as `(text, color)`.
type FileDecorations = BTreeMap<String, BTreeMap<usize, (String, Option<String>)>>;

/// What the app remembers about plugins between frames.
#[derive(Debug, Default)]
pub struct PluginState {
    /// A plugin waiting for the user to pick or type something, with its request id.
    pub waiting: Option<(String, Value)>,
    /// The document version last sent with `changed`, by file.
    changed: HashMap<PathBuf, u64>,
    /// The selections last sent with `selection`.
    selection: Option<SentSelections>,
    /// The files open last frame, to spot closed ones.
    open: BTreeSet<PathBuf>,
    /// Whether `idle` was sent since the last input.
    idle_sent: bool,
    /// The file waiting for plugins to answer `before_save`.
    pending_save: Option<PathBuf>,
    /// Set while saving after plugins answered, so the save does not ask them again.
    saving: bool,
    /// Text plugins show after lines, by file, plugin and line.
    decorations: HashMap<PathBuf, FileDecorations>,
    /// Code actions plugins offered, as `(plugin, actions)`, by menu index.
    pub code_actions: Vec<(String, Vec<Action>)>,
    /// What plugins put on the screen and which keys they take.
    screen: ScreenState,
    /// Reads how much memory plugin processes use, made the first time it is needed.
    system: Option<System>,
    /// When the plugin health was last read.
    health_at: Option<Instant>,
    /// The memory each plugin process used when last read, in bytes.
    memory: BTreeMap<String, u64>,
    /// The document version each plugin was last asked for the problems in, by plugin and file.
    diagnosed: HashMap<(String, PathBuf), u64>,
    /// The tasks plugins offered when last asked.
    pub tasks: Vec<Task>,
    /// The themes plugins added to the config, by name.
    themes: BTreeMap<String, ThemeConfig>,
}

/// Reads a severity name.
fn severity(name: Option<&str>) -> Severity {
    match name {
        Some("warning") => Severity::Warning,
        Some("info" | "information") => Severity::Info,
        Some("hint") => Severity::Hint,
        _ => Severity::Error,
    }
}

/// Returns the name of `severity` for plugins.
fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

/// Returns the language of a file as plugins see it, its extension.
pub fn language(path: Option<&Path>) -> Option<String> {
    path.and_then(Path::extension)
        .map(|ext| ext.to_string_lossy().into_owned())
}

/// Reads the completions a plugin answered `provide/completion` with.
pub fn completion_items(answer: &Value) -> Vec<CompletionItem> {
    answer["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let label = item["label"].as_str()?.to_owned();
                    let kind = match item["kind"].as_str() {
                        Some("function" | "method") => ItemKind::Function,
                        Some("variable" | "value") => ItemKind::Variable,
                        Some("field" | "property") => ItemKind::Field,
                        Some("type" | "class" | "struct" | "interface") => ItemKind::Type,
                        Some("module" | "namespace") => ItemKind::Module,
                        Some("keyword") => ItemKind::Keyword,
                        Some("constant") => ItemKind::Constant,
                        _ => ItemKind::Other,
                    };
                    Some(CompletionItem {
                        detail: item["detail"].as_str().unwrap_or_default().to_owned(),
                        insert: item["insert"].as_str().unwrap_or(&label).to_owned(),
                        filter: item["filter"].as_str().unwrap_or(&label).to_owned(),
                        kind,
                        label,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the code actions plugins answered `provide/code_actions` with, as
/// `(title, plugin, actions)`. Broken ones are left out.
pub fn code_actions(answers: Vec<(String, Value)>) -> Vec<(String, String, Vec<Action>)> {
    answers
        .into_iter()
        .flat_map(|(plugin, answer)| {
            answer["actions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |offer| {
                    let title = offer["title"].as_str()?.to_owned();
                    let actions = parse_actions(&offer).ok()?;
                    Some((title, plugin.clone(), actions))
                })
        })
        .collect()
}

/// Asks `providers` for `provider` in the background, or returns `None` when there are none.
pub fn ask_these(
    providers: Vec<Plugin>,
    provider: &str,
    params: Value,
    timeout: Duration,
) -> Option<impl Future<Output = Vec<(String, Value)>> + Send + 'static> {
    if providers.is_empty() {
        return None;
    }
    let method = format!("provide/{provider}");
    Some(async move { ask_providers(providers, &method, params, timeout).await })
}

/// How long plugins get to say which tasks they have.
const TASKS_TIMEOUT: Duration = Duration::from_secs(3);

/// Reads the tasks plugins answered `provide/tasks` with, each run in `cwd` relative to
/// `root`.
pub fn plugin_tasks(answers: &[(String, Value)], root: &Path) -> Vec<Task> {
    answers
        .iter()
        .flat_map(|(_, answer)| answer["tasks"].as_array().cloned().unwrap_or_default())
        .filter_map(|task| {
            Some(Task {
                name: task["name"].as_str()?.to_owned(),
                command: task["command"].as_str()?.to_owned(),
                cwd: absolute(root, task["cwd"].as_str().unwrap_or("")),
            })
        })
        .collect()
}

/// Returns `path` from a plugin made absolute against `root`.
fn absolute(root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

/// Reads the places plugins answered `provide/definition` or `provide/references` with, as
/// `(path, line, column)`, either a `locations` list or one location. Relative paths are
/// against `root`, and a missing path means `current`.
pub fn locations(
    answers: &[(String, Value)],
    root: &Path,
    current: Option<&Path>,
) -> Vec<(PathBuf, usize, usize)> {
    let place = |item: &Value| {
        let path = match item["path"].as_str() {
            Some(path) => absolute(root, path),
            None => current?.to_owned(),
        };
        let line = usize::try_from(item["line"].as_u64()?).ok()?;
        let column = item["column"]
            .as_u64()
            .and_then(|column| usize::try_from(column).ok())
            .unwrap_or(0);
        Some((path, line, column))
    };
    answers
        .iter()
        .flat_map(|(_, answer)| match answer["locations"].as_array() {
            Some(items) => items.iter().filter_map(place).collect(),
            None => place(answer).into_iter().collect::<Vec<_>>(),
        })
        .collect()
}

/// Returns the short kind name mog shows for a symbol kind a plugin named, like `fn` for
/// `function`.
fn symbol_kind(kind: &str) -> String {
    match kind {
        "function" => "fn",
        "variable" => "let",
        "constant" => "const",
        "module" | "namespace" | "package" => "mod",
        "property" => "field",
        "constructor" => "new",
        "interface" => "trait",
        "enum_member" | "enummember" => "variant",
        "type_parameter" => "type",
        other => other,
    }
    .to_owned()
}

/// Reads the symbols plugins answered `provide/symbols` with. Relative paths are against
/// `root`, and a missing path means `current`.
pub fn symbol_entries(
    answers: &[(String, Value)],
    root: &Path,
    current: Option<&Path>,
) -> Vec<SymbolEntry> {
    answers
        .iter()
        .flat_map(|(_, answer)| answer["symbols"].as_array().cloned().unwrap_or_default())
        .filter_map(|item| {
            let path = match item["path"].as_str() {
                Some(path) => absolute(root, path),
                None => current?.to_owned(),
            };
            let number = |key: &str| {
                item[key]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(0)
            };
            Some(SymbolEntry {
                name: item["name"].as_str()?.to_owned(),
                kind: symbol_kind(item["kind"].as_str().unwrap_or("symbol")),
                detail: item["detail"].as_str().unwrap_or_default().to_owned(),
                depth: number("depth"),
                line: number("line"),
                column: number("column"),
                path,
            })
        })
        .collect()
}

/// Returns the first formatting a plugin answered `provide/formatting` with, as the plugin and
/// its changes.
pub fn format_changes(answers: &[(String, Value)]) -> Option<(String, Vec<Change>)> {
    answers.iter().find_map(|(plugin, answer)| {
        let edit = parse_action(&json!({ "type": "edit", "changes": answer.get("changes")? }));
        match edit {
            Ok(Action::Edit(edit)) => Some((plugin.clone(), to_changes(edit.changes))),
            _ => None,
        }
    })
}

/// Turns plugin changes into editor changes.
fn to_changes(edits: Vec<Edit>) -> Vec<Change> {
    edits
        .into_iter()
        .map(|edit| Change {
            start: edit.start,
            end: edit.end,
            text: edit.text,
        })
        .collect()
}

/// Returns the transactions since a version as one list of changes to apply in order.
fn sequential_changes(transactions: &[&Transaction]) -> Vec<Value> {
    let mut changes = Vec::new();
    for tx in transactions {
        // changes in one transaction are relative to the text before it, so shift each by the
        // ones before it to make them apply one after another
        let mut shift: isize = 0;
        for change in tx.changes() {
            let start = change.start.saturating_add_signed(shift);
            let end = change.end.saturating_add_signed(shift);
            changes.push(json!({ "start": start, "end": end, "text": change.text }));
            let inserted = isize::try_from(change.text.chars().count()).unwrap_or(isize::MAX);
            let removed = isize::try_from(change.end - change.start).unwrap_or(isize::MAX);
            shift += inserted - removed;
        }
    }
    changes
}

impl App {
    /// Starts the plugins from the config and the plugin folder.
    ///
    /// Not part of [`App::new`] so snapshots and tests do not run other programs.
    pub fn start_plugins(&mut self) {
        let mut problems = self.plugins.configure(&self.ui.config.plugins);
        problems.extend(self.apply_plugin_contributions());
        self.refresh_commands();
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
    }

    /// Restarts every plugin, for `plugins.restart`.
    pub(super) fn restart_plugins(&mut self) {
        let problems = self.plugins.restart_all();
        self.ui.plugin_segments.clear();
        self.ui.plugin_widgets.clear();
        self.ui.cursor_style = CursorStyle::default();
        self.plugin_state.screen = ScreenState::default();
        self.refresh_commands();
        self.editor.set_status(if problems.is_empty() {
            "plugins restarted".to_owned()
        } else {
            problems.join("; ")
        });
    }

    /// Shows every plugin's state and log in the output panel, for `plugins.log`.
    pub(super) fn show_plugin_log(&mut self) {
        self.refresh_plugin_health(true);
        let report = self.plugins.report(&self.plugin_state.memory);
        self.show_output("plugins", &report);
    }

    /// Reads how much memory and time each plugin takes, for the resource monitor and the
    /// plugin log, at most every [`HEALTH_EVERY`] unless `now`.
    fn refresh_plugin_health(&mut self, now: bool) {
        let state = &mut self.plugin_state;
        let due = state
            .health_at
            .is_none_or(|at| at.elapsed() >= HEALTH_EVERY);
        if !now && (!due || self.ui.idle) {
            return;
        }
        state.health_at = Some(Instant::now());
        let health = self.plugins.health();
        let pids: Vec<Pid> = health
            .iter()
            .filter_map(|(_, pid, _)| pid.map(Pid::from_u32))
            .collect();
        state.memory.clear();
        if !pids.is_empty() {
            let system = state.system.get_or_insert_with(System::new);
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&pids),
                true,
                ProcessRefreshKind::nothing().with_memory(),
            );
            for (name, pid, _) in &health {
                let memory = pid.and_then(|pid| system.process(Pid::from_u32(pid)));
                if let Some(process) = memory {
                    state.memory.insert(name.clone(), process.memory());
                }
            }
        }
        self.ui.plugin_health = health
            .into_iter()
            .map(|(name, _, stats)| PluginHealth {
                memory: state.memory.get(&name).copied(),
                p95: stats.percentile(0.95),
                timeouts: stats.timeouts,
                name,
            })
            .collect();
    }

    /// Shows `text` in the output panel under `title`.
    fn show_output(&mut self, title: &str, text: &str) {
        self.ui.output.start(title);
        for line in text.lines() {
            self.ui.output.push(line.to_owned());
        }
        self.ui.output.running = false;
        self.ui.open(Overlay::Output);
    }

    /// Rebuilds the palette, key list and right click menu from the keymap and the plugin
    /// commands, binding the keys plugins suggest when nothing else uses them.
    pub(super) fn refresh_commands(&mut self) {
        self.bind_plugin_keys();
        let mut palette = commands::palette(&self.keymap);
        for (name, title, _) in self.plugins.palette() {
            let keys = self
                .keymap
                .chords_for(&Command::Custom(name.clone()))
                .iter()
                .map(ToString::to_string)
                .collect();
            palette.push(CommandInfo { name, title, keys });
        }
        self.ui.commands = palette;
        self.ui.bindings = commands::bindings(&self.keymap);
        self.ui.plugin_menu = self.plugins.menu();
    }

    /// Returns what the editor looks like, for a plugin.
    pub(super) fn plugin_context(&self) -> Value {
        let document = self.editor.document();
        let mut context = self.cursor_context();
        // a huge file would be slow to copy and to send, editor/text can read parts of it
        context["text"] = json!((!document.is_large()).then(|| document.text().to_string()));
        context
    }

    /// Returns where the cursor is and which file it is in, without the text.
    fn cursor_context(&self) -> Value {
        let document = self.editor.document();
        let text = document.text();
        let selection = document.selection();
        let line = text.char_to_line(selection.head);
        let selections: Vec<Value> = [selection]
            .into_iter()
            .chain(document.cursors().iter().copied())
            .map(|range| json!({ "anchor": range.anchor, "head": range.head }))
            .collect();
        json!({
            "root": self.ui.root.to_string_lossy(),
            "path": document.path().map(|path| path.to_string_lossy()),
            "language": language(document.path()),
            "length": text.len_chars(),
            "lines": text.len_lines(),
            "version": document.version(),
            "selection": { "anchor": selection.anchor, "head": selection.head },
            "selections": selections,
            "line": line,
            "column": selection.head - text.line_to_char(line),
            "modified": document.is_modified(),
        })
    }

    /// Runs the plugin command `name`, like `plugin.words.count` or `plugin.snip.insert:hi`.
    pub(super) fn run_plugin_command(&mut self, name: &str, args: Value) {
        let args = match (split_command(name), args) {
            (Some((_, _, Some(arg))), Value::Null) => json!(arg),
            (_, args) => args,
        };
        let context = self.plugin_context();
        if !self.plugins.run(name, context, args) {
            self.editor.set_status(format!("no plugin has {name}"));
        }
    }

    /// Acts on something a plugin did or plugin work that finished.
    pub(super) fn handle_plugin(&mut self, update: PluginUpdate) {
        match update {
            PluginUpdate::Event(PluginEvent::Ready { plugin, hello }) => {
                // a plugin started by a file's language missed that file getting focus
                let catch_up = hello.events.contains("opened");
                self.plugins.ready(&plugin, hello);
                self.refresh_commands();
                if catch_up && let Some(running) = self.plugins.get(&plugin) {
                    let path = self.editor.document().path();
                    running.event(
                        "opened",
                        json!({
                            "path": path.map(|path| path.to_string_lossy()),
                            "language": language(path),
                        }),
                    );
                }
            }
            PluginUpdate::Event(PluginEvent::Notification {
                plugin,
                method,
                params,
            }) => self.plugin_notification(&plugin, &method, &params),
            PluginUpdate::Event(PluginEvent::Request {
                plugin,
                id,
                method,
                params,
            }) => self.answer_plugin(plugin, id, &method, &params),
            PluginUpdate::Event(PluginEvent::Log { plugin, line }) => {
                self.plugins.log(&plugin, line)
            }
            PluginUpdate::Event(PluginEvent::Exited { plugin, reason }) => {
                self.plugin_exited(&plugin, reason);
            }
            PluginUpdate::Ran(plugin, Ok(actions)) => {
                if let Err(err) = self.apply_actions(actions) {
                    self.editor.set_status(format!("{plugin}: {err}"));
                }
            }
            PluginUpdate::Ran(plugin, Err(err)) => {
                self.plugins.log(&plugin, err.clone());
                self.editor.set_status(format!("{plugin}: {err}"));
            }
            PluginUpdate::BeforeSave {
                path,
                version,
                answers,
            } => self.finish_before_save(&path, version, answers),
            PluginUpdate::Key {
                plugin,
                chord,
                answer,
            } => self.finish_key(&plugin, chord, answer),
            PluginUpdate::Diagnostics {
                plugin,
                path,
                version,
                answer,
            } => self.finish_diagnostics(&plugin, &path, version, answer),
            PluginUpdate::Tasks(answers) => {
                self.plugin_state.tasks = plugin_tasks(&answers, &self.ui.root);
                self.show_tasks();
            }
        }
    }

    /// Asks plugins that provide `diagnostics` for the problems in each open file that changed
    /// since they were last asked.
    fn request_plugin_diagnostics(&mut self) {
        let mut asks = Vec::new();
        for document in self.editor.documents() {
            let Some(path) = document.path() else {
                continue;
            };
            if document.is_large() {
                continue;
            }
            let language = language(Some(path));
            for plugin in self.plugins.providers("diagnostics", language.as_deref()) {
                let key = (plugin.name().to_owned(), path.to_owned());
                if self.plugin_state.diagnosed.get(&key) == Some(&document.version()) {
                    continue;
                }
                let params = json!({
                    "path": path.to_string_lossy(),
                    "language": language,
                    "version": document.version(),
                    "text": document.text().to_string(),
                });
                asks.push((key, document.version(), plugin, params));
            }
        }
        for ((name, path), version, plugin, params) in asks {
            self.plugin_state
                .diagnosed
                .insert((name.clone(), path.clone()), version);
            self.plugins.spawn(async move {
                let answer = plugin
                    .request("provide/diagnostics", params, DIAGNOSTICS_TIMEOUT)
                    .await;
                PluginUpdate::Diagnostics {
                    plugin: name,
                    path,
                    version,
                    answer,
                }
            });
        }
    }

    /// Shows the problems a plugin found in a file, if it did not change since.
    fn finish_diagnostics(
        &mut self,
        plugin: &str,
        path: &Path,
        version: u64,
        answer: Result<Value, String>,
    ) {
        let current = self
            .find_document(Some(path))
            .is_some_and(|index| self.editor.documents()[index].version() == version);
        if !current {
            return;
        }
        let result = answer.and_then(|answer| {
            let diagnostics = answer.get("diagnostics").cloned().unwrap_or(json!([]));
            let params = json!({ "path": path.to_string_lossy(), "diagnostics": diagnostics });
            self.set_plugin_diagnostics(plugin, &params)
        });
        if let Err(err) = result {
            self.plugins
                .log(plugin, format!("provide/diagnostics: {err}"));
        }
    }

    /// Asks plugins that provide `tasks` for theirs and shows every task once they answer.
    ///
    /// Returns `false` when no plugin provides tasks, so the tasks can be shown right away.
    pub(super) fn ask_plugin_tasks(&mut self) -> bool {
        let providers = self.plugins.providers_any("tasks");
        let params = json!({ "root": self.ui.root.to_string_lossy() });
        let Some(asked) = ask_these(providers, "tasks", params, TASKS_TIMEOUT) else {
            return false;
        };
        self.plugins
            .spawn(async move { PluginUpdate::Tasks(asked.await) });
        true
    }

    /// Cleans up after a plugin that stopped and says so.
    fn plugin_exited(&mut self, plugin: &str, reason: Option<String>) {
        let message = self.plugins.exited(plugin, reason);
        self.ui
            .plugin_segments
            .retain(|segment| segment.plugin.split('/').next() != Some(plugin));
        // a list or prompt it asked for would never be answered
        if self
            .plugin_state
            .waiting
            .as_ref()
            .is_some_and(|(waiting, _)| waiting == plugin)
        {
            self.plugin_state.waiting = None;
            if matches!(self.ui.overlay, Some(Overlay::PluginPick)) {
                self.ui.close();
            }
            if self
                .ui
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.kind == PromptKind::Plugin)
            {
                self.ui.prompt = None;
                if self.ui.overlay == Some(Overlay::Prompt) {
                    self.ui.close();
                }
            }
        }
        for decorations in self.plugin_state.decorations.values_mut() {
            decorations.remove(plugin);
        }
        self.plugin_state
            .diagnosed
            .retain(|(asked, _), _| asked != plugin);
        self.clear_plugin_screen(plugin);
        self.sync_decorations();
        let source = format!("plugin:{plugin}");
        for document in self.editor.documents_mut() {
            document.set_source_diagnostics(&source, Vec::new());
        }
        self.refresh_commands();
        self.editor.set_status(message);
    }

    /// Handles a notification a plugin sent.
    fn plugin_notification(&mut self, plugin: &str, method: &str, params: &Value) {
        if let Some(result) = self.screen_notification(plugin, method, params) {
            if let Err(err) = result {
                self.plugins.log(plugin, format!("{method}: {err}"));
            }
            return;
        }
        match method {
            "actions" => {
                let applied = parse_actions(params).and_then(|actions| self.apply_actions(actions));
                if let Err(err) = applied {
                    self.plugins.log(plugin, err.clone());
                    self.editor.set_status(format!("{plugin}: {err}"));
                }
            }
            "status" => self
                .editor
                .set_status(params["text"].as_str().unwrap_or_default()),
            "notify" => {
                let text = params["text"].as_str().unwrap_or_default().to_owned();
                self.plugins.log(plugin, text.clone());
                self.editor.set_status(format!("{plugin}: {text}"));
            }
            "log" => self.plugins.log(
                plugin,
                params["text"].as_str().unwrap_or_default().to_owned(),
            ),
            "segment" => {
                let segment = parse_segment(params);
                let key = match params["id"].as_str() {
                    Some(id) => format!("{plugin}/{id}"),
                    None => plugin.to_owned(),
                };
                self.set_segment(PluginSegment {
                    command: segment.command,
                    bg: segment.bg,
                    bold: segment.bold,
                    side: if segment.left {
                        Side::Left
                    } else {
                        Side::Right
                    },
                    ..PluginSegment::new(key, segment.text, segment.color)
                });
            }
            "progress" => {
                let id = params["id"].as_str().unwrap_or("progress");
                let key = format!("{plugin}/progress/{id}");
                if params["done"].as_bool().unwrap_or(false) {
                    self.set_segment(PluginSegment::new(key, "", None));
                    return;
                }
                let title = params["title"].as_str().unwrap_or(plugin);
                let text = match params["percentage"].as_u64() {
                    Some(percent) => format!("{title} {}%", percent.min(100)),
                    None => format!("{title}\u{2026}"),
                };
                self.set_segment(PluginSegment::new(key, text, Some("accent".into())));
            }
            "diagnostics" => {
                if let Err(err) = self.set_plugin_diagnostics(plugin, params) {
                    self.plugins.log(plugin, format!("diagnostics: {err}"));
                }
            }
            "decorations" => {
                if let Err(err) = self.set_plugin_decorations(plugin, params) {
                    self.plugins.log(plugin, format!("decorations: {err}"));
                }
            }
            "output" => {
                let title = params["title"].as_str().unwrap_or(plugin).to_owned();
                let text = params["text"].as_str().unwrap_or_default().to_owned();
                self.show_output(&title, &text);
            }
            other => self
                .plugins
                .log(plugin, format!("sent {other}, which mog does not know")),
        }
    }

    /// Puts a plugin segment in the status line, replacing the one with the same name, or
    /// removes it when its text is empty.
    fn set_segment(&mut self, segment: PluginSegment) {
        let segments = &mut self.ui.plugin_segments;
        let at = segments
            .iter()
            .position(|other| other.plugin == segment.plugin);
        match (at, segment.text.is_empty()) {
            (Some(at), true) => {
                segments.remove(at);
            }
            (None, true) => {}
            (Some(at), false) => segments[at] = segment,
            (None, false) => segments.push(segment),
        }
    }

    /// Replaces the diagnostics `plugin` reports for a file.
    fn set_plugin_diagnostics(&mut self, plugin: &str, params: &Value) -> Result<(), String> {
        let path = self.resolve_path(params["path"].as_str().map(Path::new));
        let index = self
            .find_document(path.as_deref())
            .ok_or("the file is not open")?;
        let document = &mut self.editor.documents_mut()[index];
        let text = document.text();
        let len = text.len_chars();
        let at = |line: &Value, column: &Value| -> Option<usize> {
            let line = usize::try_from(line.as_u64()?).ok()?;
            if line >= text.len_lines() {
                return Some(len);
            }
            let start = text.line_to_char(line);
            let column = usize::try_from(column.as_u64().unwrap_or(0)).ok()?;
            Some((start + column).min(len))
        };
        let diagnostics = params["diagnostics"]
            .as_array()
            .ok_or("needs a `diagnostics` list")?
            .iter()
            .map(|item| {
                let (from, to) = match (item["start"].as_u64(), item["end"].as_u64()) {
                    (Some(start), end) => {
                        let start = usize::try_from(start).unwrap_or(len).min(len);
                        let end = end
                            .and_then(|end| usize::try_from(end).ok())
                            .unwrap_or(start)
                            .clamp(start, len);
                        (start, end)
                    }
                    (None, _) => {
                        let from = at(&item["line"], &item["column"])
                            .ok_or("a diagnostic needs `start` or `line`")?;
                        let to = at(&item["end_line"], &item["end_column"])
                            .unwrap_or(from)
                            .max(from);
                        (from, to)
                    }
                };
                Ok(Diagnostic {
                    from,
                    to,
                    severity: severity(item["severity"].as_str()),
                    message: item["message"].as_str().unwrap_or_default().to_owned(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        document.set_source_diagnostics(&format!("plugin:{plugin}"), diagnostics);
        Ok(())
    }

    /// Replaces the text `plugin` shows after lines of a file.
    fn set_plugin_decorations(&mut self, plugin: &str, params: &Value) -> Result<(), String> {
        let path = self
            .resolve_path(params["path"].as_str().map(Path::new))
            .or_else(|| self.editor.document().path().map(ToOwned::to_owned))
            .ok_or("decorations need a file with a path")?;
        let mut lines = BTreeMap::new();
        for item in params["decorations"]
            .as_array()
            .ok_or("needs a `decorations` list")?
        {
            let line = item["line"]
                .as_u64()
                .and_then(|line| usize::try_from(line).ok())
                .ok_or("a decoration needs a `line`")?;
            let text = item["text"].as_str().unwrap_or_default().to_owned();
            let color = item["color"].as_str().map(str::to_owned);
            lines.insert(line, (text, color));
        }
        let file = self.plugin_state.decorations.entry(path).or_default();
        if lines.is_empty() {
            file.remove(plugin);
        } else {
            file.insert(plugin.to_owned(), lines);
        }
        self.sync_decorations();
        Ok(())
    }

    /// Merges every plugin's decorations into what the editor draws.
    fn sync_decorations(&mut self) {
        self.ui.plugin_decorations = self
            .plugin_state
            .decorations
            .iter()
            .map(|(path, plugins)| {
                let mut merged: BTreeMap<usize, (String, Option<String>)> = BTreeMap::new();
                for lines in plugins.values() {
                    for (line, (text, color)) in lines {
                        merged
                            .entry(*line)
                            .and_modify(|(shown, _)| {
                                shown.push_str("  ");
                                shown.push_str(text);
                            })
                            .or_insert_with(|| (text.clone(), color.clone()));
                    }
                }
                (path.clone(), merged)
            })
            .collect();
    }

    /// Returns `path` made absolute against the project, if given.
    fn resolve_path(&self, path: Option<&Path>) -> Option<PathBuf> {
        path.map(|path| {
            if path.is_absolute() {
                path.to_owned()
            } else {
                self.ui.root.join(path)
            }
        })
    }

    /// Returns the index of the open document at `path`, or the focused one when `None`.
    fn find_document(&self, path: Option<&Path>) -> Option<usize> {
        match path {
            None => Some(self.editor.active()),
            Some(path) => self
                .editor
                .documents()
                .iter()
                .position(|document| document.path() == Some(path)),
        }
    }

    /// Answers a question a plugin asked, right away or once the user did what it asks for.
    fn answer_plugin(&mut self, plugin: String, id: Value, method: &str, params: &Value) {
        let result = match method {
            "editor/context" => Ok(self.plugin_context()),
            "ui/layout" => Ok(self.screen_layout()),
            "editor/text" => self.read_text(params),
            "editor/documents" => Ok(self.list_documents()),
            "editor/diagnostics" => self.list_diagnostics(params),
            "editor/select" => {
                let path = self.resolve_path(params["path"].as_str().map(Path::new));
                parse_selections(&params["selections"])
                    .and_then(|selections| self.select(path, selections))
                    .map(|()| json!({}))
            }
            "editor/save" => {
                let path = self.resolve_path(params["path"].as_str().map(Path::new));
                self.save_document(path.as_deref()).map(|()| json!({}))
            }
            "actions" => parse_actions(params)
                .and_then(|actions| self.apply_actions(actions))
                .map(|()| json!({})),
            "ui/pick" | "ui/prompt"
                if self.plugin_state.waiting.is_some() || self.ui.overlay.is_some() =>
            {
                Err("mog is already asking something, try again later".to_owned())
            }
            "ui/pick" => {
                let title = params["title"].as_str().unwrap_or("pick one").to_owned();
                let items: Vec<PickerItem> = params["items"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| match item {
                                Value::String(label) => PickerItem::new(label),
                                Value::Object(_) => {
                                    let row =
                                        PickerItem::new(item["label"].as_str().unwrap_or_default())
                                            .detail(item["detail"].as_str().unwrap_or_default())
                                            .hint(item["hint"].as_str().unwrap_or_default());
                                    match item["preview"].as_str() {
                                        Some(preview) => row.preview(preview),
                                        None => row,
                                    }
                                }
                                other => PickerItem::new(other.to_string()),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.ui.plugin_pick = Some(PluginPick {
                    title,
                    items,
                    multi: params["multi"].as_bool().unwrap_or(false),
                });
                self.ui.open(Overlay::PluginPick);
                self.plugin_state.waiting = Some((plugin, id));
                return;
            }
            "ui/prompt" => {
                let title = params["title"].as_str().unwrap_or("type something");
                let text = params["text"].as_str().unwrap_or_default();
                let hint = params["hint"]
                    .as_str()
                    .unwrap_or("enter to send, esc to cancel");
                self.ui.ask(PromptKind::Plugin, title, text, hint);
                self.plugin_state.waiting = Some((plugin, id));
                return;
            }
            other => Err(format!("mog does not know {other}")),
        };
        self.plugins.respond(&plugin, id, result);
    }

    /// Answers `editor/text`: an open file or a part of it, or a file from disk.
    fn read_text(&self, params: &Value) -> Result<Value, String> {
        let wanted = self.resolve_path(params["path"].as_str().map(Path::new));
        let range = (params["start"].as_u64(), params["end"].as_u64());
        if let Some(index) = self.find_document(wanted.as_deref()) {
            let document = &self.editor.documents()[index];
            let text = document.text();
            let len = text.len_chars();
            let to_offset = |value: Option<u64>, default: usize| {
                value.map_or(Ok(default), |value| {
                    usize::try_from(value)
                        .ok()
                        .filter(|value| *value <= len)
                        .ok_or_else(|| format!("{value} is past the end, the text has {len} chars"))
                })
            };
            let start = to_offset(range.0, 0)?;
            let end = to_offset(range.1, len)?;
            if start > end {
                return Err(format!("start {start} is after end {end}"));
            }
            return Ok(json!({
                "text": text.slice(start..end).to_string(),
                "version": document.version(),
                "length": len,
            }));
        }
        let path = wanted.ok_or("no file is open")?;
        let text = fs::read_to_string(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        Ok(json!({ "text": text, "version": null, "length": text.chars().count() }))
    }

    /// Answers `editor/documents`.
    fn list_documents(&self) -> Value {
        let active = self.editor.active();
        let documents: Vec<Value> = self
            .editor
            .documents()
            .iter()
            .enumerate()
            .map(|(index, document)| {
                json!({
                    "path": document.path().map(|path| path.to_string_lossy()),
                    "name": document.name(),
                    "language": language(document.path()),
                    "version": document.version(),
                    "modified": document.is_modified(),
                    "active": index == active,
                })
            })
            .collect();
        json!({ "documents": documents })
    }

    /// Answers `editor/diagnostics`.
    fn list_diagnostics(&self, params: &Value) -> Result<Value, String> {
        let path = self.resolve_path(params["path"].as_str().map(Path::new));
        let index = self
            .find_document(path.as_deref())
            .ok_or("the file is not open")?;
        let document = &self.editor.documents()[index];
        let text = document.text();
        let diagnostics: Vec<Value> = document
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let line = text.char_to_line(diagnostic.from.min(text.len_chars()));
                json!({
                    "start": diagnostic.from,
                    "end": diagnostic.to,
                    "line": line,
                    "column": diagnostic.from - text.line_to_char(line),
                    "severity": severity_name(diagnostic.severity),
                    "message": diagnostic.message,
                })
            })
            .collect();
        Ok(json!({ "diagnostics": diagnostics }))
    }

    /// Sets the selections of the document at `path`, opening it if needed.
    fn select(
        &mut self,
        path: Option<PathBuf>,
        selections: Vec<(usize, usize)>,
    ) -> Result<(), String> {
        if let Some(path) = &path
            && self.find_document(Some(path)).is_none()
        {
            self.editor
                .open(path)
                .map_err(|err| format!("could not open {}: {err}", path.display()))?;
        }
        let index = self
            .find_document(path.as_deref())
            .ok_or("the file is not open")?;
        let len = self.editor.documents()[index].text().len_chars();
        if let Some((anchor, head)) = selections
            .iter()
            .find(|(anchor, head)| *anchor > len || *head > len)
        {
            return Err(format!(
                "selection {anchor}..{head} is past the end, the text has {len} chars"
            ));
        }
        self.editor.focus(index);
        let (anchor, head) = selections[0];
        self.editor.select(anchor, head);
        let extras = selections[1..]
            .iter()
            .map(|&(anchor, head)| Range::new(anchor, head))
            .collect();
        self.editor.document_mut().set_cursors(extras);
        Ok(())
    }

    /// Saves the document at `path`, or the focused one, without asking plugins first.
    fn save_document(&mut self, path: Option<&Path>) -> Result<(), String> {
        let index = self.find_document(path).ok_or("the file is not open")?;
        if self.editor.documents()[index].path().is_none() {
            return Err("the file has no name yet".into());
        }
        let focused = self.editor.active();
        self.editor.focus(index);
        self.plugin_state.saving = true;
        self.execute_command(Command::Save);
        self.plugin_state.saving = false;
        let saved = !self.editor.document().is_modified();
        self.editor.focus(focused);
        if saved {
            Ok(())
        } else {
            Err("the file could not be saved".into())
        }
    }

    /// Tells a waiting plugin the user closed the picker or prompt without an answer.
    pub(super) fn sync_plugin_ui(&mut self) {
        let asking = matches!(self.ui.overlay, Some(Overlay::PluginPick))
            || self
                .ui
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.kind == PromptKind::Plugin);
        if !asking && let Some((plugin, id)) = self.plugin_state.waiting.take() {
            self.plugins.respond(&plugin, id, Ok(Value::Null));
        }
    }

    /// Passes the row picked from a plugin's list back to it.
    pub(super) fn plugin_picked(&mut self) {
        let picked = self.ui.plugin_picked.take().unwrap_or_default();
        let Some(pick) = self.ui.plugin_pick.as_ref() else {
            return;
        };
        let chosen: Vec<(usize, String)> = picked
            .into_iter()
            .filter_map(|index| Some((index, pick.items.get(index)?.label.clone())))
            .collect();
        let Some((index, item)) = chosen.first().cloned() else {
            return;
        };
        let mut answer = json!({ "index": index, "item": item });
        if pick.multi {
            answer["indices"] = json!(chosen.iter().map(|(index, _)| index).collect::<Vec<_>>());
            answer["items"] = json!(chosen.iter().map(|(_, item)| item).collect::<Vec<_>>());
        }
        if let Some((plugin, id)) = self.plugin_state.waiting.take() {
            self.plugins.respond(&plugin, id, Ok(answer));
        }
    }

    /// Passes what was typed in a plugin's prompt back to it.
    pub(super) fn plugin_prompted(&mut self, text: &str) {
        if let Some((plugin, id)) = self.plugin_state.waiting.take() {
            self.plugins
                .respond(&plugin, id, Ok(json!({ "text": text })));
        }
    }

    /// Does what a plugin asked for, stopping at the first thing that fails.
    ///
    /// # Errors
    ///
    /// Returns why an action could not be done.
    pub(super) fn apply_actions(&mut self, actions: Vec<Action>) -> Result<(), String> {
        for action in actions {
            match action {
                Action::Status(text) => self.editor.set_status(text),
                Action::Notify { text, level } => {
                    let mark = match level {
                        Level::Info => "",
                        Level::Warning => "\u{26a0} ",
                        Level::Error => "\u{2716} ",
                    };
                    self.editor.set_status(format!("{mark}{text}"));
                }
                Action::Insert(text) => self.execute_command(Command::InsertText(text)),
                Action::Command { name, args } => {
                    if split_command(&name).is_some() {
                        self.run_plugin_command(&name, args);
                    } else {
                        let command = name
                            .parse()
                            .map_err(|err: UnknownCommand| err.to_string())?;
                        self.execute_command(command);
                    }
                }
                Action::Open { path, line, column } => {
                    let path = self.resolve_path(Some(&path)).unwrap_or(path);
                    self.editor
                        .open(&path)
                        .map_err(|err| format!("could not open {}: {err}", path.display()))?;
                    if let Some(line) = line {
                        let text = self.editor.document().text();
                        let line = line.min(text.len_lines().saturating_sub(1));
                        let start = text.line_to_char(line);
                        let line_len = text.line(line).len_chars();
                        let pos = start + column.unwrap_or(0).min(line_len);
                        self.editor.select(pos, pos);
                    }
                }
                Action::Edit(edit) => self.apply_workspace_edit(vec![edit])?,
                Action::WorkspaceEdit(edits) => self.apply_workspace_edit(edits)?,
                Action::Select { path, selections } => {
                    let path = self.resolve_path(path.as_deref());
                    self.select(path, selections)?;
                }
                Action::Save(path) => {
                    let path = self.resolve_path(path.as_deref());
                    self.save_document(path.as_deref())?;
                }
                Action::Output { title, text } => self.show_output(&title, &text),
            }
        }
        Ok(())
    }

    /// Tells plugins a file got focus and starts plugins that wait for its language.
    pub(super) fn plugin_focus_changed(&mut self, path: Option<&Path>) {
        let language = language(path);
        if let Some(language) = &language {
            let problems = self.plugins.activate_language(language);
            if !problems.is_empty() {
                self.editor.set_status(problems.join("; "));
            }
        }
        self.plugins.event(
            "opened",
            &json!({ "path": path.map(|path| path.to_string_lossy()), "language": language }),
        );
    }

    /// Tells plugins a file was saved.
    pub(super) fn plugin_saved(&self, path: &Path) {
        self.plugins.event(
            "saved",
            &json!({ "path": path.to_string_lossy(), "language": language(Some(path)) }),
        );
    }

    /// Tells plugins the diagnostics of a file changed.
    pub(super) fn plugin_diagnostics_changed(&self, path: &Path) {
        if !self.plugins.wants("diagnostics") {
            return;
        }
        let Some(document) = self
            .editor
            .documents()
            .iter()
            .find(|document| document.path() == Some(path))
        else {
            return;
        };
        let count = |severity| {
            document
                .diagnostics()
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        self.plugins.event(
            "diagnostics",
            &json!({
                "path": path.to_string_lossy(),
                "errors": count(Severity::Error),
                "warnings": count(Severity::Warning),
            }),
        );
    }

    /// Tells plugins a task finished.
    pub(super) fn plugin_task_finished(&self, name: &str, code: Option<i32>) {
        self.plugins.event(
            "task_finished",
            &json!({ "name": name, "code": code, "success": code == Some(0) }),
        );
    }

    /// Tells plugins the git status changed.
    pub(super) fn plugin_git_changed(&self) {
        self.plugins
            .event("git_changed", &json!({ "branch": self.ui.branch }));
    }

    /// Tells plugins the config changed.
    pub(super) fn plugin_config_changed(&self) {
        self.plugins.event("config_changed", &json!({}));
    }

    /// Sends plugins what changed since the last frame: edits, closed files, where the cursor
    /// rests and whether the user went idle. Also restarts plugins that crashed a while ago.
    pub(super) fn sync_plugin_events(&mut self, typing: bool) {
        self.sync_plugin_screen();
        let mut problems = self.plugins.restart_due();
        problems.extend(self.plugins.check_health());
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
        self.refresh_plugin_health(false);
        let open: BTreeSet<PathBuf> = self
            .editor
            .documents()
            .iter()
            .filter_map(|document| document.path().map(ToOwned::to_owned))
            .collect();
        for closed in self.plugin_state.open.difference(&open) {
            self.plugins
                .event("closed", &json!({ "path": closed.to_string_lossy() }));
            self.plugin_state.changed.remove(closed);
            self.plugin_state
                .diagnosed
                .retain(|(_, path), _| path != closed);
        }
        self.plugin_state.open = open;
        if !typing && self.plugins.wants("changed") {
            self.send_changes();
        }
        if !typing {
            self.request_plugin_diagnostics();
        }
        if self.plugins.wants("selection") && self.last_input.elapsed() >= SELECTION_DELAY {
            let document = self.editor.document();
            let selections: Vec<(usize, usize)> = [document.selection()]
                .into_iter()
                .chain(document.cursors().iter().copied())
                .map(|range| (range.anchor, range.head))
                .collect();
            let now = (document.path().map(ToOwned::to_owned), selections);
            if self.plugin_state.selection.as_ref() != Some(&now) {
                let list: Vec<Value> = now
                    .1
                    .iter()
                    .map(|(anchor, head)| json!({ "anchor": anchor, "head": head }))
                    .collect();
                self.plugins.event(
                    "selection",
                    &json!({
                        "path": now.0.as_ref().map(|path| path.to_string_lossy()),
                        "selections": list,
                    }),
                );
                self.plugin_state.selection = Some(now);
            }
        }
        if self.ui.idle && !self.plugin_state.idle_sent {
            self.plugin_state.idle_sent = true;
            self.plugins.event("idle", &json!({}));
        } else if !self.ui.idle {
            self.plugin_state.idle_sent = false;
        }
    }

    /// Sends `changed` for every file edited since the last one, as changes to apply in order
    /// when they are known and as the whole text otherwise.
    fn send_changes(&mut self) {
        let mut events = Vec::new();
        for document in self.editor.documents() {
            let Some(path) = document.path() else {
                continue;
            };
            let version = document.version();
            let sent = self.plugin_state.changed.get(path).copied();
            if sent == Some(version) || (sent.is_none() && !document.is_modified()) {
                continue;
            }
            let mut params = json!({ "path": path.to_string_lossy(), "version": version });
            match sent.and_then(|sent| document.changes_since(sent)) {
                Some(transactions) => params["changes"] = json!(sequential_changes(&transactions)),
                None if document.is_large() => {}
                None => params["text"] = json!(document.text().to_string()),
            }
            events.push((path.to_owned(), version, params));
        }
        for (path, version, params) in events {
            self.plugins.event("changed", &params);
            self.plugin_state.changed.insert(path, version);
        }
    }

    /// Asks plugins listening for `before_save` to change the focused file before it is saved.
    ///
    /// Returns `true` when the save waits for them and happens later.
    pub(super) fn before_save(&mut self) -> bool {
        if self.plugin_state.saving || self.plugin_state.pending_save.is_some() {
            return self.plugin_state.pending_save.is_some();
        }
        let listeners = self.plugins.listeners("before_save");
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            return false;
        };
        if listeners.is_empty() || document.is_large() {
            return false;
        }
        let version = document.version();
        let params = json!({
            "path": path.to_string_lossy(),
            "language": language(Some(&path)),
            "version": version,
            "text": document.text().to_string(),
        });
        self.plugin_state.pending_save = Some(path.clone());
        self.plugins.spawn(async move {
            let asks = listeners.into_iter().map(|plugin| {
                let params = params.clone();
                async move {
                    let answer = plugin
                        .request("before_save", params, BEFORE_SAVE_TIMEOUT)
                        .await;
                    (plugin.name().to_owned(), answer)
                }
            });
            let answers = future::join_all(asks).await;
            PluginUpdate::BeforeSave {
                path,
                version,
                answers,
            }
        });
        true
    }

    /// Applies what plugins answered to `before_save` and saves the file.
    fn finish_before_save(
        &mut self,
        path: &Path,
        version: u64,
        answers: Vec<(String, Result<Value, String>)>,
    ) {
        self.plugin_state.pending_save = None;
        let Some(index) = self.find_document(Some(path)) else {
            return;
        };
        let focused = self.editor.active();
        self.editor.focus(index);
        let mut problems = Vec::new();
        let mut changes = Vec::new();
        for (plugin, answer) in answers {
            let edit = answer.and_then(|answer| {
                parse_action(&json!({
                    "type": "edit",
                    "changes": answer.get("changes").cloned().unwrap_or(json!([])),
                }))
            });
            match edit {
                Ok(Action::Edit(edit)) => changes.extend(to_changes(edit.changes)),
                Ok(_) => {}
                Err(err) => problems.push(format!("{plugin}: {err}")),
            }
        }
        // the user kept typing, so the changes were worked out for older text
        if self.editor.document().version() == version
            && !changes.is_empty()
            && let Err(err) = self.editor.try_apply_changes(changes)
        {
            problems.push(format!("before save: {err}"));
        }
        self.plugin_state.saving = true;
        self.execute_command(Command::Save);
        self.plugin_state.saving = false;
        self.editor.focus(focused);
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
    }

    /// Asks plugins that provide `provider` for files like the focused one, in the background,
    /// returning the answers to `done` as `(plugin, answer)` pairs.
    pub(super) fn ask_plugins(
        &self,
        provider: &str,
        params: Value,
        timeout: Duration,
    ) -> Option<impl Future<Output = Vec<(String, Value)>> + Send + 'static> {
        let providers = self
            .plugins
            .providers(provider, language(self.editor.document().path()).as_deref());
        ask_these(providers, provider, params, timeout)
    }
}
