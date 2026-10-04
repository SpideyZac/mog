//! The `mog` binary, the entry point of the editor.

mod app;
mod terminal;

use anyhow::Result;

use app::App;

/// Runs the editor.
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up or the app fails.
#[tokio::main]
async fn main() -> Result<()> {
    let mut tui = terminal::init()?;
    let result = App::new().run(&mut tui).await;
    terminal::restore()?;
    result
}
