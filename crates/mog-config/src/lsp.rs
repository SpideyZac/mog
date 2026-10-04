//! Language server settings.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// The servers known out of the box as `(name, command, args, extensions)`.
const BUILTIN_SERVERS: &[(&str, &str, &[&str], &[&str])] = &[
    ("rust", "rust-analyzer", &[], &["rs"]),
    ("python", "pyright-langserver", &["--stdio"], &["py"]),
    (
        "typescript",
        "typescript-language-server",
        &["--stdio"],
        &["ts", "tsx", "js", "jsx"],
    ),
    ("go", "gopls", &[], &["go"]),
    ("c", "clangd", &[], &["c", "h", "cpp", "hpp", "cc"]),
    ("lua", "lua-language-server", &[], &["lua"]),
];

/// How to run one language server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Whether the server is started at all.
    pub enabled: bool,
    /// The program to run.
    pub command: String,
    /// The arguments to pass to it.
    pub args: Vec<String>,
    /// The file extensions the server handles, without the dot.
    pub extensions: Vec<String>,
    /// The language id sent to the server. Defaults to the server name.
    pub language_id: Option<String>,
    /// Settings for the server itself, like `check.command = "clippy"` for rust-analyzer.
    ///
    /// They are sent as initialization options and given to the server when it asks for its
    /// configuration.
    pub settings: Value,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: String::new(),
            args: Vec::new(),
            extensions: Vec::new(),
            language_id: None,
            settings: Value::Null,
        }
    }
}

/// Returns the built in servers merged with `overrides`, which win by name.
///
/// An override of a built in server only replaces what it sets, so changing just the command
/// keeps the built in extensions and arguments. Disabled servers are left out.
pub fn merged_servers(
    overrides: &BTreeMap<String, ServerConfig>,
) -> BTreeMap<String, ServerConfig> {
    let mut servers: BTreeMap<String, ServerConfig> = BUILTIN_SERVERS
        .iter()
        .map(|(name, command, args, extensions)| {
            let config = ServerConfig {
                command: (*command).to_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                extensions: extensions.iter().map(|ext| (*ext).to_owned()).collect(),
                ..ServerConfig::default()
            };
            ((*name).to_owned(), config)
        })
        .collect();
    for (name, server) in overrides {
        let mut server = server.clone();
        if let Some(builtin) = servers.get(name) {
            if server.command.is_empty() {
                server.command.clone_from(&builtin.command);
            }
            if server.args.is_empty() {
                server.args.clone_from(&builtin.args);
            }
            if server.extensions.is_empty() {
                server.extensions.clone_from(&builtin.extensions);
            }
        }
        servers.insert(name.clone(), server);
    }
    servers.retain(|_, server| server.enabled && !server.command.is_empty());
    servers
}

#[cfg(test)]
/// Tests for server merging.
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use super::{ServerConfig, merged_servers};
    use crate::Config;

    /// Overrides replace built ins and disabled servers are dropped.
    #[test]
    fn overrides_and_disables() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "go".to_owned(),
            ServerConfig {
                enabled: false,
                ..ServerConfig::default()
            },
        );
        overrides.insert(
            "zig".to_owned(),
            ServerConfig {
                command: "zls".into(),
                extensions: vec!["zig".into()],
                ..ServerConfig::default()
            },
        );
        let servers = merged_servers(&overrides);
        assert!(!servers.contains_key("go"));
        assert_eq!(servers["zig"].command, "zls");
        assert_eq!(servers["rust"].command, "rust-analyzer");
    }

    /// Overriding only the command keeps the built in extensions and arguments.
    #[test]
    fn partial_override_keeps_builtin_fields() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "python".to_owned(),
            ServerConfig {
                command: "my-pyright".into(),
                ..ServerConfig::default()
            },
        );
        let servers = merged_servers(&overrides);
        assert_eq!(servers["python"].command, "my-pyright");
        assert_eq!(servers["python"].extensions, ["py"]);
        assert_eq!(servers["python"].args, ["--stdio"]);
    }

    /// Server settings are read from a nested table, dotted keys included.
    #[test]
    fn reads_settings() {
        let text = "[lsp.rust.settings]\ncheck.command = \"clippy\"\n";
        let config = Config::parse(text, Path::new("test.toml")).expect("valid");
        let servers = config.language_servers();
        assert_eq!(servers["rust"].settings["check"]["command"], "clippy");
        assert_eq!(servers["rust"].command, "rust-analyzer");
    }
}
