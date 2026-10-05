//! Starting plugins from the config and routing their commands and events.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use mog_config::PluginConfig;
use mog_plugin::{Action, Plugin, PluginCommand, PluginEvent};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// The prefix of every plugin command name, like `plugin.words.count`.
pub const PREFIX: &str = "plugin.";

/// Something plugins reported.
#[derive(Debug)]
pub enum PluginUpdate {
    /// A plugin did something on its own.
    Event(PluginEvent),
    /// A command finished, with what it asks for or why it failed.
    Ran(String, Result<Vec<Action>, String>),
}

/// The running plugins and the commands they added.
pub struct Plugins {
    /// The running plugins by name.
    running: HashMap<String, Plugin>,
    /// The commands of each plugin that finished starting.
    commands: BTreeMap<String, Vec<PluginCommand>>,
    /// Where plugins send events.
    sender: UnboundedSender<PluginEvent>,
    /// Events from plugins.
    events: UnboundedReceiver<PluginEvent>,
    /// Where finished commands report.
    ran_tx: UnboundedSender<(String, Result<Vec<Action>, String>)>,
    /// Finished commands.
    ran: UnboundedReceiver<(String, Result<Vec<Action>, String>)>,
}

/// Returns the full command name for `command` of `plugin`.
pub fn command_name(plugin: &str, command: &str) -> String {
    format!("{PREFIX}{plugin}.{command}")
}

impl Plugins {
    /// Creates the manager with nothing running.
    pub fn new() -> Self {
        let (sender, events) = mpsc::unbounded_channel();
        let (ran_tx, ran) = mpsc::unbounded_channel();
        Self {
            running: HashMap::new(),
            commands: BTreeMap::new(),
            sender,
            events,
            ran_tx,
            ran,
        }
    }

    /// Starts every enabled plugin in `configs` for the project at `root`.
    ///
    /// Returns a message for each one that could not start.
    pub fn start(&mut self, configs: &BTreeMap<String, PluginConfig>, root: &Path) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, config) in configs {
            if !config.enabled || config.command.is_empty() || self.running.contains_key(name) {
                continue;
            }
            match Plugin::start(
                name,
                &config.command,
                &config.args,
                root,
                self.sender.clone(),
            ) {
                Ok(plugin) => {
                    self.running.insert(name.clone(), plugin);
                }
                Err(err) => problems.push(format!("plugin {name} could not start: {err}")),
            }
        }
        problems
    }

    /// Remembers the commands a plugin offered when it started.
    pub fn ready(&mut self, plugin: String, commands: Vec<PluginCommand>) {
        self.commands.insert(plugin, commands);
    }

    /// Forgets a plugin that stopped.
    pub fn exited(&mut self, plugin: &str) {
        self.running.remove(plugin);
        self.commands.remove(plugin);
    }

    /// Returns every plugin command as `(full name, title, suggested keys)`.
    pub fn palette(&self) -> Vec<(String, String, Vec<String>)> {
        self.commands
            .iter()
            .flat_map(|(plugin, commands)| {
                commands.iter().map(move |command| {
                    (
                        command_name(plugin, &command.name),
                        command.title.clone(),
                        command.keys.clone(),
                    )
                })
            })
            .collect()
    }

    /// Runs the plugin command called `full`, like `plugin.words.count`, with `context`.
    ///
    /// Returns `false` if no running plugin has that command.
    pub fn run(&self, full: &str, context: Value) -> bool {
        let Some((plugin, command)) = full
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.split_once('.'))
        else {
            return false;
        };
        let Some(running) = self.running.get(plugin).cloned() else {
            return false;
        };
        let (plugin, command) = (plugin.to_owned(), command.to_owned());
        let ran = self.ran_tx.clone();
        tokio::spawn(async move {
            let result = running.run(&command, context).await;
            let _ = ran.send((plugin, result));
        });
        true
    }

    /// Tells every plugin something happened, like `opened` or `saved`.
    pub fn event(&self, kind: &str, path: Option<&Path>) {
        for plugin in self.running.values() {
            plugin.event(kind, path);
        }
    }

    /// Waits for the next plugin event or finished command.
    pub async fn update(&mut self) -> Option<PluginUpdate> {
        tokio::select! {
            Some(event) = self.events.recv() => Some(PluginUpdate::Event(event)),
            Some((plugin, result)) = self.ran.recv() => Some(PluginUpdate::Ran(plugin, result)),
            else => None,
        }
    }
}

#[cfg(test)]
/// Tests for the plugin manager.
mod tests {
    use mog_plugin::PluginCommand;

    use super::{Plugins, command_name};

    /// Commands get namespaced by plugin and unknown ones do not run.
    #[tokio::test]
    async fn names_commands() {
        let mut plugins = Plugins::new();
        plugins.ready(
            "words".into(),
            vec![PluginCommand {
                name: "count".into(),
                title: "Words: Count".into(),
                keys: vec!["alt+w".into()],
            }],
        );
        assert_eq!(
            plugins.palette(),
            [(
                "plugin.words.count".to_owned(),
                "Words: Count".to_owned(),
                vec!["alt+w".to_owned()]
            )]
        );
        assert_eq!(command_name("a", "b"), "plugin.a.b");
        assert!(!plugins.run("plugin.words.count", serde_json::Value::Null));
    }
}
