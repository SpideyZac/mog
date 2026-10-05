//! Settings a project keeps for itself in `.mog/config.toml`, and which projects are trusted.
//!
//! A project config can name programs to run as language servers, so mog only uses one after
//! the user trusts it, and asks again whenever the file changes.

use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Config, ConfigError, DebugConfig, TaskConfig, config_dir};

/// The folder in a project that holds its mog settings.
pub const PROJECT_DIR: &str = ".mog";

/// The name of the project config file inside [`PROJECT_DIR`].
const PROJECT_FILE: &str = "config.toml";

/// The file in the config folder listing trusted project configs.
const TRUST_FILE: &str = "trusted_projects";

/// The settings a project can override.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    /// Language servers by name, changing only what each one sets.
    pub lsp: BTreeMap<String, ProjectServer>,
    /// Tasks by name, replacing global ones with the same name.
    pub tasks: BTreeMap<String, TaskConfig>,
    /// Debuggers by name, replacing global ones with the same name.
    pub debug: BTreeMap<String, DebugConfig>,
}

/// One language server as a project changes it. Missing fields keep the global value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectServer {
    /// Whether the server runs for this project.
    pub enabled: Option<bool>,
    /// The program to run.
    pub command: Option<String>,
    /// The arguments to pass to it.
    pub args: Option<Vec<String>>,
    /// The file extensions it handles, without the dot.
    pub extensions: Option<Vec<String>>,
    /// The language id sent to it.
    pub language_id: Option<String>,
    /// Settings for the server, merged key by key into the global ones.
    pub settings: Option<Value>,
}

/// A project config file that was found.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectFile {
    /// Where it is.
    pub path: PathBuf,
    /// Its text, which trust is checked against.
    pub text: String,
    /// What it says.
    pub config: ProjectConfig,
}

/// Returns where the project config of the project at `root` lives.
pub fn project_config_path(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR).join(PROJECT_FILE)
}

impl ProjectConfig {
    /// Parses a project config from TOML text. `path` is only used in error messages.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if the text is not a valid project config.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Returns `true` if the project sets nothing.
    pub fn is_empty(&self) -> bool {
        self.lsp.is_empty() && self.tasks.is_empty() && self.debug.is_empty()
    }

    /// Changes `config` by what this project sets.
    pub fn apply(&self, config: &mut Config) {
        config.tasks.extend(self.tasks.clone());
        config.debug.extend(self.debug.clone());
        for (name, project) in &self.lsp {
            // a new entry only fills in what the project set, the rest comes from built ins
            let server = config.lsp.entry(name.clone()).or_default();
            if let Some(enabled) = project.enabled {
                server.enabled = enabled;
            }
            if let Some(command) = &project.command {
                server.command.clone_from(command);
            }
            if let Some(args) = &project.args {
                server.args.clone_from(args);
            }
            if let Some(extensions) = &project.extensions {
                server.extensions.clone_from(extensions);
            }
            if let Some(language_id) = &project.language_id {
                server.language_id = Some(language_id.clone());
            }
            if let Some(settings) = &project.settings {
                merge_json(&mut server.settings, settings.clone());
            }
        }
    }
}

/// Merges `over` into `base`, key by key for objects and replacing everything else.
fn merge_json(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                merge_json(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, over) => *base = over,
    }
}

/// Reads the project config of the project at `root`, if it has one.
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn read_project(root: &Path) -> Result<Option<ProjectFile>, ConfigError> {
    let path = project_config_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ConfigError::Read { path, source }),
    };
    let config = ProjectConfig::parse(&text, &path)?;
    Ok(Some(ProjectFile { path, text, config }))
}

/// Returns the line the trust file holds for `file`, its hash then its path.
fn trust_line(file: &ProjectFile) -> String {
    let hash: String = Sha256::digest(file.text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = fs::canonicalize(&file.path).unwrap_or_else(|_| file.path.clone());
    format!("{hash} {}", path.display())
}

/// Returns the path part of a trust file `line`.
fn trusted_path(line: &str) -> &str {
    line.split_once(' ').map_or("", |(_, path)| path)
}

/// Returns whether `file` was trusted exactly as it is now, using the trust file in `dir`.
fn is_trusted_in(dir: &Path, file: &ProjectFile) -> bool {
    let wanted = trust_line(file);
    fs::read_to_string(dir.join(TRUST_FILE))
        .is_ok_and(|text| text.lines().any(|line| line == wanted))
}

/// Remembers `file` as trusted in the trust file in `dir`, forgetting older versions of it.
fn trust_in(dir: &Path, file: &ProjectFile) -> Result<(), ConfigError> {
    let path = dir.join(TRUST_FILE);
    let line = trust_line(file);
    let mut lines: Vec<String> = fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|old| trusted_path(old) != trusted_path(&line))
        .map(str::to_owned)
        .collect();
    lines.push(line);
    fs::create_dir_all(dir).map_err(|source| ConfigError::Write {
        path: path.clone(),
        source,
    })?;
    fs::write(&path, lines.join("\n") + "\n").map_err(|source| ConfigError::Write { path, source })
}

