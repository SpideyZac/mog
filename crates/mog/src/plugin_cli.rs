//! `mog plugin`: creating, installing, listing, removing and checking plugins from the shell.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, IsTerminal as _, Write as _},
    path::{self, Path, PathBuf},
    process,
};

use anyhow::{Context, Result, anyhow, bail};
use mog_config::Config;
use mog_plugin::{MANIFEST_FILE, Manifest, Spec, discover};
use mog_plugin_test::{Script, run_script};

use crate::{
    cli::{Language, PluginAction},
    plugins::{plugin_dir, resolve},
};

mod doctor;
mod source;

use source::{Record, Source, Trust, fetch, newer};

/// What `mog plugin install` says before it installs anything.
const UNSANDBOXED: &str = "\
WARNING: plugins are not sandboxed. A plugin runs as you, and mog does not limit what it does.
It can read, change and delete any file you can, read every environment variable (API keys
and tokens included), see everything you open and type, use the network and run programs.
Only install plugins you have read or whose authors you trust.";

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
        PluginAction::Install {
            source,
            rev,
            key,
            allow_unsigned,
            yes,
        } => {
            let source = Source::parse(&source, rev.as_deref())?;
            confirm(UNSANDBOXED, &format!("install {source}"), yes)?;
            let trust = Trust {
                key,
                allow_unsigned,
            };
            let (manifest, record) = install(&dir, &source, &trust, None).await?;
            println!(
                "installed {} {} into {}, {}",
                manifest.name,
                manifest.version,
                dir.join(&manifest.name).display(),
                describe(&record)
            );
            println!("it runs as you with no sandbox, so remove it if you stop trusting it");
            Ok(())
        }
        PluginAction::Outdated => outdated(&dir),
        PluginAction::Update {
            name,
            rev,
            allow_unsigned,
            yes,
        } => update(&dir, name.as_deref(), rev, allow_unsigned, yes).await,
        PluginAction::Remove { name } => remove(&dir, &name),
        PluginAction::Doctor { name } => doctor::run(&dir, name.as_deref()).await,
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

/// Warns with `warning` and asks whether to `what`, unless `yes` says to go ahead.
///
/// # Errors
///
/// Returns an error if the answer is not yes, or there is nobody to ask.
fn confirm(warning: &str, what: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    eprintln!("{warning}\n");
    if !io::stdin().is_terminal() {
        bail!("pass --yes to {what} without asking");
    }
    eprint!("{what}? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        bail!("nothing changed");
    }
    Ok(())
}

