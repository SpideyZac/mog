//! Running a debugging session: starting the adapter, breakpoints, stepping and reading state.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use mog_config::DebugConfig;
use mog_dap::{DapEvent, DebugClient, Frame, client::program_path};
use mog_tui::debug_panel::VariableEntry;
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// How many structured variables get their children shown.
const EXPANDED: usize = 8;

/// How many children of one variable are shown.
const CHILDREN: usize = 12;

/// An answer to a request the debugger sent in the background.
#[derive(Debug)]
pub enum DebugReply {
    /// The call stack of the stopped thread, innermost first.
    Stack(Vec<Frame>),
    /// The variables of a frame, with scope headings.
    Variables(Vec<VariableEntry>),
    /// Something failed, with a message for the status line.
    Failed(String),
}

/// Something a debugging session reported.
#[derive(Debug)]
pub enum DebugUpdate {
    /// The adapter reported an event.
    Event(DapEvent),
    /// A background request finished.
    Reply(DebugReply),
}

/// Returns the debugger in `debuggers` for `path`, by file extension, or the only one there is.
pub fn pick(
    debuggers: &BTreeMap<String, DebugConfig>,
    path: Option<&Path>,
) -> Option<(String, DebugConfig)> {
    let extension = path
        .and_then(Path::extension)
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    debuggers
        .iter()
        .find(|(_, config)| config.extensions.iter().any(|ext| ext == extension))
        .or_else(|| {
            (debuggers.len() == 1)
                .then(|| debuggers.iter().next())
                .flatten()
        })
        .map(|(name, config)| (name.clone(), config.clone()))
}

