//! Command line arguments.

use std::path::PathBuf;

use clap::Parser;

/// A terminal code editor that mogs other editors.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// The file to open.
    pub file: Option<PathBuf>,
}
