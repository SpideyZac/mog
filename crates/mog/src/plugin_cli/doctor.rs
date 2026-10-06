//! `mog plugin doctor`: starts plugins and points out what will not work the way their author
//! expects, like a command in the manifest the plugin does not offer or a provider for files it
//! never starts for.

use std::{
    cmp::Reverse,
    collections::BTreeMap,
    env,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use mog_config::Config;
use mog_core::{KeyChord, Keymap};
use mog_plugin::{
    Activation, Hello, Languages, Manifest, PROTOCOL_VERSION, Plugin, PluginEvent, discover,
    protocol::{EVENTS, OWN_EVENTS},
};
use tokio::{sync::mpsc, time};

use super::source::{Record, Source};
use crate::plugins::{command_name, resolve};

/// How long `doctor` waits for a plugin to say hello.
const WAIT: Duration = Duration::from_secs(15);

/// How long a plugin may take to start before it counts as slow.
const SLOW_START: Duration = Duration::from_secs(2);

/// How bad something the doctor found is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Worth knowing.
    Note,
    /// Likely not what the author meant.
    Warning,
    /// Broken: something will fail.
    Problem,
}

/// Something the doctor found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// How bad it is.
    pub level: Level,
    /// What it is.
    pub text: String,
}

/// Returns a finding at `level` saying `text`.
fn finding(level: Level, text: impl Into<String>) -> Finding {
    Finding {
        level,
        text: text.into(),
    }
}

/// Returns what is off between the plugin `name`'s manifest and what it said when it started.
pub fn check(
    name: &str,
    manifest: Option<&Manifest>,
    hello: &Hello,
    keymap: &Keymap,
) -> Vec<Finding> {
    let mut found = Vec::new();
    if hello.protocol < PROTOCOL_VERSION {
        found.push(finding(
            Level::Note,
            format!(
                "it speaks protocol {}, events, providers and the newer requests need {PROTOCOL_VERSION}",
                hello.protocol
            ),
        ));
    }
    for event in &hello.events {
        if !EVENTS.contains(&event.as_str()) && !OWN_EVENTS.contains(&event.as_str()) {
            found.push(finding(
                Level::Warning,
                format!("it listens for {event}, which mog never sends"),
            ));
        }
    }
    for provider in &hello.unknown_providers {
        found.push(finding(
            Level::Warning,
            format!("it provides {provider}, which mog does not know, so it is never asked"),
        ));
    }
    if hello.events.contains("before_save") {
        found.push(finding(
            Level::Note,
            "it listens for before_save, so every save waits up to 2s for it",
        ));
    }
    for command in &hello.commands {
        for key in &command.keys {
            let full = command_name(name, &command.name);
            match key.parse::<KeyChord>() {
                Err(_) => found.push(finding(
                    Level::Warning,
                    format!("{full} suggests the key {key}, which is not a key mog knows"),
                )),
                Ok(chord) => {
                    if let Some(bound) = keymap.resolve(&chord) {
                        found.push(finding(
                            Level::Note,
                            format!(
                                "{full} suggests {key}, but that runs {bound}, so it is not bound"
                            ),
                        ));
                    }
                }
            }
        }
    }
    let Some(manifest) = manifest else {
        return found;
    };
    if manifest.protocol != hello.protocol {
        found.push(finding(
            Level::Warning,
            format!(
                "the manifest says protocol {} but the plugin speaks {}",
                manifest.protocol, hello.protocol
            ),
        ));
    }
    for command in &manifest.commands {
        if !hello
            .commands
            .iter()
            .any(|other| other.name == command.name)
        {
            found.push(finding(
                Level::Problem,
                format!(
                    "{} is in the manifest but the plugin does not offer it, running it fails",
                    command_name(name, &command.name)
                ),
            ));
        }
    }
    let lazy = !manifest.starts_at_once();
    if lazy {
        for command in &hello.commands {
            if !manifest
                .commands
                .iter()
                .any(|other| other.name == command.name)
            {
                found.push(finding(
                    Level::Warning,
                    format!(
                        "{} only shows up once the plugin runs, list it in the manifest",
                        command_name(name, &command.name)
                    ),
                ));
            }
        }
    }
    let on_command = manifest.activation.contains(&Activation::Command);
    if on_command && manifest.commands.is_empty() {
        found.push(finding(
            Level::Problem,
            "it starts when one of its commands runs, but the manifest lists none, so it never \
             starts",
        ));
    }
    let languages: Vec<&str> = manifest
        .activation
        .iter()
        .filter_map(|activation| match activation {
            Activation::Language(language) => Some(language.as_str()),
            _ => None,
        })
        .collect();
    if lazy {
        for (provider, covered) in &hello.providers {
            let missing = match covered {
                Languages::All => Some("every file".to_owned()),
                Languages::Some(wanted) => {
                    let missing: Vec<&str> = wanted
                        .iter()
                        .map(String::as_str)
                        .filter(|language| !languages.contains(language))
                        .collect();
                    (!missing.is_empty()).then(|| format!("{} files", missing.join(", ")))
                }
            };
            if let Some(missing) = missing {
                found.push(finding(
                    Level::Warning,
                    format!(
                        "it provides {provider} for {missing} but does not start for them, add \
                         `language:` activations or `startup`"
                    ),
                ));
            }
        }
    }
    found
}

