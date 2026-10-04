//! The application state and event loop.

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::widgets::Paragraph;

use crate::terminal::Tui;

/// The running editor.
pub struct App {
    /// Whether the event loop should stop after the current iteration.
    quit: bool,
}

impl App {
    /// Creates a new app.
    pub fn new() -> Self {
        Self { quit: false }
    }

    /// Runs the event loop until the user quits.
    ///
    /// # Errors
    ///
    /// Returns an error if drawing or reading terminal events fails.
    pub async fn run(&mut self, terminal: &mut Tui) -> Result<()> {
        let mut events = EventStream::new();
        while !self.quit {
            terminal.draw(|frame| frame.render_widget(Paragraph::new("mog"), frame.area()))?;
            if let Some(event) = events.next().await {
                self.handle_event(event?);
            }
        }
        Ok(())
    }

    /// Reacts to a single terminal event.
    fn handle_event(&mut self, event: Event) {
        // windows reports releases too and we only care about presses
        if let Event::Key(key) = event
            && key.kind == KeyEventKind::Press
        {
            self.handle_key(key);
        }
    }

    /// Reacts to a key press.
    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            self.quit = true;
        }
    }
}
