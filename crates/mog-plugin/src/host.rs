//! The Lua runtime plugins run in and the `mog` table they talk to.

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use mlua::{Error as LuaError, Function, Lua, Result as LuaResult, Table, Value};

/// The file loaded from each plugin directory.
const ENTRY_FILE: &str = "init.lua";

/// The name of the global table plugins use.
const API_TABLE: &str = "mog";

/// The hidden table inside [`API_TABLE`] holding registered commands.
const COMMANDS_TABLE: &str = "_commands";

/// The hidden table inside [`API_TABLE`] holding event handlers.
const HANDLERS_TABLE: &str = "_handlers";

/// Something a plugin asked the editor to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRequest {
    /// Show a message in the status line.
    Notify(String),
    /// Run an editor command by name.
    RunCommand(String),
}

/// Runs Lua plugins and collects what they ask for.
///
/// Plugins never touch editor state directly. They queue [`PluginRequest`]s that the editor
/// drains with [`PluginHost::take_requests`].
pub struct PluginHost {
    /// The Lua state every plugin shares.
    lua: Lua,
    /// Requests queued by plugins since the last drain.
    requests: Rc<RefCell<Vec<PluginRequest>>>,
    /// The names of the loaded plugins.
    loaded: Vec<String>,
}

impl PluginHost {
    /// Creates a host with the `mog` table installed.
    ///
    /// # Errors
    ///
    /// Returns an error if the Lua state cannot be set up.
    pub fn new() -> LuaResult<Self> {
        let lua = Lua::new();
        let requests = Rc::new(RefCell::new(Vec::new()));
        install_api(&lua, &requests)?;
        Ok(Self {
            lua,
            requests,
            loaded: Vec::new(),
        })
    }

    /// Runs `source` as a plugin called `name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the code fails to parse or raises an error.
    pub fn load_source(&mut self, name: &str, source: &str) -> LuaResult<()> {
        self.lua.load(source).set_name(name).exec()?;
        self.loaded.push(name.to_owned());
        Ok(())
    }

    /// Loads every `<dir>/<plugin>/init.lua`, in name order.
    ///
    /// Returns a message for each plugin that failed. A missing `dir` is not an error.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<String> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut plugins: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join(ENTRY_FILE).is_file())
            .collect();
        plugins.sort();
        let mut problems = Vec::new();
        for path in plugins {
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let result = fs::read_to_string(path.join(ENTRY_FILE))
                .map_err(|err| err.to_string())
                .and_then(|source| {
                    self.load_source(&name, &source)
                        .map_err(|err| err.to_string())
                });
            if let Err(err) = result {
                problems.push(format!("plugin {name} failed: {err}"));
            }
        }
        problems
    }

    /// Returns the names of the loaded plugins.
    pub fn loaded(&self) -> &[String] {
        &self.loaded
    }

    /// Returns `true` if a plugin registered a command called `name`.
    pub fn has_command(&self, name: &str) -> bool {
        self.command(name).is_some()
    }

    /// Runs the plugin command called `name`.
    ///
    /// Returns `Ok(false)` if no plugin registered it.
    ///
    /// # Errors
    ///
    /// Returns an error if the command raises an error.
    pub fn run_command(&self, name: &str) -> LuaResult<bool> {
        let Some(command) = self.command(name) else {
            return Ok(false);
        };
        command.call::<()>(())?;
        Ok(true)
    }

    /// Calls every handler registered for `event` with `arg`.
    ///
    /// Returns a message for each handler that failed.
    pub fn emit(&self, event: &str, arg: &str) -> Vec<String> {
        let Ok(handlers) = self.hidden_table(HANDLERS_TABLE) else {
            return Vec::new();
        };
        let Ok(list) = handlers.get::<Option<Table>>(event) else {
            return Vec::new();
        };
        list.into_iter()
            .flat_map(|list| list.sequence_values::<Function>().collect::<Vec<_>>())
            .filter_map(|handler| handler.and_then(|handler| handler.call::<()>(arg)).err())
            .map(|err| format!("{event} handler failed: {err}"))
            .collect()
    }

    /// Returns and clears the requests plugins queued.
    pub fn take_requests(&self) -> Vec<PluginRequest> {
        self.requests.take()
    }

    /// Looks up a registered command.
    fn command(&self, name: &str) -> Option<Function> {
        self.hidden_table(COMMANDS_TABLE)
            .ok()?
            .get::<Option<Function>>(name)
            .ok()
            .flatten()
    }

    /// Returns one of the hidden tables inside the `mog` table.
    fn hidden_table(&self, name: &str) -> LuaResult<Table> {
        self.lua.globals().get::<Table>(API_TABLE)?.get(name)
    }
}

