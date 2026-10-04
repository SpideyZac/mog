//! The application state and event loop.

use std::{
    env,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mog_config::Config;
use mog_core::{Command, Editor, Keymap, Outcome};
use mog_lsp::LspEvent;
use mog_plugin::{PluginHost, PluginRequest};
use mog_tui::{Compositor, Context, EditorView, StatusLine, Theme, input};
use ratatui::layout::Rect;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    time::{self, MissedTickBehavior},
};

use crate::{
    ai::Assistant,
    cli::Args,
    clipboard,
    lsp::{self, LanguageServers},
    plugins, settings,
    terminal::Tui,
};

/// The time between animation frames, about 30 per second.
const FRAME_TIME: Duration = Duration::from_millis(33);

/// How many rounds of plugin requests are handled after one command.
const MAX_PLUGIN_ROUNDS: usize = 8;

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
    /// The Lua plugins, or `None` if Lua failed to start.
    plugins: Option<PluginHost>,
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
        let plugins = plugins::start(&mut problems);

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
        let mut app = Self {
            editor,
            keymap,
            compositor,
            theme: Theme::default(),
            lsp,
            lsp_events,
            assistant: Assistant::new(providers),
            plugins,
            screen: Rect::default(),
            quit: false,
        };
        app.emit_document_event("open");
        app.drain_plugin_requests();
        app
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

    /// Runs a command, then anything plugins asked for while it ran.
    fn run_command(&mut self, command: Command) {
        self.execute_command(command);
        self.drain_plugin_requests();
    }

    /// Calls the plugin handlers for `event` with the focused document path.
    fn emit_document_event(&mut self, event: &str) {
        let Some(host) = &self.plugins else {
            return;
        };
        let path = self
            .editor
            .document()
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let problems = host.emit(event, &path);
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
    }

    /// Carries out the requests plugins queued.
    ///
    /// Plugin commands can queue more requests, so this repeats a bounded number of times to
    /// stop a plugin that keeps asking from freezing the editor.
    fn drain_plugin_requests(&mut self) {
        for _ in 0..MAX_PLUGIN_ROUNDS {
            let Some(host) = &self.plugins else {
                return;
            };
            let requests = host.take_requests();
            if requests.is_empty() {
                return;
            }
            for request in requests {
                match request {
                    PluginRequest::Notify(message) => self.editor.set_status(message),
                    PluginRequest::RunCommand(name) => match name.parse() {
                        Ok(command) => self.execute_command(command),
                        Err(err) => self.editor.set_status(err.to_string()),
                    },
                }
            }
        }
    }

    /// Runs a command and acts on its outcome.
    fn execute_command(&mut self, command: Command) {
        let saving = command == Command::Save;
        match self.editor.execute(command) {
            Outcome::Done if saving => self.emit_document_event("save"),
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
            Outcome::Unhandled(Command::Custom(name))
                if self
                    .plugins
                    .as_ref()
                    .is_some_and(|host| host.has_command(&name)) =>
            {
                if let Some(Err(err)) = self.plugins.as_ref().map(|host| host.run_command(&name)) {
                    self.editor.set_status(format!("{name} failed: {err}"));
                }
            }
            Outcome::Unhandled(command) => {
                self.editor
                    .set_status(format!("{command} is not available yet"));
            }
        }
    }
}
