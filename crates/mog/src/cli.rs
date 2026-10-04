//! Command line arguments.

use std::path::PathBuf;

use clap::Parser;

/// A terminal code editor that mogs other editors.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// The file or folder to open. A folder is shown in the file explorer.
    pub path: Option<PathBuf>,
}
