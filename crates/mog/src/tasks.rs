//! Finding a project's tasks, like build and test, and running them in the background.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use mog_config::TaskConfig;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time,
};

/// How long to wait for the last output after a task exits, in case something it started still
/// holds its output open.
const OUTPUT_GRACE: Duration = Duration::from_millis(500);

/// Something a running task did.
#[derive(Debug)]
pub enum TaskEvent {
    /// It printed a line.
    Line(String),
    /// It finished, with its exit code if it had one.
    Done(Option<i32>),
}

/// A task ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// The name, like `build`.
    pub name: String,
    /// The command line, run through the shell.
    pub command: String,
    /// The folder it runs in.
    pub cwd: PathBuf,
}

/// Returns the tasks mog finds in the project at `root` from files like `Cargo.toml`.
fn detected(root: &Path) -> BTreeMap<String, String> {
    let mut tasks = BTreeMap::new();
    let mut add = |name: &str, command: &str| {
        tasks
            .entry(name.to_owned())
            .or_insert_with(|| command.to_owned());
    };
    if root.join("Cargo.toml").is_file() {
        add("build", "cargo build");
        add("check", "cargo clippy --all-targets");
        add("test", "cargo test");
        add("run", "cargo run");
    }
    if root.join("go.mod").is_file() {
        add("build", "go build ./...");
        add("test", "go test ./...");
        add("check", "go vet ./...");
    }
    if let Ok(text) = fs::read_to_string(root.join("package.json"))
        && let Ok(package) = serde_json::from_str::<Value>(&text)
        && let Some(scripts) = package["scripts"].as_object()
    {
        let runner = if root.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if root.join("yarn.lock").is_file() {
            "yarn"
        } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
            "bun"
        } else {
            "npm"
        };
        for script in scripts.keys() {
            add(script, &format!("{runner} run {script}"));
        }
    }
    if root.join("Makefile").is_file() || root.join("makefile").is_file() {
        add("build", "make");
        add("test", "make test");
    }
    if root.join("pyproject.toml").is_file() || root.join("pytest.ini").is_file() {
        add("test", "python -m pytest");
    }
    tasks
}

/// Returns every task of the project at `root`, the configured ones winning over detected ones.
pub fn tasks(root: &Path, configured: &BTreeMap<String, TaskConfig>) -> Vec<Task> {
    let mut tasks: BTreeMap<String, Task> = detected(root)
        .into_iter()
        .map(|(name, command)| {
            let task = Task {
                name: name.clone(),
                command,
                cwd: root.to_owned(),
            };
            (name, task)
        })
        .collect();
    for (name, config) in configured {
        if config.command.trim().is_empty() {
            continue;
        }
        tasks.insert(
            name.clone(),
            Task {
                name: name.clone(),
                command: config.command.clone(),
                cwd: root.join(&config.cwd),
            },
        );
    }
    tasks.into_values().collect()
}

/// Returns the shell command that runs `line`.
fn shell(line: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", line]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", line]);
        command
    }
}

/// Sends every line `reader` prints to `sender`.
async fn forward(reader: impl AsyncRead + Unpin, sender: UnboundedSender<TaskEvent>) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if sender.send(TaskEvent::Line(line)).is_err() {
            break;
        }
    }
}

/// The task that is running, if any, and its output.
pub struct Runner {
    /// The running process, to stop it.
    child: Option<Child>,
    /// What reads the output of the running process, done once all of it is sent.
    readers: Option<JoinHandle<()>>,
    /// Where output goes.
    sender: UnboundedSender<TaskEvent>,
    /// Output waiting to be shown.
    events: UnboundedReceiver<TaskEvent>,
}

impl Runner {
    /// Creates a runner with nothing running.
    pub fn new() -> Self {
        let (sender, events) = mpsc::unbounded_channel();
        Self {
            child: None,
            readers: None,
            sender,
            events,
        }
    }

