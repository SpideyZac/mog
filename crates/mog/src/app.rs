//! The application state and event loop.

use std::{
    env, mem,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::{StreamExt, future};
use mog_config::Config;
use mog_core::{Command, Editor, FileTree, KeyChord, Keymap, Outcome};
use mog_lsp::LspEvent;
use mog_tui::{
    Compositor, Context, EditorView, EventResult, Explorer, Focus, StatusLine, Theme, Ui, input,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver},
    time::{self, MissedTickBehavior},
};

use crate::{
    ai::Assistant,
    cli::Args,
    clipboard,
    git::{self, Git},
    lsp::{self, LanguageServers},
    settings,
    terminal::Tui,
    watch::FolderWatcher,
};

/// The time between animation frames, about 30 per second.
const FRAME_TIME: Duration = Duration::from_millis(33);

/// How often housekeeping like refreshing git runs.
const HOUSEKEEPING_TIME: Duration = Duration::from_secs(1);

/// How often git status is refreshed even when no files changed, to catch commits made elsewhere.
const GIT_REFRESH_TIME: Duration = Duration::from_secs(15);

/// How long a snapshot waits for background work before drawing.
const SNAPSHOT_SETTLE: Duration = Duration::from_millis(800);

/// How many rounds of layer requests are run after one event, so requests that queue more
/// requests cannot freeze the editor.
const MAX_REQUEST_ROUNDS: usize = 8;

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
    /// The state shared with the layers.
    ui: Ui,
    /// The language servers for the project.
    lsp: LanguageServers,
    /// Events coming back from language servers.
    lsp_events: UnboundedReceiver<LspEvent>,
    /// The AI providers and their pending replies.
    assistant: Assistant,
    /// The git state of the project.
    git: Git,
    /// Watches the open folder for changes, if a folder is open.
    watcher: Option<FolderWatcher>,
    /// Whether files changed on disk since the last housekeeping.
    files_changed: bool,
    /// When git status was last refreshed.
    git_refreshed: Instant,
    /// The screen size at the last draw, used to place mouse events.
    screen: Rect,
    /// Whether the event loop should stop after the current iteration.
    quit: bool,
}

impl App {
    /// Creates a new app, opening the file or folder named in `args` if there is one.
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
        let (theme, theme_problem) = settings::theme(&config);
        problems.extend(theme_problem);

        let (file, tree) = match args.path {
            Some(path) if path.is_dir() => match FileTree::new(&path) {
                Ok(tree) => (None, Some(tree)),
                Err(err) => {
                    problems.push(format!("could not open {}: {err}", path.display()));
                    (None, None)
                }
            },
            path => (path, None),
        };
        let root = tree.as_ref().map_or_else(
            || env::current_dir().unwrap_or_default(),
            |tree| tree.root().to_owned(),
        );

        let mut ui = Ui::new(config.clone());
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        if let Some(tree) = tree {
            ui.has_explorer = true;
            ui.explorer_open = config.ui.explorer;
            compositor.push(Box::new(Explorer::new(tree)));
        }
        compositor.push(Box::new(settings::flair_layer(&config)));
        compositor.push(Box::new(StatusLine::new()));

