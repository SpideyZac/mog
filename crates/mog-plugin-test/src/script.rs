//! Tests written as TOML: files to open, then steps that each do one thing and say what should
//! happen. `docs/plugins.md` describes the format.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use mog_plugin::{DEFAULT_TIMEOUT, Spec, protocol::OWN_EVENTS};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    host::{FakeHost, Seen},
    matching::{matches, mismatch},
};

/// How long a step waits for what it expects, unless it says otherwise.
const WAIT: Duration = Duration::from_secs(2);

/// How long a step with nothing to wait for lets the plugin work.
const SETTLE: Duration = Duration::from_millis(50);

/// A test of a plugin.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// Settings handed to the plugin in `initialize`, instead of the config's.
    pub settings: Option<Value>,
    /// Files open before the first step, the last one focused.
    #[serde(default)]
    pub files: Vec<ScriptFile>,
    /// What to do, in order.
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// A file open in the fake editor.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptFile {
    /// Where it is, relative to the folder of the script.
    pub path: PathBuf,
    /// Its text, read from disk when left out.
    pub text: Option<String>,
}

/// One thing to do and what should happen.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// What the step checks, for the report.
    pub name: Option<String>,
    /// Opens or focuses this file first, with `text` if given.
    pub open: Option<PathBuf>,
    /// Replaces the text of the focused file first, sending `changed`.
    pub text: Option<String>,
    /// Selects `[anchor, head]` first.
    pub select: Option<[usize; 2]>,
    /// Answers the next `ui/pick` with this index, or closes the list for `false`.
    pub pick: Option<Value>,
    /// Answers the next `ui/prompt` with this text, or closes it for `false`.
    pub prompt: Option<Value>,
    /// Runs this command of the plugin.
    pub command: Option<String>,
    /// The arguments for `command`.
    pub args: Option<Value>,
    /// Sends this event.
    pub event: Option<String>,
    /// Asks for this feature, like `completion`.
    pub provide: Option<String>,
    /// Sends this request as it is.
    pub request: Option<String>,
    /// The params for `event`, `provide` and `request`.
    pub params: Option<Value>,
    /// Sends these keys one after another.
    pub keys: Option<Vec<String>>,
    /// Asks the plugin to tidy the focused file before saving it.
    pub before_save: Option<bool>,
    /// How many milliseconds to wait for what is expected.
    pub wait: Option<u64>,
    /// What should happen.
    #[serde(default)]
    pub expect: Expect,
}

/// What should happen after a step.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// The answer has at least this, see [`matches`].
    pub result: Option<Value>,
    /// It fails with an error containing this.
    pub error: Option<String>,
    /// The status line contains this.
    pub status: Option<String>,
    /// The focused file holds exactly this.
    pub text: Option<String>,
    /// The main selection is `[anchor, head]`.
    pub selection: Option<[usize; 2]>,
    /// The plugin sent these notifications during the step, in order.
    #[serde(default)]
    pub notifications: Vec<Expected>,
    /// The plugin sent these requests during the step, in order.
    #[serde(default)]
    pub requests: Vec<Expected>,
    /// A line the plugin printed or logged during the step contains this.
    pub log: Option<String>,
    /// The plugin asked mog to run these commands during the step.
    #[serde(default)]
    pub commands: Vec<String>,
    /// The plugin saved a file during the step.
    pub saved: Option<bool>,
    /// The output panel text contains this.
    pub output: Option<String>,
}

/// A message the plugin should send.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    /// The method.
    pub method: String,
    /// The params have at least this.
    #[serde(default)]
    pub params: Value,
}

/// How one step went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    /// The step's name, or what it did.
    pub name: String,
    /// What did not happen as expected, empty when it passed.
    pub problems: Vec<String>,
}

impl StepReport {
    /// Returns whether the step went as expected.
    pub fn passed(&self) -> bool {
        self.problems.is_empty()
    }
}

impl Script {
    /// Reads a script from `text`.
    ///
    /// # Errors
    ///
    /// Returns why it is not a valid script.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|err| err.to_string())
    }

    /// Reads the script at `path`.
    ///
    /// # Errors
    ///
    /// Returns why it cannot be read or is not a valid script.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        Self::parse(&text).map_err(|err| format!("{}: {err}", path.display()))
    }
}

impl Step {
    /// Returns what the step does, for the report when it has no name.
    fn describe(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let did = [
            self.command
                .as_ref()
                .map(|command| format!("command {command}")),
            self.event.as_ref().map(|event| format!("event {event}")),
            self.provide
                .as_ref()
                .map(|provider| format!("provide {provider}")),
            self.request
                .as_ref()
                .map(|method| format!("request {method}")),
            self.keys
                .as_ref()
                .map(|keys| format!("keys {}", keys.join(" "))),
            self.before_save.map(|_| "before save".to_owned()),
            self.open
                .as_ref()
                .map(|path| format!("open {}", path.display())),
            self.text.as_ref().map(|_| "type".to_owned()),
        ];
        did.into_iter()
            .flatten()
            .next()
            .unwrap_or_else(|| "wait".to_owned())
    }

