//! Configuration loading for mog.
//!
//! The config is a TOML file at `<config dir>/mog/config.toml`. Every field has a default so an
//! empty or missing file is a valid config.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use toml::de::Error as TomlError;

pub mod ai;
pub mod lsp;

pub use ai::{AiConfig, ClaudeConfig, CopilotConfig};
pub use lsp::ServerConfig;

/// The environment variable that overrides the config directory.
pub const CONFIG_DIR_ENV: &str = "MOG_CONFIG_DIR";

/// The name of the config file inside the config directory.
const CONFIG_FILE: &str = "config.toml";

/// An error from loading the config.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file exists but could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The file that failed.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// The config file is not valid TOML or has wrong types.
    #[error("invalid config in {path}: {source}")]
    Parse {
        /// The file that failed.
        path: PathBuf,
        /// The underlying error.
        source: TomlError,
    },
}

/// The whole configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Editing behavior.
    pub editor: EditorConfig,
    /// Key binding overrides from chord, like `ctrl+d`, to command name, like `select_all`.
    ///
    /// Binding a chord to `""` removes its default binding.
    pub keys: BTreeMap<String, String>,
    /// The decorative extras.
    pub flair: FlairConfig,
    /// Language servers by name. Entries here override or extend the built in ones.
    pub lsp: BTreeMap<String, ServerConfig>,
    /// AI providers.
    pub ai: AiConfig,
}

/// Settings for editing behavior.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// The width of a tab stop in cells.
    pub tab_width: usize,
    /// Whether the tab key inserts spaces.
    pub insert_spaces: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_width: 4,
            insert_spaces: true,
        }
    }
}

/// Settings for the decorative extras.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FlairConfig {
    /// Whether any flair is shown at all.
    pub enabled: bool,
    /// The ids of flairs to turn off.
    pub disabled: Vec<String>,
}

impl Default for FlairConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disabled: Vec::new(),
        }
    }
}

impl Config {
    /// Returns every enabled language server, built in ones included.
    pub fn language_servers(&self) -> BTreeMap<String, ServerConfig> {
        lsp::merged_servers(&self.lsp)
    }

    /// Parses a config from TOML text. `path` is only used in error messages.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if the text is not a valid config.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Loads the config from [`config_dir`], or the defaults if there is no config file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self, ConfigError> {
        let Some(dir) = config_dir() else {
            return Ok(Self::default());
        };
        let path = dir.join(CONFIG_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, &path),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read { path, source }),
        }
    }
}

/// Returns the directory mog keeps its config and plugins in.
///
/// This is [`CONFIG_DIR_ENV`] if set, otherwise `mog` inside the platform config directory.
pub fn config_dir() -> Option<PathBuf> {
    env::var_os(CONFIG_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|dir| dir.join("mog")))
}

#[cfg(test)]
/// Tests for config parsing.
mod tests {
    use std::path::Path;

    use super::Config;

    /// An empty file gives the defaults.
    #[test]
    fn empty_config_is_default() {
        let config = Config::parse("", Path::new("test.toml")).expect("valid config");
        assert_eq!(config, Config::default());
    }

    /// Set fields override defaults and the rest stay default.
    #[test]
    fn partial_editor_config() {
        let config =
            Config::parse("[editor]\ntab_width = 2\n", Path::new("test.toml")).expect("valid");
        assert_eq!(config.editor.tab_width, 2);
        assert!(config.editor.insert_spaces);
    }

    /// Key overrides are read as plain strings.
    #[test]
    fn key_overrides() {
        let text = "[keys]
\"ctrl+d\" = \"select_all\"
";
        let config = Config::parse(text, Path::new("test.toml")).expect("valid");
        assert_eq!(config.keys["ctrl+d"], "select_all");
    }

    /// Unknown keys are reported instead of silently ignored.
    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("[editor]\ntabs = 2\n", Path::new("test.toml")).is_err());
    }
}