/// Returns notes about where the plugin in `folder` was installed from.
fn origin(folder: &Path) -> Vec<Finding> {
    let Some(record) = Record::read(folder) else {
        return Vec::new();
    };
    let mut found = vec![finding(
        Level::Note,
        format!("installed from {}", record.source),
    )];
    if matches!(record.source, Source::Archive { .. }) && record.key.is_none() {
        found.push(finding(
            Level::Warning,
            "it came from an archive that was not signed, so nothing says who wrote it",
        ));
    }
    found
}

/// Starts each plugin in `dir` and the config, or just the one called `only`, and reports what it
/// offers and what looks wrong.
///
/// # Errors
///
/// Returns an error when a plugin failed to start or has problems.
pub async fn run(dir: &Path, only: Option<&str>) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let mut manifests = BTreeMap::new();
    for manifest in discover(dir) {
        match manifest {
            Ok(manifest) => {
                manifests.insert(manifest.name.clone(), manifest);
            }
            Err(err) => println!("broken: {err}"),
        }
    }
    let mut names: Vec<String> = manifests
        .keys()
        .chain(config.plugins.keys())
        .cloned()
        .collect();
    names.sort();
    names.dedup();
    names.retain(|name| only.is_none_or(|only| only == name));
    if names.is_empty() {
        bail!("no plugins to check");
    }
    let mut keymap = Keymap::default();
    for (key, command) in &config.keys {
        if let (Ok(chord), Ok(command)) = (key.parse(), command.parse()) {
            keymap.bind(chord, command);
        }
    }
    let root = env::current_dir().unwrap_or_default();
    let mut failed = 0;
    for name in names {
        let configured = config.plugins.get(&name);
        if configured.is_some_and(|config| !config.enabled) {
            println!("{name}: turned off in the config");
            continue;
        }
        let manifest = manifests.remove(&name);
        let (spec, manifest) = match resolve(&name, configured, manifest) {
            Ok(resolved) => resolved,
            Err(err) => {
                println!("{name}: {err}");
                failed += 1;
                continue;
            }
        };
        println!("{name}: {} {}", spec.command, spec.args.join(" "));
        let mut found = manifest
            .as_ref()
            .map(|manifest| origin(&manifest.dir))
            .unwrap_or_default();
        let (events, mut receiver) = mpsc::channel(64);
        let started = Instant::now();
        let plugin = match Plugin::start(&spec, &root, events) {
            Ok(plugin) => plugin,
            Err(err) => {
                println!("  problem: could not start: {err}");
                failed += 1;
                continue;
            }
        };
        let answer = time::timeout(WAIT, async {
            let mut log = Vec::new();
            while let Some((_, event)) = receiver.recv().await {
                match event {
                    PluginEvent::Ready { hello, .. } => return Ok((hello, log)),
                    PluginEvent::Exited { reason, .. } => {
                        return Err(reason.unwrap_or_else(|| "it exited".into()));
                    }
                    PluginEvent::Log { line, .. } => log.push(line),
                    _ => {}
                }
            }
            Err("it went away".to_owned())
        })
        .await;
        let took = started.elapsed();
        drop(plugin);
        let (hello, log) = match answer {
            Ok(Ok(answer)) => answer,
            Ok(Err(reason)) => {
                println!("  problem: it stopped before saying hello: {reason}");
                failed += 1;
                continue;
            }
            Err(_) => {
                println!(
                    "  problem: it did not answer initialize in {}s",
                    WAIT.as_secs()
                );
                failed += 1;
                continue;
            }
        };
        println!(
            "  ok, protocol {}, started in {}ms",
            hello.protocol,
            took.as_millis()
        );
        for command in &hello.commands {
            println!(
                "  command {} \"{}\"",
                command_name(&name, &command.name),
                command.title
            );
        }
        if !hello.events.is_empty() {
            let events: Vec<&str> = hello.events.iter().map(String::as_str).collect();
            println!("  listens for {}", events.join(", "));
        }
        for (provider, _) in &hello.providers {
            println!("  provides {provider}");
        }
        if took > SLOW_START {
            found.push(finding(
                Level::Warning,
                format!(
                    "it took {:.1}s to start, which mog waits for when it starts lazily",
                    took.as_secs_f32()
                ),
            ));
        }
        found.extend(check(&name, manifest.as_ref(), &hello, &keymap));
        found.extend(
            log.into_iter()
                .map(|line| finding(Level::Note, format!("stderr: {line}"))),
        );
        found.sort_by_key(|finding| Reverse(finding.level));
        for finding in &found {
            let label = match finding.level {
                Level::Problem => "problem",
                Level::Warning => "warning",
                Level::Note => "note",
            };
            println!("  {label}: {}", finding.text);
        }
        if found.iter().any(|finding| finding.level == Level::Problem) {
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} plugin(s) need attention");
    }
    Ok(())
}