    /// Returns why the step is not one thing, if it tries to do several.
    fn check(&self) -> Option<String> {
        let actions = [
            self.command.is_some(),
            self.event.is_some(),
            self.provide.is_some(),
            self.request.is_some(),
            self.keys.is_some(),
            self.before_save == Some(true),
        ];
        (actions.into_iter().filter(|action| *action).count() > 1).then(|| {
            "a step does one of command, event, provide, request, keys and before_save".to_owned()
        })
    }
}

/// Returns the messages in `seen` that `expected` asks for and are missing, in order.
fn missing(seen: &[Seen], expected: &[Expected], what: &str) -> Vec<String> {
    let mut rest = seen.iter();
    let mut problems = Vec::new();
    for wanted in expected {
        let found = rest.by_ref().any(|seen| {
            seen.method == wanted.method
                && (wanted.params.is_null() || matches(&wanted.params, &seen.params))
        });
        if !found {
            let close: Vec<String> = seen
                .iter()
                .filter(|seen| seen.method == wanted.method)
                .filter_map(|seen| mismatch(&wanted.params, &seen.params))
                .collect();
            let hint = if close.is_empty() {
                let methods: Vec<&str> = seen.iter().map(|seen| seen.method.as_str()).collect();
                format!("it sent {}", list_or_nothing(&methods))
            } else {
                close.join("; ")
            };
            problems.push(format!(
                "no {what} {} like {}: {hint}",
                wanted.method, wanted.params
            ));
            break;
        }
    }
    problems
}

/// Returns `items` joined with commas, or `nothing`.
fn list_or_nothing(items: &[&str]) -> String {
    if items.is_empty() {
        "nothing".to_owned()
    } else {
        items.join(", ")
    }
}

/// Where the host's lists were when a step started, to look only at what came after.
#[derive(Debug, Clone, Copy)]
struct Mark {
    /// How many notifications there were.
    notifications: usize,
    /// How many requests there were.
    requests: usize,
    /// How many log lines there were.
    log: usize,
    /// How many commands there were.
    commands: usize,
    /// How many saves there were.
    saved: usize,
}

impl Mark {
    /// Marks where `host` is now.
    fn of(host: &FakeHost) -> Self {
        Self {
            notifications: host.notifications().len(),
            requests: host.requests().len(),
            log: host.log().len(),
            commands: host.commands().len(),
            saved: host.saved().len(),
        }
    }
}

/// Returns what in `expect` did not happen in `host` since `mark`.
fn unmet(host: &FakeHost, expect: &Expect, mark: Mark) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(status) = &expect.status
        && !host
            .status()
            .is_some_and(|now| now.contains(status.as_str()))
    {
        problems.push(format!(
            "the status line is {:?}, not {status:?}",
            host.status().unwrap_or_default()
        ));
    }
    if let Some(text) = &expect.text
        && host.text() != *text
    {
        problems.push(format!("the text is {:?}, not {text:?}", host.text()));
    }
    if let Some([anchor, head]) = expect.selection
        && host.active().selection() != (anchor, head)
    {
        let (now_anchor, now_head) = host.active().selection();
        problems.push(format!(
            "the selection is [{now_anchor}, {now_head}], not [{anchor}, {head}]"
        ));
    }
    problems.extend(missing(
        &host.notifications()[mark.notifications..],
        &expect.notifications,
        "notification",
    ));
    problems.extend(missing(
        &host.requests()[mark.requests..],
        &expect.requests,
        "request",
    ));
    if let Some(log) = &expect.log
        && !host.log()[mark.log..]
            .iter()
            .any(|line| line.contains(log.as_str()))
    {
        problems.push(format!("nothing in the log contains {log:?}"));
    }
    for command in &expect.commands {
        if !host.commands()[mark.commands..]
            .iter()
            .any(|(name, _)| name == command)
        {
            problems.push(format!("it did not ask mog to run {command}"));
        }
    }
    if let Some(saved) = expect.saved
        && (host.saved().len() > mark.saved) != saved
    {
        problems.push(if saved {
            "it saved nothing".to_owned()
        } else {
            "it saved a file".to_owned()
        });
    }
    if let Some(output) = &expect.output
        && !host
            .output()
            .is_some_and(|(_, text)| text.contains(output.as_str()))
    {
        problems.push(format!("the output panel does not contain {output:?}"));
    }
    problems
}

/// Does the setup of `step`, like opening a file or queueing answers.
fn prepare(host: &mut FakeHost, step: &Step) -> Result<(), String> {
    if let Some(pick) = &step.pick {
        let index = pick.as_u64().and_then(|index| usize::try_from(index).ok());
        host.queue_pick(index);
    }
    if let Some(prompt) = &step.prompt {
        host.queue_prompt(prompt.as_str());
    }
    match (&step.open, &step.text) {
        (Some(path), Some(text)) => host.open(path, text),
        (Some(path), None) => {
            if host.focus(path).is_err() {
                let full = host.root().join(path);
                let text = fs::read_to_string(&full)
                    .map_err(|err| format!("could not read {}: {err}", full.display()))?;
                host.open(path, &text);
            }
        }
        (None, Some(text)) => host.set_text(text),
        (None, None) => {}
    }
    if let Some([anchor, head]) = step.select {
        host.select(anchor, head);
    }
    Ok(())
}

