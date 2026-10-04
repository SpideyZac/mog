//! The application state and event loop.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mog_config::Config;
use mog_core::{Command, Editor, Keymap, Outcome};
use mog_tui::{Compositor, Context, EditorView, StatusLine, Theme, input};
use ratatui::layout::Rect;
use tokio::time::{self, MissedTickBehavior};

use crate::cli::Args;
use crate::clipboard;
use crate::settings;
use crate::terminal::Tui;

/// The time between animation frames, about 30 per second.
const FRAME_TIME: Duration = Duration::from_millis(33);

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
    /// The screen size at the last draw, used to place mouse events.
    screen: Rect,
    /// Whether the event loop should stop after the current iteration.
    quit: bool,
}

impl App {
    /// Creates a new app, opening the file named in `args` if there is one.
    pub fn new(args: Args) -> Self {
        let mut problems = Vec::new();
        let config = Config::load().unwrap_or_else(|err| {
            problems.push(err.to_string());
            Config::default()
        });
        let (keymap, key_problems) = settings::keymap(&config);
        problems.extend(key_problems);

        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        compositor.push(Box::new(settings::flair_layer(&config)));
        compositor.push(Box::new(StatusLine::new()));

        let mut editor = Editor::new(clipboard::open());
        editor.set_options(settings::options(&config));
        if !problems.is_empty() {
            editor.set_status(problems.join("; "));
        }
        if let Some(path) = args.file
            && let Err(err) = editor.open(&path)
        {
            editor.set_status(format!("could not open {}: {err}", path.display()));
        }
        Self {
            editor,
            keymap,
            compositor,
            theme: Theme::default(),
            screen: Rect::default(),
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
        let mut frames = time::interval(FRAME_TIME);
        frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_tick = Instant::now();
        while !self.quit {
            self.draw(terminal)?;
            let animating = self.compositor.is_animating();
            tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.handle_event(event?),
                    None => break,
                },
                _ = frames.tick(), if animating => {
                    let now = Instant::now();
                    self.compositor.tick(now - last_tick);
                    last_tick = now;
                }
            }
        }
        Ok(())
    }

    /// Draws one frame.
    fn draw(&mut self, terminal: &mut Tui) -> Result<()> {
        terminal.draw(|frame| {
            self.screen = frame.area();
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
            Event::Mouse(mouse) => {
                let mut cx = Context {
                    editor: &mut self.editor,
                    theme: &self.theme,
                };
                self.compositor.handle_mouse(mouse, self.screen, &mut cx);
            }
            Event::Paste(text) => self.run_command(Command::InsertText(text)),
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
