//! The application state and event loop.

use std::{
    env, fs, mem,
    path::{Path, PathBuf},
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
use lsp_types::Range as LspRange;
use mog_ai::CompletionRequest;
use mog_audio::{Audio, Mood, Sfx};
use mog_config::{Config, save_setting};
use mog_core::{
    Command, Document, Editor, FileTree, KeyChord, Keymap, Outcome, Range, Severity, Transaction,
    movement,
};
use mog_flair::{GraphView, builtin::screensaver};
use mog_lsp::{
    LspEvent, convert,
    features::{CodeAction, FileEdits},
};
use mog_term::TerminalPanel;
use mog_tui::{
    ChatPanel, CompletionMenu, Compositor, Context, ContextMenu, EditorView, EventResult, Explorer,
    Focus, Minimap, Overlay, Pane, Popups, PromptKind, SearchBar, SettingsPanel, SplitState,
    StatusLine, Tabs, Theme, Ui, UiEvent,
    completion::{self, CompletionState},
    input,
    menu::{self, MenuAction, MenuItem},
    search,
    settings::{SettingKey, change as settings_change, persisted},
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Position, Rect},
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time::{self, MissedTickBehavior},
};

use crate::{
    ai::{AiReply, Assistant},
    cli::Args,
    clipboard, commands,
    discord::{Presence, Status},
    git::{self, Git},
    lsp::{self, LanguageServers, LspReply},
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

/// How many chars before the cursor a ghost suggestion gets to see.
const GHOST_CONTEXT_BEFORE: usize = 4000;

/// How many chars after the cursor a ghost suggestion gets to see.
const GHOST_CONTEXT_AFTER: usize = 1000;

/// What the chat says when no AI is set up.
const NO_AI: &str = "no ai provider is enabled. add [ai.claude] enabled = true to the config \
and put your key in ANTHROPIC_API_KEY, then restart mog.";

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
    /// Where feature requests send their answers.
    lsp_sender: UnboundedSender<LspReply>,
    /// Answers to feature requests.
    lsp_replies: UnboundedReceiver<LspReply>,
    /// The id of the newest completion request, so older answers are ignored.
    completion_request: u64,
    /// The code actions offered last, picked by index from the menu.
    code_actions: Vec<CodeAction>,
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
    /// When the last key or mouse event came in, to know when the screensaver is up.
    last_input: Instant,
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
        ui.commands = commands::palette(&keymap);
        ui.bindings = commands::bindings(&keymap);
        ui.root.clone_from(&root);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        compositor.push(Box::new(EditorView::side()));
        compositor.push(Box::new(Tabs::new()));
        compositor.push(Box::new(ChatPanel::new()));
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
        compositor.push(Box::new(flair));
        compositor.push(Box::new(StatusLine::new()));
        compositor.push(Box::new(CompletionMenu::new()));
        compositor.push(Box::new(Popups::new()));
        compositor.push(Box::new(SettingsPanel::new()));
        compositor.push(Box::new(GraphView::new()));
        compositor.push(Box::new(ContextMenu::new()));

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
        let startup = args.run;
        let (reply_sender, lsp_replies) = mpsc::unbounded_channel();
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
            code_actions: Vec::new(),
            assistant: Assistant::new(providers),
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
            last_input: Instant::now(),
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
        app
    }

    /// Starts, stops or adjusts sound to match the settings.
    fn apply_audio_settings(&mut self) {
        let settings = &self.ui.config.audio;
        let wanted = settings.sound_effects || settings.music;
        if wanted && self.audio.is_none() {
            self.audio = Some(Audio::start());
        }
        if let Some(audio) = &self.audio {
            audio.set_volume(settings.volume);
            audio.set_music(settings.music);
        }
    }

    /// Connects to or leaves Discord to match the settings.
    fn apply_discord_settings(&mut self) {
        let settings = &self.ui.config.discord;
        if !settings.enabled {
            self.discord = None;
            return;
        }
        if settings.client_id.is_empty() {
            self.editor
                .set_status("discord needs [discord] client_id in the config, see the example");
            self.discord = None;
            return;
        }
        if self
            .discord
            .as_ref()
            .is_none_or(|presence| presence.client_id() != settings.client_id)
        {
            self.discord = Some(Presence::start(&settings.client_id, &settings.large_image));
            self.discord_status = None;
        }
        self.update_presence();
    }

    /// Tells Discord about the focused file if it changed.
    fn update_presence(&mut self) {
        let Some(presence) = &self.discord else {
            return;
        };
        let document = self.editor.document();
        let details = if !self.ui.config.discord.show_file {
            "editing something secret".to_owned()
        } else if document.path().is_some() {
            format!("editing {}", document.name())
        } else {
            "staring at a blank file".to_owned()
        };
        let project = self
            .ui
            .root
            .file_name()
            .map_or_else(|| "mog".into(), |name| name.to_string_lossy());
        let state = match self.last_problems.0 {
            0 => format!("in {project}"),
            1 => format!("in {project}, 1 error"),
            errors => format!("in {project}, {errors} errors"),
        };
        let status = Status { details, state };
        if self.discord_status.as_ref() != Some(&status) {
            presence.update(status.clone());
            self.discord_status = Some(status);
        }
    }

    /// Plays sounds for what happened since the last frame.
    fn play_sounds(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };
        let effects = self.ui.config.audio.sound_effects;
        for event in &self.ui.events {
            let sfx = match event {
                UiEvent::Typed(_) => Some(Sfx::Key),
                UiEvent::Deleted => Some(Sfx::Delete),
                UiEvent::Saved => Some(Sfx::Save),
                UiEvent::Opened => Some(Sfx::Open),
                UiEvent::Diagnostics { errors, .. } => {
                    let sound = match (self.sound_errors, *errors) {
                        (0, n) if n > 0 => Some(Sfx::Error),
                        (n, 0) if n > 0 => Some(Sfx::Fixed),
                        _ => None,
                    };
                    self.sound_errors = *errors;
                    let mood = if *errors > 0 { Mood::Tense } else { Mood::Calm };
                    audio.set_mood(mood);
                    sound
                }
                UiEvent::Activity => None,
            };
            if let Some(sfx) = sfx.filter(|_| effects) {
                audio.play(sfx);
            }
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
            self.watch_focus();
            self.play_sounds();
            self.draw(terminal)?;
            let animating = self.compositor.is_animating();
            tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.handle_event(event?),
                    None => break,
                },
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.lsp_replies.recv() => self.handle_lsp_reply(reply),
                Some(reply) = self.assistant.reply() => self.handle_ai_reply(reply),
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
            self.sync_git();
            tokio::select! {
                Some(update) = self.git.update() => git::apply(&mut self.ui, update),
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.lsp_replies.recv() => self.handle_lsp_reply(reply),
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
        if let Some(cursor) = cursor {
            queue!(backend, MoveTo(cursor.x, cursor.y), Show)?;
        }
        execute!(backend, EndSynchronizedUpdate)?;
        Ok(())
    }

    /// Returns `true` while the matrix screensaver covers the screen.
    fn screensaver_showing(&self) -> bool {
        let flair = &self.ui.config.flair;
        flair.enabled
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
                if let Some(chord) = input::key_chord(key) {
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

    /// Offers a key to the layers, then runs its bound command if none of them took it.
    fn handle_key(&mut self, chord: KeyChord) {
        self.ui.events.push(UiEvent::Activity);
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

    /// Changes a setting by one step as if it was picked in the settings menu.
    fn change_setting(&mut self, key: SettingKey) {
        settings_change(&mut self.ui.config, &key, 1);
        self.ui.setting_changes.push(key);
        self.apply_setting_changes();
    }

    /// Saves and applies settings changed in the settings menu.
    fn apply_setting_changes(&mut self) {
        let changes = mem::take(&mut self.ui.setting_changes);
        if changes.is_empty() {
            return;
        }
        for key in &changes {
            let (path, value) = persisted(&self.ui.config, key);
            if let Err(err) = save_setting(&path, &value) {
                self.editor
                    .set_status(format!("could not save setting: {err}"));
            }
        }
        let (theme, problem) = settings::theme(&self.ui.config);
        self.theme = theme;
        if let Some(problem) = problem {
            self.editor.set_status(problem);
        }
        self.editor.set_options(settings::options(&self.ui.config));
        self.apply_audio_settings();
        self.apply_discord_settings();
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
        let document = self.editor.document();
        let path = document.path().map(ToOwned::to_owned);
        if path != self.last_focus {
            self.last_focus = path;
            self.ui.events.push(UiEvent::Opened);
        }
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

    /// Asks where to save the focused document.
    fn ask_save_as(&mut self) {
        let current = self
            .editor
            .document()
            .path()
            .map(|path| {
                path.strip_prefix(&self.ui.root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .unwrap_or_default();
        let hint = format!("relative to {}", self.ui.root.display());
        self.ui
            .ask(PromptKind::SaveAs, "\u{21e9} save as", current, hint);
    }

    /// Acts on an answered prompt.
    fn submit_prompt(&mut self) {
        let Some(prompt) = self.ui.submitted.take() else {
            return;
        };
        let text = prompt.text.trim();
        if text.is_empty() {
            return;
        }
        let resolve = |base: &Path| {
            let path = Path::new(text);
            if path.is_absolute() {
                path.to_owned()
            } else {
                base.join(path)
            }
        };
        let result = match prompt.kind {
            PromptKind::GotoLine => {
                if let Ok(line) = text.parse() {
                    self.ui.focus = Focus::Editor;
                    self.execute_command(Command::GotoLine(line));
                }
                Ok(())
            }
            PromptKind::SaveAs => {
                let path = resolve(&self.ui.root);
                let result = self.editor.save_as(&path);
                if result.is_ok() {
                    self.saved();
                }
                result
            }
            PromptKind::NewFile(dir) => {
                let path = resolve(&dir);
                let result = if text.ends_with('/') || text.ends_with('\\') {
                    fs::create_dir_all(&path)
                } else {
                    path.parent()
                        .map_or(Ok(()), fs::create_dir_all)
                        .and_then(|()| fs::OpenOptions::new().create(true).append(true).open(&path))
                        .and_then(|_| self.editor.open(&path))
                };
                if result.is_ok() {
                    self.ui.focus = Focus::Editor;
                }
                result
            }
            PromptKind::RenameFile(old) => {
                let new = resolve(old.parent().unwrap_or(&self.ui.root));
                let result = fs::rename(&old, &new);
                if result.is_ok() {
                    for document in self.editor.documents_mut() {
                        if document.path() == Some(old.as_path()) {
                            document.set_path(&new);
                        }
                    }
                    self.editor
                        .set_status(format!("renamed to {}", new.display()));
                }
                result
            }
            PromptKind::DeleteFile(path) => {
                if !matches!(text, "y" | "yes") {
                    return;
                }
                let result = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                if result.is_ok() {
                    self.editor
                        .set_status(format!("deleted {}, gone forever", path.display()));
                }
                result
            }
            PromptKind::RenameSymbol => {
                self.request_rename(text.to_owned());
                Ok(())
            }
        };
        self.ui.refresh_explorer = true;
        if let Err(err) = result {
            self.editor.set_status(format!("that did not work: {err}"));
        }
    }

    /// Asks the AI for a ghost suggestion at the cursor once typing pauses.
    fn suggest_ghost(&self) {
        if !self.ui.config.ai.ghost_text || self.assistant.provider_name().is_none() {
            return;
        }
        let document = self.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let prefix_start = head.saturating_sub(GHOST_CONTEXT_BEFORE);
        let suffix_end = (head + GHOST_CONTEXT_AFTER).min(text.len_chars());
        let request = CompletionRequest {
            prefix: text.slice(prefix_start..head).to_string(),
            suffix: text.slice(head..suffix_end).to_string(),
            language: document
                .path()
                .and_then(|path| path.extension())
                .map(|ext| ext.to_string_lossy().into_owned()),
        };
        self.assistant
            .suggest(request, self.editor.active(), document.version(), head);
    }

    /// Sends what was typed in the chat panel.
    fn send_chat(&mut self) {
        let text = mem::take(&mut self.ui.chat.input);
        if text.trim().is_empty() || self.ui.chat.waiting {
            self.ui.chat.input = text;
            return;
        }
        self.ui.chat.messages.push((true, text));
        self.ui.chat.scroll = 0;
        if self.assistant.chat(&self.ui.chat.messages) {
            self.ui.chat.waiting = true;
        } else {
            self.ui.chat.messages.push((false, NO_AI.to_owned()));
        }
    }

    /// Acts on a finished AI request.
    fn handle_ai_reply(&mut self, reply: AiReply) {
        match reply {
            AiReply::Chat(text) => {
                self.ui.chat.waiting = false;
                self.ui.chat.messages.push((false, text));
                self.ui.chat.scroll = 0;
            }
            AiReply::ChatFailed(reason) => {
                self.ui.chat.waiting = false;
                self.ui
                    .chat
                    .messages
                    .push((false, format!("that failed: {reason}")));
            }
            AiReply::Ghost {
                document,
                version,
                pos,
                text,
            } => {
                let current = self.editor.document();
                let fresh = document == self.editor.active()
                    && version == current.version()
                    && pos == current.selection().head;
                if fresh {
                    self.ui.ghost = Some((document, version, pos, text));
                }
            }
        }
    }

    /// Opens or closes the completion menu after `ch` was typed.
    fn auto_complete(&mut self, ch: char) {
        if !self.ui.config.editor.auto_complete {
            return;
        }
        let word = ch.is_alphanumeric() || ch == '_';
        if word {
            if self.ui.completion.is_none() {
                self.request_completion(false);
            }
        } else if matches!(ch, '.' | ':') {
            self.ui.completion = None;
            self.request_completion(false);
        } else {
            self.ui.completion = None;
        }
    }

    /// Asks the language server for completions at the cursor.
    fn request_completion(&mut self, manual: bool) {
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            return;
        };
        let Some(client) = self.lsp.client_for(&path) else {
            if manual {
                self.editor.set_status("no language server for this file");
            }
            return;
        };
        let head = document.selection().head;
        let anchor = completion::word_start(&self.editor, head);
        let position = convert::char_to_position(document.text(), head);
        self.completion_request += 1;
        let request = self.completion_request;
        let index = self.editor.active();
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let items = client.completion(&path, position).await.unwrap_or_default();
            let items = items.into_iter().map(lsp::to_item).collect();
            let _ = sender.send(LspReply::Completion {
                request,
                document: index,
                anchor,
                items,
            });
        });
    }

    /// Sends a hover, definition or format request for the focused file.
    fn request_feature(&mut self, feature: &str) {
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            self.editor
                .set_status("save the file first so a language server can see it");
            return;
        };
        let Some(client) = self.lsp.client_for(&path) else {
            self.editor.set_status("no language server for this file");
            return;
        };
        let head = document.selection().head;
        let position = convert::char_to_position(document.text(), head);
        let version = document.version();
        let options = self.editor.options();
        let (tab_size, spaces) = (
            u32::try_from(options.tab_width).unwrap_or(4),
            options.insert_spaces,
        );
        let line = document.text().char_to_line(head);
        let diagnostics = lsp::diagnostics_on_line(document, line);
        let sender = self.lsp_sender.clone();
        let feature = feature.to_owned();
        tokio::spawn(async move {
            let reply = match feature.as_str() {
                "hover" => match client.hover(&path, position).await {
                    Ok(Some(text)) => LspReply::Hover(text, head),
                    _ => LspReply::Nothing("nothing to say about that".into()),
                },
                "definition" => match client.definition(&path, position).await {
                    Ok(Some((target, at))) => LspReply::Definition(target, at),
                    _ => LspReply::Nothing("no definition found".into()),
                },
                "references" => match client.references(&path, position).await {
                    Ok(found) if !found.is_empty() => LspReply::References(found),
                    _ => LspReply::Nothing("no references found".into()),
                },
                "actions" => {
                    let range = LspRange::new(position, position);
                    match client.code_actions(&path, range, diagnostics).await {
                        Ok(actions) if !actions.is_empty() => LspReply::Actions(actions),
                        _ => LspReply::Nothing("no code actions here".into()),
                    }
                }
                _ => match client.formatting(&path, tab_size, spaces).await {
                    Ok(edits) => LspReply::Format(path, version, edits),
                    Err(err) => LspReply::Nothing(format!("could not format: {err}")),
                },
            };
            let _ = sender.send(reply);
        });
    }

    /// Asks the language server to rename the symbol at the cursor to `new_name`.
    fn request_rename(&mut self, new_name: String) {
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            return;
        };
        let Some(client) = self.lsp.client_for(&path) else {
            self.editor.set_status("no language server for this file");
            return;
        };
        let position = convert::char_to_position(document.text(), document.selection().head);
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let reply = match client.rename(&path, position, &new_name).await {
                Ok(files) if !files.is_empty() => LspReply::Rename(files),
                Ok(_) => LspReply::Nothing("nothing to rename here".into()),
                Err(err) => LspReply::Nothing(format!("could not rename: {err}")),
            };
            let _ = sender.send(reply);
        });
    }

    /// Applies rename edits to open documents and writes the rest straight to disk.
    fn apply_rename(&mut self, files: FileEdits) {
        let focused = self.editor.active();
        let mut count = 0;
        for (path, edits) in files {
            count += edits.len();
            let open = self
                .editor
                .documents()
                .iter()
                .position(|document| document.path() == Some(path.as_path()));
            if let Some(index) = open {
                self.editor.focus(index);
                let changes = lsp::to_changes(self.editor.document(), &edits);
                self.editor.apply_changes(changes);
                continue;
            }
            let result = Document::open(&path).and_then(|mut document| {
                let changes = lsp::to_changes(&document, &edits);
                let tx = Transaction::new(changes);
                document.apply(tx, Range::point(0), false);
                document.save()
            });
            if let Err(err) = result {
                self.editor
                    .set_status(format!("could not edit {}: {err}", path.display()));
            }
        }
        self.editor.focus(focused);
        self.editor.set_status(format!("renamed in {count} places"));
    }

    /// Acts on an answer to a feature request.
    fn handle_lsp_reply(&mut self, reply: LspReply) {
        match reply {
            LspReply::Completion {
                request,
                document,
                anchor,
                items,
            } => {
                if request != self.completion_request || document != self.editor.active() {
                    return;
                }
                self.ui.completion =
                    (!items.is_empty()).then(|| CompletionState::new(items, anchor, document));
            }
            LspReply::Hover(text, pos) => self.ui.hover = Some((text, pos)),
            LspReply::Definition(path, position) => {
                if let Err(err) = self.editor.open(&path) {
                    self.editor
                        .set_status(format!("could not open {}: {err}", path.display()));
                    return;
                }
                let pos = convert::position_to_char(self.editor.document().text(), position);
                self.editor.select(pos, pos);
            }
            LspReply::Format(path, version, edits) => {
                let document = self.editor.document();
                if document.path() != Some(path.as_path()) || document.version() != version {
                    return;
                }
                let changes = lsp::to_changes(document, &edits);
                let count = changes.len();
                self.editor.apply_changes(changes);
                self.editor.set_status(if count == 0 {
                    "already formatted, mog approves".to_owned()
                } else {
                    format!("formatted ({count} edits)")
                });
            }
            LspReply::Rename(files) => self.apply_rename(files),
            LspReply::References(found) => {
                self.ui.references = found
                    .into_iter()
                    .map(|(path, position)| {
                        let line = usize::try_from(position.line).unwrap_or(0);
                        let column = usize::try_from(position.character).unwrap_or(0);
                        let preview = lsp::line_preview(&self.editor, &path, line);
                        (path, line, column, preview)
                    })
                    .collect();
                self.ui.open(Overlay::References);
            }
            LspReply::Actions(actions) => {
                let items = actions
                    .iter()
                    .enumerate()
                    .map(|(i, (title, _))| MenuItem {
                        label: title.clone(),
                        keys: String::new(),
                        action: Some(MenuAction::Run(Command::Custom(format!("lsp.action.{i}")))),
                    })
                    .collect();
                self.code_actions = actions;
                let at = self.ui.cursor_screen.unwrap_or_default();
                menu::open_menu(&mut self.ui, Position::new(at.x, at.y + 1), items);
            }
            LspReply::Nothing(message) => self.editor.set_status(message),
        }
    }

    /// Reacts to the focused document being saved.
    fn saved(&mut self) {
        self.ui.events.push(UiEvent::Saved);
        // a save as needs the new path opened on the server before it hears about the save
        self.sync_language_servers();
        if let Some(path) = self.editor.document().path() {
            self.lsp.saved(path);
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
            LspEvent::Exited { server, reason } => {
                self.lsp.exited(&server);
                self.editor
                    .set_status(lsp::exit_message(&server, reason.as_deref()));
            }
        }
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
            self.ui.hover = None;
            self.ui.ghost = None;
            self.assistant.cancel_suggestion();
            if let Some(UiEvent::Typed(ch)) = self.ui.events.last().cloned() {
                self.auto_complete(ch);
                self.suggest_ghost();
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

    /// Runs an app level command like `explorer.toggle`.
    fn execute_custom(&mut self, name: &str) {
        match name {
            "ai.explain" => {
                let document = self.editor.document();
                let selection = document.selection();
                if selection.is_empty() {
                    self.editor.set_status("select some code to explain first");
                    return;
                }
                let code = document
                    .text()
                    .slice(selection.from()..selection.to())
                    .to_string();
                let name = document.name();
                self.ui.chat.open = true;
                self.ui.chat.input = format!("explain this code from {name}:\n\n{code}");
                self.send_chat();
            }
            "ai.chat" => {
                self.ui.chat.open = !self.ui.chat.open || self.ui.focus != Focus::Chat;
                self.ui.focus = if self.ui.chat.open {
                    Focus::Chat
                } else {
                    Focus::Editor
                };
            }
            "ai.send" => self.send_chat(),
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
            "finder.files" => self.ui.open(Overlay::Finder),
            "search.find" => search::open(&mut self.ui, &self.editor, false),
            "search.replace" => search::open(&mut self.ui, &self.editor, true),
            "search.next" | "search.prev" => {
                if self.ui.search.query.is_empty() {
                    search::open(&mut self.ui, &self.editor, false);
                    return;
                }
                let was_open = self.ui.search.open;
                self.ui.search.open = true;
                search::step(&mut self.ui, &mut self.editor, name == "search.next");
                if !was_open {
                    search::close(&mut self.ui);
                }
            }
            "ui.escape" => {
                if self.ui.search.open {
                    search::close(&mut self.ui);
                } else {
                    let head = self.editor.document().selection().head;
                    self.editor.select(head, head);
                }
            }
            "help.keys" => self.ui.open(Overlay::Keys),
            "goto.prompt" => {
                let lines = self.editor.document().text().len_lines();
                self.ui.ask(
                    PromptKind::GotoLine,
                    "\u{21b3} go to line",
                    "",
                    format!("a line from 1 to {lines}"),
                );
            }
            "prompt.submit" => self.submit_prompt(),
            "problems.list" => self.ui.open(Overlay::Problems),
            "problems.next" | "problems.prev" => {
                let message = self.editor.goto_problem(name == "problems.next");
                let message = message.map_or_else(
                    || "no problems, mog is proud of you".to_owned(),
                    |message| message.lines().next().unwrap_or_default().to_owned(),
                );
                self.editor.set_status(message);
            }
            "file.save_as" => self.ask_save_as(),
            "file.new" => self.editor.new_document(),
            "file.create" => {
                let root = self.ui.root.clone();
                self.ui.ask(
                    PromptKind::NewFile(root),
                    "\u{271a} new file",
                    "",
                    "a path in the project folder, end with / to make a folder",
                );
            }
            "settings.open" => self.ui.open(Overlay::Settings),
            "split.toggle" => {
                self.ui.split = match self.ui.split.take() {
                    Some(_) => None,
                    None => Some(SplitState {
                        focused: Pane::Main,
                        other_document: self.editor.active(),
                        other_view: self.editor.view().clone(),
                    }),
                };
            }
            "split.focus" => {
                let Some(split) = self.ui.split.as_mut() else {
                    return;
                };
                // swap what the panes show so the focused one is always the active document
                let document = self.editor.active();
                let view = self.editor.view().clone();
                self.editor.focus(split.other_document);
                *self.editor.view_mut() = split.other_view.clone();
                split.other_document = document;
                split.other_view = view;
                split.focused = match split.focused {
                    Pane::Main => Pane::Side,
                    Pane::Side => Pane::Main,
                };
                self.ui.focus = Focus::Editor;
            }
            "lsp.complete" => self.request_completion(true),
            "lsp.hover" => self.request_feature("hover"),
            "lsp.definition" => self.request_feature("definition"),
            "lsp.format" => self.request_feature("format"),
            "lsp.references" => self.request_feature("references"),
            "lsp.actions" => self.request_feature("actions"),
            action if action.starts_with("lsp.action.") => {
                let index = action["lsp.action.".len()..].parse::<usize>().ok();
                if let Some((title, edits)) = index.and_then(|i| self.code_actions.get(i).cloned())
                {
                    self.apply_rename(edits);
                    self.editor.set_status(title);
                }
            }
            "lsp.rename" => {
                let document = self.editor.document();
                let head = document.selection().head;
                // at the end of a word the word is behind the cursor
                let text = document.text();
                let is_word = |pos: usize| {
                    text.get_char(pos)
                        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
                };
                let at = if !is_word(head) && head > 0 && is_word(head - 1) {
                    head - 1
                } else {
                    head
                };
                let (from, to) = movement::word_at(text, at);
                let word = text.slice(from..to).to_string();
                self.ui.ask(
                    PromptKind::RenameSymbol,
                    "\u{270e} rename symbol",
                    word,
                    "enter to rename everywhere",
                );
            }
            "audio.toggle_music" => self.change_setting(SettingKey::Audio("music")),
            "audio.toggle_effects" => self.change_setting(SettingKey::Audio("sound_effects")),
            "flair.toggle" => self.change_setting(SettingKey::FlairEnabled),
            "theme.next" => {
                self.change_setting(SettingKey::Theme);
                self.editor
                    .set_status(format!("theme: {}", self.ui.config.ui.theme));
            }
            "graph.toggle" => {
                if self.ui.overlay == Some(Overlay::Graph) {
                    self.ui.close();
                } else {
                    self.ui.open(Overlay::Graph);
                }
            }
            "terminal.toggle" => {
                let focused = self.ui.terminal_open && self.ui.focus == Focus::Terminal;
                self.ui.terminal_open = !focused;
                self.ui.focus = if focused {
                    Focus::Editor
                } else {
                    Focus::Terminal
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