    /// Returns whether a task is running.
    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    /// Starts `task`, stopping any task that is still running.
    ///
    /// # Errors
    ///
    /// Returns why the shell could not be started.
    pub fn start(&mut self, task: &Task) -> Result<(), String> {
        self.stop();
        // a fresh channel so lines from the stopped task cannot mix in
        let (sender, events) = mpsc::unbounded_channel();
        self.sender = sender;
        self.events = events;
        let mut child = shell(&task.command)
            .current_dir(&task.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // compilers color their output for terminals, which the output panel cannot show
            .env("NO_COLOR", "1")
            .env("CARGO_TERM_COLOR", "never")
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| format!("could not start `{}`: {err}", task.command))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let sender = self.sender.clone();
        self.readers = Some(tokio::spawn(async move {
            let out = stdout.map(|out| tokio::spawn(forward(out, sender.clone())));
            let err = stderr.map(|err| tokio::spawn(forward(err, sender.clone())));
            for reader in [out, err].into_iter().flatten() {
                let _ = reader.await;
            }
        }));
        self.child = Some(child);
        Ok(())
    }

    /// Stops the running task, if any.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }

    /// Waits for the next line or for the task to finish.
    pub async fn event(&mut self) -> Option<TaskEvent> {
        let child = self.child.as_mut()?;
        tokio::select! {
            biased;
            Some(event) = self.events.recv() => Some(event),
            status = child.wait() => {
                // lines can still be on their way after the process is gone, so let them land
                // before saying it is done
                if let Some(readers) = self.readers.take() {
                    let _ = time::timeout(OUTPUT_GRACE, readers).await;
                }
                let code = status.ok().and_then(|status| status.code());
                self.child = None;
                Some(TaskEvent::Done(code))
            }
        }
    }

    /// Takes lines that arrived after the task finished.
    pub fn drain(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            if let TaskEvent::Line(line) = event {
                lines.push(line);
            }
        }
        lines
    }
}

#[cfg(test)]
/// Tests for tasks.
mod tests {
    use std::{collections::BTreeMap, env, fs, process, time::Duration};

    use mog_config::TaskConfig;
    use tokio::time;

    use super::{Runner, Task, TaskEvent, tasks};

    /// Cargo projects get build and test tasks, and configured tasks win.
    #[test]
    fn detects_and_merges_tasks() {
        let root = env::temp_dir().join(format!("mog-tasks-{}", process::id()));
        fs::create_dir_all(&root).expect("dir");
        fs::write(root.join("Cargo.toml"), "[package]\n").expect("write");
        fs::write(
            root.join("package.json"),
            r#"{ "scripts": { "lint": "eslint ." } }"#,
        )
        .expect("write");
        let mut configured = BTreeMap::new();
        configured.insert(
            "build".to_owned(),
            TaskConfig {
                command: "make all".into(),
                cwd: String::new(),
            },
        );
        let found = tasks(&root, &configured);
        let names: Vec<(&str, &str)> = found
            .iter()
            .map(|task| (task.name.as_str(), task.command.as_str()))
            .collect();
        assert!(names.contains(&("build", "make all")));
        assert!(names.contains(&("test", "cargo test")));
        assert!(names.contains(&("lint", "npm run lint")));
        let _ = fs::remove_dir_all(root);
    }

    /// A task's output comes back line by line, then its exit code.
    #[tokio::test]
    async fn runs_a_task() {
        let mut runner = Runner::new();
        let task = Task {
            name: "echo".into(),
            command: "echo hello".into(),
            cwd: env::temp_dir(),
        };
        runner.start(&task).expect("started");
        let mut lines = Vec::new();
        let code = loop {
            match time::timeout(Duration::from_secs(10), runner.event()).await {
                Ok(Some(TaskEvent::Line(line))) => lines.push(line),
                Ok(Some(TaskEvent::Done(code))) => break code,
                other => panic!("unexpected {other:?}"),
            }
        };
        lines.extend(runner.drain());
        assert_eq!(code, Some(0));
        assert!(lines.iter().any(|line| line.trim() == "hello"), "{lines:?}");
        assert!(!runner.is_running());
    }
}
