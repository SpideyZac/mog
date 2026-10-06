//! The messages of the plugin protocol and strict parsing of what plugins send.
//!
//! Anything a plugin sends that mog cannot use is an error with a reason, never silently
//! dropped, so a plugin author sees what went wrong.

use std::{collections::BTreeSet, path::PathBuf};

use mog_core::when::When;
use serde_json::{Value, json};

/// The newest version of the plugin protocol mog speaks.
pub const PROTOCOL_VERSION: u32 = 2;

/// The oldest version of the plugin protocol mog still speaks.
pub const OLDEST_PROTOCOL: u32 = 1;

/// The events a plugin hears when it does not say which it wants, as in protocol 1.
pub const DEFAULT_EVENTS: &[&str] = &["opened", "saved"];

/// Every event mog can send, for plugins to subscribe to.
pub const EVENTS: &[&str] = &[
    "opened",
    "closed",
    "saved",
    "changed",
    "selection",
    "diagnostics",
    "config_changed",
    "idle",
    "task_finished",
    "git_changed",
    "before_save",
];

/// Events a plugin gets for things it set up itself, without listing them in `events`.
pub const OWN_EVENTS: &[&str] = &[
    "click",
    "timer",
    "segment_click",
    "toast_click",
    "panel_click",
    "panel_closed",
];

/// Every request a plugin can send to mog.
pub const REQUESTS: &[&str] = &[
    "editor/context",
    "editor/text",
    "editor/documents",
    "editor/diagnostics",
    "editor/select",
    "editor/save",
    "actions",
    "ui/pick",
    "ui/prompt",
    "ui/layout",
];

/// Every notification a plugin can send to mog.
pub const NOTIFICATIONS: &[&str] = &[
    "actions",
    "status",
    "notify",
    "log",
    "segment",
    "progress",
    "diagnostics",
    "decorations",
    "output",
    "draw",
    "clear",
    "cursor",
    "capture",
    "timer",
    "toast",
    "panel",
    "canvas",
];

/// Every action a plugin can ask for.
pub const ACTIONS: &[&str] = &[
    "status",
    "notify",
    "insert",
    "edit",
    "workspace_edit",
    "open",
    "command",
    "select",
    "save",
    "output",
];

/// Every provider a plugin can register.
pub const PROVIDERS: &[&str] = &[
    "completion",
    "hover",
    "formatting",
    "code_actions",
    "definition",
    "references",
    "symbols",
    "diagnostics",
    "tasks",
];

/// A tool a plugin offers the AI chat, which the model may call while it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTool {
    /// The name inside the plugin, letters, digits, `_` and `-`.
    pub name: String,
    /// What it does and when to use it, for the model.
    pub description: String,
    /// A JSON schema of its input, an object.
    pub input_schema: Value,
}

/// Reads the tools in a list like `[{ "name", "description", "input_schema" }]`.
///
/// # Errors
///
/// Returns why a tool cannot be used, like a name the model cannot call.
pub fn parse_tools(value: &Value) -> Result<Vec<PluginTool>, String> {
    let Some(tools) = value.as_array() else {
        return Ok(Vec::new());
    };
    tools
        .iter()
        .map(|tool| {
            let name = string(tool, "name", "a tool")?;
            let valid = (1..=48).contains(&name.len())
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
            if !valid {
                return Err(format!(
                    "`{name}` is not a valid tool name, use up to 48 letters, digits, _ and -"
                ));
            }
            let input_schema = match &tool["input_schema"] {
                Value::Null => json!({ "type": "object", "properties": {} }),
                schema if schema["type"] == "object" => schema.clone(),
                _ => {
                    return Err(format!(
                        "the input_schema of tool {name} must be an object schema"
                    ));
                }
            };
            Ok(PluginTool {
                description: tool["description"].as_str().unwrap_or_default().to_owned(),
                name,
                input_schema,
            })
        })
        .collect()
}

