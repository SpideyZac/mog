//! Starting the plugin host.

use mog_config::config_dir;
use mog_plugin::PluginHost;

/// The directory inside the config directory that holds plugins.
const PLUGIN_DIR: &str = "plugins";

/// Starts the plugin host and loads every installed plugin.
///
/// Problems are appended to `problems`. Returns `None` if Lua itself could not start.
pub fn start(problems: &mut Vec<String>) -> Option<PluginHost> {
    let mut host = match PluginHost::new() {
        Ok(host) => host,
        Err(err) => {
            problems.push(format!("plugins are off, lua failed to start: {err}"));
            return None;
        }
    };
    if let Some(dir) = config_dir() {
        problems.extend(host.load_dir(&dir.join(PLUGIN_DIR)));
    }
    Some(host)
}