#[cfg(test)]
/// Tests for the plugin doctor.
mod tests {
    use std::path::Path;

    use mog_core::Keymap;
    use mog_plugin::{Manifest, protocol::parse_hello};
    use serde_json::json;

    use super::{Level, check};

    /// Mismatches between the manifest and the plugin are found, a matching plugin is fine.
    #[test]
    fn finds_mismatches() {
        let manifest = Manifest::parse(
            "name = \"todo\"\ncommand = \"x\"\nprotocol = 2\nactivation = [\"language:md\"]\n\
             [[commands]]\nname = \"list\"\ntitle = \"List\"\n",
            Path::new("."),
        )
        .expect("manifest");
        let hello = parse_hello(&json!({
            "protocolVersion": 2,
            "commands": [
                { "name": "count", "title": "Count", "keys": ["ctrl+s", "nonsense+key"] }
            ],
            "events": ["opened", "typed"],
            "providers": { "completion": ["md", "rs"], "telepathy": true },
        }))
        .expect("hello");
        let found = check("todo", Some(&manifest), &hello, &Keymap::default());
        let says = |level: Level, text: &str| {
            found
                .iter()
                .any(|finding| finding.level == level && finding.text.contains(text))
        };
        assert!(
            says(Level::Problem, "plugin.todo.list is in the manifest"),
            "{found:#?}"
        );
        assert!(
            says(Level::Warning, "plugin.todo.count only shows up"),
            "{found:#?}"
        );
        assert!(says(Level::Warning, "listens for typed"), "{found:#?}");
        assert!(says(Level::Warning, "provides telepathy"), "{found:#?}");
        assert!(
            says(Level::Warning, "completion for rs files"),
            "{found:#?}"
        );
        assert!(!says(Level::Warning, "completion for md"), "{found:#?}");
        assert!(says(Level::Note, "suggests ctrl+s"), "{found:#?}");
        assert!(says(Level::Warning, "nonsense+key"), "{found:#?}");
        let fine = parse_hello(&json!({
            "protocolVersion": 2,
            "commands": [{ "name": "list", "title": "List" }],
            "events": ["opened"],
            "providers": { "completion": ["md"] },
        }))
        .expect("hello");
        assert!(check("todo", Some(&manifest), &fine, &Keymap::default()).is_empty());
    }

    /// A plugin that starts on a command but lists none never starts.
    #[test]
    fn finds_plugins_that_never_start() {
        let manifest = Manifest::parse(
            "name = \"lazy\"\ncommand = \"x\"\nactivation = [\"command\"]\n",
            Path::new("."),
        )
        .expect("manifest");
        let hello = parse_hello(&json!({ "protocolVersion": 2 })).expect("hello");
        let found = check("lazy", Some(&manifest), &hello, &Keymap::default());
        assert!(
            found
                .iter()
                .any(|finding| finding.level == Level::Problem && finding.text.contains("never")),
            "{found:#?}"
        );
    }
}
