//! Plugin manifests, the `plugin.toml` that says what a plugin is before it runs.
//!
//! With a manifest mog can list a plugin's commands right away and start the plugin only when
//! one of its activation events happens.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::protocol::{OLDEST_PROTOCOL, PROTOCOL_VERSION, PluginCommand};

/// The name of the manifest file in a plugin folder.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// The placeholder in `command` and `args` that becomes the plugin folder.
pub const DIR_PLACEHOLDER: &str = "${plugin_dir}";

/// When a plugin with a manifest starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// As soon as mog starts.
    Startup,
    /// The first time one of its commands runs.
    Command,
    /// The first time a file with this extension gets focus.
    Language(String),
}

impl Activation {
    /// Reads an activation like `startup`, `command` or `language:md`.
    fn parse(text: &str) -> Result<Self, String> {
        match text.split_once(':') {
            None if text == "startup" => Ok(Self::Startup),
            None if text == "command" => Ok(Self::Command),
            Some(("language", language)) if !language.is_empty() => {
                Ok(Self::Language(language.to_owned()))
            }
            _ => Err(format!(
                "unknown activation `{text}`, use startup, command or language:<extension>"
            )),
        }
    }
}

/// A command as written in a manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    /// The name inside the plugin.
    name: String,
    /// What the palette shows.
    title: Option<String>,
    /// Keys the plugin would like bound to it.
    #[serde(default)]
    keys: Vec<String>,
    /// Whether it shows in the right click menu.
    #[serde(default)]
    menu: bool,
}

/// A manifest as written in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    /// The plugin name.
    name: String,
    /// The plugin version.
    #[serde(default)]
    version: String,
    /// One line about what it does.
    #[serde(default)]
    description: String,
    /// The protocol version it speaks.
    #[serde(default = "default_protocol")]
    protocol: u32,
    /// The program to run.
    command: String,
    /// The program to run on Windows, where it often has another name, like `python` for
    /// `python3`.
    command_windows: Option<String>,
    /// Its arguments.
    #[serde(default)]
    args: Vec<String>,
    /// When it starts.
    #[serde(default)]
    activation: Vec<String>,
    /// How many seconds a command may take.
    timeout: Option<u64>,
    /// The commands it adds.
    #[serde(default)]
    commands: Vec<RawCommand>,
}

/// Returns the protocol a manifest speaks when it does not say.
fn default_protocol() -> u32 {
    PROTOCOL_VERSION
}

/// A plugin's `plugin.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The plugin name, which namespaces its commands.
    pub name: String,
    /// The plugin version.
    pub version: String,
    /// One line about what it does.
    pub description: String,
    /// The protocol version it speaks.
    pub protocol: u32,
    /// The program to run, with [`DIR_PLACEHOLDER`] filled in.
    pub command: String,
    /// Its arguments, with [`DIR_PLACEHOLDER`] filled in.
    pub args: Vec<String>,
    /// When it starts. Empty means at startup.
    pub activation: Vec<Activation>,
    /// How many seconds a command may take, if not the default.
    pub timeout: Option<u64>,
    /// The commands it adds, known before it runs.
    pub commands: Vec<PluginCommand>,
    /// The folder the manifest is in.
    pub dir: PathBuf,
}