/// Replaces `${name}` in every string of `value` with the matching value from `vars`.
pub fn substitute(value: &Value, vars: &[(&str, String)]) -> Value {
    match value {
        Value::String(text) => {
            let mut text = text.clone();
            for (name, replacement) in vars {
                text = text.replace(&format!("${{{name}}}"), replacement);
            }
            Value::String(text)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| substitute(item, vars)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), substitute(item, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Returns the variables that `${...}` in debugger arguments can use.
pub fn variables(root: &Path, file: Option<&Path>) -> Vec<(&'static str, String)> {
    let text = |path: &Path| path.to_string_lossy().into_owned();
    vec![
        ("root", text(root)),
        (
            "rootName",
            root.file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        ),
        ("file", file.map(text).unwrap_or_default()),
        (
            "fileDirname",
            file.and_then(Path::parent).map(text).unwrap_or_default(),
        ),
        (
            "fileBasenameNoExtension",
            file.and_then(Path::file_stem)
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned()),
        ),
        ("exe", if cfg!(windows) { ".exe" } else { "" }.to_owned()),
    ]
}

/// Returns the Python DLL a broken LLVM lldb next to `adapter` asks for, like `python311.dll`.
///
/// LLVM's Windows builds of lldb link one exact Python version and fail to start without it,
/// saying which one only when lldb itself is run.
fn missing_python_dll(adapter: &Path) -> Option<String> {
    let lldb = adapter.with_file_name("lldb.exe");
    if !lldb.is_file() {
        return None;
    }
    let output = Command::new(&lldb)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let start = stderr.find("unable to find '")? + "unable to find '".len();
    let name = stderr[start..].split('\'').next()?;
    (name.starts_with("python") && name.ends_with(".dll")).then(|| name.to_owned())
}

/// Returns the folders a Windows Python install may keep its DLL in.
fn python_folders() -> Vec<PathBuf> {
    // folders that hold one install per version, like `Python311`
    let mut parents = Vec::new();
    let mut folders = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        parents.push(local.join("Programs").join("Python"));
        parents.push(local.join("Python"));
    }
    if let Some(roaming) = env::var_os("APPDATA").map(PathBuf::from) {
        parents.push(roaming.join("uv").join("python"));
    }
    if let Some(home) = env::var_os("USERPROFILE").map(PathBuf::from) {
        parents.push(home.join(".pyenv").join("pyenv-win").join("versions"));
        folders.push(home.join(".platformio").join("python3"));
        folders.push(
            home.join("scoop")
                .join("apps")
                .join("python")
                .join("current"),
        );
        folders.push(home.join("miniconda3"));
        folders.push(home.join("anaconda3"));
    }
    parents.push(PathBuf::from("C:\\"));
    parents.push(PathBuf::from("C:\\Program Files"));
    for parent in parents {
        if let Ok(entries) = fs::read_dir(&parent) {
            folders.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    folders
}

/// Returns extra environment that lets `command` start, for adapters known to need help.
///
/// Only LLVM's `lldb-dap` on Windows needs any: it is pointed at a Python install that has the
/// DLL it was built against. Returns why it cannot start when no such install is found.
pub fn adapter_env(command: &str) -> Result<Vec<(OsString, OsString)>, String> {
    let adapter = program_path(command);
    let is_lldb = adapter
        .file_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case("lldb-dap"));
    if !cfg!(windows) || !is_lldb {
        return Ok(Vec::new());
    }
    let Some(dll) = missing_python_dll(&adapter) else {
        return Ok(Vec::new());
    };
    let Some(folder) = python_folders()
        .into_iter()
        .find(|folder| folder.join(&dll).is_file())
    else {
        // python311.dll is Python 3.11
        let digits = dll.trim_start_matches("python").trim_end_matches(".dll");
        let version = match digits.split_at_checked(1) {
            Some((major, minor)) => format!("{major}.{minor}"),
            None => digits.to_owned(),
        };
        return Err(format!(
            "lldb-dap needs {dll}, install Python {version} (uv python install {version} works)"
        ));
    };
    let mut path = OsString::from(folder.as_os_str());
    if let Some(current) = env::var_os("PATH") {
        path.push(";");
        path.push(current);
    }
    Ok(vec![
        ("PATH".into(), path),
        ("PYTHONHOME".into(), folder.into_os_string()),
    ])
}

/// One debugging session at a time and what it reports.
pub struct Debugger {
    /// The adapter, while a session runs.
    client: Option<DebugClient>,
    /// The thread that stopped last.
    thread: Option<i64>,
    /// What the adapter reports.
    events: UnboundedReceiver<DapEvent>,
    /// Where background requests send their answers.
    replies_tx: UnboundedSender<DebugReply>,
    /// Answers to background requests.
    replies: UnboundedReceiver<DebugReply>,
}

impl Debugger {
    /// Creates a debugger with no session.
    pub fn new() -> Self {
        let (_, events) = mpsc::unbounded_channel();
        let (replies_tx, replies) = mpsc::unbounded_channel();
        Self {
            client: None,
            thread: None,
            events,
            replies_tx,
            replies,
        }
    }

    /// Returns whether a session is running.
    pub fn is_active(&self) -> bool {
        self.client.is_some()
    }

    /// Starts a session with the debugger called `name`, set up by `config`, launching or
    /// attaching with `arguments`.
    ///
    /// # Errors
    ///
    /// Returns why the adapter could not be started.
    pub fn start(
        &mut self,
        name: &str,
        config: &DebugConfig,
        arguments: Value,
        root: &Path,
    ) -> Result<(), String> {
        self.stop();
        // a fresh channel so a session that is shutting down cannot talk over this one
        let (events_tx, events) = mpsc::unbounded_channel();
        self.events = events;
        let env = adapter_env(&config.command)?;
        let client = DebugClient::start(&config.command, &config.args, root, &env, events_tx)
            .map_err(|err| format!("could not start {}: {err}", config.command))?;
        let replies = self.replies_tx.clone();
        let request = if config.request.is_empty() {
            "launch".to_owned()
        } else {
            config.request.clone()
        };
        let adapter = name.to_owned();
        let starter = client.clone();
        tokio::spawn(async move {
            if let Err(err) = starter.initialize(&adapter).await {
                let _ = replies.send(DebugReply::Failed(format!("{adapter} refused: {err}")));
                return;
            }
            // some adapters only answer this after configuration is done, so it runs alone
            if let Err(err) = starter.request(&request, arguments).await {
                let _ = replies.send(DebugReply::Failed(format!("could not {request}: {err}")));
            }
        });
        self.client = Some(client);
        self.thread = None;
        Ok(())
    }

    /// Sends every breakpoint and says configuration is done, once the adapter is ready.
    pub fn configure(&self, breakpoints: BTreeMap<PathBuf, BTreeSet<usize>>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let replies = self.replies_tx.clone();
        tokio::spawn(async move {
            for (path, lines) in breakpoints {
                let lines: Vec<usize> = lines.into_iter().collect();
                if let Err(err) = client.set_breakpoints(&path, &lines).await {
                    let _ = replies.send(DebugReply::Failed(format!("breakpoints: {err}")));
                }
            }
            let _ = client
                .request("setExceptionBreakpoints", json!({ "filters": [] }))
                .await;
            if let Err(err) = client.request("configurationDone", json!({})).await {
                let _ = replies.send(DebugReply::Failed(format!("could not start: {err}")));
            }
        });
    }

    /// Tells the adapter the breakpoints of `path` are now `lines`.
    pub fn set_breakpoints(&self, path: PathBuf, lines: Vec<usize>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let replies = self.replies_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = client.set_breakpoints(&path, &lines).await {
                let _ = replies.send(DebugReply::Failed(format!("breakpoints: {err}")));
            }
        });
    }

    /// Reads the call stack after the program stopped in `thread`.
    pub fn stopped(&mut self, thread: Option<i64>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if thread.is_some() {
            self.thread = thread;
        }
        let known = self.thread;
        let replies = self.replies_tx.clone();
        tokio::spawn(async move {
            let thread = match known {
                Some(thread) => thread,
                None => match client.threads().await {
                    Ok(threads) if !threads.is_empty() => threads[0].0,
                    _ => return,
                },
            };
            let reply = match client.stack_trace(thread).await {
                Ok(frames) => DebugReply::Stack(frames),
                Err(err) => DebugReply::Failed(format!("call stack: {err}")),
            };
            let _ = replies.send(reply);
        });
    }

    /// Reads the variables of the frame with `frame` id.
    pub fn load_variables(&self, frame: i64) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let replies = self.replies_tx.clone();
        tokio::spawn(async move {
            let scopes = match client.scopes(frame).await {
                Ok(scopes) => scopes,
                Err(err) => {
                    let _ = replies.send(DebugReply::Failed(format!("variables: {err}")));
                    return;
                }
            };
            let mut entries = Vec::new();
            let mut expanded = 0;
            for scope in scopes.iter().filter(|scope| !scope.expensive) {
                entries.push(VariableEntry {
                    name: scope.name.clone(),
                    value: String::new(),
                    depth: 0,
                });
                for variable in client.variables(scope.reference).await.unwrap_or_default() {
                    entries.push(VariableEntry {
                        name: variable.name.clone(),
                        value: variable.value.clone(),
                        depth: 1,
                    });
                    if variable.reference > 0 && expanded < EXPANDED {
                        expanded += 1;
                        let children = client.variables(variable.reference).await;
                        for child in children.unwrap_or_default().into_iter().take(CHILDREN) {
                            entries.push(VariableEntry {
                                name: child.name,
                                value: child.value,
                                depth: 2,
                            });
                        }
                    }
                }
            }
            let _ = replies.send(DebugReply::Variables(entries));
        });
    }

    /// Sends a stepping `command`, like `continue`, `next`, `stepIn`, `stepOut` or `pause`.
    pub fn step(&self, command: &str) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let thread = self.thread.unwrap_or(1);
        let replies = self.replies_tx.clone();
        let command = command.to_owned();
        tokio::spawn(async move {
            if let Err(err) = client
                .request(&command, json!({ "threadId": thread }))
                .await
            {
                let _ = replies.send(DebugReply::Failed(format!("{command}: {err}")));
            }
        });
    }

    /// Ends the session, stopping the program.
    pub fn stop(&mut self) {
        // dropping the last client makes the connection disconnect and stop the adapter
        self.client = None;
        self.thread = None;
    }

    /// Waits for the next adapter event or answer to a background request.
    pub async fn update(&mut self) -> Option<DebugUpdate> {
        tokio::select! {
            Some(event) = self.events.recv() => Some(DebugUpdate::Event(event)),
            Some(reply) = self.replies.recv() => Some(DebugUpdate::Reply(reply)),
            else => None,
        }
    }
}

