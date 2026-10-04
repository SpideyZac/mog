//! The `mog` binary, the entry point of the editor.

mod ai;
mod app;
mod cli;
mod clipboard;
mod commands;
mod discord;
mod git;
mod lsp;
mod settings;
mod terminal;
mod watch;

use std::time::Duration;

use anyhow::Result;
use app::App;
use clap::Parser;
use cli::Args;
use mog_config::Config;

/// Runs the editor.
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up or the app fails.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.keys {
        let config = Config::load().unwrap_or_default();
        let (keymap, _) = settings::keymap(&config);
        println!("{}", commands::key_table(&keymap));
        return Ok(());
    }
    if let Some(size) = args.snapshot.clone() {
        let wait = Duration::from_millis(args.snapshot_wait);
        let commands = args.snapshot_run.clone();
        let text = App::new(args).snapshot(&size, wait, &commands).await?;
        println!("{text}");
        return Ok(());
    }
    let mut tui = terminal::init()?;
    let result = App::new(args).run(&mut tui).await;
    terminal::restore()?;
    result
}