impl Manifest {
    /// Reads a manifest from `text`, for a plugin in `dir`.
    ///
    /// # Errors
    ///
    /// Returns why the manifest cannot be used.
    pub fn parse(text: &str, dir: &Path) -> Result<Self, String> {
        let raw: RawManifest = toml::from_str(text).map_err(|err| err.to_string())?;
        if raw.name.is_empty() || raw.name.contains(['.', ' ', ':']) {
            return Err(format!("`{}` is not a valid plugin name", raw.name));
        }
        if !(OLDEST_PROTOCOL..=PROTOCOL_VERSION).contains(&raw.protocol) {
            return Err(format!(
                "it speaks protocol {} but this mog speaks {OLDEST_PROTOCOL} to {PROTOCOL_VERSION}",
                raw.protocol
            ));
        }
        let dir_text = dir.to_string_lossy();
        // a verbatim windows path cannot take the forward slash a manifest writes after it
        let dir_text = dir_text
            .strip_prefix(r"\\?\")
            .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(&dir_text);
        let fill = |text: &str| text.replace(DIR_PLACEHOLDER, dir_text);
        let commands = raw
            .commands
            .into_iter()
            .map(|command| {
                if command.name.is_empty() || command.name.contains([' ', ':']) {
                    return Err(format!("`{}` is not a valid command name", command.name));
                }
                Ok(PluginCommand {
                    title: command.title.unwrap_or_else(|| command.name.clone()),
                    name: command.name,
                    keys: command.keys,
                    menu: command.menu,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            name: raw.name,
            version: raw.version,
            description: raw.description,
            protocol: raw.protocol,
            command: fill(match (&raw.command_windows, cfg!(windows)) {
                (Some(command), true) => command,
                _ => &raw.command,
            }),
            args: raw.args.iter().map(|arg| fill(arg)).collect(),
            activation: raw
                .activation
                .iter()
                .map(|text| Activation::parse(text))
                .collect::<Result<_, _>>()?,
            timeout: raw.timeout,
            commands,
            dir: dir.to_owned(),
        })
    }

    /// Reads the manifest in the plugin folder `dir`.
    ///
    /// # Errors
    ///
    /// Returns why the manifest cannot be read or used.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join(MANIFEST_FILE);
        let text = fs::read_to_string(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        Self::parse(&text, dir).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Returns whether the plugin starts as soon as mog does.
    pub fn starts_at_once(&self) -> bool {
        self.activation.is_empty() || self.activation.contains(&Activation::Startup)
    }

    /// Returns whether focusing a file with extension `language` starts the plugin.
    pub fn starts_for_language(&self, language: &str) -> bool {
        self.activation.iter().any(
            |activation| matches!(activation, Activation::Language(other) if other == language),
        )
    }
}

/// Reads every plugin folder in `dir`, each a folder with a [`MANIFEST_FILE`].
///
/// Folders without a manifest are skipped, broken manifests are returned as errors.
pub fn discover(dir: &Path) -> Vec<Result<Manifest, String>> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join(MANIFEST_FILE).is_file())
        .collect();
    folders.sort();
    folders
        .iter()
        .map(|folder| Manifest::read(folder))
        .collect()
}

#[cfg(test)]
/// Tests for manifests.
mod tests {
    use std::{env, fs, path::Path, process};

    use super::{Activation, Manifest, discover};

    /// A full manifest reads with its placeholders filled in.
    #[test]
    fn reads_a_manifest() {
        let dir = Path::new("/plugins/words");
        let manifest = Manifest::parse(
            r#"
            name = "words"
            version = "1.0.0"
            command = "python3"
            args = ["${plugin_dir}/words.py"]
            activation = ["command", "language:md"]
            timeout = 5

            [[commands]]
            name = "count"
            title = "Words: Count"
            keys = ["alt+w"]
            menu = true
            "#,
            dir,
        )
        .expect("valid");
        assert_eq!(manifest.args[0], format!("{}/words.py", dir.display()));
        assert_eq!(
            manifest.activation,
            [Activation::Command, Activation::Language("md".into())]
        );
        assert!(!manifest.starts_at_once());
        assert!(manifest.starts_for_language("md"));
        assert!(manifest.commands[0].menu);
        assert_eq!(manifest.timeout, Some(5));
    }

    /// Bad names, protocols and activations are reported.
    #[test]
    fn rejects_bad_manifests() {
        let dir = Path::new("/p");
        assert!(Manifest::parse("name = \"a.b\"\ncommand = \"x\"", dir).is_err());
        assert!(Manifest::parse("name = \"a\"\ncommand = \"x\"\nprotocol = 9", dir).is_err());
        assert!(
            Manifest::parse(
                "name = \"a\"\ncommand = \"x\"\nactivation = [\"soon\"]",
                dir
            )
            .is_err()
        );
        assert!(Manifest::parse("name = \"a\"\ncommand = \"x\"\ncolour = 1", dir).is_err());
        let minimal = Manifest::parse("name = \"a\"\ncommand = \"x\"", dir).expect("minimal");
        assert!(minimal.starts_at_once());
    }

    /// Plugin folders are found by their manifest.
    #[test]
    fn discovers_plugins() {
        let dir = env::temp_dir().join(format!("mog-discover-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("good")).expect("dir");
        fs::create_dir_all(dir.join("broken")).expect("dir");
        fs::create_dir_all(dir.join("empty")).expect("dir");
        fs::write(
            dir.join("good/plugin.toml"),
            "name = \"good\"\ncommand = \"x\"",
        )
        .expect("write");
        fs::write(dir.join("broken/plugin.toml"), "name = ").expect("write");
        let found = discover(&dir);
        assert_eq!(found.len(), 2);
        assert!(found[0].is_err());
        assert_eq!(found[1].as_ref().expect("good").name, "good");
        let _ = fs::remove_dir_all(&dir);
    }
}
