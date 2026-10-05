//! Terminal setup and teardown.

use std::{
    io::{self, Stdout, stdout},
    panic,
    sync::atomic::{AtomicBool, Ordering},
};

use crossterm::{
    cursor::SetCursorStyle,
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use mog_tui::{CursorShape, CursorStyle};
use ratatui::{Terminal, backend::CrosstermBackend};

/// Whether the kitty keyboard protocol is on, so [`restore`] turns it off again.
static KEYS_ENHANCED: AtomicBool = AtomicBool::new(false);

/// Whether a plugin changed the cursor shape, so [`restore`] puts the usual one back.
static CURSOR_STYLED: AtomicBool = AtomicBool::new(false);

/// The terminal type the editor draws to.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Puts the terminal into raw mode on the alternate screen.
///
/// With `kitty_keys` set, also turns on the kitty keyboard protocol if the terminal has it. Also
/// installs a panic hook that calls [`restore`] so a panic never leaves the terminal broken.
///
/// # Errors
///
/// Returns an error if the terminal cannot be configured.
pub fn init(kitty_keys: bool) -> io::Result<Tui> {
    install_panic_hook();
    enable_raw_mode()?;
    // asking needs raw mode and must happen before anything else reads input
    if kitty_keys && supports_keyboard_enhancement().unwrap_or(false) {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS;
        if execute!(stdout(), PushKeyboardEnhancementFlags(flags)).is_ok() {
            KEYS_ENHANCED.store(true, Ordering::Relaxed);
        }
    }
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
    if KEYS_ENHANCED.swap(false, Ordering::Relaxed) {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    if CURSOR_STYLED.swap(false, Ordering::Relaxed) {
        let _ = execute!(stdout(), SetCursorStyle::DefaultUserShape);
    }
    let _ = execute!(stdout(), DisableBracketedPaste);
    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Returns the terminal command that gives the cursor `style`.
pub fn cursor_style(style: CursorStyle) -> SetCursorStyle {
    if style != CursorStyle::default() {
        CURSOR_STYLED.store(true, Ordering::Relaxed);
    }
    match (style.shape, style.blink) {
        (CursorShape::Default, _) => SetCursorStyle::DefaultUserShape,
        (CursorShape::Block, true) => SetCursorStyle::BlinkingBlock,
        (CursorShape::Block, false) => SetCursorStyle::SteadyBlock,
        (CursorShape::Bar, true) => SetCursorStyle::BlinkingBar,
        (CursorShape::Bar, false) => SetCursorStyle::SteadyBar,
        (CursorShape::Underline, true) => SetCursorStyle::BlinkingUnderScore,
        (CursorShape::Underline, false) => SetCursorStyle::SteadyUnderScore,
    }
}

/// Returns whether the kitty keyboard protocol is on, so every chord reaches mog.
pub fn keys_enhanced() -> bool {
    KEYS_ENHANCED.load(Ordering::Relaxed)
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
