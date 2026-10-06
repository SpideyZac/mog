//! `mog plugin`: creating, installing, listing, removing and checking plugins from the shell.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, IsTerminal as _, Write as _},
    path::{self, Path, PathBuf},
    process::{self, Command},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use mog_config::Config;
use mog_plugin::{MANIFEST_FILE, Manifest, Plugin, PluginEvent, Spec, discover};
use mog_plugin_test::{Script, run_script};
use tokio::{sync::mpsc, time};

use crate::{
    cli::{Language, PluginAction},
    plugins::{plugin_dir, resolve},
};

/// What `mog plugin install` says before it installs anything.
const UNSANDBOXED: &str = "\
WARNING: plugins are not sandboxed. A plugin runs as you, and mog does not limit what it does.
It can read, change and delete any file you can, read every environment variable (API keys
and tokens included), see everything you open and type, use the network and run programs.
Only install plugins you have read or whose authors you trust.";

/// How long `doctor` waits for a plugin to say hello.
const DOCTOR_WAIT: Duration = Duration::from_secs(15);

/// Runs a `mog plugin` action.
///
/// # Errors
///
/// Returns an error describing what went wrong, for the shell.
pub async fn run(action: PluginAction) -> Result<()> {
    let dir = plugin_dir().ok_or_else(|| anyhow!("this system has no config folder"))?;
    match action {
        PluginAction::List => list(&dir),
        PluginAction::New { name, language } => new(&dir, &name, language),
        PluginAction::Install { source, yes } => {
            if !yes {
                confirm_install(&source)?;
            }
            install(&dir, &source)
        }
        PluginAction::Remove { name } => remove(&dir, &name),
        PluginAction::Doctor { name } => doctor(&dir, name.as_deref()).await,
        PluginAction::Test { plugin, scripts } => test(&dir, &plugin, scripts).await,
    }
}

/// Checks `name` can name a plugin.
fn check_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if !valid {
        bail!("`{name}` is not a valid plugin name, use letters, digits, - and _");
    }
    Ok(())
}

/// Prints every plugin mog would run.
fn list(dir: &Path) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    println!("plugins folder: {}", dir.display());
    let mut seen = BTreeMap::new();
    for manifest in discover(dir) {
        match manifest {
            Ok(manifest) => {
                seen.insert(manifest.name.clone(), manifest);
            }
            Err(err) => println!("  broken: {err}"),
        }
    }
    let names: Vec<String> = seen
        .keys()
        .chain(config.plugins.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if names.is_empty() {
        println!("no plugins yet, try `mog plugin new hello`");
    }
    for name in names {
        let configured = config.plugins.get(&name);
        let enabled = configured.is_none_or(|config| config.enabled);
        let manifest = seen.get(&name);
        let about = manifest.map_or_else(String::new, |manifest| {
            format!("{} {}", manifest.version, manifest.description)
        });
        let state = if enabled { "on " } else { "off" };
        let source = match (manifest, configured) {
            (Some(_), _) => "folder",
            (None, _) => "config",
        };
        println!("  [{state}] {name} ({source}) {about}");
    }
    Ok(())
}

/// Writes `contents` to `path`, refusing to overwrite.
fn create(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    fs::write(path, contents).with_context(|| format!("could not write {}", path.display()))
}

/// Creates a plugin called `name` in `language` from a template.
fn new(dir: &Path, name: &str, language: Language) -> Result<()> {
    check_name(name)?;
    let folder = dir.join(name);
    if folder.exists() {
        bail!("{} already exists", folder.display());
    }
    fs::create_dir_all(&folder)
        .with_context(|| format!("could not create {}", folder.display()))?;
    // python is python3 on most systems but just python on windows
    let (program, windows, main, source) = match language {
        Language::Python => (
            "python3",
            "\ncommand_windows = \"python\"",
            "main.py",
            python_template(name),
        ),
        Language::Node => ("node", "", "main.js", node_template(name)),
    };
    let manifest = format!(
        "name = \"{name}\"\nversion = \"0.1.0\"\ndescription = \"Says hello.\"\nprotocol = 2\n\
         command = \"{program}\"{windows}\nargs = [\"${{plugin_dir}}/{main}\"]\n\
         activation = [\"command\"]\n\n[[commands]]\nname = \"hello\"\n\
         title = \"{name}: Say hello\"\n"
    );
    create(&folder.join(MANIFEST_FILE), &manifest)?;
    create(&folder.join(main), &source)?;
    println!("created {}", folder.display());
    println!("restart mog or run plugins.restart, then run `{name}: Say hello` from the palette");
    Ok(())
}

