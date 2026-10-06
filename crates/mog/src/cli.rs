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
    /// Copies a plugin folder, or clones a git repository, into the plugins folder.
    Install {
        /// A folder with a plugin.toml, or a git url.
        source: String,
        /// Install without asking. Plugins are not sandboxed, so only for ones you trust.
        #[arg(long)]
        yes: bool,
    },
    /// Deletes a plugin from the plugins folder.
    Remove {
        /// The plugin name.
        name: String,
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