        let mut editor = Editor::new(clipboard::open());
        editor.set_options(settings::options(&config));
        if !problems.is_empty() {
            editor.set_status(problems.join("; "));
        }
        let (lsp_sender, lsp_events) = mpsc::unbounded_channel();
        let watcher = ui.has_explorer.then(|| FolderWatcher::new(&root)).flatten();
        let mut git = Git::new(&root);
        git.refresh();
        let lsp = LanguageServers::new(config.language_servers(), root, lsp_sender);
        if let Some(path) = file
            && let Err(err) = editor.open(&path)
        {
            editor.set_status(format!("could not open {}: {err}", path.display()));
        }
        Self {
            editor,
            keymap,
            compositor,
            theme,
            ui,
            lsp,
            lsp_events,
            assistant: Assistant::new(providers),
            git,
            watcher,
            files_changed: false,
            git_refreshed: Instant::now(),
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
        let mut housekeeping = time::interval(HOUSEKEEPING_TIME);
        housekeeping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_tick = Instant::now();
        while !self.quit {
            self.sync_language_servers();
            self.sync_git();
            self.draw(terminal)?;
            let animating = self.compositor.is_animating();
            tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.handle_event(event?),
                    None => break,
                },
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.assistant.reply() => self.editor.set_status(reply),
                Some(update) = self.git.update() => git::apply(&mut self.ui, update),
                () = changed(self.watcher.as_ref()) => self.files_changed = true,
                _ = housekeeping.tick() => self.housekeeping(),
                _ = frames.tick(), if animating => {
                    let now = Instant::now();
                    self.compositor.tick(now - last_tick);
                    last_tick = now;
                }
            }
            self.run_requests();
        }
        Ok(())
    }

    /// Renders one frame of `size`, like `120x40`, to text after letting background work settle.
    ///
    /// # Errors
    ///
    /// Returns an error if the size cannot be parsed or drawing fails.
    pub async fn snapshot(&mut self, size: &str) -> Result<String> {
        let (width, height) = size
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .ok_or_else(|| anyhow!("snapshot size should look like 120x40"))?;
        let deadline = time::sleep(SNAPSHOT_SETTLE);
        tokio::pin!(deadline);
        loop {
            self.sync_git();
            tokio::select! {
                Some(update) = self.git.update() => git::apply(&mut self.ui, update),
                () = &mut deadline => break,
            }
        }
        self.compositor.tick(SNAPSHOT_SETTLE);
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| {
            self.screen = frame.area();
            let mut cx = Context {
                editor: &mut self.editor,
                theme: &self.theme,
                ui: &mut self.ui,
            };
            self.compositor.render(frame, &mut cx);
        })?;
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..height)
            .map(|y| {
                let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
                row.trim_end().to_owned()
            })
            .collect();
        Ok(rows.join("\n"))
    }

    /// Draws one frame.
    fn draw(&mut self, terminal: &mut Tui) -> Result<()> {
        terminal.draw(|frame| {
            self.screen = frame.area();
            let mut cx = Context {
                editor: &mut self.editor,
                theme: &self.theme,
                ui: &mut self.ui,
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
                if let Some(chord) = input::key_chord(key) {
                    self.handle_key(chord);
                }
            }
            Event::Mouse(mouse) => {
                let mut cx = Context {
                    editor: &mut self.editor,
                    theme: &self.theme,
                    ui: &mut self.ui,
                };
                self.compositor.handle_mouse(mouse, self.screen, &mut cx);
            }
            Event::Paste(text) => self.execute_command(Command::InsertText(text)),
            _ => {}
        }
    }

    /// Offers a key to the layers, then runs its bound command if none of them took it.
    fn handle_key(&mut self, chord: KeyChord) {
        let mut cx = Context {
            editor: &mut self.editor,
            theme: &self.theme,
            ui: &mut self.ui,
        };
        if self.compositor.handle_key(chord, &mut cx) == EventResult::Consumed {
            return;
        }
        if let Some(command) = self.keymap.resolve(&chord) {
            self.execute_command(command);
        }
    }

    /// Runs the commands layers asked for.
    fn run_requests(&mut self) {
        for _ in 0..MAX_REQUEST_ROUNDS {
            if self.ui.requests.is_empty() {
                return;
            }
            for command in mem::take(&mut self.ui.requests) {
                self.execute_command(command);
            }
        }
    }

    /// Asks git about open files and the cursor line.
    fn sync_git(&mut self) {
        for document in self.editor.documents() {
            if let Some(path) = document.path() {
                self.git.ensure_base(path);
            }
        }
        if !self.ui.config.ui.git_blame {
            return;
        }
        let document = self.editor.document();
        if let Some(path) = document.path() {
            let line = document.text().char_to_line(document.selection().head);
            self.git.request_blame(path, line, document.version(), || {
                document.text().to_string()
            });
        }
    }

    /// Runs periodic work like refreshing the explorer and git after files changed.
    fn housekeeping(&mut self) {
        let changed = mem::take(&mut self.files_changed);
        if changed {
            self.ui.refresh_explorer = true;
        }
        if changed || self.git_refreshed.elapsed() >= GIT_REFRESH_TIME {
            self.git.refresh();
            self.git_refreshed = Instant::now();
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
    fn execute_command(&mut self, command: Command) {
        match self.editor.execute(command) {
            Outcome::Done => {}
            Outcome::Quit => self.quit = true,
            Outcome::Unhandled(Command::Custom(name)) => self.execute_custom(&name),
            Outcome::Unhandled(command) => {
                self.editor
                    .set_status(format!("{command} is not available yet"));
            }
        }
    }

    /// Runs an app level command like `explorer.toggle`.
    fn execute_custom(&mut self, name: &str) {
        match name {
            "ai.explain" => {
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
            "explorer.toggle" => {
                if !self.ui.has_explorer {
                    self.editor
                        .set_status("open a folder to get a file explorer: mog <folder>");
                    return;
                }
                self.ui.explorer_open = !self.ui.explorer_open;
                self.ui.focus = if self.ui.explorer_open {
                    Focus::Explorer
                } else {
                    Focus::Editor
                };
            }
            "explorer.focus" => {
                if self.ui.has_explorer {
                    self.ui.explorer_open = true;
                    self.ui.focus = Focus::Explorer;
                }
            }
            _ => self
                .editor
                .set_status(format!("{name} is not available yet")),
        }
    }
}

/// Waits for `watcher` to see a change, or forever when there is no watcher.
async fn changed(watcher: Option<&FolderWatcher>) {
    match watcher {
        Some(watcher) => watcher.changed().await,
        None => future::pending().await,
    }
}
