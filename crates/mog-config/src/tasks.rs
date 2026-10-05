//! Settings for tasks like building and testing, and for debuggers.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

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
    /// `${root}`, `${rootName}`, `${file}`, `${fileDirname}`, `${fileBasenameNoExtension}` and
    /// `${exe}` in strings are filled in when debugging starts.
    pub arguments: Value,
    /// A task to run first, like a build, by name.
    pub before: String,
}

/// Returns the debuggers that work out of the box, if their adapter is installed.
fn builtin_debuggers() -> BTreeMap<String, DebugConfig> {
    let mut debuggers = BTreeMap::new();
    debuggers.insert(
        "lldb".to_owned(),
        DebugConfig {
            command: "lldb-dap".into(),
            extensions: ["rs", "c", "h", "cpp", "cc", "cxx", "hpp", "zig", "swift"]
                .map(String::from)
                .to_vec(),
            arguments: json!({
                "program": "${root}/target/debug/${rootName}${exe}",
                "cwd": "${root}",
            }),
            before: "build".into(),
            ..DebugConfig::default()
        },
    );
    debuggers.insert(
        "python".to_owned(),
        DebugConfig {
            command: "python".into(),
            args: vec!["-m".into(), "debugpy.adapter".into()],
            extensions: vec!["py".into()],
            arguments: json!({
                "program": "${file}",
                "cwd": "${root}",
                "console": "internalConsole",
                "justMyCode": true,
            }),
            ..DebugConfig::default()
        },
    );
    debuggers
}

/// Returns the built in debuggers with `overrides` on top, which replace them by name.
pub fn merged_debuggers(
    overrides: &BTreeMap<String, DebugConfig>,
) -> BTreeMap<String, DebugConfig> {
    let mut debuggers = builtin_debuggers();
    debuggers.extend(overrides.clone());
    debuggers.retain(|_, debugger| !debugger.command.is_empty());
    debuggers
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