/// Returns the main file of a new Python plugin.
fn python_template(name: &str) -> String {
    format!(
        r#""""The {name} plugin for mog. See docs/plugins.md in the mog repository."""

import os

# mog puts the sdk that ships with it on the import path
from mog_plugin import Plugin, status

plugin = Plugin()


@plugin.command("hello", title="{name}: Say hello")
def hello(context, args):
    name = os.path.basename(context.get("path") or "this file")
    return [status(f"hello from {name}, you are looking at {{name}}")]


if __name__ == "__main__":
    plugin.run()
"#
    )
}

/// Returns the main file of a new Node plugin.
fn node_template(name: &str) -> String {
    format!(
        r#"// The {name} plugin for mog. See docs/plugins.md in the mog repository.

const path = require("node:path");
// mog puts the sdk that ships with it on NODE_PATH
const {{ Plugin, status }} = require("mog-plugin");

const plugin = new Plugin();

plugin.command("hello", {{ title: "{name}: Say hello" }}, (context) => {{
  const file = context.path ? path.basename(context.path) : "this file";
  return [status(`hello from {name}, you are looking at ${{file}}`)];
}});

plugin.run();
"#
    )
}

/// Copies the folder `from` into `to`, skipping a `.git` folder.
fn copy_folder(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            if entry.file_name() != ".git" {
                copy_folder(&entry.path(), &target)?;
            }
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Returns whether `source` looks like a git url rather than a folder.
fn is_git_url(source: &str) -> bool {
    source.starts_with("https://")
        || source.starts_with("http://")
        || source.starts_with("git@")
        || source.starts_with("ssh://")
        || source.ends_with(".git")
}

/// Warns that plugins are not sandboxed and asks whether to install `source`.
///
/// # Errors
///
/// Returns an error if the answer is not yes, or there is nobody to ask.
fn confirm_install(source: &str) -> Result<()> {
    eprintln!("{UNSANDBOXED}\n");
    if !io::stdin().is_terminal() {
        bail!("pass --yes to install {source} without asking");
    }
    eprint!("install {source}? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        bail!("not installed");
    }
    Ok(())
}

/// Installs the plugin at `source` into the plugins folder.
fn install(dir: &Path, source: &str) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let staging = dir.join(format!(".installing-{}", process::id()));
    let _ = fs::remove_dir_all(&staging);
    let fetched = if is_git_url(source) {
        let status = Command::new("git")
            .args(["clone", "--depth", "1", source])
            .arg(&staging)
            .status()
            .context("could not run git, is it installed?")?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("git clone failed"))
        }
    } else {
        let from = PathBuf::from(source);
        if !from.join(MANIFEST_FILE).is_file() {
            bail!("{} has no {MANIFEST_FILE}", from.display());
        }
        copy_folder(&from, &staging).with_context(|| format!("could not copy {source}"))
    };
    let installed = fetched.and_then(|()| {
        let manifest = Manifest::read(&staging).map_err(|err| anyhow!(err))?;
        check_name(&manifest.name)?;
        let target = dir.join(&manifest.name);
        if target.exists() {
            bail!(
                "{} is already installed, remove it first with `mog plugin remove {}`",
                manifest.name,
                manifest.name
            );
        }
        fs::rename(&staging, &target)
            .with_context(|| format!("could not move the plugin to {}", target.display()))?;
        Ok(manifest)
    });
    let _ = fs::remove_dir_all(&staging);
    let manifest = installed?;
    println!(
        "installed {} {} into {}",
        manifest.name,
        manifest.version,
        dir.join(&manifest.name).display()
    );
    println!("it runs as you with no sandbox, so remove it if you stop trusting it");
    Ok(())
}

