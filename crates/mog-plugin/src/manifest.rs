//! Plugin manifests, the `plugin.toml` that says what a plugin is before it runs.
//!
//! With a manifest mog can list a plugin's commands right away and start the plugin only when
//! one of its activation events happens.

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use mog_core::when::When;
use serde::Deserialize;
use serde_json::Value;

use crate::protocol::{OLDEST_PROTOCOL, PROTOCOL_VERSION, PluginCommand, PluginTool, parse_tools};

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
    /// When it shows in the right click menu.
    when: Option<String>,
}

/// What a plugin adds to mog without running, as written in the file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawContributions {
    /// Theme files, relative to the plugin folder.
    themes: Vec<String>,
    /// Key bindings it suggests, key to command.
    keys: BTreeMap<String, String>,
    /// Extra highlight query files by language, relative to the plugin folder.
    highlights: BTreeMap<String, String>,
    /// Tools for the AI chat.
    tools: Vec<Value>,
}

/// What a plugin adds to mog without running.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Contributions {
    /// Theme files, each a table like a `[themes.<name>]` in the config, named after the file.
    pub themes: Vec<PathBuf>,
    /// Key bindings it suggests, key to command, bound when the key is free.
    pub keys: BTreeMap<String, String>,
    /// Tree-sitter highlight query files added to a language's own, by language.
    pub highlights: BTreeMap<String, PathBuf>,
    /// Tools for the AI chat, offered while the plugin runs.
    pub tools: Vec<PluginTool>,
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
    /// What it adds without running.
    #[serde(default)]
    contributes: RawContributions,
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
    /// What it adds without running.
    pub contributes: Contributions,
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
                let when = command
                    .when
                    .as_deref()
                    .map(|when| {
                        when.parse::<When>()
                            .map_err(|err| format!("command {}: {err}", command.name))
                    })
                    .transpose()?;
                Ok(PluginCommand {
                    title: command.title.unwrap_or_else(|| command.name.clone()),
                    name: command.name,
                    keys: command.keys,
                    menu: command.menu,
                    when,
                })
            })
            .collect::<Result<_, String>>()?;
        let raw_contributes = raw.contributes;
        let inside = |file: &str| -> Result<PathBuf, String> {
            let relative = Path::new(file);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(format!("`{file}` must be a path inside the plugin folder"));
            }
            Ok(dir.join(relative))
        };
        let contributes = Contributions {
            themes: raw_contributes
                .themes
                .iter()
                .map(|file| inside(file))
                .collect::<Result<_, _>>()?,
            keys: raw_contributes.keys,
            highlights: raw_contributes
                .highlights
                .iter()
                .map(|(language, file)| Ok((language.clone(), inside(file)?)))
                .collect::<Result<_, String>>()?,
            tools: parse_tools(&Value::Array(raw_contributes.tools))?,
        };
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
            contributes,
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
            when = "language == md && selection"
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
        assert_eq!(
            manifest.commands[0]
                .when
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("language == md && selection")
        );
        let broken = "name = \"a\"\ncommand = \"x\"\n[[commands]]\nname = \"b\"\nwhen = \"((\"";
        assert!(Manifest::parse(broken, dir).is_err());
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

    /// Contributions read with their files inside the plugin folder, and ones outside refused.
    #[test]
    fn reads_contributions() {
        let dir = Path::new("/plugins/neon");
        let manifest = Manifest::parse(
            r#"
            name = "neon"
            command = "x"

            [contributes]
            themes = ["themes/neon.toml"]
            keys = { "alt+n" = "plugin.neon.glow" }
            highlights = { md = "queries/md.scm" }

            [[contributes.tools]]
            name = "glow"
            description = "Makes things glow"
            input_schema = { type = "object", properties = { what = { type = "string" } } }
            "#,
            dir,
        )
        .expect("valid");
        let contributes = &manifest.contributes;
        assert_eq!(contributes.themes, [dir.join("themes/neon.toml")]);
        assert_eq!(contributes.keys["alt+n"], "plugin.neon.glow");
        assert_eq!(contributes.highlights["md"], dir.join("queries/md.scm"));
        assert_eq!(contributes.tools[0].name, "glow");
        assert_eq!(
            contributes.tools[0].input_schema["properties"]["what"]["type"],
            "string"
        );
        let outside = "name = \"a\"\ncommand = \"x\"\n[contributes]\nthemes = [\"../evil.toml\"]";
        assert!(Manifest::parse(outside, dir).is_err());
        let bad_tool = "name = \"a\"\ncommand = \"x\"\n[[contributes.tools]]\nname = \"has space\"";
        assert!(Manifest::parse(bad_tool, dir).is_err());
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
