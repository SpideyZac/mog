//! Language server settings.

use std::collections::BTreeMap;

use serde::Deserialize;

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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: String::new(),
            args: Vec::new(),
            extensions: Vec::new(),
            language_id: None,
        }
    }
}

/// Returns the built in servers merged with `overrides`, which win by name.
///
/// Disabled servers are left out.
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
    servers.extend(overrides.clone());
    servers.retain(|_, server| server.enabled && !server.command.is_empty());
    servers
}

#[cfg(test)]
/// Tests for server merging.
mod tests {
    use std::collections::BTreeMap;

    use super::{ServerConfig, merged_servers};

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
}