/// Deletes the installed plugin called `name`.
fn remove(dir: &Path, name: &str) -> Result<()> {
    check_name(name)?;
    let folder = dir.join(name);
    if !folder.join(MANIFEST_FILE).is_file() {
        bail!("no plugin called {name} in {}", dir.display());
    }
    fs::remove_dir_all(&folder)
        .with_context(|| format!("could not delete {}", folder.display()))?;
    println!("removed {name}");
    Ok(())
}

/// Works out how to run `plugin`, a plugin name or a folder with a manifest, and returns that
/// with the plugin folder if it has one.
fn find_plugin(dir: &Path, plugin: &str) -> Result<(Spec, Option<PathBuf>)> {
    let config = Config::load().unwrap_or_default();
    let folder = Path::new(plugin);
    let manifest = if folder.join(MANIFEST_FILE).is_file() {
        // the plugin runs in another folder, so a relative one would point nowhere
        let folder = path::absolute(folder).unwrap_or_else(|_| folder.to_owned());
        Some(Manifest::read(&folder).map_err(|err| anyhow!(err))?)
    } else {
        discover(dir)
            .into_iter()
            .filter_map(Result::ok)
            .find(|manifest| manifest.name == plugin)
    };
    let name = manifest
        .as_ref()
        .map_or_else(|| plugin.to_owned(), |manifest| manifest.name.clone());
    if manifest.is_none() && !config.plugins.contains_key(&name) {
        bail!("no plugin called {plugin}, and no folder there with a {MANIFEST_FILE}");
    }
    let folder = manifest.as_ref().map(|manifest| manifest.dir.clone());
    let (spec, _) =
        resolve(&name, config.plugins.get(&name), manifest).map_err(|err| anyhow!(err))?;
    Ok((spec, folder))
}

/// Returns the `.toml` files in `folder`, sorted.
fn scripts_in(folder: &Path) -> Vec<PathBuf> {
    let mut scripts: Vec<PathBuf> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    scripts.sort();
    scripts
}

/// Runs `plugin` against a fake editor with each of `scripts`, or the ones in its `tests`
/// folder, and prints how every step went.
async fn test(dir: &Path, plugin: &str, scripts: Vec<PathBuf>) -> Result<()> {
    let (spec, folder) = find_plugin(dir, plugin)?;
    let scripts = if scripts.is_empty() {
        let tests = folder
            .ok_or_else(|| anyhow!("{plugin} has no folder, name the scripts to run"))?
            .join("tests");
        scripts_in(&tests)
    } else {
        scripts
    };
    if scripts.is_empty() {
        bail!("no test scripts, put some in the tests folder of the plugin");
    }
    let (mut passed, mut failed) = (0, 0);
    for path in scripts {
        println!("{}", path.display());
        let script = Script::read(&path).map_err(|err| anyhow!(err))?;
        let root = path.parent().unwrap_or(Path::new(".")).to_owned();
        let reports = match run_script(&spec, &script, &root).await {
            Ok(reports) => reports,
            Err(err) => {
                println!("  FAIL could not start: {err}");
                failed += 1;
                continue;
            }
        };
        for report in reports {
            if report.passed() {
                passed += 1;
                println!("  ok   {}", report.name);
            } else {
                failed += 1;
                println!("  FAIL {}", report.name);
                for problem in &report.problems {
                    println!("       {problem}");
                }
            }
        }
    }
    println!("{passed} passed, {failed} failed");
    if failed > 0 {
        bail!("{failed} step(s) failed");
    }
    Ok(())
}

