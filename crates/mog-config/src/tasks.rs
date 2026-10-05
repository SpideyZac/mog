//! Settings for tasks like building and testing, and for debuggers.

use serde::Deserialize;
use serde_json::Value;

/// A command that builds, tests or runs the project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskConfig {
    /// The command line, run through the shell, like `cargo build`.
    pub command: String,
    /// The folder to run it in, relative to the project. Empty runs it in the project.
    pub cwd: String,
}

/// How to start a debugger that speaks the debug adapter protocol.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DebugConfig {
    /// The debug adapter program, like `lldb-dap` or `python`.
    pub command: String,
    /// The arguments to pass to it, like `["-m", "debugpy.adapter"]`.
    pub args: Vec<String>,
    /// The file extensions this debugger is picked for, without the dot.
    pub extensions: Vec<String>,
    /// Either `launch` to start the program or `attach` to join a running one.
    pub request: String,
    /// The arguments of the launch or attach request, which differ for every adapter.
    ///
    /// `${file}` and `${root}` in strings are replaced with the focused file and the project.
    pub arguments: Value,
    /// A task to run first, like a build, by name.
    pub before: String,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            extensions: Vec::new(),
            request: "launch".into(),
            arguments: Value::Null,
            before: String::new(),
        }
    }
}
