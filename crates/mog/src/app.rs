//! The application state and event loop.

use std::env;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mog_config::Config;
use mog_core::{Command, Editor, Keymap, Outcome};
use mog_lsp::LspEvent;
use mog_tui::{Compositor, Context, EditorView, StatusLine, Theme, input};
use ratatui::layout::Rect;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::{self, MissedTickBehavior};

use crate::ai::Assistant;
use crate::cli::Args;
use crate::clipboard;
use crate::lsp::{self, LanguageServers};
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
    /// The language servers for the project.
    lsp: LanguageServers,
    /// Events coming back from language servers.
    lsp_events: UnboundedReceiver<LspEvent>,
    /// The AI providers and their pending replies.
    assistant: Assistant,
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
        let (providers, ai_problems) = settings::ai_providers(&config);
        problems.extend(ai_problems);

        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        compositor.push(Box::new(settings::flair_layer(&config)));
        compositor.push(Box::new(StatusLine::new()));

        let mut editor = Editor::new(clipboard::open());
        editor.set_options(settings::options(&config));
        if !problems.is_empty() {
            editor.set_status(problems.join("; "));
        }
        let (lsp_sender, lsp_events) = mpsc::unbounded_channel();
        let root = env::current_dir().unwrap_or_default();
        let lsp = LanguageServers::new(config.language_servers(), root, lsp_sender);
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
            lsp,
            lsp_events,
            assistant: Assistant::new(providers),
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
            self.sync_language_servers();
            self.draw(terminal)?;
            let animating = self.compositor.is_animating();
            tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.handle_event(event?),
                    None => break,
                },
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.assistant.reply() => self.editor.set_status(reply),
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

    /// Tells language servers about opened and changed documents.
    fn sync_language_servers(&mut self) {
        let problems = self.lsp.sync(self.editor.documents());
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
    }

    /// Reacts to an event from a language server.
    fn handle_lsp_event(&mut self, event: LspEvent) {
        match event {
            LspEvent::Ready { .. } => {}
            LspEvent::Diagnostics { params, .. } => {
                lsp::apply_diagnostics(&mut self.editor, params);
            }
            LspEvent::Message { server, text } => {
                self.editor.set_status(format!("{server}: {text}"));
            }
            LspEvent::Exited { server } => {
                self.lsp.exited(&server);
                self.editor
                    .set_status(format!("{server} language server stopped"));
            }
        }
    }

    /// Runs a command and acts on its outcome.
    fn run_command(&mut self, command: Command) {
        match self.editor.execute(command) {
            Outcome::Done => {}
            Outcome::Quit => self.quit = true,
            Outcome::Unhandled(Command::Custom(name)) if name == "ai.explain" => {
                let document = self.editor.document();
                let selection = document.selection();
                let message = if selection.is_empty() {
                    "select some code to explain first".to_owned()
                } else {
                    let code = document.text().slice(selection.from()..selection.to());
                    self.assistant.explain(code.to_string())
                };
                self.editor.set_status(message);
            }
            Outcome::Unhandled(command) => {
                self.editor
                    .set_status(format!("{command} is not available yet"));
            }
        }
    }
}
