//! Command line arguments.

use std::path::PathBuf;

use clap::Parser;

/// A terminal code editor that mogs other editors.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// The file or folder to open. A folder is shown in the file explorer.
    pub path: Option<PathBuf>,
    /// Runs a command after starting, like `--run explorer.toggle`. Can be repeated.
    #[arg(long, value_name = "COMMAND")]
    pub run: Vec<String>,
    /// Prints every key binding and exits.
    #[arg(long)]
    pub keys: bool,
    /// Renders one frame of the given size, like `120x40`, as text and exits. For debugging.
    #[arg(long, hide = true, value_name = "WxH")]
    pub snapshot: Option<String>,
}