/// Installs the global `mog` table.
fn install_api(lua: &Lua, requests: &Rc<RefCell<Vec<PluginRequest>>>) -> LuaResult<()> {
    let api = lua.create_table()?;
    api.set("version", env!("CARGO_PKG_VERSION"))?;
    api.set(COMMANDS_TABLE, lua.create_table()?)?;
    api.set(HANDLERS_TABLE, lua.create_table()?)?;

    let queue = Rc::clone(requests);
    api.set(
        "notify",
        lua.create_function(move |_, message: String| {
            queue.borrow_mut().push(PluginRequest::Notify(message));
            Ok(())
        })?,
    )?;

    let queue = Rc::clone(requests);
    api.set(
        "run",
        lua.create_function(move |_, command: String| {
            queue.borrow_mut().push(PluginRequest::RunCommand(command));
            Ok(())
        })?,
    )?;

    api.set(
        "command",
        lua.create_function(|lua, (name, command): (String, Function)| {
            if !name.contains('.') {
                return Err(LuaError::runtime(
                    "command names need a namespace like `myplugin.wave`",
                ));
            }
            let api: Table = lua.globals().get(API_TABLE)?;
            api.get::<Table>(COMMANDS_TABLE)?.set(name, command)
        })?,
    )?;

    api.set(
        "on",
        lua.create_function(|lua, (event, handler): (String, Function)| {
            let api: Table = lua.globals().get(API_TABLE)?;
            let handlers: Table = api.get(HANDLERS_TABLE)?;
            let list = match handlers.get::<Value>(event.as_str())? {
                Value::Table(list) => list,
                _ => {
                    let list = lua.create_table()?;
                    handlers.set(event, &list)?;
                    list
                }
            };
            list.push(handler)
        })?,
    )?;

    lua.globals().set(API_TABLE, api)
}

#[cfg(test)]
/// Tests for [`PluginHost`].
mod tests {
    use super::{PluginHost, PluginRequest};

    /// Notifications and command runs are queued for the editor.
    #[test]
    fn queues_requests() {
        let mut host = PluginHost::new().expect("host");
        host.load_source("test", "mog.notify('hi') mog.run('save')")
            .expect("load");
        assert_eq!(
            host.take_requests(),
            [
                PluginRequest::Notify("hi".into()),
                PluginRequest::RunCommand("save".into())
            ]
        );
        assert!(host.take_requests().is_empty());
    }

    /// Registered commands can be run by name.
    #[test]
    fn runs_commands() {
        let mut host = PluginHost::new().expect("host");
        host.load_source(
            "test",
            "mog.command('test.wave', function() mog.notify('o/') end)",
        )
        .expect("load");
        assert!(host.has_command("test.wave"));
        assert!(host.run_command("test.wave").expect("run"));
        assert!(!host.run_command("test.missing").expect("run"));
        assert_eq!(host.take_requests(), [PluginRequest::Notify("o/".into())]);
    }

    /// Commands without a namespace are rejected.
    #[test]
    fn rejects_bare_command_names() {
        let mut host = PluginHost::new().expect("host");
        assert!(
            host.load_source("test", "mog.command('wave', function() end)")
                .is_err()
        );
    }

    /// Event handlers get the event argument.
    #[test]
    fn emits_events() {
        let mut host = PluginHost::new().expect("host");
        host.load_source(
            "test",
            "mog.on('open', function(path) mog.notify(path) end)",
        )
        .expect("load");
        assert!(host.emit("open", "a.rs").is_empty());
        assert!(host.emit("save", "a.rs").is_empty());
        assert_eq!(host.take_requests(), [PluginRequest::Notify("a.rs".into())]);
    }
}
