//! Command line arguments.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// A terminal code editor that mogs other editors.
#[derive(Debug, Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
pub struct Args {
    /// A tool to run instead of the editor.
    #[command(subcommand)]
    pub tool: Option<Tool>,
    /// The file or folder to open. A folder is shown in the file explorer.
    pub path: Option<PathBuf>,
    /// Runs a command after starting, like `--run explorer.toggle`. Can be repeated.
    #[arg(long, value_name = "COMMAND")]
    pub run: Vec<String>,
    /// Prints every key binding and exits.
    #[arg(long)]
    pub keys: bool,
    /// Installs the newest release from GitHub and exits.
    #[arg(long)]
    pub update: bool,
    /// Renders one frame of the given size, like `120x40`, as text and exits. For debugging.
    #[arg(long, hide = true, value_name = "WxH")]
    pub snapshot: Option<String>,
    /// How long a snapshot lets background work run, in milliseconds.
    #[arg(long, hide = true, default_value_t = 800)]
    pub snapshot_wait: u64,
    /// Commands a snapshot runs after waiting, then waits again before drawing.
    #[arg(long, hide = true, value_name = "COMMAND")]
    pub snapshot_run: Vec<String>,
}

/// Tools that run instead of the editor.
#[derive(Debug, Subcommand)]
pub enum Tool {
    /// Manages plugins.
    Plugin {
        /// What to do.
        #[command(subcommand)]
        action: PluginAction,
    },
}

/// What `mog plugin` does.
#[derive(Debug, Subcommand)]
pub enum PluginAction {
    /// Lists the plugins in the plugins folder and the config.
    List,
    /// Creates a new plugin in the plugins folder from a template.
    New {
        /// The plugin name, which namespaces its commands.
        name: String,
        /// The language to write it in.
        #[arg(long, value_enum, default_value_t = Language::Python)]
        language: Language,
    },
    /// Copies a plugin folder, clones a git repository, or unpacks a signed archive into the
    /// plugins folder.
    Install {
        /// A folder with a plugin.toml, a git url (`url#v1.2.0` pins a version), or a `.tar.gz`
        /// or `.zip` url or file with a `.minisig` signature next to it.
        source: String,
        /// The git tag, branch or commit to install.
        #[arg(long)]
        rev: Option<String>,
        /// The minisign public key from the author that the archive must be signed with.
        #[arg(long)]
        key: Option<String>,
        /// Install an archive with no signature.
        #[arg(long)]
        allow_unsigned: bool,
        /// Install without asking. Plugins are not sandboxed, so only for ones you trust.
        #[arg(long)]
        yes: bool,
    },
    /// Lists installed plugins that have a newer version.
    Outdated,
    /// Updates a plugin, or every one with an update, from where it was installed.
    Update {
        /// The plugin, every one when left out.
        name: Option<String>,
        /// The git tag, branch or commit to move to, instead of the newest.
        #[arg(long)]
        rev: Option<String>,
        /// Update from an archive with no signature.
        #[arg(long)]
        allow_unsigned: bool,
        /// Update without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Deletes a plugin from the plugins folder.
    Remove {
        /// The plugin name.
        name: String,
    },
    /// Runs a plugin against a fake editor with test scripts, without starting mog.
    Test {
        /// The plugin name, or the folder with its plugin.toml.
        plugin: String,
        /// The scripts to run, every `.toml` in its `tests` folder when left out.
        scripts: Vec<PathBuf>,
    },
    /// Starts plugins and checks they answer, showing what they offer.
    Doctor {
        /// Only this plugin.
        name: Option<String>,
    },
}

/// A language a new plugin can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Language {
    /// Python 3 with the single file Python SDK.
    Python,
    /// Node with the single file JavaScript SDK.
    Node,
}
