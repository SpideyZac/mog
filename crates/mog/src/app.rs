//! The application state and event loop.

use std::{
    collections::{HashMap, HashSet},
    env, fs, mem,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
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
use lsp_types::{Position as LspPosition, PublishDiagnosticsParams, Range as LspRange};
use mog_ai::{CompletionFile, CompletionRequest, CopilotEvent, CopilotStatus, DeviceCode};
use mog_audio::{Audio, Mood, Sfx};
use mog_config::{
    Config, Install, ProjectFile, SettingValue, ThemeConfig, config_path, project,
    project_config_path, save_setting, theme::COLOR_NAMES,
};
use mog_core::{
    Change, Command, Document, Editor, FileTree, KeyChord, Keymap, Outcome, Range, Severity,
    Transaction, movement,
    project_search::{self, ProjectResults},
    search::{self as core_search, Matcher},
};
use mog_flair::{GraphView, builtin::screensaver};
use mog_lsp::{
    LspEvent, convert,
    features::{CodeAction, FileEdits},
};
use mog_term::TerminalPanel;
use mog_tui::{
    Annotations, ChatPanel, CompletionMenu, Compositor, Context, ContextMenu, CopilotState,
    EditorView, EventResult, Explorer, Focus, Ghost, Minimap, Overlay, Pane, Popups,
    ProjectSearchPanel, PromptKind, ReleaseNotesPopup, SearchBar, SettingsPanel, SignatureHint,
    SplitState, StatusLine, Tabs, Theme, ThemeEditor, Ui, UiEvent,
    completion::{self, CompletionState},
    ghost, input,
    menu::{self, MenuAction, MenuItem},
    popups::SYMBOL_SEARCH_COMMAND,
    project_search as project_search_panel,
    release_notes::{self, ReleaseNotes},
    search::{self, Toggle},
    settings::{SettingKey, change as settings_change, persisted},
    theme::to_hex,
    theme_editor::{self, ThemeDraft},
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Position, Rect},
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task,
    time::{self, MissedTickBehavior},
};

