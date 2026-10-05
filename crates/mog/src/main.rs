//! The `mog` binary, the entry point of the editor.

mod ai;
mod app;
mod cli;
mod clipboard;
mod commands;
mod debug;
mod discord;
mod git;
mod lsp;
mod plugin_cli;
mod plugins;
mod session;
mod settings;
mod tasks;
mod terminal;
mod update;
mod watch;

use std::time::Duration;

use anyhow::{Result, anyhow};
use app::App;
use clap::Parser;
use cli::{Args, Tool};
use mog_config::Config;

/// Runs the editor.
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up or the app fails.
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(Tool::Plugin { action }) = args.tool {
        return plugin_cli::run(action).await;
    }
    if args.keys {
        let config = Config::load().unwrap_or_default();
        let (keymap, _) = settings::keymap(&config);
        println!("{}", commands::key_table(&keymap));
        return Ok(());
    }
    if args.update {
        match update::update_now().await {
            Ok(message) => println!("{message}"),
            Err(err) => return Err(anyhow!("could not update: {err}")),
        }
        return Ok(());
    }
    if let Some(size) = args.snapshot.clone() {
        let wait = Duration::from_millis(args.snapshot_wait);
        let commands = args.snapshot_run.clone();
        let text = App::new(args).snapshot(&size, wait, &commands).await?;
        println!("{text}");
        return Ok(());
    }
    let mut app = App::new(args);
    let mut tui = terminal::init(app.wants_kitty_keys())?;
    // windows reads keys from the console directly so it never loses chords
    app.set_legacy_keys(!cfg!(windows) && !terminal::keys_enhanced());
    app.restore_session();
    app.start_plugins();
    app.start_updates();
    let result = app.run(&mut tui).await;
    terminal::restore()?;
    result
}