/// Starts each plugin, or just the one called `only`, and reports what it says.
async fn doctor(dir: &Path, only: Option<&str>) -> Result<()> {
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
    let root = env::current_dir().unwrap_or_default();
    let mut failed = 0;
    for name in names {
        let configured = config.plugins.get(&name);
        if configured.is_some_and(|config| !config.enabled) {
            println!("{name}: turned off in the config");
            continue;
        }
        let spec = match resolve(&name, configured, manifests.remove(&name)) {
            Ok((spec, _)) => spec,
            Err(err) => {
                println!("{name}: {err}");
                failed += 1;
                continue;
            }
        };
        println!("{name}: {} {}", spec.command, spec.args.join(" "));
        let (events, mut receiver) = mpsc::channel(64);
        let plugin = match Plugin::start(&spec, &root, events) {
            Ok(plugin) => plugin,
            Err(err) => {
                println!("  could not start: {err}");
                failed += 1;
                continue;
            }
        };
        let answer = time::timeout(DOCTOR_WAIT, async {
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
        drop(plugin);
        match answer {
            Ok(Ok((hello, log))) => {
                println!("  ok, protocol {}", hello.protocol);
                for command in &hello.commands {
                    println!(
                        "  command plugin.{name}.{} \"{}\"",
                        command.name, command.title
                    );
                }
                if !hello.events.is_empty() {
                    let events: Vec<&str> = hello.events.iter().map(String::as_str).collect();
                    println!("  listens for {}", events.join(", "));
                }
                for (provider, _) in &hello.providers {
                    println!("  provides {provider}");
                }
                for line in log {
                    println!("  stderr: {line}");
                }
            }
            Ok(Err(reason)) => {
                println!("  failed: {reason}");
                failed += 1;
            }
            Err(_) => {
                println!("  did not answer in {}s", DOCTOR_WAIT.as_secs());
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} plugin(s) need attention");
    }
    Ok(())
}

#[cfg(test)]
/// Tests for the plugin tools.
mod tests {
    use std::{env, fs, path::Path, process};

    use super::{check_name, install, is_git_url, new, remove, test};
    use crate::cli::Language;

    /// Names stay simple so they work as folder names and command prefixes.
    #[test]
    fn checks_names() {
        assert!(check_name("words-2").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("../evil").is_err());
        assert!(check_name("a.b").is_err());
    }

    /// The todo example passes the test scripts in its folder, when Python is around.
    #[tokio::test]
    async fn runs_the_todo_example_tests() {
        let has_python = ["python3", "python"].into_iter().any(|program| {
            process::Command::new(program)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        });
        if !has_python {
            return;
        }
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/todo");
        test(
            Path::new("no-plugins-here"),
            &example.to_string_lossy(),
            Vec::new(),
        )
        .await
        .expect("the tests pass");
    }

    /// Git urls are told apart from folders.
    #[test]
    fn spots_git_urls() {
        assert!(is_git_url("https://github.com/me/plugin"));
        assert!(is_git_url("git@github.com:me/plugin.git"));
        assert!(!is_git_url("./plugins/words"));
    }

    /// A new plugin can be installed elsewhere and removed, and names never clash.
    #[test]
    fn creates_installs_and_removes() {
        let base = env::temp_dir().join(format!("mog-plugin-cli-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let made = base.join("made");
        let installed = base.join("installed");
        new(&made, "hello", Language::Node).expect("created");
        assert!(made.join("hello/plugin.toml").is_file());
        assert!(made.join("hello/main.js").is_file());
        assert!(new(&made, "hello", Language::Python).is_err());
        install(&installed, &made.join("hello").to_string_lossy()).expect("installed");
        assert!(installed.join("hello/main.js").is_file());
        assert!(install(&installed, &made.join("hello").to_string_lossy()).is_err());
        remove(&installed, "hello").expect("removed");
        assert!(!installed.join("hello").exists());
        assert!(remove(&installed, "hello").is_err());
        let _ = fs::remove_dir_all(&base);
    }
}