/// Returns whether `file` was trusted exactly as it is now.
pub fn is_trusted(file: &ProjectFile) -> bool {
    config_dir().is_some_and(|dir| is_trusted_in(&dir, file))
}

/// Remembers `file` as trusted until it changes.
///
/// # Errors
///
/// Returns an error if there is no config folder or the trust file cannot be written.
pub fn trust(file: &ProjectFile) -> Result<(), ConfigError> {
    let dir = config_dir().ok_or(ConfigError::NoConfigDir)?;
    trust_in(&dir, file)
}

#[cfg(test)]
/// Tests for project configs.
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use serde_json::json;

    use super::{ProjectConfig, ProjectFile, is_trusted_in, read_project, trust_in};
    use crate::Config;

    /// Creates an empty scratch folder unique to `name`.
    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mog-proj-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// A project changes only what it sets and merges settings key by key.
    #[test]
    fn project_overrides_only_what_it_sets() {
        let mut config = Config::parse(
            "[lsp.rust]\nargs = [\"--a\"]\n[lsp.rust.settings]\ncheck.command = \"check\"\n\
             cargo.features = [\"x\"]\n[lsp.go]\nenabled = false\n",
            "global.toml".as_ref(),
        )
        .expect("valid global");
        let project = ProjectConfig::parse(
            "[lsp.rust.settings]\ncheck.command = \"clippy\"\n[lsp.python]\ncommand = \"pylsp\"\n",
            "project.toml".as_ref(),
        )
        .expect("valid project");
        project.apply(&mut config);
        let rust = &config.lsp["rust"];
        assert_eq!(rust.args, ["--a"]);
        assert_eq!(rust.settings["check"]["command"], json!("clippy"));
        assert_eq!(rust.settings["cargo"]["features"], json!(["x"]));
        assert!(!config.lsp["go"].enabled);
        let servers = config.language_servers();
        assert_eq!(servers["python"].command, "pylsp");
        assert_eq!(servers["python"].extensions, ["py"]);
        assert!(!servers.contains_key("go"));
    }

    /// Project tasks and debuggers replace global ones by name.
    #[test]
    fn project_tasks_replace_global_ones() {
        let mut config = Config::parse(
            "[tasks.build]\ncommand = \"make\"\n[tasks.lint]\ncommand = \"lint\"\n",
            "global.toml".as_ref(),
        )
        .expect("valid global");
        let project = ProjectConfig::parse(
            "[tasks.build]\ncommand = \"cargo build\"\n[debug.rust]\ncommand = \"lldb-dap\"\n\
             [debug.rust.arguments]\nprogram = \"${root}/target/debug/app\"\n",
            "project.toml".as_ref(),
        )
        .expect("valid project");
        assert!(!project.is_empty());
        project.apply(&mut config);
        assert_eq!(config.tasks["build"].command, "cargo build");
        assert_eq!(config.tasks["lint"].command, "lint");
        assert_eq!(config.debug["rust"].request, "launch");
        assert_eq!(
            config.debug["rust"].arguments["program"],
            json!("${root}/target/debug/app")
        );
    }

    /// Only language server settings belong in a project config.
    #[test]
    fn rejects_other_sections() {
        assert!(ProjectConfig::parse("[ui]\ntheme = \"paper\"\n", "p.toml".as_ref()).is_err());
    }

    /// Trust sticks to the exact text and goes away when the file changes.
    #[test]
    fn trust_follows_the_text() {
        let root = scratch("trust");
        let home = scratch("trust-home");
        assert_eq!(read_project(&root).expect("readable"), None);
        fs::create_dir_all(root.join(".mog")).expect("mkdir");
        fs::write(
            root.join(".mog/config.toml"),
            "[lsp.rust]\nenabled = false\n",
        )
        .expect("write");
        let file: ProjectFile = read_project(&root).expect("readable").expect("exists");
        assert!(!is_trusted_in(&home, &file));
        trust_in(&home, &file).expect("trust");
        assert!(is_trusted_in(&home, &file));
        fs::write(
            root.join(".mog/config.toml"),
            "[lsp.rust]\ncommand = \"evil\"\n",
        )
        .expect("write");
        let changed = read_project(&root).expect("readable").expect("exists");
        assert!(!is_trusted_in(&home, &changed));
        trust_in(&home, &changed).expect("trust");
        let lines = fs::read_to_string(home.join("trusted_projects")).expect("read");
        assert_eq!(lines.lines().count(), 1);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&home);
    }
}
