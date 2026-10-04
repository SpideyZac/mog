//! The application state and event loop.

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mog_core::{Command, Editor, Keymap, MemoryClipboard, Outcome};
use mog_tui::{Compositor, Context, EditorView, StatusLine, Theme, input};

use crate::terminal::Tui;

/// The running editor.
pub struct App {
    /// The editing state.
    editor: Editor,
    /// The key bindings.
    keymap: Keymap,
    /// The layers drawn on screen.
    compositor: Compositor,
    /// The active theme.
    theme: Theme,
    /// Whether the event loop should stop after the current iteration.
    quit: bool,
}

impl App {
    /// Creates a new app with an empty document.
    pub fn new() -> Self {
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        compositor.push(Box::new(StatusLine::new()));
        Self {
            editor: Editor::new(Box::new(MemoryClipboard::default())),
            keymap: Keymap::default(),
            compositor,
            theme: Theme::default(),
            quit: false,
        }
    }

    /// Runs the event loop until the user quits.
    ///
    /// # Errors
    ///
    /// Returns an error if drawing or reading terminal events fails.
    pub async fn run(&mut self, terminal: &mut Tui) -> Result<()> {
        let mut events = EventStream::new();
        while !self.quit {
            self.draw(terminal)?;
            if let Some(event) = events.next().await {
                self.handle_event(event?);
            }
        }
        Ok(())
    }

    /// Draws one frame.
    fn draw(&mut self, terminal: &mut Tui) -> Result<()> {
        terminal.draw(|frame| {
            let mut cx = Context {
                editor: &mut self.editor,
                theme: &self.theme,
            };
            self.compositor.render(frame, &mut cx);
        })?;
        Ok(())
    }

    /// Reacts to a single terminal event.
    fn handle_event(&mut self, event: Event) {
        match event {
            // windows reports releases too and we only care about presses
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let command = input::key_chord(key).and_then(|chord| self.keymap.resolve(&chord));
                if let Some(command) = command {
                    self.run_command(command);
                }
            }
            _ => {}
        }
    }

    /// Runs a command and acts on its outcome.
    fn run_command(&mut self, command: Command) {
        match self.editor.execute(command) {
            Outcome::Done => {}
            Outcome::Quit => self.quit = true,
            Outcome::Unhandled(command) => {
                self.editor
                    .set_status(format!("{command} is not available yet"));
            }
        }
    }
}