use crate::{
    ai::{AiReply, Assistant},
    cli::Args,
    clipboard, commands,
    discord::{Presence, Status},
    git::{self, Git},
    lsp::{self, LanguageServers, LspReply},
    session::{Session, SessionFile, State, Swap},
    settings::{self, ProjectStatus},
    terminal::Tui,
    update::{self, Release, UpdateEvent, Updater},
    watch::FolderWatcher,
};

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
        compositor.push(Box::new(StatusLine::new()));
        compositor.push(Box::new(CompletionMenu::new()));
        compositor.push(Box::new(Popups::new()));
        compositor.push(Box::new(SettingsPanel::new()));
        compositor.push(Box::new(ThemeEditor::new()));
        compositor.push(Box::new(ReleaseNotesPopup::new()));
        compositor.push(Box::new(ProjectSearchPanel::new()));
        compositor.push(Box::new(GraphView::new()));
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
            assistant: Assistant::new(providers),
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

    /// Opens the files from the last time this folder was open and offers to bring back unsaved
    /// work from a mog that crashed.
    ///
    /// Not part of [`App::new`] so snapshots and tests start clean.
    pub fn restore_session(&mut self) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let root = self.ui.root.clone();
        let wanted = self.ui.config.editor.restore_session && self.ui.has_explorer;
        // a file given on the command line is a one off, not the project session
        let opened_file = self.editor.document().path().is_some();
        if wanted && !opened_file {
            self.session_enabled = true;
            if let Some(session) = state.load_session(&root) {
                self.apply_session(&session);
                self.last_session = Some(session);
            }
        }
        self.recoverable = state
            .orphaned_swaps(&root)
            .into_iter()
            .filter(|(_, swap)| {
                // a swap that matches the file on disk has nothing to recover
                swap.path.as_ref().is_none_or(|path| {
                    fs::read_to_string(path).ok().as_deref() != Some(swap.text.as_str())
                })
            })
            .collect();
        if self.recoverable.is_empty() {
            return;
        }
        let names: Vec<String> = self
            .recoverable
            .iter()
            .map(|(_, swap)| {
                swap.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map_or_else(|| "untitled".into(), |name| name.to_string_lossy().into())
            })
            .collect();
        self.ui.ask(
            PromptKind::RecoverSwaps,
            "\u{26a0} mog did not close cleanly, recover unsaved work?",
            "",
            format!("y brings back {}, n throws it away", names.join(", ")),
        );
    }

    /// Opens the files in `session` and puts the cursors, scroll and split back.
    fn apply_session(&mut self, session: &Session) {
        let mut opened = Vec::new();
        for file in &session.files {
            if !file.path.is_file() || self.editor.open(&file.path).is_err() {
                opened.push(None);
                continue;
            }
            let len = self.editor.document().text().len_chars();
            self.editor.select(file.anchor.min(len), file.head.min(len));
            let lines = self.editor.document().text().len_lines();
            self.editor.view_mut().scroll_line = file.scroll_line.min(lines.saturating_sub(1));
            opened.push(Some(self.editor.active()));
        }
        let index = |at: usize| opened.get(at).copied().flatten();
        if let Some(split) = session.split.and_then(index) {
            self.ui.split = Some(SplitState {
                focused: Pane::Main,
                other_document: split,
                other_view: self.editor.view().clone(),
            });
        }
        if let Some(active) = index(session.active) {
            self.editor.focus(active);
        }
        self.ui.explorer_open = session.explorer_open && self.ui.has_explorer;
    }

    /// Returns what is open now as a session.
    fn current_session(&self) -> Session {
        let mut files = Vec::new();
        let mut indexes = HashMap::new();
        for (index, document) in self.editor.documents().iter().enumerate() {
            let Some(path) = document.path() else {
                continue;
            };
            let view = if index == self.editor.active() {
                self.editor.view().scroll_line
            } else {
                0
            };
            let selection = document.selection();
            indexes.insert(index, files.len());
            files.push(SessionFile {
                path: path.to_owned(),
                anchor: selection.anchor,
                head: selection.head,
                scroll_line: view,
            });
        }
        Session {
            active: indexes.get(&self.editor.active()).copied().unwrap_or(0),
            split: self
                .ui
                .split
                .as_ref()
                .and_then(|split| indexes.get(&split.other_document).copied()),
            explorer_open: self.ui.explorer_open,
            files,
        }
    }

    /// Saves the open files for next time if they changed.
    fn save_session(&mut self) {
        let Some(state) = self.state.as_ref().filter(|_| self.session_enabled) else {
            return;
        };
        let session = self.current_session();
        if self.last_session.as_ref() != Some(&session) {
            state.save_session(&self.ui.root, &session);
            self.last_session = Some(session);
        }
    }

    /// Writes unsaved documents to swap files and removes the swaps of ones that were saved or
    /// closed.
    fn write_swaps(&mut self) {
        let Some(state) = &self.state else {
            return;
        };
        let mut wanted = HashSet::new();
        for (index, document) in self.editor.documents().iter().enumerate() {
            // writing a huge file every second would stall typing
            if !document.is_modified() || document.is_large() {
                continue;
            }
            let key = document.path().map_or_else(
                || format!("untitled-{index}"),
                |path| path.to_string_lossy().into_owned(),
            );
            if self.swaps.get(&key).map(|(version, _)| *version) != Some(document.version()) {
                let text = document.text().to_string();
                let file = state.write_swap(&key, document.path(), &self.ui.root, text);
                self.swaps.insert(key.clone(), (document.version(), file));
            }
            wanted.insert(key);
        }
        self.swaps.retain(|key, (_, file)| {
            let keep = wanted.contains(key);
            if !keep {
                let _ = fs::remove_file(file);
            }
            keep
        });
    }

    /// Puts back the undo history saved for files that were just opened.
    fn restore_undo(&mut self) {
        let Some(state) = self
            .state
            .as_ref()
            .filter(|_| self.ui.config.editor.persistent_undo)
        else {
            return;
        };
        for document in self.editor.documents_mut() {
            let Some(path) = document.path().map(ToOwned::to_owned) else {
                continue;
            };
            if document.version() != 0 || !self.undo_checked.insert(path.clone()) {
                continue;
            }
            if let Some(history) = state.load_undo(&path, &document.text().to_string()) {
                document.restore_history(history);
            }
        }
    }

    /// Saves the undo history of the focused file, which was just saved.
    fn save_undo(&self) {
        let Some(state) = self
            .state
            .as_ref()
            .filter(|_| self.ui.config.editor.persistent_undo)
        else {
            return;
        };
        let document = self.editor.document();
        if let Some(path) = document.path() {
            state.save_undo(path, &document.text().to_string(), document.history());
        }
    }

    /// Brings back the unsaved work the user chose to recover.
    fn recover_swaps(&mut self) {
        for (file, swap) in mem::take(&mut self.recoverable) {
            match &swap.path {
                Some(path) => {
                    if let Err(err) = self.editor.open(path) {
                        self.editor
                            .set_status(format!("could not open {}: {err}", path.display()));
                        continue;
                    }
                }
                None => self.editor.new_document(),
            }
            let end = self.editor.document().text().len_chars();
            self.editor.apply_changes(vec![Change {
                start: 0,
                end,
                text: swap.text,
            }]);
            let _ = fs::remove_file(file);
        }
        self.editor
            .set_status("recovered, save to keep it. ctrl+z goes back to the file on disk");
    }

    /// Saves what should outlive this run and cleans up swap files, right before quitting.
    fn shut_down(&mut self) {
        self.save_session();
        if let Some(state) = self
            .state
            .as_ref()
            .filter(|_| self.ui.config.editor.persistent_undo)
        {
            for document in self.editor.documents() {
                if let Some(path) = document.path().filter(|_| !document.is_modified()) {
                    state.save_undo(path, &document.text().to_string(), document.history());
                }
            }
        }
        // quitting with unsaved changes was confirmed, so they are meant to go
        for (_, file) in self.swaps.values() {
            let _ = fs::remove_file(file);
        }
        self.swaps.clear();
    }

    /// Shows what is new after an update and looks for the next one.
    ///
    /// Not part of [`App::new`] so snapshots stay offline.
    pub fn start_updates(&mut self) {
        if let Some(previous) = update::remember_version()
            && update::is_newer(update::VERSION, &previous)
        {
            self.updater.notes(Some(format!("v{}", update::VERSION)));
        }
        if self.ui.config.updates.check {
            self.updater.check(false, self.ui.config.updates.install);
        }
    }

    /// Acts on finished update work.
    fn handle_update(&mut self, event: UpdateEvent) {
        match event {
            UpdateEvent::Checked {
                result: Ok(Some(release)),
                install,
                ..
            } => {
                let version = release.version().to_owned();
                self.update = Some(release.clone());
                let message = match update::cannot_install() {
                    None if install => {
                        self.updater.install(release);
                        format!("downloading mog v{version} in the background")
                    }
                    None => format!("mog v{version} is out, run Help: Update mog"),
                    Some(reason) => format!("mog v{version} is out, but {reason}"),
                };
                self.editor.set_status(message);
            }
            UpdateEvent::Checked {
                result: Ok(None),
                manual: true,
                ..
            } => self.editor.set_status(format!(
                "mog v{} is the newest, keep mogging",
                update::VERSION
            )),
            UpdateEvent::Checked {
                result: Err(err),
                manual: true,
                ..
            } => self
                .editor
                .set_status(format!("could not check for updates: {err}")),
            UpdateEvent::Checked { .. } => {}
            UpdateEvent::Installed(Ok(release)) => {
                self.update = None;
                self.editor.set_status(format!(
                    "updated to mog v{}, restart to use it. Help: What's new has the notes",
                    release.version()
                ));
                self.installed = Some(release);
            }
            UpdateEvent::Installed(Err(err)) => {
                self.editor.set_status(format!("update failed: {err}"));
            }
            UpdateEvent::Notes(Ok(release)) => self.show_release_notes(&release),
            UpdateEvent::Notes(Err(err)) => self
                .editor
                .set_status(format!("could not get the release notes: {err}")),
        }
    }

    /// Opens the notes of `release` in a popup.
    fn show_release_notes(&mut self, release: &Release) {
        let newer = update::is_newer(release.version(), update::VERSION);
        let installed = self
            .installed
            .as_ref()
            .is_some_and(|installed| installed.tag_name == release.tag_name);
        let title = if newer && !installed {
            format!("mog v{} is out", release.version())
        } else {
            format!("what's new in mog v{}", release.version())
        };
        self.ui.release_notes = Some(ReleaseNotes {
            title,
            body: release.body.clone().unwrap_or_default(),
            url: release.html_url.clone(),
            can_update: newer && !installed && update::cannot_install().is_none(),
        });
        self.ui.open(Overlay::ReleaseNotes);
    }

    /// Starts, stops or adjusts sound to match the settings.
    fn apply_audio_settings(&mut self) {
        let settings = &self.ui.config.audio;
        let serious = self.ui.config.ui.serious;
        let wanted = (settings.sound_effects || settings.music) && !serious;
        if wanted && self.audio.is_none() {
            self.audio = Some(Audio::start());
        }
        if let Some(audio) = &self.audio {
            audio.set_volume(settings.volume);
            audio.set_music(settings.music && !serious);
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
            "mogging something secret".to_owned()
        } else if document.path().is_some() {
            format!("mogging {}", document.name())
        } else {
            "mogging a blank file".to_owned()
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
        let effects = self.ui.config.audio.sound_effects && !self.ui.config.ui.serious;
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
        let mut housekeeping = time::interval(HOUSEKEEPING_TIME);
        housekeeping.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_tick = Instant::now();
        while !self.quit {
            let idle_at = self.idle_at();
            let typing = Instant::now() < idle_at;
            if !typing {
                self.sync_language_servers();
                for params in mem::take(&mut self.pending_diagnostics) {
                    lsp::apply_diagnostics(&mut self.editor, params);
                }
                self.request_marks();
                self.offer_install();
            }
            self.sync_signature();
            self.sync_git();
            self.watch_focus();
            self.restore_undo();
            self.play_sounds();
            self.ui.idle = self.last_input.elapsed() >= IDLE_AFTER;
            self.draw(terminal)?;
            let animating = self.compositor.is_animating();
            let next_frame = last_tick + self.frame_time();
            tokio::select! {
                event = events.next() => match event {
                    Some(event) => self.handle_event(event?),
                    None => break,
                },
                Some(event) = self.lsp_events.recv() => self.handle_lsp_event(event),
                Some(reply) = self.lsp_replies.recv() => self.handle_lsp_reply(reply),
                Some(reply) = self.assistant.reply() => self.handle_ai_reply(reply),
                Some(update) = self.git.update() => git::apply(&mut self.ui, update),
                Some(event) = self.updater.event() => self.handle_update(event),
                Some((generation, results)) = self.project_results.recv() => {
                    self.show_project_results(generation, results);
                }
                () = changed(self.watcher.as_ref()) => self.files_changed = true,
                () = time::sleep_until(idle_at.into()), if typing => {}
                _ = housekeeping.tick() => self.housekeeping(),
                () = time::sleep_until(next_frame.into()), if animating => {
                    let now = Instant::now();
                    self.compositor.tick(now - last_tick);
                    last_tick = now;
                }
            }
            self.run_requests();
        }
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
                Some(update) = self.git.update() => git::apply(&mut self.ui, update),
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

    /// Applies the key binding change the key list asked for and saves it to the config.
    ///
    /// The new chord replaces every chord the command had, and takes the chord away from
    /// whatever used it before.
    fn apply_rebind(&mut self) {
        let Some((name, chord)) = self.ui.rebind.take() else {
            return;
        };
        let Ok(command) = name.parse::<Command>() else {
            return;
        };
        let chord = match chord.map(|chord| chord.parse::<KeyChord>()).transpose() {
            Ok(chord) => chord,
            Err(err) => {
                self.editor.set_status(err.to_string());
                return;
            }
        };
        let mut changes: Vec<(String, String)> = self
            .keymap
            .chords_for(&command)
            .into_iter()
            .filter(|old| Some(*old) != chord)
            .map(|old| (old.to_string(), String::new()))
            .collect();
        let title = self.ui.title_of(&name).to_owned();
        let message = match chord {
            Some(chord) => {
                changes.push((chord.to_string(), name.clone()));
                let taken = self
                    .keymap
                    .resolve(&chord)
                    .filter(|other| *other != command && chord.typed_char().is_none());
                match taken {
                    Some(other) => format!(
                        "{chord} now runs {title}, it was {}",
                        self.ui.title_of(&other.to_string())
                    ),
                    None => format!("{chord} now runs {title}"),
                }
            }
            None => format!("{title} has no keys now"),
        };
        for (chord, command) in changes {
            if let Err(err) = save_setting(&["keys", &chord], &SettingValue::Text(command.clone()))
            {
                self.editor
                    .set_status(format!("could not save the binding: {err}"));
                return;
            }
            self.ui.config.keys.insert(chord, command);
        }
        let (keymap, problems) = settings::keymap(&self.ui.config);
        self.keymap = keymap;
        self.ui.commands = commands::palette(&self.keymap);
        self.ui.bindings = commands::bindings(&self.keymap);
        // reopening refills the list with the new keys
        self.ui.open(Overlay::Keys);
        self.editor.set_status(if problems.is_empty() {
            message
        } else {
            problems.join("; ")
        });
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
        self.apply_config();
    }

    /// Applies the theme, editing options, sound and Discord from the live config.
    fn apply_config(&mut self) {
        let (theme, problem) = settings::theme(&self.ui.config);
        self.theme = theme;
        if let Some(problem) = problem {
            self.editor.set_status(problem);
        }
        self.editor.set_options(settings::options(&self.ui.config));
        self.apply_audio_settings();
        self.apply_discord_settings();
    }

    /// Reads the config file again and applies everything in it.
    ///
    /// A broken file is reported and the current config is kept.
    fn reload_config(&mut self) {
        let mut config = match Config::load() {
            Ok(config) => config,
            Err(err) => {
                self.editor
                    .set_status(format!("config not reloaded: {err}"));
                return;
            }
        };
        let project = settings::apply_project(&mut config, &self.ui.root);
        let (keymap, mut problems) = settings::keymap(&config);
        // restarting the ai drops the copilot server, so only do it when its settings changed
        if config.ai != self.ui.config.ai {
            let (providers, ai_problems) = settings::ai_providers(&config, &self.ui.root);
            problems.extend(ai_problems);
            self.assistant = Assistant::new(providers);
            self.ui.copilot = None;
        }
        self.keymap = keymap;
        self.ui.commands = commands::palette(&self.keymap);
        self.ui.bindings = commands::bindings(&self.keymap);
        self.lsp.reconfigure(config.language_servers());
        self.ui.config = config;
        self.editor.set_status("config reloaded");
        self.apply_config();
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
        self.project_status(project);
    }

    /// Opens the project config in the editor, creating it first if there is none.
    fn open_project_config(&mut self) {
        let path = project_config_path(&self.ui.root);
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::write(&path, NEW_PROJECT_CONFIG));
            if let Err(err) = created {
                self.editor
                    .set_status(format!("could not create {}: {err}", path.display()));
                return;
            }
        }
        self.ui.close();
        self.ui.focus = Focus::Editor;
        match self.editor.open(&path) {
            Ok(()) => self
                .editor
                .set_status("language server settings for this project only, saving applies them"),
            Err(err) => self
                .editor
                .set_status(format!("could not open {}: {err}", path.display())),
        }
    }

    /// Opens the config file in the editor, creating it first if there is none.
    fn open_config(&mut self) {
        let Some(path) = config_path() else {
            self.editor
                .set_status("there is no config folder on this system");
            return;
        };
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::write(&path, NEW_CONFIG));
            if let Err(err) = created {
                self.editor
                    .set_status(format!("could not create {}: {err}", path.display()));
                return;
            }
        }
        self.ui.close();
        self.ui.focus = Focus::Editor;
        match self.editor.open(&path) {
            Ok(()) => self
                .editor
                .set_status("saving the config reloads it, or run Settings: Reload config"),
            Err(err) => self
                .editor
                .set_status(format!("could not open {}: {err}", path.display())),
        }
    }

    /// Returns `true` if `path` is the config file.
    fn is_config(path: &Path) -> bool {
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        config_path().is_some_and(|config| canonical(&config) == canonical(path))
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

    /// Asks git about open files and the cursor line.
    fn sync_git(&mut self) {
        for document in self.editor.documents() {
            if let Some(path) = document.path().filter(|_| !document.is_large()) {
                self.git.ensure_base(path);
            }
        }
        if !self.ui.config.ui.git_blame || self.editor.document().is_large() {
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
        self.write_swaps();
        self.save_session();
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
        if prompt.kind == PromptKind::CopilotSignIn {
            if let Some(code) = self.copilot_code.take() {
                self.editor
                    .set_status("finish signing in to copilot in your browser");
                self.assistant.copilot_finish_sign_in(code);
            }
            return;
        }
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
            PromptKind::TrustProject => {
                if let Some(file) = self.untrusted_project.take()
                    && matches!(text, "y" | "yes")
                {
                    match project::trust(&file) {
                        Ok(()) => self.reload_config(),
                        Err(err) => self.editor.set_status(format!("could not trust it: {err}")),
                    }
                }
                return;
            }
            PromptKind::ReplaceAll => {
                if matches!(text, "y" | "yes") {
                    self.replace_in_project();
                }
                self.ui.open(Overlay::ProjectSearch);
                return;
            }
            PromptKind::SaveTheme => {
                self.save_theme(text);
                return;
            }
            PromptKind::RecoverSwaps => {
                if matches!(text, "y" | "yes") {
                    self.recover_swaps();
                } else {
                    for (file, _) in mem::take(&mut self.recoverable) {
                        let _ = fs::remove_file(file);
                    }
                }
                return;
            }
            PromptKind::InstallServer(command) => {
                if matches!(text, "y" | "yes") {
                    self.ui.terminal_open = true;
                    self.ui.focus = Focus::Terminal;
                    self.ui.terminal_input.extend_from_slice(command.as_bytes());
                    self.ui.terminal_input.push(b'\r');
                    self.editor.set_status(
                        "installing, run Code: Restart language servers once it is done",
                    );
                }
                return;
            }
            PromptKind::CopilotSignIn => Ok(()),
        };
        self.ui.refresh_explorer = true;
        if let Err(err) = result {
            self.editor.set_status(format!("that did not work: {err}"));
        }
    }

    /// Opens project find and replace, starting from the selected text if there is some.
    fn open_project_search(&mut self, replacing: bool) {
        if !self.ui.has_explorer {
            self.editor
                .set_status("open a folder to search the whole project: mog <folder>");
            return;
        }
        let document = self.editor.document();
        let selection = document.selection();
        let selected = document
            .text()
            .slice(selection.from()..selection.to())
            .to_string();
        let state = &mut self.ui.project_search;
        state.replacing = replacing;
        if !selected.is_empty() && !selected.contains('\n') && selected != state.query {
            state.query = selected;
            state.changed(&mut self.ui.requests);
        }
        self.ui.open(Overlay::ProjectSearch);
    }

    /// Starts searching the project for the current query, cancelling any older search.
    fn run_project_search(&mut self) {
        let state = &self.ui.project_search;
        let generation = state.generation;
        self.project_generation.store(generation, Ordering::Relaxed);
        if state.query.is_empty() {
            return;
        }
        let open: HashMap<PathBuf, String> = self
            .editor
            .documents()
            .iter()
            .filter_map(|document| Some((document.path()?.to_owned(), document.text().to_string())))
            .collect();
        let (root, query, options) = (self.ui.root.clone(), state.query.clone(), state.options());
        let current = self.project_generation.clone();
        let sender = self.project_sender.clone();
        task::spawn_blocking(move || {
            let cancelled = || current.load(Ordering::Relaxed) != generation;
            let results = project_search::search_project(
                &root,
                &query,
                options,
                &open,
                PROJECT_LIMIT,
                &cancelled,
            );
            if !cancelled() {
                let _ = sender.send((generation, results));
            }
        });
    }

    /// Shows what a project search found, unless a newer search started since.
    fn show_project_results(&mut self, generation: u64, results: Result<ProjectResults, String>) {
        let state = &mut self.ui.project_search;
        if generation != state.generation {
            return;
        }
        state.searching = false;
        match results {
            Ok(results) => {
                state.results = results;
                state.error = None;
            }
            Err(err) => {
                state.results = ProjectResults::default();
                state.error = Some(err);
            }
        }
    }

    /// Opens the match picked in project search.
    fn pick_project_match(&mut self) {
        let state = &mut self.ui.project_search;
        let Some(found) = state
            .picked
            .take()
            .and_then(|index| state.results.matches.get(index).cloned())
        else {
            return;
        };
        self.ui.close();
        self.ui.focus = Focus::Editor;
        if let Err(err) = self.editor.open(&found.path) {
            self.editor
                .set_status(format!("could not open {}: {err}", found.path.display()));
            return;
        }
        let text = self.editor.document().text();
        let line = found.line.min(text.len_lines().saturating_sub(1));
        let from = (text.line_to_char(line) + found.column).min(text.len_chars());
        let to = (from + found.len).min(text.len_chars());
        self.editor.select(from, to);
    }

    /// Asks before replacing every match in the project.
    fn ask_replace_in_project(&mut self) {
        let state = &self.ui.project_search;
        if state.query.is_empty() || state.results.matches.is_empty() {
            self.editor.set_status("nothing to replace");
            return;
        }
        let more = if state.results.truncated { "+" } else { "" };
        let title = format!(
            "replace {}{more} matches in {} files with \"{}\"?",
            state.results.matches.len(),
            state.results.files,
            state.replacement
        );
        self.ui.ask(
            PromptKind::ReplaceAll,
            title,
            "",
            "type y and enter. open files change in the editor, others are saved right away",
        );
    }

    /// Replaces every match of the project search, in open documents and on disk.
    fn replace_in_project(&mut self) {
        let state = self.ui.project_search.clone();
        let focused = self.editor.active();
        let open: HashMap<PathBuf, String> = self
            .editor
            .documents()
            .iter()
            .filter_map(|document| Some((document.path()?.to_owned(), document.text().to_string())))
            .collect();
        let Ok(matcher) = Matcher::new(&state.query, state.options()) else {
            return;
        };
        let Ok(results) = project_search::search_project(
            &self.ui.root,
            &state.query,
            state.options(),
            &open,
            usize::MAX,
            &|| false,
        ) else {
            return;
        };
        let mut paths: Vec<PathBuf> = results
            .matches
            .iter()
            .map(|found| found.path.clone())
            .collect();
        paths.dedup();
        let (mut replaced, mut files, mut problems) = (0, 0, Vec::new());
        for path in paths {
            let index = self
                .editor
                .documents()
                .iter()
                .position(|document| document.path() == Some(path.as_path()));
            let count = match index {
                Some(index) => {
                    self.editor.focus(index);
                    let changes = core_search::replace_changes(
                        self.editor.document().text(),
                        &matcher,
                        &state.replacement,
                    );
                    let count = changes.len();
                    self.editor.apply_changes(changes);
                    count
                }
                None => {
                    match project_search::replace_in_file(&path, &matcher, &state.replacement) {
                        Ok(count) => count,
                        Err(err) => {
                            problems.push(format!("{}: {err}", path.display()));
                            0
                        }
                    }
                }
            };
            if count > 0 {
                replaced += count;
                files += 1;
            }
        }
        self.editor.focus(focused);
        self.ui.refresh_explorer = true;
        self.editor.set_status(if problems.is_empty() {
            format!("replaced {replaced} matches in {files} files")
        } else {
            format!(
                "replaced {replaced} matches, failed: {}",
                problems.join("; ")
            )
        });
        self.ui.project_search.changed(&mut self.ui.requests);
    }

    /// Saves the theme being edited as `[themes.<name>]` and switches to it.
    fn save_theme(&mut self, name: &str) {
        let Some(draft) = self.ui.theme_draft.take() else {
            return;
        };
        let mut custom = ThemeConfig::default();
        for (index, color_name) in COLOR_NAMES.iter().enumerate() {
            let hex = to_hex(draft.palette.get(index));
            let saved = save_setting(
                &["themes", name, color_name],
                &SettingValue::Text(hex.clone()),
            );
            if let Err(err) = saved {
                self.editor
                    .set_status(format!("could not save the theme: {err}"));
                self.apply_config();
                return;
            }
            if let Some(slot) = custom.color_mut(color_name) {
                *slot = Some(hex);
            }
        }
        self.ui.config.themes.insert(name.to_owned(), custom);
        self.ui.config.ui.theme = name.to_owned();
        self.ui.setting_changes.push(SettingKey::Theme);
        self.apply_setting_changes();
        self.editor
            .set_status(format!("saved theme {name}, it mogs"));
    }

    /// Previews the theme being edited, or puts the saved theme back once editing stopped.
    fn sync_theme_draft(&mut self) {
        let editing = self.ui.overlay == Some(Overlay::ThemeEditor)
            || self
                .ui
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.kind == PromptKind::SaveTheme);
        match &self.ui.theme_draft {
            Some(draft) if editing => {
                if self.theme.palette != draft.palette {
                    self.theme = Theme::from_palette(&draft.name, draft.palette);
                }
            }
            Some(_) => {
                self.ui.theme_draft = None;
                self.apply_config();
            }
            None => {}
        }
    }

    /// Asks the AI for a ghost suggestion at the cursor once typing pauses, or right away with
    /// several suggestions if the user `invoked` it.
    fn suggest_ghost(&self, invoked: bool) {
        if !self.ui.config.ai.ghost_text
            || !self.assistant.can_suggest()
            || self.editor.document().is_large()
        {
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
            file: Some(CompletionFile {
                path: document.path().map(ToOwned::to_owned),
                index: self.editor.active(),
                text: text.to_string(),
                cursor: head,
                tab_size: self.editor.options().tab_width,
                insert_spaces: self.editor.options().insert_spaces,
            }),
            invoked,
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
                items,
                more: true,
            } => {
                let shown = self.ui.ghost.as_mut().filter(|ghost| {
                    ghost.document == document && ghost.version == version && ghost.pos == pos
                });
                if let Some(ghost) = shown
                    && !ghost::add_more(ghost, items)
                {
                    self.editor.set_status("no other suggestions");
                }
            }
            AiReply::Ghost {
                document,
                version,
                pos,
                items,
                more: false,
            } => {
                let ghost = Ghost::new(document, version, pos, items)
                    .filter(|ghost| ghost.is_fresh(&self.editor));
                if ghost.is_some() {
                    self.ui.ghost = ghost;
                }
            }
            AiReply::Copilot(event) => self.handle_copilot_event(event),
            AiReply::CopilotCode(code) => {
                self.editor.copy_text(code.user_code.clone());
                self.ui.ask(
                    PromptKind::CopilotSignIn,
                    "sign in to copilot",
                    code.user_code.clone(),
                    format!(
                        "copied, enter opens {} to paste it",
                        code.uri.trim_start_matches("https://")
                    ),
                );
                self.copilot_code = Some(code);
            }
            AiReply::CopilotDone(message, status) => {
                if let Some(status) = status {
                    self.set_copilot_status(&status);
                }
                self.editor.set_status(message);
            }
        }
    }

    /// Acts on something the Copilot server reported.
    fn handle_copilot_event(&mut self, event: CopilotEvent) {
        match event {
            CopilotEvent::Status(status) => self.set_copilot_status(&status),
            CopilotEvent::ShowDocument { uri, external } => self.show_document(&uri, external),
            CopilotEvent::Message(text) => self.editor.set_status(format!("copilot: {text}")),
            CopilotEvent::Exited(reason) => {
                let reason = reason.unwrap_or_else(|| "no reason given".into());
                self.editor.set_status(format!("copilot stopped: {reason}"));
                self.ui.copilot = Some(CopilotState::Problem);
            }
        }
    }

    /// Shows the Copilot `status` in the status line, explaining it when it gets worse.
    fn set_copilot_status(&mut self, status: &CopilotStatus) {
        let state = match status {
            CopilotStatus::Starting => CopilotState::Starting,
            CopilotStatus::Ready => CopilotState::Ready,
            CopilotStatus::SignedOut => CopilotState::SignedOut,
            CopilotStatus::Problem(_) => CopilotState::Problem,
        };
        if self.ui.copilot != Some(state) {
            match status {
                CopilotStatus::SignedOut => self
                    .editor
                    .set_status("copilot is signed out, run Copilot: Sign in from the palette"),
                CopilotStatus::Problem(problem) => {
                    self.editor.set_status(format!("copilot: {problem}"));
                }
                CopilotStatus::Starting | CopilotStatus::Ready => {}
            }
        }
        self.ui.copilot = Some(state);
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
        self.sync_language_servers();
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
        self.sync_language_servers();
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

    /// Asks for inlay hints and semantic tokens of the focused file, once per version.
    fn request_marks(&mut self) {
        let settings = &self.ui.config.ui;
        let want_hints = settings.inlay_hints;
        let want_tokens = settings.semantic_highlighting && settings.syntax_highlighting;
        if !want_hints && !want_tokens {
            return;
        }
        let document = self.editor.document();
        let Some(path) = document
            .path()
            .filter(|_| !document.is_large())
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let key = (path.clone(), document.version());
        if self.marks_requested.as_ref() == Some(&key) {
            return;
        }
        // until the handshake is done the server has not said what it can do
        let Some(client) = self
            .lsp
            .client_for(&path)
            .filter(|client| client.capabilities().is_some())
        else {
            return;
        };
        self.marks_requested = Some(key);
        let hints = want_hints && client.supports("inlayHintProvider");
        let tokens = want_tokens && client.supports("semanticTokensProvider");
        if !hints && !tokens {
            return;
        }
        let text = document.text();
        let end = convert::char_to_position(text, text.len_chars());
        let range = LspRange::new(LspPosition::new(0, 0), end);
        let version = document.version();
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let hints = if hints {
                client.inlay_hints(&path, range).await.ok()
            } else {
                None
            };
            let tokens = if tokens {
                client.semantic_tokens(&path).await.ok()
            } else {
                None
            };
            let _ = sender.send(LspReply::Marks {
                path,
                version,
                hints,
                tokens,
            });
        });
    }

    /// Asks for the signature of the call around the cursor.
    fn request_signature(&mut self) {
        self.sync_language_servers();
        let document = self.editor.document();
        let Some(path) = document.path().map(ToOwned::to_owned) else {
            return;
        };
        let Some(client) = self.lsp.client_for(&path) else {
            return;
        };
        let head = document.selection().head;
        let line = document.text().char_to_line(head);
        let position = convert::char_to_position(document.text(), head);
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let signature = client.signature_help(&path, position).await.ok().flatten();
            let _ = sender.send(LspReply::Signature(signature, line));
        });
    }

    /// Hides the signature once the cursor leaves the line of the call.
    fn sync_signature(&mut self) {
        let document = self.editor.document();
        let line = document.text().char_to_line(document.selection().head);
        if self
            .ui
            .signature
            .as_ref()
            .is_some_and(|signature| signature.line != line)
        {
            self.ui.signature = None;
        }
    }

    /// Asks for the symbols of the focused file, or of the whole project matching `query`.
    fn request_symbols(&mut self, query: Option<String>) {
        self.sync_language_servers();
        let path = self.editor.document().path().map(ToOwned::to_owned);
        if query.is_none() && path.is_none() {
            self.editor
                .set_status("save the file first so a language server can see it");
            return;
        }
        let Some(client) = self.lsp.client_for_or_any(path.as_deref()) else {
            self.editor.set_status("no language server is running");
            return;
        };
        let sender = self.lsp_sender.clone();
        tokio::spawn(async move {
            let symbols = match (&query, &path) {
                (Some(query), _) => client.workspace_symbols(query).await,
                (None, Some(path)) => client.document_symbols(path).await,
                (None, None) => Ok(Vec::new()),
            };
            let reply = match symbols {
                Ok(symbols) => LspReply::Symbols { query, symbols },
                Err(err) => LspReply::Nothing(format!("could not get symbols: {err}")),
            };
            let _ = sender.send(reply);
        });
    }

    /// Offers to install a language server that was not found, if one was and nothing else is
    /// asking.
    fn offer_install(&mut self) {
        if self.ui.overlay.is_some() {
            return;
        }
        let Some((program, install)) = self.lsp.take_missing() else {
            return;
        };
        match install {
            Install::Run(command) => self.ui.ask(
                PromptKind::InstallServer(command.to_owned()),
                format!("install {program}?"),
                "",
                format!("y runs `{command}` in the terminal"),
            ),
            Install::Manual(how) => self
                .editor
                .set_status(format!("{program} is not installed: {how}")),
        }
    }

    /// Asks the language server to rename the symbol at the cursor to `new_name`.
    fn request_rename(&mut self, new_name: String) {
        self.sync_language_servers();
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
            LspReply::Marks {
                path,
                version,
                hints,
                tokens,
            } => {
                let Some(document) = self
                    .editor
                    .documents_mut()
                    .iter_mut()
                    .find(|document| document.path() == Some(path.as_path()))
                else {
                    return;
                };
                // marks for an older version would land on the wrong text
                if document.version() != version {
                    return;
                }
                if let Some(hints) = hints {
                    let hints = lsp::to_hints(document, hints);
                    document.set_inlay_hints(hints);
                }
                if let Some(tokens) = tokens {
                    let tokens = lsp::to_tokens(document, tokens);
                    document.set_semantic_tokens(tokens);
                }
            }
            LspReply::Signature(signature, line) => {
                let cursor = self.editor.document();
                let cursor_line = cursor.text().char_to_line(cursor.selection().head);
                self.ui.signature =
                    signature
                        .filter(|_| line == cursor_line)
                        .map(|found| SignatureHint {
                            label: found.label,
                            active: found.active,
                            documentation: found.documentation,
                            line,
                        });
            }
            LspReply::Symbols { query, symbols } => {
                let workspace = query.is_some();
                // answers to an older query are stale once more was typed
                if query.is_some_and(|query| query != self.ui.symbol_query) {
                    return;
                }
                if !workspace && symbols.is_empty() {
                    self.editor.set_status("no symbols in this file");
                    return;
                }
                self.ui.symbols = symbols.into_iter().map(lsp::to_symbol_entry).collect();
                self.ui.symbols_version += 1;
                let overlay = if workspace {
                    Overlay::WorkspaceSymbols
                } else {
                    Overlay::Symbols
                };
                if self.ui.overlay != Some(overlay) {
                    self.ui.open(overlay);
                }
            }
            LspReply::Nothing(message) => self.editor.set_status(message),
        }
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
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        if Self::is_config(&path)
            || canonical(&path) == canonical(&project_config_path(&self.ui.root))
        {
            self.reload_config();
        }
    }

    /// Tells language servers about opened and changed documents.
    fn sync_language_servers(&mut self) {
        let problems = self.lsp.sync(self.editor.documents());
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
    }

    /// Returns when typing will have paused long enough for language servers to check the file.
    fn idle_at(&self) -> Instant {
        self.last_edit + Duration::from_millis(self.ui.config.editor.diagnostics_delay)
    }

    /// Reacts to an event from a language server.
    fn handle_lsp_event(&mut self, event: LspEvent) {
        match event {
            LspEvent::Ready { .. } => {}
            LspEvent::Diagnostics { params, .. } => {
                if Instant::now() < self.idle_at() {
                    // keep only the newest set for each file
                    self.pending_diagnostics
                        .retain(|pending| pending.uri != params.uri);
                    self.pending_diagnostics.push(params);
                } else {
                    lsp::apply_diagnostics(&mut self.editor, params);
                }
            }
            LspEvent::Message { server, text } => {
                self.editor.set_status(format!("{server}: {text}"));
            }
            LspEvent::ShowDocument { uri, external, .. } => self.show_document(&uri, external),
            LspEvent::Notification { method, .. }
                if matches!(
                    method.as_str(),
                    "workspace/semanticTokens/refresh" | "workspace/inlayHint/refresh"
                ) =>
            {
                self.marks_requested = None;
            }
            LspEvent::Notification { .. } => {}
            LspEvent::Exited { server, reason } => {
                if self.lsp.exited(&server) {
                    self.editor
                        .set_status(lsp::exit_message(&server, reason.as_deref()));
                }
            }
        }
    }

    /// Shows a document a server asked for, in the browser if it is not a local file.
    fn show_document(&mut self, uri: &str, external: bool) {
        let path = uri.parse().ok().and_then(|uri| convert::uri_to_path(&uri));
        match path {
            Some(path) if !external => {
                if let Err(err) = self.editor.open(&path) {
                    self.editor
                        .set_status(format!("could not open {}: {err}", path.display()));
                }
            }
            _ => {
                if let Err(err) = open::that_detached(uri) {
                    self.editor
                        .set_status(format!("could not open {uri}: {err}"));
                }
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
            ghost::MORE_COMMAND => self.suggest_ghost(true),
            "copilot.sign_in" => self.assistant.copilot_sign_in(),
            "copilot.sign_out" => self.assistant.copilot_sign_out(),
            "copilot.status" => self.assistant.copilot_check(),
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
            "search.toggle_case" | "search.toggle_word" | "search.toggle_regex" => {
                let toggle = match name {
                    "search.toggle_case" => Toggle::Case,
                    "search.toggle_word" => Toggle::Word,
                    _ => Toggle::Regex,
                };
                if self.ui.overlay == Some(Overlay::ProjectSearch) {
                    let state = &mut self.ui.project_search;
                    state.flip(toggle);
                    state.changed(&mut self.ui.requests);
                } else {
                    self.ui.search.flip(toggle);
                    search::refresh(&mut self.ui, &self.editor);
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
            "keys.rebind" => self.apply_rebind(),
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
            "config.open" => self.open_config(),
            "config.reload" => self.reload_config(),
            "config.open_project" => self.open_project_config(),
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
            "lsp.signature" => self.request_signature(),
            "lsp.symbols" => self.request_symbols(None),
            "lsp.workspace_symbols" => {
                self.ui.symbol_query.clear();
                self.ui.symbols.clear();
                self.ui.symbols_version += 1;
                self.ui.open(Overlay::WorkspaceSymbols);
                self.request_symbols(Some(String::new()));
            }
            SYMBOL_SEARCH_COMMAND => self.request_symbols(Some(self.ui.symbol_query.clone())),
            "lsp.restart" => {
                self.lsp.restart();
                self.marks_requested = None;
                self.editor
                    .set_status("language servers restart with the next file that needs one");
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
            "theme.edit" => {
                self.ui.theme_draft = Some(ThemeDraft::new(&self.theme));
                self.ui.open(Overlay::ThemeEditor);
            }
            theme_editor::CANCEL_COMMAND => {
                self.ui.theme_draft = None;
                self.ui.close();
                self.apply_config();
            }
            theme_editor::SAVE_COMMAND => {
                let Some(draft) = &self.ui.theme_draft else {
                    return;
                };
                let name = if self.ui.config.themes.contains_key(&draft.name) {
                    draft.name.clone()
                } else {
                    format!("my-{}", draft.name)
                };
                self.ui.ask(
                    PromptKind::SaveTheme,
                    "\u{25d0} save theme as",
                    name,
                    "saved as [themes.<name>] in your config",
                );
            }
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
            "help.release_notes" => match self.installed.clone().or_else(|| self.update.clone()) {
                Some(release) => self.show_release_notes(&release),
                None => {
                    self.editor
                        .set_status("getting the release notes from github");
                    self.updater.notes(None);
                }
            },
            "update.check" => {
                self.editor.set_status("checking for updates");
                self.updater.check(true, self.ui.config.updates.install);
            }
            release_notes::UPDATE_COMMAND => match self.update.clone() {
                Some(release) => {
                    let message = match update::cannot_install() {
                        Some(reason) => format!("can not update: {reason}"),
                        None if self.updater.install(release) => {
                            "downloading the update in the background".to_owned()
                        }
                        None => "already updating, hang on".to_owned(),
                    };
                    self.editor.set_status(message);
                }
                None => {
                    self.editor.set_status("checking for updates");
                    self.updater.check(true, true);
                }
            },
            release_notes::OPEN_COMMAND => {
                let url = self
                    .ui
                    .release_notes
                    .as_ref()
                    .map_or("https://github.com/SpideyZac/mog/releases", |notes| {
                        notes.url.as_str()
                    });
                if let Err(err) = open::that_detached(url) {
                    self.editor
                        .set_status(format!("could not open {url}: {err}"));
                }
            }
            "annotate.toggle" => {
                self.ui.annotate.toggle();
                if self.ui.annotate.active {
                    self.editor.set_status(
                        "drawing: left drag draws, right drag erases, 1 to 8 pick a color, esc \
                         when done",
                    );
                }
            }
            "annotate.clear" => self.ui.annotate.clear(),
            "terminal.toggle" => {
                let focused = self.ui.terminal_open && self.ui.focus == Focus::Terminal;
                self.ui.terminal_open = !focused;
                self.ui.focus = if focused {
                    Focus::Editor
                } else {
                    Focus::Terminal
                };
            }
            "project_search.open" => self.open_project_search(false),
            "project_search.replace" => self.open_project_search(true),
            project_search_panel::RUN_COMMAND => self.run_project_search(),
            project_search_panel::PICK_COMMAND => self.pick_project_match(),
            project_search_panel::REPLACE_ALL_COMMAND => self.ask_replace_in_project(),
            "terminal.restart" => {
                self.ui.terminal_restart = true;
                self.ui.terminal_open = true;
                self.ui.focus = Focus::Terminal;
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

#[cfg(test)]
/// Scripted tests that drive the whole app through terminal events.
mod tests {
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use clap::Parser;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mog_config::Config;
    use mog_core::{Key, KeyChord};
    use mog_tui::PromptKind;

    use super::App;
    use crate::{
        cli::Args,
        session::{State, Swap},
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
            text: "saved\nand more\n".into(),
        };
        let swap_file = dir.join("state").join("swap").join("left.json");
        fs::create_dir_all(swap_file.parent().expect("folder")).expect("swap dir");
        fs::write(&swap_file, serde_json::to_string(&swap).expect("json")).expect("swap");
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
        use ratatui::{Terminal, backend::TestBackend};

        use super::{FRAME_TIME, IDLE_FRAME_TIME};

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
        let start = std::time::Instant::now();
        for _ in 0..frames {
            app.compositor.tick(FRAME_TIME);
            terminal
                .draw(|frame| {
                    let mut cx = mog_tui::Context {
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
}
