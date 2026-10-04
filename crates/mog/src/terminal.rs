//! Terminal setup and teardown.

use std::io::{self, Stdout, stdout};
use std::panic;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// The terminal type the editor draws to.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Puts the terminal into raw mode on the alternate screen.
///
/// Also installs a panic hook that calls [`restore`] so a panic never leaves the terminal broken.
///
/// # Errors
///
/// Returns an error if the terminal cannot be configured.
pub fn init() -> io::Result<Tui> {
    install_panic_hook();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    // some terminals cannot do bracketed paste and typing the paste out still works there
    let _ = execute!(stdout(), EnableBracketedPaste);
    Terminal::new(CrosstermBackend::new(stdout()))
}

/// Restores the terminal to the state it was in before [`init`].
///
/// # Errors
///
/// Returns an error if the terminal cannot be reset.
pub fn restore() -> io::Result<()> {
    let _ = execute!(stdout(), DisableBracketedPaste);
    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Chains a panic hook that restores the terminal before the default hook prints.
fn install_panic_hook() {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // already panicking so there is nothing useful to do with the error
        let _ = restore();
        hook(info);
    }));
}