#[cfg(test)]
/// Tests for debugging helpers.
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use mog_config::DebugConfig;
    use serde_json::json;

    use super::{pick, substitute, variables};

    /// Debuggers are picked by the extension of the file.
    #[test]
    fn picks_by_extension() {
        let mut debuggers = BTreeMap::new();
        for (name, ext) in [("lldb", "rs"), ("python", "py")] {
            debuggers.insert(
                name.to_owned(),
                DebugConfig {
                    command: name.into(),
                    extensions: vec![ext.into()],
                    ..DebugConfig::default()
                },
            );
        }
        let found = pick(&debuggers, Some(Path::new("/p/app.py"))).expect("python");
        assert_eq!(found.0, "python");
        assert!(pick(&debuggers, Some(Path::new("/p/notes.md"))).is_none());
    }

    /// Variables are filled in through nested arguments.
    #[test]
    fn substitutes_variables() {
        let vars = variables(
            Path::new("/code/app"),
            Some(Path::new("/code/app/src/main.py")),
        );
        let arguments = json!({ "program": "${root}/target/debug/${rootName}${exe}",
            "args": ["${file}"], "stopOnEntry": false });
        let filled = substitute(&arguments, &vars);
        let exe = if cfg!(windows) { ".exe" } else { "" };
        assert_eq!(
            filled["program"],
            json!(format!("/code/app/target/debug/app{exe}"))
        );
        assert_eq!(filled["args"][0], json!("/code/app/src/main.py"));
        assert_eq!(filled["stopOnEntry"], json!(false));
    }
}
