//! Settings for the built in terminal.

use serde::Deserialize;

/// Settings for the terminal panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// The shell to run. Empty picks PowerShell on Windows and `$SHELL` elsewhere.
    pub shell: String,
    /// The arguments to pass to the shell.
    pub args: Vec<String>,
}
