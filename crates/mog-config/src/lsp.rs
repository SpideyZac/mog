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
    (
        "c",
        "clangd",
        &[],
        &["c", "h", "cpp", "hpp", "cc", "cxx", "hh"],
    ),
    ("lua", "lua-language-server", &[], &["lua"]),
    ("zig", "zls", &[], &["zig"]),
    ("java", "jdtls", &[], &["java"]),
    ("csharp", "csharp-ls", &[], &["cs"]),
    ("kotlin", "kotlin-language-server", &[], &["kt", "kts"]),
    ("swift", "sourcekit-lsp", &[], &["swift"]),
    ("ruby", "ruby-lsp", &[], &["rb"]),
    ("php", "intelephense", &["--stdio"], &["php"]),
    ("scala", "metals", &[], &["scala", "sbt"]),
    (
        "haskell",
        "haskell-language-server-wrapper",
        &["--lsp"],
        &["hs"],
    ),
    ("ocaml", "ocamllsp", &[], &["ml", "mli"]),
    ("elixir", "elixir-ls", &[], &["ex", "exs"]),
    ("dart", "dart", &["language-server"], &["dart"]),
    ("bash", "bash-language-server", &["start"], &["sh", "bash"]),
    (
        "html",
        "vscode-html-language-server",
        &["--stdio"],
        &["html", "htm"],
    ),
    (
        "css",
        "vscode-css-language-server",
        &["--stdio"],
        &["css", "scss"],
    ),
    (
        "json",
        "vscode-json-language-server",
        &["--stdio"],
        &["json", "jsonc"],
    ),
    (
        "yaml",
        "yaml-language-server",
        &["--stdio"],
        &["yml", "yaml"],
    ),
    ("toml", "taplo", &["lsp", "stdio"], &["toml"]),
    ("markdown", "marksman", &[], &["md"]),
    ("vue", "vue-language-server", &["--stdio"], &["vue"]),
];

/// How to install each built in server, as `(command, how, runnable)`.
///
/// `how` is a shell command when `runnable` is set, otherwise directions for a person.
const INSTALL: &[(&str, &str, bool)] = &[
    ("rust-analyzer", "rustup component add rust-analyzer", true),
    ("pyright-langserver", "npm i -g pyright", true),
    (
        "typescript-language-server",
        "npm i -g typescript-language-server typescript",
        true,
    ),
    ("gopls", "go install golang.org/x/tools/gopls@latest", true),
    (
        "clangd",
        "install clangd from your package manager or LLVM",
        false,
    ),
    (
        "lua-language-server",
        "get it from your package manager or github.com/LuaLS/lua-language-server",
        false,
    ),
    (
        "zls",
        "get it from your package manager or github.com/zigtools/zls",
        false,
    ),
    (
        "jdtls",
        "get it from your package manager or download.eclipse.org/jdtls",
        false,
    ),
    ("csharp-ls", "dotnet tool install --global csharp-ls", true),
    (
        "kotlin-language-server",
        "get it from your package manager or github.com/fwcd/kotlin-language-server",
        false,
    ),
    (
        "sourcekit-lsp",
        "it comes with the Swift toolchain from swift.org",
        false,
    ),
    ("ruby-lsp", "gem install ruby-lsp", true),
    ("intelephense", "npm i -g intelephense", true),
    ("metals", "cs install metals", true),
    ("haskell-language-server-wrapper", "ghcup install hls", true),
    ("ocamllsp", "opam install ocaml-lsp-server", true),
    (
        "elixir-ls",
        "get it from github.com/elixir-lsp/elixir-ls",
        false,
    ),
    ("dart", "it comes with the Dart SDK from dart.dev", false),
    (
        "bash-language-server",
        "npm i -g bash-language-server",
        true,
    ),
    (
        "vscode-html-language-server",
        "npm i -g vscode-langservers-extracted",
        true,
    ),
    (
        "vscode-css-language-server",
        "npm i -g vscode-langservers-extracted",
        true,
    ),
    (
        "vscode-json-language-server",
        "npm i -g vscode-langservers-extracted",
        true,
    ),
    (
        "yaml-language-server",
        "npm i -g yaml-language-server",
        true,
    ),
    (
        "taplo",
        "cargo install taplo-cli --locked --features lsp",
        true,
    ),
    (
        "marksman",
        "get it from your package manager or github.com/artempyanykh/marksman",
        false,
    ),
    ("vue-language-server", "npm i -g @vue/language-server", true),
];

/// How to get a missing language server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// A shell command that installs it.
    Run(&'static str),
    /// Directions for a person, when there is no one command for every system.
    Manual(&'static str),
}

/// Returns how to install the built in server run as `command`, if it is one.
pub fn install_hint(command: &str) -> Option<Install> {
    INSTALL
        .iter()
        .find(|(known, _, _)| *known == command)
        .map(|(_, how, runnable)| {
            if *runnable {
                Install::Run(how)
            } else {
                Install::Manual(how)
            }
        })
}

/// Built in servers whose language id differs from their name.
const LANGUAGE_IDS: &[(&str, &str)] = &[("bash", "shellscript")];

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
                language_id: LANGUAGE_IDS
                    .iter()
                    .find(|(server, _)| server == name)
                    .map(|(_, id)| (*id).to_owned()),
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
            if server.language_id.is_none() {
                server.language_id.clone_from(&builtin.language_id);
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

    use super::{BUILTIN_SERVERS, Install, ServerConfig, install_hint, merged_servers};
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

    /// Every built in server says how to install it.
    #[test]
    fn builtins_have_install_hints() {
        for (_, command, _, _) in BUILTIN_SERVERS {
            assert!(install_hint(command).is_some(), "{command}");
        }
        assert_eq!(
            install_hint("rust-analyzer"),
            Some(Install::Run("rustup component add rust-analyzer"))
        );
        assert_eq!(install_hint("my-own-server"), None);
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
