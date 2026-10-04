//! Colors and styles.

use ratatui::style::{Color, Modifier, Style};

/// The styles used to draw the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The background behind everything.
    pub background: Style,
    /// Plain document text.
    pub text: Style,
    /// Selected text.
    pub selection: Style,
    /// The line the cursor is on.
    pub cursor_line: Style,
    /// Line numbers.
    pub gutter: Style,
    /// The line number of the cursor line.
    pub gutter_active: Style,
    /// The status line background and text.
    pub status: Style,
    /// The highlighted badge at the start of the status line.
    pub status_badge: Style,
    /// Status messages.
    pub status_message: Style,
}

impl Default for Theme {
    /// Returns the built in theme, a dark purple one with loud accents.
    fn default() -> Self {
        let bg = Color::Rgb(22, 18, 32);
        let fg = Color::Rgb(220, 214, 240);
        let accent = Color::Rgb(255, 92, 205);
        let dim = Color::Rgb(92, 84, 120);
        Self {
            background: Style::new().bg(bg),
            text: Style::new().fg(fg),
            selection: Style::new().bg(Color::Rgb(74, 52, 120)),
            cursor_line: Style::new().bg(Color::Rgb(32, 27, 46)),
            gutter: Style::new().fg(dim),
            gutter_active: Style::new().fg(accent),
            status: Style::new().fg(fg).bg(Color::Rgb(40, 32, 60)),
            status_badge: Style::new().fg(bg).bg(accent).add_modifier(Modifier::BOLD),
            status_message: Style::new().fg(Color::Rgb(120, 230, 200)),
        }
    }
}