/// A command a plugin adds to the palette.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginCommand {
    /// The name inside the plugin, like `count_words`.
    pub name: String,
    /// What the palette shows, like `Words: Count`.
    pub title: String,
    /// Keys the plugin would like bound to it, used when nothing else has them.
    pub keys: Vec<String>,
    /// Whether the command shows in the editor's right click menu.
    pub menu: bool,
    /// When it shows in the right click menu, always when `None`.
    pub when: Option<When>,
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

/// Changes to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    /// The file, the focused one when `None`.
    pub path: Option<PathBuf>,
    /// The document version the changes were worked out for, checked when given.
    pub version: Option<u64>,
    /// The changes, which must not overlap.
    pub changes: Vec<Edit>,
}

/// How loud a notification is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Something worth knowing.
    Info,
    /// Something that may be wrong.
    Warning,
    /// Something that went wrong.
    Error,
}

/// Something a plugin asks mog to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Show a message in the status line.
    Status(String),
    /// Show a message with a level, kept in the plugin log too.
    Notify {
        /// The message.
        text: String,
        /// How loud it is.
        level: Level,
    },
    /// Change one document, in one undo step.
    Edit(FileEdit),
    /// Change several files at once. Nothing changes if any part does not fit.
    WorkspaceEdit(Vec<FileEdit>),
    /// Replace the selection, or type at the cursor.
    Insert(String),
    /// Open a file, at a line and column from 0 if given.
    Open {
        /// The file.
        path: PathBuf,
        /// The line to put the cursor on.
        line: Option<usize>,
        /// The column to put the cursor on.
        column: Option<usize>,
    },
    /// Run a mog command by name, like `save` or `terminal.toggle`, with arguments for plugin
    /// commands.
    Command {
        /// The command name.
        name: String,
        /// The arguments, `null` when none.
        args: Value,
    },
    /// Set the selections of a document, the first one being the main cursor.
    Select {
        /// The file, the focused one when `None`.
        path: Option<PathBuf>,
        /// The selections as `(anchor, head)` char offsets.
        selections: Vec<(usize, usize)>,
    },
    /// Save a file, the focused one when `None`.
    Save(Option<PathBuf>),
    /// Show text in the output panel.
    Output {
        /// The panel title.
        title: String,
        /// The text.
        text: String,
    },
}

/// A short text a plugin puts in the status line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Segment {
    /// The text, empty to remove it.
    pub text: String,
    /// A theme color name or hex code, if not the plain status color.
    pub color: Option<String>,
    /// A command clicking it runs.
    pub command: Option<String>,
    /// A background color, a theme color name or hex code.
    pub bg: Option<String>,
    /// Whether the text is bold.
    pub bold: bool,
    /// Whether it goes on the left, after the file name, instead of the right.
    pub left: bool,
}

/// The languages a provider works for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Languages {
    /// Every file.
    All,
    /// Files with these extensions, like `md`.
    Some(BTreeSet<String>),
}

impl Languages {
    /// Returns whether a file with the extension `language` is covered.
    pub fn covers(&self, language: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Some(languages) => language.is_some_and(|language| languages.contains(language)),
        }
    }
}

/// What a plugin said about itself when it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The protocol version the plugin speaks.
    pub protocol: u32,
    /// The commands it adds.
    pub commands: Vec<PluginCommand>,
    /// The events it wants to hear.
    pub events: BTreeSet<String>,
    /// The providers it registers, by name like `completion`.
    pub providers: Vec<(String, Languages)>,
    /// Providers it listed that mog does not know, so it is never asked for them.
    pub unknown_providers: Vec<String>,
    /// Tools it offers the AI chat.
    pub tools: Vec<PluginTool>,
}

impl Hello {
    /// Returns whether the plugin provides `provider` for files with extension `language`.
    pub fn provides(&self, provider: &str, language: Option<&str>) -> bool {
        self.providers
            .iter()
            .any(|(name, languages)| name == provider && languages.covers(language))
    }
}

/// Reads a non negative offset.
fn offset(value: &Value) -> Option<usize> {
    value.as_u64().and_then(|n| usize::try_from(n).ok())
}

