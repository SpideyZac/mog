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
    /// Error counts and markers.
    pub error: Style,
    /// Warning counts and markers.
    pub warning: Style,
    /// The file explorer background and file names.
    pub sidebar: Style,
    /// The folder name at the top of the file explorer.
    pub sidebar_title: Style,
    /// The file in the explorer that is open in the editor.
    pub sidebar_active: Style,
    /// Folder names in the file explorer.
    pub directory: Style,
    /// The line between the file explorer and the editor.
    pub border: Style,
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
            error: Style::new().fg(Color::Rgb(255, 85, 110)),
            warning: Style::new().fg(Color::Rgb(255, 196, 87)),
            sidebar: Style::new().fg(fg).bg(Color::Rgb(27, 22, 39)),
            sidebar_title: Style::new().fg(accent).add_modifier(Modifier::BOLD),
            sidebar_active: Style::new().fg(accent).bg(Color::Rgb(40, 32, 60)),
            directory: Style::new().fg(Color::Rgb(150, 140, 255)),
            border: Style::new().fg(Color::Rgb(48, 40, 68)),
        }
    }
}