/// Moves the plugin in `staging` to `target`, replacing what is there only when `replace`, and
/// putting the old one back if the move fails.
fn put_in_place(staging: &Path, target: &Path, replace: bool) -> Result<()> {
    if !target.exists() {
        return fs::rename(staging, target)
            .with_context(|| format!("could not move the plugin to {}", target.display()));
    }
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !replace {
        bail!(
            "{name} is already installed, update it with `mog plugin update {name}` or remove it \
             with `mog plugin remove {name}`"
        );
    }
    let old = target.with_file_name(format!(".old-{name}-{}", process::id()));
    let _ = fs::remove_dir_all(&old);
    fs::rename(target, &old).with_context(|| format!("could not move {}", target.display()))?;
    if let Err(err) = fs::rename(staging, target) {
        let _ = fs::rename(&old, target);
        return Err(err)
            .with_context(|| format!("could not move the plugin to {}", target.display()));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// Installs the plugin from `source` into the plugins folder `dir`. When `replacing` names a
/// plugin, it replaces that one and the new one must have the same name.
///
/// Returns its manifest and where it came from.
async fn install(
    dir: &Path,
    source: &Source,
    trust: &Trust,
    replacing: Option<&str>,
) -> Result<(Manifest, Record)> {
    fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let staging = dir.join(format!(".installing-{}", process::id()));
    let _ = fs::remove_dir_all(&staging);
    let installed = async {
        let record = fetch(source, &staging, trust).await?;
        let manifest = Manifest::read(&staging).map_err(|err| anyhow!(err))?;
        check_name(&manifest.name)?;
        if let Some(name) = replacing
            && manifest.name != name
        {
            bail!("{source} is the plugin {}, not {name}", manifest.name);
        }
        record.write(&staging)?;
        put_in_place(&staging, &dir.join(&manifest.name), replacing.is_some())?;
        Ok((manifest, record))
    }
    .await;
    let _ = fs::remove_dir_all(&staging);
    installed
}

/// Describes an installed plugin's record in a few words, like `git v1.2.0 (abc12345)`.
fn describe(record: &Record) -> String {
    match &record.source {
        Source::Folder { path } => format!("copied from {}", path.display()),
        Source::Git { url, rev } => {
            let commit = record.commit.as_deref().unwrap_or_default();
            let at = rev.as_deref().unwrap_or("the default branch");
            format!("git {url} at {at} ({})", commit.get(..8).unwrap_or(commit))
        }
        Source::Archive { location } => {
            let signed = if record.key.is_some() {
                "signed"
            } else {
                "UNSIGNED"
            };
            format!("{signed} archive {location}")
        }
    }
}

/// Prints the plugins installed with `mog plugin install` that have an update.
fn outdated(dir: &Path) -> Result<()> {
    let mut any = false;
    for manifest in discover(dir).into_iter().filter_map(Result::ok) {
        let name = &manifest.name;
        let Some(record) = Record::read(&manifest.dir) else {
            println!("  {name}: not installed with mog plugin install, update it by hand");
            continue;
        };
        any = true;
        match (&record.source, newer(&record)) {
            (_, Ok(Some(newer))) => println!(
                "  {name}: {} -> {}, run `mog plugin update {name}`",
                newer.from, newer.to
            ),
            (Source::Git { rev: Some(rev), .. }, Ok(None)) if rev.len() >= 7 => {
                println!("  {name}: up to date, or pinned to {rev}");
            }
            (Source::Git { .. }, Ok(None)) => println!("  {name}: up to date"),
            (Source::Archive { .. }, Ok(None)) => {
                println!("  {name}: from an archive, `mog plugin update {name}` fetches it again");
            }
            (Source::Folder { .. }, Ok(None)) => {
                println!("  {name}: from a folder, `mog plugin update {name}` copies it again");
            }
            (_, Err(err)) => println!("  {name}: could not check, {err}"),
        }
    }
    if !any {
        println!("no plugins were installed with mog plugin install");
    }
    Ok(())
}

/// Updates the plugin called `name`, or every one with an update, to `rev` or the newest
/// version.
async fn update(
    dir: &Path,
    name: Option<&str>,
    rev: Option<String>,
    allow_unsigned: bool,
    yes: bool,
) -> Result<()> {
    let installed: Vec<(Manifest, Record)> = discover(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|manifest| name.is_none_or(|name| manifest.name == name))
        .filter_map(|manifest| {
            let record = Record::read(&manifest.dir)?;
            Some((manifest, record))
        })
        .collect();
    if installed.is_empty() {
        bail!(match name {
            Some(name) => format!("{name} was not installed with mog plugin install"),
            None => "no plugins were installed with mog plugin install".to_owned(),
        });
    }
    if rev.is_some() && name.is_none() {
        bail!("--rev needs the name of the plugin to update");
    }
    let mut updated = 0;
    for (manifest, record) in installed {
        let plugin = manifest.name.as_str();
        let (source, change) = match (&record.source, &rev) {
            (Source::Git { .. }, Some(rev)) => {
                (record.source.at(Some(rev.clone())), format!("to {rev}"))
            }
            (Source::Git { .. }, None) => match newer(&record)? {
                Some(newer) => (
                    record.source.at(newer.rev.clone()),
                    format!("from {} to {}", newer.from, newer.to),
                ),
                None => {
                    println!("{plugin} is up to date");
                    continue;
                }
            },
            (_, Some(_)) => bail!("--rev only works for plugins installed from git"),
            (other, None) => (other.clone(), format!("from {other}")),
        };
        let trust = Trust {
            key: record.key.clone(),
            allow_unsigned,
        };
        confirm(
            "An update runs new code as you, with no sandbox. Read what changed if you can.",
            &format!("update {plugin} {change}"),
            yes,
        )?;
        let (manifest, new_record) = install(dir, &source, &trust, Some(plugin)).await?;
        if new_record.sha256.is_some() && new_record.sha256 == record.sha256 {
            println!("{plugin} did not change");
        } else {
            println!(
                "updated {plugin} to {} ({})",
                manifest.version,
                describe(&new_record)
            );
        }
        updated += 1;
    }
    if updated == 0 && name.is_none() {
        println!("everything is up to date");
    }
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

#[cfg(test)]
/// Tests for the plugin tools.
mod tests {
    use std::{env, fs, path::Path, process};

    use mog_plugin::Manifest;

    use super::{
        check_name, install, new, remove,
        source::{Record, Source, Trust},
        test,
    };
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

    /// Runs git in `dir`, panicking if it fails.
    fn git(dir: &Path, args: &[&str]) {
        let status = process::Command::new("git")
            .args(["-c", "user.name=mog", "-c", "user.email=mog@example.com"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    /// A plugin installed from git at a tag knows a newer tag exists and updates to it.
    #[tokio::test]
    async fn installs_and_updates_from_git() {
        if process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let base = env::temp_dir().join(format!("mog-plugin-git-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let work = base.join("work");
        fs::create_dir_all(&work).expect("dir");
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        for version in ["1.0.0", "1.1.0"] {
            let manifest = format!("name = \"hello\"\nversion = \"{version}\"\ncommand = \"x\"\n");
            fs::write(work.join("plugin.toml"), manifest).expect("manifest");
            git(&work, &["add", "."]);
            git(&work, &["commit", "--quiet", "-m", version]);
            git(&work, &["tag", &format!("v{version}")]);
        }
        let bare = base.join("hello.git");
        git(&base, &["clone", "--quiet", "--bare", "work", "hello.git"]);
        let plugins = base.join("plugins");
        let url = bare.to_string_lossy().replace('\\', "/");
        let source = Source::parse(&format!("{url}#v1.0.0"), None).expect("source");
        let (manifest, record) = install(&plugins, &source, &Trust::default(), None)
            .await
            .expect("installed");
        assert_eq!(manifest.version, "1.0.0");
        assert!(!plugins.join("hello/.git").exists());
        assert_eq!(Record::read(&plugins.join("hello")), Some(record.clone()));
        let newer = super::newer(&record).expect("checked").expect("newer");
        assert_eq!(
            (newer.from.as_str(), newer.to.as_str()),
            ("v1.0.0", "v1.1.0")
        );
        super::update(&plugins, Some("hello"), None, false, true)
            .await
            .expect("updated");
        let updated = Manifest::read(&plugins.join("hello")).expect("manifest");
        assert_eq!(updated.version, "1.1.0");
        let record = Record::read(&plugins.join("hello")).expect("record");
        assert!(super::newer(&record).expect("checked").is_none());
        let _ = fs::remove_dir_all(base);
    }

    /// A new plugin can be installed elsewhere, replaced, and removed, and names never clash.
    #[tokio::test]
    async fn creates_installs_and_removes() {
        let base = env::temp_dir().join(format!("mog-plugin-cli-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let made = base.join("made");
        let installed = base.join("installed");
        new(&made, "hello", Language::Node).expect("created");
        assert!(made.join("hello/plugin.toml").is_file());
        assert!(made.join("hello/main.js").is_file());
        assert!(new(&made, "hello", Language::Python).is_err());
        let source = Source::Folder {
            path: made.join("hello"),
        };
        let trust = Trust::default();
        install(&installed, &source, &trust, None)
            .await
            .expect("installed");
        assert!(installed.join("hello/main.js").is_file());
        assert!(
            Record::read(&installed.join("hello")).is_some(),
            "it remembers where it came from"
        );
        assert!(install(&installed, &source, &trust, None).await.is_err());
        install(&installed, &source, &trust, Some("hello"))
            .await
            .expect("replaced");
        assert!(
            install(&installed, &source, &trust, Some("other"))
                .await
                .is_err(),
            "an update cannot swap in another plugin"
        );
        remove(&installed, "hello").expect("removed");
        assert!(!installed.join("hello").exists());
        assert!(remove(&installed, "hello").is_err());
        let _ = fs::remove_dir_all(&base);
    }
}
