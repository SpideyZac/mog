//! The application state and event loop.

use std::{
    collections::{HashMap, HashSet},
    env, fs, mem,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{Event, EventStream, KeyEventKind, MouseEventKind},
    execute, queue,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use futures::{StreamExt, future};
use lsp_types::PublishDiagnosticsParams;
use mog_ai::DeviceCode;
use mog_audio::Audio;
use mog_config::{Config, DebugConfig, ProjectFile, project_config_path};
use mog_core::{
    Command, Editor, FileTree, KeyChord, Keymap, Outcome, Severity, project_search::ProjectResults,
};
use mog_dap::Frame;
use mog_flair::{GraphView, builtin::screensaver};
use mog_lsp::{LspEvent, convert, features::CodeAction};
use mog_term::TerminalPanel;
use mog_tui::{
    Annotations, ChatPanel, CompletionMenu, Compositor, Context, ContextMenu, CursorStyle,
    DebugPanel, EditorView, EventResult, Explorer, Focus, GitPanel, Minimap, OutputPanel, Overlay,
    PluginCanvases, PluginPanels, PluginWidgets, Popups, ProjectSearchPanel, PromptKind,
    ReleaseNotesPopup, SearchBar, SettingsPanel, StatusLine, Tabs, Theme, ThemeEditor, Toasts, Ui,
    UiEvent, input,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::{self, MissedTickBehavior},
};

use crate::{
    ai::{Assistant, Exclusions},
    cli::Args,
    clipboard, commands,
    debug::{DebugUpdate, Debugger},
    discord::{Presence, Status},
    git::Git,
    lsp::{self, LanguageServers, LspReply},
    plugins::{self, Plugins},
    session::{Session, State, Swap},
    settings::{self, ProjectStatus},
    tasks::{Runner, Task},
    terminal::{self, Tui},
    update::{Release, Updater},
    watch::FolderWatcher,
};

mod ambience;
mod assistant;
mod config;
mod custom;
mod debugging;
mod language;
mod persistence;
mod plugin_host;
mod project_find;
mod prompts;
mod source_control;
mod task_runner;
mod updates;

use plugin_host::PluginState;

/// The time between animation frames, about 30 per second.
const FRAME_TIME: Duration = Duration::from_millis(33);

/// The time between animation frames once nobody has touched mog for [`IDLE_AFTER`].
const IDLE_FRAME_TIME: Duration = Duration::from_millis(200);

/// The time between frames of the screensaver, which is the only thing on screen then.
const SCREENSAVER_FRAME_TIME: Duration = Duration::from_millis(66);

/// How long without input before ambient animations slow down to save power.
const IDLE_AFTER: Duration = Duration::from_secs(10);

/// How often housekeeping like refreshing git runs.
const HOUSEKEEPING_TIME: Duration = Duration::from_secs(1);

/// How often git status is refreshed even when no files changed, to catch commits made elsewhere.
const GIT_REFRESH_TIME: Duration = Duration::from_secs(15);

/// How long a plugin provider may take before mog stops waiting for it.
const PROVIDER_TIMEOUT: Duration = Duration::from_millis(1500);

/// How long a snapshot waits for background work before drawing.
const SNAPSHOT_SETTLE: Duration = Duration::from_millis(800);

/// The most matches a project search keeps.
const PROJECT_LIMIT: usize = 2000;

/// How many chars before the cursor a ghost suggestion gets to see.
const GHOST_CONTEXT_BEFORE: usize = 4000;

/// How many chars after the cursor a ghost suggestion gets to see.
const GHOST_CONTEXT_AFTER: usize = 1000;

/// What the chat says when no AI is set up.
const NO_AI: &str = "no ai provider is enabled for chat. add [ai.claude] enabled = true to the \
config and put your key in ANTHROPIC_API_KEY, then restart mog.";

/// What a new config file starts with when it is opened from the editor.
const NEW_CONFIG: &str = "# mog config. saving this file reloads it.
# every option is listed in examples/config.toml in the mog repo.

# [ui]
# theme = \"mog\"

# [keys]
# \"alt+w\" = \"close_tab\"

# settings for a language server, here running clippy instead of check on save
# [lsp.rust.settings]
# check.command = \"clippy\"
";

/// What a new project config starts with when it is opened from the editor.
const NEW_PROJECT_CONFIG: &str =
    "# language server settings just for this project. they change only what they set
# in your global config, settings tables merge key by key.
# mog asks before using this file, and again whenever it changes.

# [lsp.rust.settings]
# check.command = \"clippy\"

# [lsp.python]
# command = \"pylsp\"

# [lsp.go]
# enabled = false
";

/// How many rounds of layer requests are run after one event, so requests that queue more
/// requests cannot freeze the editor.
const MAX_REQUEST_ROUNDS: usize = 8;

/// What a project search found or why it could not run, with the generation it was for.
type ProjectReply = (u64, Result<ProjectResults, String>);

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
    /// Where feature requests send their answers.
    lsp_sender: UnboundedSender<LspReply>,
    /// Answers to feature requests.
    lsp_replies: UnboundedReceiver<LspReply>,
    /// The id of the newest completion request, so older answers are ignored.
    completion_request: u64,
    /// When a document was last edited, so language servers wait for typing to pause.
    last_edit: Instant,
    /// Diagnostics that came in while typing, shown once it pauses.
    pending_diagnostics: Vec<PublishDiagnosticsParams>,
    /// The code actions offered last, picked by index from the menu.
    code_actions: Vec<CodeAction>,
    /// The file and version inlay hints and semantic tokens were last asked for.
    marks_requested: Option<(PathBuf, u64)>,
    /// The AI providers and their pending replies.
    assistant: Assistant,
    /// A Copilot sign in waiting for the user to confirm the code popup.
    copilot_code: Option<DeviceCode>,
    /// The git state of the project.
    git: Git,
    /// Watches the open folder for changes, if a folder is open.
    watcher: Option<FolderWatcher>,
    /// Whether files changed on disk since the last housekeeping.
    files_changed: bool,
    /// When git status was last refreshed.
    git_refreshed: Instant,
    /// The focused file at the last check, to notice switches.
    last_focus: Option<PathBuf>,
    /// The error and warning counts of the focused file at the last check.
    last_problems: (usize, usize),
    /// The sound player, started the first time sound is turned on.
    audio: Option<Audio>,
    /// The error count the sounds last reacted to.
    sound_errors: usize,
    /// The Discord status connection, while it is turned on.
    discord: Option<Presence>,
    /// The status last sent to Discord.
    discord_status: Option<Status>,
    /// The screen size at the last draw, used to place mouse events.
    screen: Rect,
    /// The cursor look last sent to the terminal.
    cursor_style: CursorStyle,
    /// When the last key or mouse event came in, to know when the screensaver is up.
    last_input: Instant,
    /// Finds and installs new releases.
    updater: Updater,
    /// A newer release that is not installed yet.
    update: Option<Release>,
    /// A release installed while running, used once mog restarts.
    installed: Option<Release>,
    /// A project config waiting for the user to trust it.
    untrusted_project: Option<ProjectFile>,
    /// Where project searches send what they found, with the generation they were for.
    project_sender: UnboundedSender<ProjectReply>,
    /// What project searches found.
    project_results: UnboundedReceiver<ProjectReply>,
    /// The generation of the newest project search, so older ones stop early.
    project_generation: Arc<AtomicU64>,
    /// Where sessions, unsaved work and undo history are kept, if anywhere.
    state: Option<State>,
    /// Whether the open files are remembered for next time.
    session_enabled: bool,
    /// The session saved last, to skip writing the same one again.
    last_session: Option<Session>,
    /// The swap files this mog wrote, by document key, with the version they hold.
    swaps: HashMap<String, (u64, PathBuf)>,
    /// Files whose saved undo history was already looked for.
    undo_checked: HashSet<PathBuf>,
    /// Unsaved work from a mog that crashed, waiting for the user to say what to do with it.
    recoverable: Vec<(PathBuf, Swap)>,
    /// Runs tasks like build and test.
    runner: Runner,
    /// The tasks offered in the picker.
    task_list: Vec<Task>,
    /// The task that ran last, to run again.
    last_task: Option<Task>,
    /// Runs debugging sessions.
    debugger: Debugger,
    /// A debugger waiting for its build task to finish, with its name.
    debug_after_task: Option<(String, DebugConfig)>,
    /// The call stack of the stopped program, innermost first.
    debug_frames: Vec<Frame>,
    /// The running plugins.
    plugins: Plugins,
    /// What the app remembers about plugins between frames.
    plugin_state: PluginState,
    /// Whether the last chat message is a reply still streaming in.
    chat_streaming: bool,
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
        Self::with_config(args, config, problems, State::open())
    }

    /// Creates a new app from `config`, showing `problems` found while loading it and keeping
    /// sessions and unsaved work in `state`.
    fn with_config(
        args: Args,
        mut config: Config,
        mut problems: Vec<String>,
        state: Option<State>,
    ) -> Self {
        let (keymap, key_problems) = settings::keymap(&config);
        problems.extend(key_problems);
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
        let project = settings::apply_project(&mut config, &root);
        let (providers, ai_problems) = settings::ai_providers(&config, &root);
        let exclusions = Exclusions::new(&root, &config.ai.exclude);
        problems.extend(ai_problems);

        let mut ui = Ui::new(config.clone());
        ui.commands = commands::palette(&keymap);
        ui.bindings = commands::bindings(&keymap);
        ui.root.clone_from(&root);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        compositor.push(Box::new(EditorView::side()));
        compositor.push(Box::new(Tabs::new()));
        compositor.push(Box::new(ChatPanel::new()));
        compositor.push(Box::new(DebugPanel::new()));
        compositor.push(Box::new(Minimap::new()));
        compositor.push(Box::new(TerminalPanel::new()));
        compositor.push(Box::new(SearchBar::new()));
        if let Some(tree) = tree {
            ui.has_explorer = true;
            ui.explorer_open = config.ui.explorer;
            compositor.push(Box::new(Explorer::new(tree)));
        }
        let flair = settings::flair_layer(&config);
        ui.flairs = flair.describe();
        ui.explorer_footer = flair.sidebar_height();
        compositor.push(Box::new(flair));
        compositor.push(Box::new(PluginCanvases::new()));
        compositor.push(Box::new(PluginWidgets::new()));
        compositor.push(Box::new(PluginPanels::new()));
        compositor.push(Box::new(Toasts::new()));
        compositor.push(Box::new(StatusLine::new()));
        compositor.push(Box::new(CompletionMenu::new()));
        compositor.push(Box::new(Popups::new()));
        compositor.push(Box::new(SettingsPanel::new()));
        compositor.push(Box::new(ThemeEditor::new()));
        compositor.push(Box::new(ReleaseNotesPopup::new()));
        compositor.push(Box::new(ProjectSearchPanel::new()));
        compositor.push(Box::new(GraphView::new()));
        compositor.push(Box::new(GitPanel::new()));
        compositor.push(Box::new(OutputPanel::new()));
        compositor.push(Box::new(ContextMenu::new()));
        compositor.push(Box::new(Annotations::new()));

        let mut editor = Editor::new(clipboard::open());
        editor.set_options(settings::options(&config));
        if !problems.is_empty() {
            editor.set_status(problems.join("; "));
        }
        let (lsp_sender, lsp_events) = mpsc::unbounded_channel();
        let watcher = ui.has_explorer.then(|| FolderWatcher::new(&root)).flatten();
        let mut git = Git::new(&root);
        git.refresh();
        let plugin_root = root.clone();
        let lsp = LanguageServers::new(config.language_servers(), root, lsp_sender);
        if let Some(path) = file
            && let Err(err) = editor.open(&path)
        {
            editor.set_status(format!("could not open {}: {err}", path.display()));
        }
        let startup = args.run;
        let (reply_sender, lsp_replies) = mpsc::unbounded_channel();
        let (project_sender, project_results) = mpsc::unbounded_channel();
        let mut app = Self {
            editor,
            keymap,
            compositor,
            theme,
            ui,
            lsp,
            lsp_events,
            lsp_sender: reply_sender,
            lsp_replies,
            completion_request: 0,
            last_edit: Instant::now(),
            pending_diagnostics: Vec::new(),
            code_actions: Vec::new(),
            marks_requested: None,
            assistant: Assistant::new(providers, exclusions),
            copilot_code: None,
            git,
            watcher,
            files_changed: false,
            git_refreshed: Instant::now(),
            last_focus: None,
            last_problems: (0, 0),
            audio: None,
            sound_errors: 0,
            discord: None,
            discord_status: None,
            screen: Rect::default(),
            cursor_style: CursorStyle::default(),
            last_input: Instant::now(),
            updater: Updater::new(),
            update: None,
            installed: None,
            untrusted_project: None,
            project_sender,
            project_results,
            project_generation: Arc::new(AtomicU64::new(0)),
            state,
            session_enabled: false,
            last_session: None,
            swaps: HashMap::new(),
            undo_checked: HashSet::new(),
            recoverable: Vec::new(),
            runner: Runner::new(),
            task_list: Vec::new(),
            last_task: None,
            debugger: Debugger::new(),
            debug_after_task: None,
            debug_frames: Vec::new(),
            plugins: Plugins::new(&plugin_root, plugins::plugin_dir()),
            plugin_state: PluginState::default(),
            chat_streaming: false,
            quit: false,
        };
        for name in startup {
            match name.parse() {
                Ok(command) => app.execute_command(command),
                Err(err) => app.editor.set_status(format!("--run: {err}")),
            }
        }
        app.run_requests();
        app.apply_audio_settings();
        app.apply_discord_settings();
        app.project_status(project);
        app
    }

    /// Reports a project config that is broken, or asks whether to trust a new one.
    fn project_status(&mut self, status: ProjectStatus) {
        match status {
            ProjectStatus::Missing | ProjectStatus::Applied => self.untrusted_project = None,
            ProjectStatus::Broken(err) => self.editor.set_status(format!("project config: {err}")),
            ProjectStatus::Untrusted(file) => {
                self.untrusted_project = Some(file);
                self.ui.ask(
                    PromptKind::TrustProject,
                    "trust this project's .mog/config.toml?",
                    "",
                    "it can choose programs to run. y trusts it until it changes",
                );
            }
        }
    }

    /// Returns whether the kitty keyboard protocol should be turned on if the terminal has it.
    pub fn wants_kitty_keys(&self) -> bool {
        self.ui.config.editor.kitty_keyboard
    }

    /// Tells the app whether the terminal sends keys the old way, which loses some chords.
    pub fn set_legacy_keys(&mut self, legacy: bool) {
        self.ui.legacy_keys = legacy;
    }

    /// Runs the event loop until the user quits.
    ///
    /// # Errors
    ///
    /// Returns an error if drawing or reading terminal events fails.
    pub async fn run(&mut self, terminal: &mut Tui) -> Result<()> {
        let mut events = EventStream::new();
        let mut housekeeping = time::interval(HOUSEKEEPING_TIME);
        housekeeping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_tick = Instant::now();
        while !self.quit {
            let idle_at = self.idle_at();
            let typing = Instant::now() < idle_at;
            if !typing {
                self.sync_language_servers();
                for params in mem::take(&mut self.pending_diagnostics) {
                    let path = convert::uri_to_path(&params.uri);
                    lsp::apply_diagnostics(&mut self.editor, params);
                    if let Some(path) = path {
                        self.plugin_diagnostics_changed(&path);
                    }
                }
                self.request_marks();
                self.offer_install();
            }
            self.sync_signature();
            self.sync_git();
            self.watch_focus();
            self.sync_plugin_events(typing);
            self.sync_plugin_ui();
            self.sync_breakpoints();
            self.restore_undo();
            self.play_sounds();
            self.ui.idle = self.last_input.elapsed() >= IDLE_AFTER;
            self.draw(terminal)?;
            let animating = self.compositor.is_animating();
            let next_timer = self.next_plugin_timer();
            let next_frame = last_tick + self.frame_time();
            tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.handle_event(event?),
                    None => break,
                },
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.lsp_replies.recv() => self.handle_lsp_reply(reply),
                Some(reply) = self.assistant.reply() => self.handle_ai_reply(reply),
                Some(update) = self.git.update() => self.handle_git(update),
                Some(event) = self.updater.event() => self.handle_update(event),
                Some(event) = self.runner.event() => self.handle_task(event),
                Some(update) = self.plugins.update() => self.handle_plugin(update),
                Some(update) = self.debugger.update() => match update {
                    DebugUpdate::Event(event) => self.handle_debug_event(event),
                    DebugUpdate::Reply(reply) => self.handle_debug_reply(reply),
                },
                Some((generation, results)) = self.project_results.recv() => {
                    self.show_project_results(generation, results);
                }
                () = changed(self.watcher.as_ref()) => self.files_changed = true,
                () = time::sleep_until(idle_at.into()), if typing => {}
                () = time::sleep_until(next_timer.unwrap_or(idle_at).into()), if next_timer.is_some() => {}
                _ = housekeeping.tick() => self.housekeeping(),
                () = time::sleep_until(next_frame.into()), if animating => {
                    let now = Instant::now();
                    self.compositor.tick(now - last_tick);
                    last_tick = now;
                }
            }
            self.run_requests();
        }
        self.debugger.stop();
        self.shut_down();
        Ok(())
    }

    /// Renders one frame of `size`, like `120x40`, to text after letting background work settle.
    ///
    /// # Errors
    ///
    /// Returns an error if the size cannot be parsed or drawing fails.
    pub async fn snapshot(
        &mut self,
        size: &str,
        wait: Duration,
        commands: &[String],
    ) -> Result<String> {
        let (width, height) = size
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .ok_or_else(|| anyhow!("snapshot size should look like 120x40"))?;
        self.settle(wait).await;
        if !commands.is_empty() {
            for name in commands {
                if let Ok(command) = name.parse() {
                    self.execute_command(command);
                }
            }
            self.run_requests();
            self.settle(wait).await;
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
            let _ = self.compositor.render(frame, &mut cx);
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

    /// Lets background work like git and language servers run for `wait`.
    async fn settle(&mut self, wait: Duration) {
        let deadline = time::sleep(wait);
        tokio::pin!(deadline);
        loop {
            self.sync_language_servers();
            self.request_marks();
            self.sync_git();
            tokio::select! {
                Some(update) = self.git.update() => self.handle_git(update),
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.lsp_replies.recv() => self.handle_lsp_reply(reply),
                Some((generation, results)) = self.project_results.recv() => {
                    self.show_project_results(generation, results);
                }
                Some(reply) = self.assistant.reply() => self.handle_ai_reply(reply),
                () = &mut deadline => break,
            }
        }
        self.watch_focus();
    }

    /// Draws one frame.
    ///
    /// The cursor stays hidden while cells are written and is only shown once it is in place,
    /// otherwise it visibly jumps to whatever was drawn last.
    fn draw(&mut self, terminal: &mut Tui) -> Result<()> {
        self.sync_theme_draft();
        queue!(terminal.backend_mut(), BeginSynchronizedUpdate, Hide)?;
        let mut cursor = None;
        terminal.draw(|frame| {
            self.screen = frame.area();
            let mut cx = Context {
                editor: &mut self.editor,
                theme: &self.theme,
                ui: &mut self.ui,
            };
            cursor = self.compositor.render(frame, &mut cx);
        })?;
        let backend = terminal.backend_mut();
        if self.ui.cursor_style != self.cursor_style {
            self.cursor_style = self.ui.cursor_style;
            queue!(backend, terminal::cursor_style(self.cursor_style))?;
        }
        if let Some(cursor) = cursor {
            queue!(backend, MoveTo(cursor.x, cursor.y), Show)?;
        }
        execute!(backend, EndSynchronizedUpdate)?;
        Ok(())
    }

    /// Returns how long to wait between animation frames, slower once mog sits idle.
    fn frame_time(&self) -> Duration {
        if self.screensaver_showing() {
            SCREENSAVER_FRAME_TIME
        } else if self.ui.idle {
            IDLE_FRAME_TIME
        } else {
            FRAME_TIME
        }
    }

    /// Returns `true` while the matrix screensaver covers the screen.
    fn screensaver_showing(&self) -> bool {
        let config = &self.ui.config;
        let flair = &config.flair;
        flair.enabled
            && !config.ui.serious
            && !config.ui.reduced_motion
            && !flair.disabled.iter().any(|id| id == "screensaver")
            && self.last_input.elapsed() >= screensaver::IDLE_TIME
    }

    /// Reacts to a single terminal event.
    fn handle_event(&mut self, event: Event) {
        if matches!(event, Event::Key(_) | Event::Mouse(_) | Event::Paste(_)) {
            let waking = self.screensaver_showing();
            self.last_input = Instant::now();
            self.ui.events.push(UiEvent::Activity);
            // the key that wakes the screensaver should not also type into the file
            let pressed = match &event {
                Event::Key(key) => key.kind != KeyEventKind::Release,
                Event::Mouse(mouse) => !matches!(mouse.kind, MouseEventKind::Moved),
                _ => true,
            };
            if waking && pressed {
                return;
            }
        }
        match event {
            // windows reports releases too and we only care about presses
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let (alt_gr, legacy) = (self.ui.config.editor.alt_gr, self.ui.legacy_keys);
                if let Some(chord) = input::key_chord(key, alt_gr, legacy) {
                    self.handle_key(chord);
                }
            }
            Event::Mouse(mouse) => {
                self.ui.events.push(UiEvent::Activity);
                let mut cx = Context {
                    editor: &mut self.editor,
                    theme: &self.theme,
                    ui: &mut self.ui,
                };
                self.compositor.handle_mouse(mouse, self.screen, &mut cx);
            }
            Event::Paste(text) if self.ui.focus == Focus::Terminal && self.ui.terminal_open => {
                self.ui.terminal_input.extend_from_slice(text.as_bytes());
            }
            Event::Paste(text) => self.execute_command(Command::InsertText(text)),
            _ => {}
        }
    }

    /// Offers a key to the plugins that take keys, then to the editor.
    fn handle_key(&mut self, chord: KeyChord) {
        self.ui.events.push(UiEvent::Activity);
        if !self.plugin_takes_key(chord) {
            self.editor_key(chord);
        }
    }

    /// Offers a key to the layers, then runs its bound command if none of them took it.
    fn editor_key(&mut self, chord: KeyChord) {
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
        self.apply_setting_changes();
        for _ in 0..MAX_REQUEST_ROUNDS {
            if self.ui.requests.is_empty() {
                return;
            }
            for command in mem::take(&mut self.ui.requests) {
                self.execute_command(command);
            }
        }
    }

    /// Notices a new focused file or changed problem counts and tells the flair about it.
    fn watch_focus(&mut self) {
        let path = self.editor.document().path().map(ToOwned::to_owned);
        if path != self.last_focus {
            self.plugin_focus_changed(path.as_deref());
            self.last_focus = path;
            self.ui.events.push(UiEvent::Opened);
            if self.editor.document().is_large() {
                self.editor.set_status(
                    "big file, so syntax colors, git, language servers and crash recovery are off \
                     for it",
                );
            }
        }
        let document = self.editor.document();
        let count = |severity| {
            document
                .diagnostics()
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        let problems = (count(Severity::Error), count(Severity::Warning));
        if problems != self.last_problems {
            self.last_problems = problems;
            self.ui.events.push(UiEvent::Diagnostics {
                errors: problems.0,
                warnings: problems.1,
            });
        }
        self.update_presence();
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
        self.write_swaps();
        self.save_session();
    }

    /// Reacts to the focused document being saved.
    fn saved(&mut self) {
        self.ui.events.push(UiEvent::Saved);
        self.save_undo();
        // a save as needs the new path opened on the server before it hears about the save
        self.sync_language_servers();
        let Some(path) = self.editor.document().path().map(ToOwned::to_owned) else {
            return;
        };
        self.lsp.saved(&path);
        self.plugin_saved(&path);
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if Self::is_config(&path)
            || canonical(&path) == canonical(&project_config_path(&self.ui.root))
        {
            self.reload_config();
        }
    }

    /// Returns when typing will have paused long enough for language servers to check the file.
    fn idle_at(&self) -> Instant {
        self.last_edit + Duration::from_millis(self.ui.config.editor.diagnostics_delay)
    }

    /// Runs a command and acts on its outcome.
    fn execute_command(&mut self, command: Command) {
        let version = self.editor.document().version();
        let event = match &command {
            Command::InsertChar(ch) => Some(UiEvent::Typed(*ch)),
            Command::InsertNewline => Some(UiEvent::Typed('\n')),
            Command::DeleteBackward | Command::DeleteForward | Command::DeleteWordBackward => {
                Some(UiEvent::Deleted)
            }
            _ => None,
        };
        if command == Command::Save && self.editor.document().path().is_none() {
            self.ask_save_as();
            return;
        }
        if command == Command::Save && self.before_save() {
            self.editor
                .set_status("letting plugins tidy up before saving...");
            return;
        }
        let saving = command == Command::Save;
        let outcome = self.editor.execute(command);
        let changed = self.editor.document().version() != version;
        if let Some(event) = event.filter(|_| changed) {
            self.ui.events.push(event);
        }
        if saving && !self.editor.document().is_modified() {
            self.saved();
        }
        if changed {
            self.last_edit = Instant::now();
            self.ui.hover = None;
            self.ui.ghost = None;
            self.assistant.cancel_suggestion();
            if let Some(UiEvent::Typed(ch)) = self.ui.events.last().cloned() {
                self.auto_complete(ch);
                self.suggest_ghost(false);
                match ch {
                    '(' | ',' => self.request_signature(),
                    ')' => self.ui.signature = None,
                    _ => {}
                }
            }
        }
        match outcome {
            Outcome::Done => {}
            Outcome::Quit => self.quit = true,
            Outcome::Unhandled(Command::Custom(name)) => self.execute_custom(&name),
            Outcome::Unhandled(Command::CommandPalette) => self.ui.open(Overlay::Palette),
            Outcome::Unhandled(command) => {
                self.editor
                    .set_status(format!("{command} is not available yet"));
            }
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

#[cfg(test)]
/// Scripted tests that drive the whole app through terminal events.
mod tests {
    use std::{
        collections::BTreeMap,
        env, fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use clap::Parser;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use mog_config::{Config, DebugConfig, PluginConfig, TaskConfig, ThemeConfig};
    use mog_core::{Command, Key, KeyChord};
    use mog_plugin::{PluginEvent, parse_actions};
    use mog_tui::{
        Context, CursorShape, PanelEvent, PluginSegment, PromptKind, Side,
        popups::PLUGIN_PICKED_COMMAND,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use serde_json::{Value, json, to_string};
    use tokio::time;

    use super::{App, FRAME_TIME, IDLE_FRAME_TIME, Overlay};
    use crate::{
        ai::call_tool,
        cli::Args,
        debug::DebugUpdate,
        plugins::PluginUpdate,
        session::{State, Swap},
        tasks::TaskEvent,
    };

    /// Counts temp folders so parallel tests never share one.
    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    /// Makes an empty temp folder for one test.
    fn temp_dir() -> PathBuf {
        let index = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("mog-app-test-{}-{index}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Starts the app on `path` with a quiet config, keeping sessions in `state`.
    fn start_with_state(path: &Path, state: State) -> App {
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.ui.git_blame = false;
        let args = Args::parse_from([Path::new("mog"), path]);
        App::with_config(args, config, Vec::new(), Some(state))
    }

    /// Starts the app on `path` with a quiet config that has no flair, sound or updates.
    fn start(path: &Path) -> App {
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.ui.git_blame = false;
        let args = Args::parse_from([Path::new("mog"), path]);
        App::with_config(args, config, Vec::new(), None)
    }

    /// Returns the terminal event for pressing `chord`, like `ctrl+s` or `x`.
    fn press(chord: &str) -> Event {
        let chord: KeyChord = chord.parse().expect("valid chord");
        let code = match chord.key {
            Key::Char(ch) if chord.mods.shift && !chord.mods.ctrl => {
                KeyCode::Char(ch.to_ascii_uppercase())
            }
            Key::Char(ch) => KeyCode::Char(ch),
            Key::Enter => KeyCode::Enter,
            Key::Backspace => KeyCode::Backspace,
            Key::Esc => KeyCode::Esc,
            Key::Tab => KeyCode::Tab,
            Key::Up => KeyCode::Up,
            Key::Down => KeyCode::Down,
            Key::Home => KeyCode::Home,
            Key::End => KeyCode::End,
            other => panic!("press does not know {other:?}"),
        };
        let mut mods = KeyModifiers::empty();
        mods.set(KeyModifiers::CONTROL, chord.mods.ctrl);
        mods.set(KeyModifiers::ALT, chord.mods.alt);
        mods.set(KeyModifiers::SHIFT, chord.mods.shift);
        Event::Key(KeyEvent::new(code, mods))
    }

    /// Feeds `text` to `app` one key at a time, with `\n` pressing enter.
    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            let event = match ch {
                '\n' => press("enter"),
                ch => Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::empty())),
            };
            app.handle_event(event);
            app.run_requests();
        }
    }

    /// Opening a file, typing, saving and quitting leaves the edit on disk.
    #[tokio::test]
    async fn open_edit_save_quit() {
        let dir = temp_dir();
        let path = dir.join("notes.txt");
        fs::write(&path, "world\n").expect("write");
        let mut app = start(&path);
        assert_eq!(app.editor.document().path(), Some(path.as_path()));
        type_text(&mut app, "hello\n");
        app.handle_event(press("ctrl+s"));
        assert_eq!(fs::read_to_string(&path).expect("read"), "hello\nworld\n");
        app.handle_event(press("ctrl+q"));
        assert!(app.quit);
        let _ = fs::remove_dir_all(dir);
    }

    /// Quitting with unsaved changes asks for a second press first.
    #[tokio::test]
    async fn quit_guards_unsaved_changes() {
        let dir = temp_dir();
        let path = dir.join("draft.txt");
        fs::write(&path, "").expect("write");
        let mut app = start(&path);
        type_text(&mut app, "unsaved");
        app.handle_event(press("ctrl+q"));
        assert!(!app.quit, "the first press should only warn");
        app.handle_event(press("ctrl+q"));
        assert!(app.quit);
        assert_eq!(fs::read_to_string(&path).expect("read"), "");
        let _ = fs::remove_dir_all(dir);
    }

    /// Breakpoints follow edits and the debugger hears where they went.
    #[tokio::test]
    async fn breakpoints_follow_edits() {
        let dir = temp_dir();
        let path = dir.join("bp.txt");
        fs::write(&path, "one\ntwo\n").expect("write");
        let mut app = start(&path);
        let second = app.editor.document().text().line_to_char(1);
        app.editor.select(second, second);
        app.execute_command("debug.toggle_breakpoint".parse().expect("command"));
        app.handle_event(press("ctrl+home"));
        type_text(&mut app, "zero\n");
        app.sync_breakpoints();
        let lines: Vec<usize> = app.ui.breakpoints[&path].iter().copied().collect();
        assert_eq!(lines, [2]);
        let _ = fs::remove_dir_all(dir);
    }

    /// Reopening a folder brings back the open file and its cursor.
    #[tokio::test]
    async fn restores_the_session() {
        let dir = temp_dir();
        let project = dir.join("project");
        fs::create_dir_all(&project).expect("project");
        let file = project.join("main.txt");
        fs::write(&file, "one\ntwo\n").expect("write");
        let state = State::at(dir.join("state"));
        let mut app = start_with_state(&project, state.clone());
        app.restore_session();
        app.editor.open(&file).expect("open");
        app.editor.select(4, 6);
        app.shut_down();
        let mut again = start_with_state(&project, state);
        again.restore_session();
        assert_eq!(again.editor.document().path(), Some(file.as_path()));
        let selection = again.editor.document().selection();
        assert_eq!((selection.anchor, selection.head), (4, 6));
        let _ = fs::remove_dir_all(dir);
    }

    /// Unsaved work left by a mog that crashed is offered back and restored.
    #[tokio::test]
    async fn recovers_unsaved_work() {
        let dir = temp_dir();
        let project = dir.join("project");
        fs::create_dir_all(&project).expect("project");
        let file = project.join("draft.txt");
        fs::write(&file, "saved\n").expect("write");
        let state = State::at(dir.join("state"));
        let swap = Swap {
            path: Some(file.clone()),
            root: project.clone(),
            // no process has this id so the swap counts as left behind
            pid: u32::MAX,
            started: 0,
            text: "saved\nand more\n".into(),
        };
        let swap_file = dir.join("state").join("swap").join("left.json");
        fs::create_dir_all(swap_file.parent().expect("folder")).expect("swap dir");
        fs::write(&swap_file, to_string(&swap).expect("json")).expect("swap");
        let mut app = start_with_state(&project, state);
        app.restore_session();
        assert_eq!(
            app.ui.prompt.as_ref().map(|prompt| prompt.kind.clone()),
            Some(PromptKind::RecoverSwaps)
        );
        type_text(&mut app, "y");
        app.handle_event(press("enter"));
        app.run_requests();
        let document = app.editor.document();
        assert_eq!(document.text().to_string(), "saved\nand more\n");
        assert!(document.is_modified());
        assert!(!swap_file.exists());
        let _ = fs::remove_dir_all(dir);
    }

    /// Prints what one animation frame costs with every flair on, to check idle cpu use.
    #[tokio::test]
    #[ignore = "a timing, run by hand with --nocapture"]
    async fn frame_cost() {
        let dir = temp_dir();
        let path = dir.join("frames.rs");
        fs::write(&path, "fn main() {\n    println!(\"hi\");\n}\n".repeat(200)).expect("write");
        let mut config = Config::default();
        config.updates.check = false;
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.editor.open(&path).expect("open");
        let mut terminal = Terminal::new(TestBackend::new(160, 50)).expect("terminal");
        let frames = 300;
        let start = Instant::now();
        for _ in 0..frames {
            app.compositor.tick(FRAME_TIME);
            terminal
                .draw(|frame| {
                    let mut cx = Context {
                        editor: &mut app.editor,
                        theme: &app.theme,
                        ui: &mut app.ui,
                    };
                    let _ = app.compositor.render(frame, &mut cx);
                })
                .expect("draw");
        }
        let each = start.elapsed() / frames;
        let busy = |gap: Duration| each.as_secs_f64() / gap.as_secs_f64() * 100.0;
        println!(
            "one frame takes {each:?}, about {:.1}% of a core while typing and {:.1}% idle, \
             animating: {}",
            busy(FRAME_TIME),
            busy(IDLE_FRAME_TIME),
            app.compositor.is_animating()
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Runs git with `args` in `dir`, returning what it printed if it worked.
    fn git(dir: &Path, args: &[&str]) -> Option<String> {
        let output = process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Staging the change at the cursor puts only that change in the index, and the panel shows
    /// it.
    #[tokio::test]
    async fn stages_a_hunk_and_shows_the_panel() {
        let dir = temp_dir();
        let setup = [
            vec!["init", "-q"],
            vec!["config", "user.email", "mog@example.com"],
            vec!["config", "user.name", "mog"],
            vec!["config", "core.autocrlf", "false"],
        ];
        let file = dir.join("list.txt");
        fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").expect("write");
        let ready = setup.iter().all(|args| git(&dir, args).is_some())
            && git(&dir, &["add", "."]).is_some()
            && git(&dir, &["commit", "-q", "-m", "first"]).is_some();
        if !ready {
            return;
        }
        let mut app = start(&dir);
        app.editor.open(&file).expect("open");
        // change the first and last lines, then stage only the first change
        type_text(&mut app, "1 ");
        app.handle_event(press("ctrl+end"));
        type_text(&mut app, "six\n");
        app.handle_event(press("ctrl+home"));
        app.execute_command("git.stage_hunk".parse().expect("command"));
        app.snapshot("80x24", Duration::from_millis(1500), &[])
            .await
            .expect("settle");
        let staged = git(&dir, &["show", ":list.txt"]).expect("index");
        assert_eq!(staged, "1 one\ntwo\nthree\nfour\nfive\n");
        let screen = app
            .snapshot("100x30", Duration::from_millis(1500), &["git.panel".into()])
            .await
            .expect("panel");
        assert!(screen.contains("STAGED"), "{screen}");
        assert!(screen.contains("+1 one"), "{screen}");
        let _ = fs::remove_dir_all(dir);
    }

    /// A task's errors land in the problems list, pointing at the right file and line.
    #[tokio::test]
    async fn task_errors_become_problems() {
        let dir = temp_dir();
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(dir.join("src").join("lib.c"), "int x\n").expect("write");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.tasks.insert(
            "build".into(),
            TaskConfig {
                command: "echo src/lib.c:1:6: error: expected semicolon".into(),
                cwd: String::new(),
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.execute_command("task.run".parse().expect("command"));
        let index = app
            .ui
            .tasks
            .iter()
            .position(|(name, _)| name == "build")
            .expect("build task");
        app.ui.picked_task = Some(index);
        app.execute_command("task.start".parse().expect("command"));
        loop {
            let event = time::timeout(Duration::from_secs(10), app.runner.event())
                .await
                .expect("in time")
                .expect("event");
            let done = matches!(event, TaskEvent::Done(_));
            app.handle_task(event);
            if done {
                break;
            }
        }
        let problem = app.ui.task_problems.first().expect("a problem");
        assert_eq!((problem.line, problem.column), (0, 5));
        assert_eq!(problem.message, "expected semicolon");
        assert!(problem.path.ends_with("lib.c"));
        let _ = fs::remove_dir_all(dir);
    }

    /// Feeds the app debugger updates until `done` says to stop.
    async fn debug_until(app: &mut App, done: impl Fn(&App) -> bool) {
        while !done(app) {
            let update = time::timeout(Duration::from_secs(60), app.debugger.update())
                .await
                .expect("the debugger answered in time")
                .expect("an update");
            match update {
                DebugUpdate::Event(event) => app.handle_debug_event(event),
                DebugUpdate::Reply(reply) => app.handle_debug_reply(reply),
            }
        }
    }

    /// Debugs a real C program with lldb-dap: stop at a breakpoint, read variables, step, finish.
    #[tokio::test]
    #[ignore = "needs clang and lldb-dap, run by hand"]
    async fn debugs_a_c_program() {
        let dir = temp_dir();
        let source = dir.join("main.c");
        fs::write(
            &source,
            "#include <stdio.h>\nint add(int a, int b) {\n    int sum = a + b;\n    return sum;\n}\n\
             int main(void) {\n    int x = add(2, 3);\n    printf(\"%d\\n\", x);\n    return 0;\n}\n",
        )
        .expect("write");
        let exe = if cfg!(windows) { "main.exe" } else { "main" };
        let built = process::Command::new("clang")
            .args(["-g", "-O0", "main.c", "-o", exe])
            .current_dir(&dir)
            .status()
            .expect("clang runs");
        assert!(built.success());
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.debug.insert(
            "c".into(),
            DebugConfig {
                command: "lldb-dap".into(),
                extensions: vec!["c".into()],
                arguments: json!({ "program": "${root}/main${exe}", "cwd": "${root}" }),
                ..DebugConfig::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.editor.open(&source).expect("open");
        let line_start = app.editor.document().text().line_to_char(2);
        app.editor.select(line_start, line_start);
        app.execute_command("debug.toggle_breakpoint".parse().expect("command"));
        app.execute_command("debug.start".parse().expect("command"));
        debug_until(&mut app, |app| {
            app.ui.debug.paused && !app.ui.debug.variables.is_empty()
        })
        .await;
        let (_, line) = app.ui.debug.stopped_at.clone().expect("stopped");
        assert_eq!(line, 2);
        assert!(app.ui.debug.frames[0].name.contains("add"));
        assert!(
            app.ui
                .debug
                .variables
                .iter()
                .any(|variable| variable.name == "a" && variable.value == "2"),
            "{:?}",
            app.ui.debug.variables
        );
        app.execute_command("debug.step_over".parse().expect("command"));
        debug_until(&mut app, |app| {
            app.ui.debug.paused && !app.ui.debug.variables.is_empty()
        })
        .await;
        assert_eq!(app.ui.debug.stopped_at.as_ref().map(|at| at.1), Some(3));
        assert!(
            app.ui
                .debug
                .variables
                .iter()
                .any(|variable| variable.name == "sum" && variable.value == "5")
        );
        app.execute_command("debug.start".parse().expect("command"));
        debug_until(&mut app, |app| !app.ui.debug.active).await;
        assert!(
            app.ui.debug.console.iter().any(|line| line.trim() == "5"),
            "{:?}",
            app.ui.debug.console
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Debugs a Python script with debugpy, which answers the launch only after configuration.
    #[tokio::test]
    #[ignore = "needs uv to fetch debugpy, run by hand"]
    async fn debugs_a_python_script() {
        let dir = temp_dir();
        let script = dir.join("app.py");
        fs::write(
            &script,
            "def add(a, b):\n    total = a + b\n    return total\n\nprint(add(2, 3))\n",
        )
        .expect("write");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.debug.insert(
            "python".into(),
            DebugConfig {
                command: "uv".into(),
                args: [
                    "run",
                    "--with",
                    "debugpy",
                    "python",
                    "-m",
                    "debugpy.adapter",
                ]
                .map(String::from)
                .to_vec(),
                extensions: vec!["py".into()],
                arguments: json!({ "program": "${file}", "cwd": "${root}",
                    "console": "internalConsole" }),
                ..DebugConfig::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.editor.open(&script).expect("open");
        let line_start = app.editor.document().text().line_to_char(2);
        app.editor.select(line_start, line_start);
        app.execute_command("debug.toggle_breakpoint".parse().expect("command"));
        app.execute_command("debug.start".parse().expect("command"));
        debug_until(&mut app, |app| {
            app.ui.debug.paused && !app.ui.debug.variables.is_empty()
        })
        .await;
        assert_eq!(app.ui.debug.stopped_at.as_ref().map(|at| at.1), Some(2));
        assert!(
            app.ui
                .debug
                .variables
                .iter()
                .any(|variable| variable.name == "total" && variable.value == "5"),
            "{:?}",
            app.ui.debug.variables
        );
        app.execute_command("debug.start".parse().expect("command"));
        debug_until(&mut app, |app| !app.ui.debug.active).await;
        let _ = fs::remove_dir_all(dir);
    }

    /// Returns the Python program on this machine, if there is one.
    fn python() -> Option<&'static str> {
        ["python3", "python"].into_iter().find(|program| {
            process::Command::new(program)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
    }

    /// Feeds the app plugin updates until `done` says to stop.
    async fn plugins_until(app: &mut App, done: impl Fn(&App) -> bool) {
        while !done(app) {
            let Ok(update) = time::timeout(Duration::from_secs(20), app.plugins.update()).await
            else {
                panic!(
                    "the plugin did not answer in time:\n{}",
                    app.plugins.report(&BTreeMap::new())
                );
            };
            let update = update.expect("an update");
            app.handle_plugin(update);
        }
    }

    /// Waits for a reply to a language feature request, answering plugins meanwhile.
    async fn feature_reply(app: &mut App) {
        loop {
            let deadline = time::sleep(Duration::from_secs(20));
            tokio::select! {
                Some(reply) = app.lsp_replies.recv() => {
                    app.handle_lsp_reply(reply);
                    return;
                }
                Some(update) = app.plugins.update() => app.handle_plugin(update),
                () = deadline => panic!("no reply in time"),
            }
        }
    }

    /// The protocol 2 example plugin marks notes, completes, offers a code action, explains on
    /// hover and tidies the file before it is saved.
    #[tokio::test]
    async fn runs_the_todo_plugin() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let file = dir.join("notes.txt");
        fs::write(&file, "first  \nTODO: water the mog\nlast\n").expect("write");
        // a copy that waits for a text file, to check language activation catches up on it
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/todo");
        let folder = dir.join("todo-plugin");
        fs::create_dir_all(&folder).expect("plugin folder");
        for name in ["todo.py", "plugin.toml"] {
            let text = fs::read_to_string(example.join(name)).expect("the example exists");
            let text = text.replace(
                "activation = [\"startup\"]",
                "activation = [\"language:txt\"]",
            );
            fs::write(folder.join(name), text).expect("copy");
        }
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.plugins.insert(
            "todo".into(),
            PluginConfig {
                path: Some(folder),
                command: python.into(),
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        // the manifest lists the command before the plugin even answers
        assert!(
            app.ui
                .commands
                .iter()
                .any(|info| info.name == "plugin.todo.list")
        );
        assert!(
            app.plugins.get("todo").is_none(),
            "it waits for a text file"
        );
        app.editor.open(&file).expect("open");
        app.watch_focus();
        plugins_until(&mut app, |app| {
            !app.editor.document().diagnostics().is_empty() && !app.ui.plugin_segments.is_empty()
        })
        .await;
        assert_eq!(
            app.editor.document().diagnostics()[0].message,
            "water the mog"
        );
        assert_eq!(app.ui.plugin_segments[0].text, "1 todo");
        assert!(app.ui.plugin_decorations[&file].contains_key(&1));
        // completion comes from the plugin with no language server around
        let end = app.editor.document().text().len_chars();
        app.editor.select(end, end);
        app.execute_command(Command::InsertText("FI".into()));
        app.request_completion(true);
        feature_reply(&mut app).await;
        let completion = app.ui.completion.as_ref().expect("completions");
        assert!(completion.items.iter().any(|item| item.label == "FIXME"));
        app.ui.completion = None;
        app.execute_command(Command::Undo);
        // hover and a code action on the note line
        let note = app.editor.document().text().line_to_char(1) + 2;
        app.editor.select(note, note);
        app.request_feature("hover");
        feature_reply(&mut app).await;
        assert!(
            app.ui
                .hover
                .as_ref()
                .is_some_and(|(text, _)| text.contains("note"))
        );
        app.request_feature("actions");
        feature_reply(&mut app).await;
        app.execute_command(Command::Custom("plugins.action.0".into()));
        assert_eq!(app.editor.document().text().to_string(), "first  \nlast\n");
        // saving waits for the plugin to trim the trailing spaces
        app.execute_command(Command::Save);
        plugins_until(&mut app, |app| !app.editor.document().is_modified()).await;
        assert_eq!(fs::read_to_string(&file).expect("saved"), "first\nlast\n");
        let _ = fs::remove_dir_all(dir);
    }

    /// Presses `keys` one after another, then lets plugins work until every key is handled.
    async fn press_keys(app: &mut App, keys: &[&str]) {
        for key in keys {
            app.handle_event(press(key));
            app.run_requests();
        }
        plugins_until(app, |app| !app.plugin_keys_pending()).await;
        app.run_requests();
    }

    /// The vim example takes the keyboard: motions, operators, insert mode, undo and the keys
    /// typed so far drawn on the screen.
    #[tokio::test]
    async fn runs_the_vim_plugin() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let file = dir.join("notes.txt");
        fs::write(
            &file,
            "one two
three
",
        )
        .expect("write");
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/vim");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.plugins.insert(
            "vim".into(),
            PluginConfig {
                path: Some(example),
                command: python.into(),
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), file.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        plugins_until(&mut app, |app| {
            app.ui.plugin_segments.iter().any(|s| s.text == "NORMAL")
        })
        .await;
        assert_eq!(app.ui.cursor_style.shape, CursorShape::Block);
        let text = |app: &App| app.editor.document().text().to_string();
        let head = |app: &App| app.editor.document().selection().head;
        press_keys(&mut app, &["w"]).await;
        assert_eq!(head(&app), 4);
        // a key waiting for its motion shows on the screen
        press_keys(&mut app, &["d"]).await;
        plugins_until(&mut app, |app| !app.ui.plugin_widgets.is_empty()).await;
        assert_eq!(app.ui.plugin_widgets[0].id, "keys");
        press_keys(&mut app, &["w"]).await;
        assert_eq!(
            text(&app),
            "one 
three
"
        );
        // keys typed right after i go into the file, not to vim
        press_keys(&mut app, &["j", "shift+i", "x", "y", "esc"]).await;
        assert_eq!(
            text(&app),
            "one 
xythree
"
        );
        assert_eq!(head(&app), 6);
        press_keys(&mut app, &["d", "d", "u"]).await;
        assert_eq!(
            text(&app),
            "one 
xythree
"
        );
        press_keys(&mut app, &["0", "2", "x"]).await;
        assert_eq!(
            text(&app),
            "one 
three
"
        );
        // ctrl keys still reach mog
        press_keys(&mut app, &["ctrl+s"]).await;
        assert_eq!(
            fs::read_to_string(&file).expect("saved"),
            "one 
three
"
        );
        app.execute_command("plugin.vim.toggle".parse().expect("command"));
        plugins_until(&mut app, |app| {
            app.ui.cursor_style.shape == CursorShape::Default
        })
        .await;
        press_keys(&mut app, &["z"]).await;
        assert!(text(&app).contains('z'));
        let _ = fs::remove_dir_all(dir);
    }

    /// The aquarium example draws fish as flair, which turning off its flair hides.
    #[tokio::test]
    async fn runs_the_aquarium_plugin() {
        if env::var("CI").is_ok() {
            return;
        }
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/aquarium");
        let mut config = Config::default();
        config.updates.check = false;
        // the critters have fish of their own
        config.flair.disabled.push("critters".into());
        config.plugins.insert(
            "aquarium".into(),
            PluginConfig {
                path: Some(example),
                command: python.into(),
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        plugins_until(&mut app, |app| app.ui.plugin_widgets.len() >= 4).await;
        assert!(app.ui.flairs.iter().any(|(id, _)| id == "plugin.aquarium"));
        let fish = |screen: &str| screen.contains("><") || screen.contains("<>");
        let screen = app
            .snapshot("100x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert!(fish(&screen), "{screen}");
        app.ui.config.flair.disabled.push("plugin.aquarium".into());
        let screen = app
            .snapshot("100x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert!(!fish(&screen), "{screen}");
        app.plugins.get("aquarium").expect("plugin").stop();
        let _ = fs::remove_dir_all(dir);
    }

    /// A plugin that knows where things are, for the provider tests.
    const PLACES_PLUGIN: &str = r#"
from mog_plugin import Plugin

plugin = Plugin()


@plugin.provide("definition")
def definition(params):
    return {"path": "notes.txt", "line": 2, "column": 1}


@plugin.provide("references")
def references(params):
    return {"locations": [{"line": 0}, {"path": "notes.txt", "line": 1, "column": 2}]}


@plugin.provide("symbols")
def symbols(params):
    if params.get("query") not in (None, "al"):
        return {"symbols": []}
    return {"symbols": [{"name": "alpha", "kind": "function", "line": 0}, {"name": "beta", "line": 1}]}


plugin.run()
"#;

    /// Plugins answer go to definition, find references and symbols with no language server.
    #[tokio::test]
    async fn plugins_provide_places_and_symbols() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let file = dir.join("notes.txt");
        fs::write(&file, "alpha\nbeta\ngamma\n").expect("write");
        let script = dir.join("places.py");
        fs::write(&script, PLACES_PLUGIN).expect("plugin");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.plugins.insert(
            "places".into(),
            PluginConfig {
                command: python.into(),
                args: vec![script.to_string_lossy().into_owned()],
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        app.editor.open(&file).expect("open");
        plugins_until(&mut app, |app| {
            !app.plugins.providers_any("symbols").is_empty()
        })
        .await;
        app.request_feature("definition");
        feature_reply(&mut app).await;
        assert_eq!(
            app.editor.document().selection().head,
            "alpha\nbeta\n".len() + 1,
            "{:?} {:?}",
            app.editor.status(),
            app.editor.document().path()
        );
        app.request_feature("references");
        feature_reply(&mut app).await;
        assert_eq!(app.ui.overlay, Some(Overlay::References));
        let places: Vec<(usize, usize)> = app
            .ui
            .references
            .iter()
            .map(|(_, line, column, _)| (*line, *column))
            .collect();
        assert_eq!(places, [(0, 0), (1, 2)]);
        app.ui.close();
        app.request_symbols(None);
        feature_reply(&mut app).await;
        let symbols: Vec<(&str, &str)> = app
            .ui
            .symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind.as_str()))
            .collect();
        assert_eq!(symbols, [("alpha", "fn"), ("beta", "symbol")]);
        let _ = fs::remove_dir_all(dir);
    }

    /// A plugin that finds shouting and has tasks, for the provider tests.
    const LINT_PLUGIN: &str = r#"
from mog_plugin import Plugin

plugin = Plugin()


@plugin.provide("diagnostics")
def diagnostics(params):
    lines = params["text"].splitlines()
    found = [{"line": i, "message": "shouty", "severity": "warning"} for i, line in enumerate(lines) if line.isupper()]
    return {"diagnostics": found}


@plugin.provide("tasks")
def tasks(params):
    return {"tasks": [{"name": "lint", "command": "echo lint"}, {"name": "build", "command": "echo plugin"}]}


plugin.run()
"#;

    /// Plugins find problems when typing pauses, and add tasks the config can still override.
    #[tokio::test]
    async fn plugins_provide_diagnostics_and_tasks() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let file = dir.join("notes.txt");
        fs::write(&file, "quiet\nLOUD\n").expect("write");
        let script = dir.join("lint.py");
        fs::write(&script, LINT_PLUGIN).expect("plugin");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.tasks.insert(
            "build".into(),
            TaskConfig {
                command: "echo configured".into(),
                cwd: String::new(),
            },
        );
        config.plugins.insert(
            "lint".into(),
            PluginConfig {
                command: python.into(),
                args: vec![script.to_string_lossy().into_owned()],
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        app.editor.open(&file).expect("open");
        plugins_until(&mut app, |app| {
            !app.plugins.providers_any("tasks").is_empty()
        })
        .await;
        app.sync_plugin_events(false);
        plugins_until(&mut app, |app| {
            !app.editor.document().diagnostics().is_empty()
        })
        .await;
        assert_eq!(app.editor.document().diagnostics()[0].message, "shouty");
        let end = app.editor.document().text().len_chars();
        app.editor.select(end, end);
        app.execute_command(Command::InsertText("MORE\n".into()));
        app.sync_plugin_events(false);
        plugins_until(&mut app, |app| {
            app.editor.document().diagnostics().len() == 2
        })
        .await;
        app.execute_command(Command::Custom("task.run".into()));
        plugins_until(&mut app, |app| app.ui.overlay == Some(Overlay::Tasks)).await;
        let tasks: Vec<(&str, &str)> = app
            .ui
            .tasks
            .iter()
            .map(|(name, command)| (name.as_str(), command.as_str()))
            .collect();
        assert!(tasks.contains(&("lint", "echo lint")), "{tasks:?}");
        assert!(tasks.contains(&("build", "echo configured")), "{tasks:?}");
        let _ = fs::remove_dir_all(dir);
    }

    /// A plugin's manifest adds a theme, a key and highlight queries before it ever runs, and the
    /// config still wins.
    #[tokio::test]
    async fn plugins_contribute_without_running() {
        let dir = temp_dir();
        let folder = dir.join("neon");
        fs::create_dir_all(folder.join("themes")).expect("folder");
        fs::write(
            folder.join("plugin.toml"),
            "name = \"neon\"\ncommand = \"definitely-not-a-program\"\nactivation = [\"command\"]\n\
             [[commands]]\nname = \"glow\"\n\
             [contributes]\nthemes = [\"themes/neon.toml\", \"themes/mine.toml\"]\n\
             keys = { \"alt+shift+n\" = \"plugin.neon.glow\", \"ctrl+s\" = \"quit\" }\n",
        )
        .expect("manifest");
        fs::write(
            folder.join("themes/neon.toml"),
            "base = \"mog\"\naccent = \"#ff00ff\"\n",
        )
        .expect("theme");
        fs::write(folder.join("themes/mine.toml"), "accent = \"#00ff00\"\n").expect("theme");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.ui.theme = "neon".into();
        config.themes.insert("mine".into(), ThemeConfig::default());
        config.plugins.insert(
            "neon".into(),
            PluginConfig {
                path: Some(folder),
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        assert_eq!(app.theme.name, "neon");
        assert_eq!(
            app.ui.config.themes["mine"],
            ThemeConfig::default(),
            "the config wins"
        );
        let chord = |key: &str| key.parse::<KeyChord>().expect("chord");
        assert_eq!(
            app.keymap.resolve(&chord("alt+shift+n")),
            Some(Command::Custom("plugin.neon.glow".into()))
        );
        assert_eq!(app.keymap.resolve(&chord("ctrl+s")), Some(Command::Save));
        let _ = fs::remove_dir_all(dir);
    }

    /// The chat gets the tools running plugins offer and can call them.
    #[tokio::test]
    async fn plugins_offer_chat_tools() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let script = dir.join("tools.py");
        fs::write(
            &script,
            "from mog_plugin import Plugin\nplugin = Plugin()\n\n\
             @plugin.tool(\"shout\", \"Shouts\")\ndef shout(tool_input):\n    \
             return tool_input[\"text\"].upper()\n\nplugin.run()\n",
        )
        .expect("plugin");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.plugins.insert(
            "loud".into(),
            PluginConfig {
                command: python.into(),
                args: vec![script.to_string_lossy().into_owned()],
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        plugins_until(&mut app, |app| !app.plugin_chat_tools().is_empty()).await;
        let tools = app.plugin_chat_tools();
        assert_eq!(tools[0].name, "loud_shout");
        let answer = call_tool(tools[0].clone(), json!({ "text": "hi" })).await;
        assert_eq!(answer.as_deref(), Ok("HI"));
        let _ = fs::remove_dir_all(dir);
    }

    /// Returns the event for clicking `button` at `(column, row)`.
    fn click(column: u16, row: u16, button: MouseButton) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(button),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        })
    }

    /// Plugin segments go on either end, and a click on one without a command is passed on,
    /// while the badge still opens the palette.
    #[tokio::test]
    async fn clicks_plugin_segments() {
        let dir = temp_dir();
        let mut app = start(&dir);
        app.ui.plugin_segments = vec![
            PluginSegment {
                side: Side::Left,
                bold: true,
                bg: Some("red".into()),
                ..PluginSegment::new("vim/mode", "NORMAL", None)
            },
            PluginSegment::new("todo", "3 todos", Some("yellow".into())),
        ];
        let screen = app
            .snapshot("100x5", Duration::ZERO, &[])
            .await
            .expect("drawn");
        let status = screen.lines().last().expect("status line").to_owned();
        let left = status.find("NORMAL").expect("left segment");
        let right = status.find("3 todos").expect("right segment");
        assert!(left < right, "{status}");
        let column = |byte: usize| u16::try_from(status[..byte].chars().count()).expect("fits");
        app.handle_event(click(column(right) + 1, 4, MouseButton::Right));
        app.handle_event(click(column(left) + 1, 4, MouseButton::Left));
        let clicks: Vec<(&str, &str)> = app
            .ui
            .segment_clicks
            .iter()
            .map(|click| (click.segment.as_str(), click.button))
            .collect();
        assert_eq!(clicks, [("todo", "right"), ("vim/mode", "left")]);
        app.handle_event(click(1, 4, MouseButton::Left));
        app.run_requests();
        assert_eq!(app.ui.overlay, Some(Overlay::Palette));
        let _ = fs::remove_dir_all(dir);
    }

    /// A plugin's list shows the preview of the highlighted row and lets several rows be chosen.
    #[tokio::test]
    async fn picks_several_with_previews() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let script = dir.join("pick.py");
        fs::write(
            &script,
            r#"
from mog_plugin import Plugin, insert

plugin = Plugin()


@plugin.command("choose", title="Choose")
def choose(context, args):
    items = [{"label": name, "preview": f"all about {name}"} for name in ("apple", "pear", "plum")]
    picked = plugin.ask("ui/pick", {"title": "fruit", "items": items, "multi": True})
    return [insert(",".join(picked["items"]))] if picked else []


plugin.run()
"#,
        )
        .expect("plugin");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.plugins.insert(
            "fruit".into(),
            PluginConfig {
                command: python.into(),
                args: vec![script.to_string_lossy().into_owned()],
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        plugins_until(&mut app, |app| {
            app.ui
                .commands
                .iter()
                .any(|info| info.name == "plugin.fruit.choose")
        })
        .await;
        app.execute_command("plugin.fruit.choose".parse().expect("command"));
        plugins_until(&mut app, |app| app.ui.overlay == Some(Overlay::PluginPick)).await;
        let screen = app
            .snapshot("140x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert!(screen.contains("all about apple"), "{screen}");
        assert!(screen.contains("[ ] pear"), "{screen}");
        for key in ["tab", "tab", "tab"] {
            app.handle_event(press(key));
        }
        app.handle_event(press("up"));
        app.handle_event(press("tab"));
        app.handle_event(press("enter"));
        app.run_requests();
        plugins_until(&mut app, |app| app.editor.document().text().len_chars() > 0).await;
        assert_eq!(app.editor.document().text().to_string(), "apple,plum");
        let _ = fs::remove_dir_all(dir);
    }

    /// Hands `app` a notification from `plugin`, as if it sent it.
    fn notify(app: &mut App, plugin: &str, method: &str, params: Value) {
        app.handle_plugin(PluginUpdate::Event(PluginEvent::Notification {
            plugin: plugin.into(),
            method: method.into(),
            params,
        }));
    }

    /// Plugin notifications show with progress and buttons, update in place, run out and close.
    #[tokio::test]
    async fn shows_toasts() {
        let dir = temp_dir();
        let mut app = start(&dir);
        notify(
            &mut app,
            "index",
            "toast",
            json!({ "id": "run", "title": "Indexing", "text": "312 files", "progress": 50,
                    "buttons": [{ "title": "Stop" }] }),
        );
        notify(
            &mut app,
            "index",
            "toast",
            json!({ "id": "gone", "title": "Quick", "timeout": 1 }),
        );
        time::sleep(Duration::from_millis(20)).await;
        let screen = app
            .snapshot("100x20", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert!(
            screen.contains("Indexing") && screen.contains("50%"),
            "{screen}"
        );
        assert!(!screen.contains("Quick"), "it ran out: {screen}");
        notify(
            &mut app,
            "index",
            "toast",
            json!({ "id": "run", "title": "Indexing", "progress": 90, "buttons": [{ "title": "Stop" }] }),
        );
        let screen = app
            .snapshot("100x20", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert!(
            screen.contains("90%") && !screen.contains("50%"),
            "{screen}"
        );
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains("[ Stop ]"))
            .expect("a button");
        let column = line[..line.find("[ Stop ]").expect("button")]
            .chars()
            .count();
        let at = |n: usize| u16::try_from(n).expect("fits");
        app.handle_event(click(at(column + 2), at(row), MouseButton::Left));
        assert_eq!(app.ui.toast_clicks.len(), 1);
        assert_eq!(app.ui.toast_clicks[0].button, 0);
        notify(
            &mut app,
            "index",
            "toast",
            json!({ "id": "run", "done": true }),
        );
        assert!(app.ui.toasts.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    /// Returns the row and column where `text` first shows on `screen`.
    fn find_on(screen: &str, text: &str) -> (u16, u16) {
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(text))
            .unwrap_or_else(|| panic!("{text} is not on the screen:\n{screen}"));
        let column = line[..line.find(text).expect("found")].chars().count();
        (
            u16::try_from(row).expect("fits"),
            u16::try_from(column).expect("fits"),
        )
    }

    /// Plugin panels dock on the right and at the bottom, take turns on a side, and report
    /// clicks and closing.
    #[tokio::test]
    async fn shows_plugin_panels() {
        let dir = temp_dir();
        let mut app = start(&dir);
        notify(
            &mut app,
            "outline",
            "panel",
            json!({ "id": "tree", "title": "Outline", "lines": ["fn main", "fn helper"] }),
        );
        notify(
            &mut app,
            "outline",
            "panel",
            json!({ "id": "log", "title": "Log", "side": "bottom", "size": 6, "lines": ["started"] }),
        );
        notify(
            &mut app,
            "other",
            "panel",
            json!({ "id": "x", "title": "Other", "lines": ["hi"] }),
        );
        let screen = app
            .snapshot("120x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert!(
            screen.contains("Other") && screen.contains("Outline"),
            "{screen}"
        );
        assert!(
            !screen.contains("fn main"),
            "the newest panel on a side is shown: {screen}"
        );
        let (log_row, _) = find_on(&screen, "started");
        assert!(log_row > 20, "{screen}");
        let (row, column) = find_on(&screen, " Outline ");
        app.handle_event(click(column + 2, row, MouseButton::Left));
        let screen = app
            .snapshot("120x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        let (row, column) = find_on(&screen, "fn helper");
        app.handle_event(click(column + 3, row, MouseButton::Left));
        assert_eq!(
            app.ui.panel_events,
            [PanelEvent::Click {
                plugin: "outline".into(),
                id: "tree".into(),
                line: 1,
                x: 3,
                button: "left",
            }]
        );
        app.ui.panel_events.clear();
        let (row, _) = find_on(&screen, " Outline ");
        let title = screen.lines().nth(usize::from(row)).expect("title row");
        let close = title.rfind('\u{00d7}').expect("a close button");
        let column = u16::try_from(title[..close].chars().count()).expect("fits");
        app.handle_event(click(column, row, MouseButton::Left));
        assert!(matches!(app.ui.panel_events[0], PanelEvent::Closed { .. }));
        notify(
            &mut app,
            "outline",
            "panel",
            json!({ "id": "log", "remove": true }),
        );
        assert!(app.ui.plugin_panels.iter().all(|panel| panel.id != "log"));
        let _ = fs::remove_dir_all(dir);
    }

    /// Plugin canvases paint cells over the editor as flair and follow the flair settings.
    #[tokio::test]
    async fn paints_plugin_canvases() {
        let dir = temp_dir();
        let mut config = Config::default();
        config.updates.check = false;
        config.ui.git_blame = false;
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        notify(
            &mut app,
            "rain",
            "canvas",
            json!({ "cells": [[2, 3, "\u{00a4}", "cyan"]], "rows": [{ "x": 5, "y": 4, "text": "MOG  RAIN" }] }),
        );
        notify(
            &mut app,
            "rain",
            "canvas",
            json!({ "clear": false, "cells": [[0, 0, "@"]] }),
        );
        let editor = app.ui.layout(Rect::new(0, 0, 100, 30)).editor;
        let at = |screen: &str, x: u16, y: u16, len: usize| -> String {
            let row = screen
                .lines()
                .nth(usize::from(editor.y + y))
                .unwrap_or_default();
            row.chars()
                .skip(usize::from(editor.x + x))
                .take(len)
                .collect()
        };
        let screen = app
            .snapshot("100x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert_eq!(at(&screen, 2, 3, 1), "\u{00a4}", "{screen}");
        assert_eq!(at(&screen, 5, 4, 3), "MOG", "{screen}");
        assert_eq!(at(&screen, 0, 0, 1), "@", "{screen}");
        assert!(app.ui.flairs.iter().any(|(id, _)| id == "plugin.rain"));
        app.ui.config.flair.disabled.push("plugin.rain".into());
        let screen = app
            .snapshot("100x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert_ne!(at(&screen, 2, 3, 1), "\u{00a4}", "{screen}");
        app.ui.config.flair.disabled.clear();
        app.ui.config.ui.reduced_motion = true;
        let screen = app
            .snapshot("100x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert_ne!(
            at(&screen, 2, 3, 1),
            "\u{00a4}",
            "a moving canvas hides: {screen}"
        );
        notify(
            &mut app,
            "rain",
            "canvas",
            json!({ "still": true, "clear": false, "cells": [] }),
        );
        let screen = app
            .snapshot("100x30", Duration::ZERO, &[])
            .await
            .expect("drawn");
        assert_eq!(
            at(&screen, 2, 3, 1),
            "\u{00a4}",
            "a still one stays: {screen}"
        );
        notify(&mut app, "rain", "canvas", json!({ "cells": [] }));
        assert!(app.ui.plugin_canvases.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    /// Runs the actions in `json` the way a plugin would ask for them.
    fn plugin_actions(app: &mut App, json: Value) -> Result<(), String> {
        let actions = parse_actions(&json).expect("valid actions");
        app.apply_actions(actions)
    }

    /// Two parts of a workspace edit for the same file both land, in offsets of the text
    /// before the edit, whether the file is open or not.
    #[tokio::test]
    async fn merges_workspace_edits_for_one_file() {
        let dir = temp_dir();
        let open = dir.join("open.txt");
        let closed = dir.join("closed.txt");
        fs::write(
            &open,
            "hello world
",
        )
        .expect("write");
        fs::write(
            &closed, "abc def
",
        )
        .expect("write");
        let mut app = start(&open);
        let edit = |path: &Path, start: usize, end: usize, text: &str| json!({ "path": path, "changes": [{ "start": start, "end": end, "text": text }] });
        let edits = json!({ "actions": [{ "type": "workspace_edit", "edits": [
            edit(&open, 0, 5, "HELLO"),
            edit(&closed, 0, 3, "ABC"),
            edit(&open, 6, 11, "WORLD"),
            edit(&closed, 4, 7, "DEF"),
        ] }] });
        plugin_actions(&mut app, edits).expect("applied");
        assert_eq!(
            app.editor.document().text().to_string(),
            "HELLO WORLD
"
        );
        assert_eq!(
            fs::read_to_string(&closed).expect("read"),
            "ABC DEF
"
        );
        app.execute_command(Command::Undo);
        assert_eq!(
            app.editor.document().text().to_string(),
            "hello world
"
        );
        let overlapping = json!({ "actions": [{ "type": "workspace_edit", "edits": [
            edit(&closed, 0, 3, "x"),
            edit(&closed, 2, 5, "y"),
        ] }] });
        assert!(plugin_actions(&mut app, overlapping).is_err());
        assert_eq!(
            fs::read_to_string(&closed).expect("read"),
            "ABC DEF
"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// A workspace edit that fails on one file leaves every other file as it was.
    #[tokio::test]
    async fn workspace_edits_are_all_or_nothing() {
        let dir = temp_dir();
        let open = dir.join("open.txt");
        let first = dir.join("first.txt");
        fs::write(
            &open, "open
",
        )
        .expect("write");
        fs::write(
            &first, "first
",
        )
        .expect("write");
        let mut app = start(&open);
        let unwritable = dir.join("missing").join("folder").join("new.txt");
        let edits = json!({ "actions": [{ "type": "workspace_edit", "edits": [
            { "path": first, "changes": [{ "start": 0, "end": 5, "text": "FIRST" }] },
            { "path": open, "changes": [{ "start": 0, "end": 4, "text": "OPEN" }] },
            { "path": unwritable, "changes": [{ "start": 0, "end": 0, "text": "new" }] },
        ] }] });
        assert!(plugin_actions(&mut app, edits).is_err());
        assert_eq!(
            fs::read_to_string(&first).expect("read"),
            "first
"
        );
        assert_eq!(
            app.editor.document().text().to_string(),
            "open
"
        );
        assert!(!unwritable.exists());
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = fs::remove_dir_all(dir);
    }

    /// The example plugin adds commands that count, edit and insert, and fills the status line.
    #[tokio::test]
    async fn runs_the_example_plugin() {
        let Some(python) = python() else {
            return;
        };
        let dir = temp_dir();
        let file = dir.join("notes.txt");
        fs::write(&file, "hello there mog\n").expect("write");
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/plugins/words.py")
            .canonicalize()
            .expect("the example exists");
        let mut config = Config::default();
        config.flair.enabled = false;
        config.updates.check = false;
        config.plugins.insert(
            "words".into(),
            PluginConfig {
                command: python.into(),
                args: vec![script.to_string_lossy().into_owned()],
                ..Default::default()
            },
        );
        let args = Args::parse_from([Path::new("mog"), dir.as_path()]);
        let mut app = App::with_config(args, config, Vec::new(), None);
        app.start_plugins();
        plugins_until(&mut app, |app| {
            app.ui
                .commands
                .iter()
                .any(|info| info.name == "plugin.words.count")
        })
        .await;
        let count = app
            .ui
            .commands
            .iter()
            .find(|info| info.name == "plugin.words.count")
            .expect("count");
        assert_eq!(count.keys, ["alt+shift+w"]);
        app.editor.open(&file).expect("open");
        app.watch_focus();
        plugins_until(&mut app, |app| !app.ui.plugin_segments.is_empty()).await;
        assert_eq!(app.ui.plugin_segments[0].text, "3w");
        app.handle_event(press("alt+shift+w"));
        plugins_until(&mut app, |app| {
            app.editor
                .status()
                .is_some_and(|status| status.contains("words"))
        })
        .await;
        assert_eq!(app.editor.status(), Some("3 words, nice"));
        app.editor.select(0, 5);
        app.execute_command("plugin.words.shout".parse().expect("command"));
        plugins_until(&mut app, |app| {
            app.editor
                .document()
                .text()
                .to_string()
                .starts_with("HELLO")
        })
        .await;
        assert_eq!(
            app.editor.document().text().to_string(),
            "HELLO there mog\n"
        );
        // the filler command asks mog to show a list and waits for the pick
        let end = app.editor.document().text().len_chars();
        app.editor.select(end, end);
        app.execute_command("plugin.words.filler".parse().expect("command"));
        plugins_until(&mut app, |app| app.ui.overlay == Some(Overlay::PluginPick)).await;
        app.ui.plugin_picked = Some(vec![2]);
        app.ui.close();
        app.execute_command(PLUGIN_PICKED_COMMAND.parse().expect("command"));
        plugins_until(&mut app, |app| {
            app.editor.document().text().to_string().ends_with("mog")
        })
        .await;
        let _ = fs::remove_dir_all(dir);
    }

    /// Undo and redo go through the keymap and back to the same text.
    #[tokio::test]
    async fn undo_and_redo() {
        let dir = temp_dir();
        let path = dir.join("undo.txt");
        fs::write(&path, "").expect("write");
        let mut app = start(&path);
        type_text(&mut app, "abc");
        app.handle_event(press("ctrl+z"));
        assert_eq!(app.editor.document().text().to_string(), "");
        app.handle_event(press("ctrl+y"));
        assert_eq!(app.editor.document().text().to_string(), "abc");
        let _ = fs::remove_dir_all(dir);
    }

    /// The command palette opens over the file and typing filters it.
    #[tokio::test]
    async fn palette_runs_a_command() {
        let dir = temp_dir();
        let path = dir.join("palette.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("write");
        let mut app = start(&path);
        app.handle_event(press("ctrl+shift+p"));
        // popups fill their list when they are first drawn
        app.snapshot("80x24", Duration::ZERO, &[])
            .await
            .expect("snapshot");
        type_text(&mut app, "select all");
        app.handle_event(press("enter"));
        app.run_requests();
        let selection = app.editor.document().selection();
        assert_eq!((selection.from(), selection.to()), (0, 14));
        let screen = app
            .snapshot("60x10", Duration::ZERO, &[])
            .await
            .expect("snapshot");
        assert!(screen.contains("palette.txt"), "{screen}");
        let _ = fs::remove_dir_all(dir);
    }

    /// The open command opens a file, and a folder swaps the whole project.
    #[tokio::test]
    async fn opens_a_file_or_folder() {
        let dir = temp_dir();
        let inner = dir.join("inner");
        fs::create_dir_all(&inner).expect("dir");
        fs::write(dir.join("a.txt"), "a").expect("write");
        let mut app = start(&dir);
        app.execute_custom("file.open");
        type_text(&mut app, "a.txt");
        app.handle_event(press("enter"));
        app.run_requests();
        assert_eq!(app.editor.document().name(), "a.txt");
        app.execute_custom("file.open");
        type_text(&mut app, "inner");
        app.handle_event(press("enter"));
        app.run_requests();
        assert!(app.ui.root.ends_with("inner"), "{:?}", app.ui.root);
        let _ = fs::remove_dir_all(dir);
    }
}