/// Reads a required string field `key` of `object`, saying what was wrong if it is missing.
fn string(object: &Value, key: &str, what: &str) -> Result<String, String> {
    object[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{what} needs a string `{key}`"))
}

/// Reads a list of strings, ignoring a missing one.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the commands of a handshake.
///
/// # Errors
///
/// Returns an error if a command has no name or a name with spaces or colons.
pub fn parse_commands(value: &Value) -> Result<Vec<PluginCommand>, String> {
    let Some(commands) = value.as_array() else {
        return Ok(Vec::new());
    };
    commands
        .iter()
        .map(|command| {
            let name = string(command, "name", "a command")?;
            if name.is_empty() || name.contains(char::is_whitespace) || name.contains(':') {
                return Err(format!("`{name}` is not a valid command name"));
            }
            let when = command["when"]
                .as_str()
                .map(|when| {
                    when.parse::<When>()
                        .map_err(|err| format!("command {name}: {err}"))
                })
                .transpose()?;
            Ok(PluginCommand {
                title: command["title"].as_str().unwrap_or(&name).to_owned(),
                keys: strings(&command["keys"]),
                menu: command["menu"].as_bool().unwrap_or(false),
                when,
                name,
            })
        })
        .collect()
}

/// Reads the languages of a provider, `true` meaning all of them.
fn parse_languages(value: &Value) -> Option<Languages> {
    match value {
        Value::Bool(true) => Some(Languages::All),
        Value::Array(_) => Some(Languages::Some(strings(value).into_iter().collect())),
        Value::Object(object) => match object.get("languages") {
            None => Some(Languages::All),
            Some(languages) => parse_languages(languages),
        },
        _ => None,
    }
}

/// Reads an `initialize` result.
///
/// # Errors
///
/// Returns why the plugin cannot be used, like a protocol mog does not speak.
pub fn parse_hello(result: &Value) -> Result<Hello, String> {
    let protocol = match &result["protocolVersion"] {
        Value::Null => OLDEST_PROTOCOL,
        value => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("protocolVersion must be a number")?,
    };
    if !(OLDEST_PROTOCOL..=PROTOCOL_VERSION).contains(&protocol) {
        return Err(format!(
            "it speaks protocol {protocol} but this mog speaks {OLDEST_PROTOCOL} to \
             {PROTOCOL_VERSION}"
        ));
    }
    let events = match &result["events"] {
        Value::Null => DEFAULT_EVENTS
            .iter()
            .map(|&event| event.to_owned())
            .collect(),
        value => strings(value).into_iter().collect(),
    };
    let listed = result["providers"].as_object();
    let providers = listed
        .map(|providers| {
            providers
                .iter()
                .filter(|(name, _)| PROVIDERS.contains(&name.as_str()))
                .filter_map(|(name, value)| Some((name.clone(), parse_languages(value)?)))
                .collect()
        })
        .unwrap_or_default();
    let unknown_providers = listed
        .into_iter()
        .flat_map(|providers| providers.keys())
        .filter(|name| !PROVIDERS.contains(&name.as_str()))
        .cloned()
        .collect();
    Ok(Hello {
        protocol,
        commands: parse_commands(&result["commands"])?,
        events,
        providers,
        unknown_providers,
        tools: parse_tools(&result["tools"])?,
    })
}

/// Reads the changes of an edit.
fn parse_changes(value: &Value) -> Result<Vec<Edit>, String> {
    value
        .as_array()
        .ok_or("an edit needs a `changes` list")?
        .iter()
        .enumerate()
        .map(|(index, change)| {
            let start = offset(&change["start"]);
            let end = offset(&change["end"]);
            match (start, end, change["text"].as_str()) {
                (Some(start), Some(end), Some(text)) if start <= end => Ok(Edit {
                    start,
                    end,
                    text: text.to_owned(),
                }),
                (Some(start), Some(end), Some(_)) => Err(format!(
                    "change {index} starts at {start} after it ends at {end}"
                )),
                _ => Err(format!(
                    "change {index} needs a numeric `start` and `end` and a string `text`"
                )),
            }
        })
        .collect()
}

/// Reads the changes to one file.
fn parse_file_edit(value: &Value) -> Result<FileEdit, String> {
    Ok(FileEdit {
        path: value["path"].as_str().map(PathBuf::from),
        version: value["version"].as_u64(),
        changes: parse_changes(&value["changes"])?,
    })
}

/// Reads one action.
///
/// # Errors
///
/// Returns why the action cannot be done, like a missing field or an unknown type.
pub fn parse_action(action: &Value) -> Result<Action, String> {
    let kind = action["type"].as_str().ok_or("an action needs a `type`")?;
    let what = format!("a `{kind}` action");
    Ok(match kind {
        "status" => Action::Status(string(action, "text", &what)?),
        "notify" => Action::Notify {
            text: string(action, "text", &what)?,
            level: match action["level"].as_str() {
                Some("warning") => Level::Warning,
                Some("error") => Level::Error,
                _ => Level::Info,
            },
        },
        "insert" => Action::Insert(string(action, "text", &what)?),
        "command" => Action::Command {
            name: string(action, "name", &what)?,
            args: action["args"].clone(),
        },
        "open" => Action::Open {
            path: PathBuf::from(string(action, "path", &what)?),
            line: offset(&action["line"]),
            column: offset(&action["column"]),
        },
        "edit" => Action::Edit(parse_file_edit(action)?),
        "workspace_edit" => Action::WorkspaceEdit(
            action["edits"]
                .as_array()
                .ok_or("a `workspace_edit` action needs an `edits` list")?
                .iter()
                .map(parse_file_edit)
                .collect::<Result<_, _>>()?,
        ),
        "select" => Action::Select {
            path: action["path"].as_str().map(PathBuf::from),
            selections: parse_selections(&action["selections"])?,
        },
        "save" => Action::Save(action["path"].as_str().map(PathBuf::from)),
        "output" => Action::Output {
            title: action["title"].as_str().unwrap_or("plugin").to_owned(),
            text: string(action, "text", &what)?,
        },
        other => return Err(format!("mog does not know the action `{other}`")),
    })
}

/// Reads selections as `{ "anchor", "head" }` objects.
///
/// # Errors
///
/// Returns an error if the list is empty or a selection is not two offsets.
pub fn parse_selections(value: &Value) -> Result<Vec<(usize, usize)>, String> {
    let selections: Vec<(usize, usize)> = value
        .as_array()
        .ok_or("selections must be a list")?
        .iter()
        .map(|selection| {
            offset(&selection["anchor"])
                .zip(offset(&selection["head"]))
                .ok_or_else(|| "a selection needs a numeric `anchor` and `head`".to_owned())
        })
        .collect::<Result<_, _>>()?;
    if selections.is_empty() {
        return Err("there must be at least one selection".into());
    }
    Ok(selections)
}

/// Reads the actions from a command result or an `actions` message.
///
/// # Errors
///
/// Returns the first problem, so no action runs when one of them is broken.
pub fn parse_actions(value: &Value) -> Result<Vec<Action>, String> {
    match &value["actions"] {
        Value::Null => Ok(Vec::new()),
        Value::Array(actions) => actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                parse_action(action).map_err(|err| format!("action {index}: {err}"))
            })
            .collect(),
        _ => Err("`actions` must be a list".into()),
    }
}

