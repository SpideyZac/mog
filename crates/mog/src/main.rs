//! The `mog` binary, the entry point of the editor.

mod ai;
mod app;
mod cli;
mod clipboard;
mod lsp;
mod settings;
mod terminal;

use anyhow::Result;
use app::App;
use clap::Parser;
use cli::Args;

/// Runs the editor.
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up or the app fails.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut tui = terminal::init()?;
    let result = App::new(args).run(&mut tui).await;
    terminal::restore()?;
    result
}
