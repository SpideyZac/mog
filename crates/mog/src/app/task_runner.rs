//! Running build and test tasks and showing their output.

use mog_core::{Severity, parse_problems};
use mog_tui::Overlay;

use super::App;
use crate::tasks::{self, Task, TaskEvent};

impl App {
    /// Returns every task: detected ones, then plugins', then the config's, each winning over
    /// the ones before it with the same name.
    pub(super) fn all_tasks(&self) -> Vec<Task> {
        let mut all = tasks::tasks(&self.ui.root, &self.ui.config.tasks);
        for task in &self.plugin_state.tasks {
            let configured = self
                .ui
                .config
                .tasks
                .get(&task.name)
                .is_some_and(|config| !config.command.trim().is_empty());
            if configured {
                continue;
            }
            all.retain(|other| other.name != task.name);
            all.push(task.clone());
        }
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// Offers the project's tasks in a picker, once plugins said which they have.
    pub(super) fn pick_task(&mut self) {
        if self.ask_plugin_tasks() {
            self.editor.set_status("asking plugins for their tasks");
            return;
        }
        self.show_tasks();
    }

    /// Shows every task in a picker.
    pub(super) fn show_tasks(&mut self) {
        self.task_list = self.all_tasks();
        if self.task_list.is_empty() {
            self.editor.set_status(
                "no tasks found, add [tasks.<name>] command = \"...\" to the config or the project",
            );
            return;
        }
        self.ui.tasks = self
            .task_list
            .iter()
            .map(|task| (task.name.clone(), task.command.clone()))
            .collect();
        self.ui.open(Overlay::Tasks);
    }

    /// Starts `task`, showing its output as it runs.
    pub(super) fn start_task(&mut self, task: Task) {
        // tasks build what is on disk, so unsaved files would be left out
        let unsaved = self
            .editor
            .documents()
            .iter()
            .filter(|document| document.is_modified() && document.path().is_some())
            .count();
        if let Err(err) = self.runner.start(&task) {
            self.editor.set_status(err);
            return;
        }
        self.ui.output.start(&task.name);
        self.ui.task_problems.clear();
        let note = if unsaved > 0 {
            format!(", {unsaved} unsaved files are not part of it")
        } else {
            String::new()
        };
        self.editor.set_status(format!(
            "running {}: {}{note}, Tasks: Show output to watch",
            task.name, task.command
        ));
        self.last_task = Some(task);
    }

    /// Shows a task's output and, once it finishes, the problems it reported.
    pub(super) fn handle_task(&mut self, event: TaskEvent) {
        match event {
            TaskEvent::Line(line) => self.ui.output.push(line),
            TaskEvent::Done(code) => {
                for line in self.runner.drain() {
                    self.ui.output.push(line);
                }
                self.ui.output.running = false;
                let Some(task) = self.last_task.clone() else {
                    return;
                };
                let output = self.ui.output.lines.join("\n");
                self.ui.task_problems = parse_problems(&output, &task.cwd, |path| path.is_file());
                let count = |severity| {
                    self.ui
                        .task_problems
                        .iter()
                        .filter(|problem| problem.severity == severity)
                        .count()
                };
                let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
                self.plugin_task_finished(&task.name, code);
                let outcome = match code {
                    Some(0) => "passed".to_owned(),
                    Some(code) => format!("failed with code {code}"),
                    None => "was stopped".to_owned(),
                };
                let found = match (errors, warnings) {
                    (0, 0) => String::new(),
                    (errors, warnings) => {
                        format!(", {errors} errors and {warnings} warnings, alt+m lists them")
                    }
                };
                self.editor
                    .set_status(format!("{} {outcome}{found}", task.name));
                if let Some((name, config)) = self.debug_after_task.take() {
                    if code == Some(0) {
                        self.launch_debugger(&name, &config);
                    } else {
                        self.editor
                            .set_status(format!("{} {outcome}, not debugging", task.name));
                    }
                }
            }
        }
    }
}