/// Reads a `segment` notification.
pub fn parse_segment(value: &Value) -> Segment {
    Segment {
        text: value["text"].as_str().unwrap_or_default().to_owned(),
        color: value["color"].as_str().map(str::to_owned),
        command: value["command"].as_str().map(str::to_owned),
        bg: value["bg"].as_str().map(str::to_owned),
        bold: value["bold"].as_bool().unwrap_or(false),
        left: value["side"].as_str() == Some("left"),
    }
}

/// Returns the `initialize` parameters mog sends.
pub fn initialize_params(root: &str, settings: &Value) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "mogVersion": env!("CARGO_PKG_VERSION"),
        "root": root,
        "settings": settings,
        "capabilities": {
            "events": EVENTS,
            "requests": REQUESTS,
            "notifications": NOTIFICATIONS,
            "actions": ACTIONS,
            "providers": PROVIDERS,
        },
    })
}

#[cfg(test)]
/// Tests for reading what plugins send.
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        Action, Edit, FileEdit, Languages, Level, PROTOCOL_VERSION, parse_actions, parse_hello,
        parse_segment,
    };

    /// Every action shape is read.
    #[test]
    fn reads_actions() {
        let value = json!({ "actions": [
            { "type": "status", "text": "hi" },
            { "type": "notify", "text": "careful", "level": "warning" },
            { "type": "edit", "version": 3, "changes": [{ "start": 0, "end": 2, "text": "x" }] },
            { "type": "open", "path": "/a.txt", "line": 3 },
            { "type": "command", "name": "save" },
            { "type": "select", "selections": [{ "anchor": 1, "head": 2 }] },
            { "type": "save" },
            { "type": "output", "title": "t", "text": "body" },
        ]});
        assert_eq!(
            parse_actions(&value).expect("valid"),
            [
                Action::Status("hi".into()),
                Action::Notify {
                    text: "careful".into(),
                    level: Level::Warning
                },
                Action::Edit(FileEdit {
                    path: None,
                    version: Some(3),
                    changes: vec![Edit {
                        start: 0,
                        end: 2,
                        text: "x".into()
                    }]
                }),
                Action::Open {
                    path: PathBuf::from("/a.txt"),
                    line: Some(3),
                    column: None,
                },
                Action::Command {
                    name: "save".into(),
                    args: serde_json::Value::Null
                },
                Action::Select {
                    path: None,
                    selections: vec![(1, 2)]
                },
                Action::Save(None),
                Action::Output {
                    title: "t".into(),
                    text: "body".into()
                },
            ]
        );
    }

    /// A broken action fails the whole list with a reason instead of being skipped.
    #[test]
    fn rejects_broken_actions() {
        let unknown =
            json!({ "actions": [{ "type": "status", "text": "a" }, { "type": "dance" }] });
        let err = parse_actions(&unknown).expect_err("unknown type");
        assert!(err.contains("action 1") && err.contains("dance"), "{err}");
        let bad_change = json!({ "actions": [{ "type": "edit", "changes": [{ "start": 2 }] }] });
        assert!(parse_actions(&bad_change).is_err());
        let backwards = json!({ "actions": [{ "type": "edit", "changes": [{ "start": 3, "end": 1, "text": "" }] }] });
        assert!(parse_actions(&backwards).is_err());
        assert!(parse_actions(&json!({ "actions": 4 })).is_err());
        assert!(parse_actions(&json!({})).expect("none").is_empty());
    }

    /// Old plugins get the old events and new ones choose theirs and register providers.
    #[test]
    fn reads_handshakes() {
        let old = parse_hello(&json!({ "commands": [{ "name": "count" }] })).expect("v1");
        assert_eq!(old.protocol, 1);
        assert!(old.events.contains("saved"));
        assert_eq!(old.commands[0].title, "count");
        let new = parse_hello(&json!({
            "protocolVersion": PROTOCOL_VERSION,
            "events": ["changed"],
            "providers": { "completion": ["md"], "hover": true, "teleport": true },
        }))
        .expect("v2");
        assert!(new.events.contains("changed") && !new.events.contains("saved"));
        assert!(new.provides("completion", Some("md")));
        assert!(!new.provides("completion", Some("rs")));
        assert!(new.provides("hover", None));
        assert_eq!(new.providers.len(), 2);
        assert_eq!(new.providers[1].1, Languages::All);
        assert!(parse_hello(&json!({ "protocolVersion": 99 })).is_err());
        assert!(parse_hello(&json!({ "commands": [{ "name": "two words" }] })).is_err());
    }

    /// Segments carry optional colors and commands.
    #[test]
    fn reads_segments() {
        let segment = parse_segment(&json!({ "text": "3w", "color": "green", "command": "x.y" }));
        assert_eq!(segment.color.as_deref(), Some("green"));
        assert_eq!(segment.command.as_deref(), Some("x.y"));
    }
}