/// Does what `step` does and returns the answer, if it asks something.
async fn act(host: &mut FakeHost, step: &Step) -> Option<Result<Value, String>> {
    let params = step.params.clone().unwrap_or_else(|| json!({}));
    if let Some(command) = &step.command {
        let args = step.args.clone().unwrap_or(Value::Null);
        return Some(host.command(command, args).await);
    }
    if let Some(provider) = &step.provide {
        return Some(host.provide(provider, params).await);
    }
    if let Some(method) = &step.request {
        return Some(host.request(method, params, DEFAULT_TIMEOUT).await);
    }
    if let Some(keys) = &step.keys {
        let mut answer = Ok(Value::Null);
        for key in keys {
            answer = host.key(key).await;
            if answer.is_err() {
                break;
            }
        }
        return Some(answer);
    }
    if step.before_save == Some(true) {
        return Some(host.before_save().await);
    }
    if let Some(event) = &step.event {
        if OWN_EVENTS.contains(&event.as_str()) {
            host.force_event(event, params);
        } else if host.hello().events.contains(event) {
            host.send_event(event, params);
        } else {
            return Some(Err(format!(
                "the plugin does not listen for {event}, mog would not send it"
            )));
        }
    }
    None
}

/// Returns what is wrong with `answer` given what `expect` says about it.
fn check_answer(answer: Option<&Result<Value, String>>, expect: &Expect) -> Vec<String> {
    let mut problems = Vec::new();
    match (answer, &expect.error) {
        (Some(Err(err)), Some(wanted)) if !err.contains(wanted.as_str()) => {
            problems.push(format!("it failed with {err:?}, not {wanted:?}"));
        }
        (Some(Err(_)), Some(_)) => {}
        (Some(Err(err)), None) => problems.push(format!("it failed: {err}")),
        (_, Some(wanted)) => problems.push(format!("it should have failed with {wanted:?}")),
        (Some(Ok(result)), None) => {
            if let Some(wanted) = &expect.result
                && let Some(problem) = mismatch(wanted, result)
            {
                problems.push(problem);
            }
        }
        (None, None) => {
            if expect.result.is_some() {
                problems.push("the step asks nothing, so there is no result to check".into());
            }
        }
    }
    problems
}

/// Runs `script` against the plugin `spec` describes, with `root` as the project folder that
/// file paths are relative to.
///
/// Returns how each step went.
///
/// # Errors
///
/// Returns why the plugin could not be started or a file could not be read.
pub async fn run_script(
    spec: &Spec,
    script: &Script,
    root: &Path,
) -> Result<Vec<StepReport>, String> {
    let mut spec = spec.clone();
    if let Some(settings) = &script.settings {
        spec.settings = settings.clone();
    }
    let host = FakeHost::start(&spec, root).await?;
    run_on(host, script).await
}

/// Runs `script` on a plugin already talking to `host`, like one connected over in-memory
/// pipes. File paths are relative to the host's project folder. `settings` in the script are
/// not used, since the plugin already started.
///
/// Returns how each step went.
///
/// # Errors
///
/// Returns why a file could not be read.
pub async fn run_on(mut host: FakeHost, script: &Script) -> Result<Vec<StepReport>, String> {
    let root = host.root().to_owned();
    for file in &script.files {
        let text = match &file.text {
            Some(text) => text.clone(),
            None => {
                let path = root.join(&file.path);
                fs::read_to_string(&path)
                    .map_err(|err| format!("could not read {}: {err}", path.display()))?
            }
        };
        host.open(&file.path, &text);
    }
    let mut reports = Vec::with_capacity(script.steps.len());
    for step in &script.steps {
        let name = step.describe();
        if let Some(problem) = step.check() {
            reports.push(StepReport {
                name,
                problems: vec![problem],
            });
            continue;
        }
        let mark = Mark::of(&host);
        let mut problems = Vec::new();
        if let Err(err) = prepare(&mut host, step) {
            problems.push(err);
        }
        let answer = act(&mut host, step).await;
        problems.extend(check_answer(answer.as_ref(), &step.expect));
        let wait = step.wait.map_or(WAIT, Duration::from_millis);
        let waits = !step.expect.notifications.is_empty()
            || !step.expect.requests.is_empty()
            || step.expect.status.is_some()
            || step.expect.text.is_some()
            || step.expect.log.is_some()
            || step.expect.output.is_some()
            || !step.expect.commands.is_empty()
            || step.expect.saved == Some(true);
        if waits {
            host.wait_for(wait, |host| unmet(host, &step.expect, mark).is_empty())
                .await;
        } else {
            host.settle(step.wait.map_or(SETTLE, Duration::from_millis))
                .await;
        }
        problems.extend(unmet(&host, &step.expect, mark));
        if let Some(reason) = host.exited() {
            problems.push(format!("the plugin stopped: {}", reason.unwrap_or("")));
        }
        reports.push(StepReport { name, problems });
        if host.exited().is_some() {
            break;
        }
    }
    host.shutdown().await;
    Ok(reports)
}
