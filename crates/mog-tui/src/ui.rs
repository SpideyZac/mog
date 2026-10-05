//! State shared by layers that is not part of the editing model.

use std::{collections::HashMap, path::PathBuf};

use mog_config::Config;
use mog_core::{Command, View};
use mog_git::FileStatus;
use ratatui::{
    layout::{Position, Rect},
    style::Style,
};

use crate::{
    annotate::AnnotateState, chat::ChatState, completion::CompletionState, ghost::Ghost,
    menu::MenuState, project_search::ProjectSearchState, release_notes::ReleaseNotes,
    search::SearchState, settings::SettingKey, status_line::STATUS_HEIGHT,
    theme_editor::ThemeDraft,
};

/// The widest the file explorer gets, in cells.
const EXPLORER_MAX_WIDTH: u16 = 30;

/// The widest the AI chat panel gets, in cells.
const CHAT_MAX_WIDTH: u16 = 52;

/// The width of the minimap, in cells.
const MINIMAP_WIDTH: u16 = 14;

/// The narrowest the editor can be and still get a minimap.
const MINIMAP_MIN_EDITOR: u16 = 70;

/// The height of the tab bar.
const TABS_HEIGHT: u16 = 1;

/// The fewest rows the editor keeps when the terminal panel is open.
const TERMINAL_MIN_EDITOR: u16 = 6;

/// The part of the screen that takes keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The document being edited.
    #[default]
    Editor,
    /// The file explorer.
    Explorer,
    /// The search bar.
    Search,
    /// The AI chat input.
    Chat,
    /// The terminal panel.
    Terminal,
}

/// One of the two editor panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    /// The left pane, the only one when there is no split.
    #[default]
    Main,
    /// The right pane of a split.
    Side,
}

/// A side by side split.
///
/// The focused pane always shows the editor's focused document. The other pane remembers which
/// document it shows and how it was scrolled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitState {
    /// The pane that has focus.
    pub focused: Pane,
    /// The document index shown in the other pane.
    pub other_document: usize,
    /// The scroll position of the other pane.
    pub other_view: View,
}

/// A popup that covers the screen and takes all input while open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// The command palette.
    Palette,
    /// The fuzzy file finder.
    Finder,
    /// The settings menu.
    Settings,
    /// The list of key bindings.
    Keys,
    /// A one line text prompt, see [`Prompt`].
    Prompt,
    /// The project graph.
    Graph,
    /// A right click menu.
    Menu,
    /// Every diagnostic in open files.
    Problems,
    /// The places a symbol is used.
    References,
    /// The theme editor.
    ThemeEditor,
    /// The notes of a mog release.
    ReleaseNotes,
    /// Find and replace across the project.
    ProjectSearch,
    /// The symbols of the focused file, an outline to jump around in.
    Symbols,
    /// Symbols from the whole project, searched as you type.
    WorkspaceSymbols,
}

/// Whether Copilot can be used, as shown in the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotState {
    /// The server is starting.
    Starting,
    /// Signed in and working.
    Ready,
    /// Nobody is signed in.
    SignedOut,
    /// Something is wrong.
    Problem,
}

/// What a [`Prompt`] is asking for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// A line number to jump to.
    GotoLine,
    /// Where to save the focused document.
    SaveAs,
    /// The name of a new file in a folder.
    NewFile(PathBuf),
    /// A new name for a file or folder.
    RenameFile(PathBuf),
    /// Confirmation to delete a file or folder.
    DeleteFile(PathBuf),
    /// A new name for the symbol under the cursor.
    RenameSymbol,
    /// Confirmation to open GitHub and finish signing in to Copilot.
    CopilotSignIn,
    /// The name to save the theme being edited under.
    SaveTheme,
    /// Whether to trust the project config.
    TrustProject,
    /// Confirmation to replace every match of a project search.
    ReplaceAll,
    /// Confirmation to run a command that installs a missing language server.
    InstallServer(String),
}

/// The signature of the call around the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHint {
    /// The whole signature, like `fn add(a: i32, b: i32) -> i32`.
    pub label: String,
    /// The chars of `label` that name the parameter being typed, as `(from, to)`.
    pub active: Option<(usize, usize)>,
    /// What the function does, if the server says.
    pub documentation: Option<String>,
    /// The line the call is on, so the hint goes away when the cursor leaves it.
    pub line: usize,
}

/// A symbol in a symbol picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    /// The symbol name.
    pub name: String,
    /// A short name for its kind, like `fn`.
    pub kind: String,
    /// Extra detail like a signature or the containing type.
    pub detail: String,
    /// How deep it is nested inside other symbols.
    pub depth: usize,
    /// The file it is in.
    pub path: PathBuf,
    /// The line it is on, from 0.
    pub line: usize,
    /// The column its name starts at, from 0.
    pub column: usize,
}

/// A one line question, like where to save a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// What the answer is for.
    pub kind: PromptKind,
    /// The popup title.
    pub title: String,
    /// What has been typed so far.
    pub text: String,
    /// A quiet hint under the input.
    pub hint: String,
}

/// A command the palette can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInfo {
    /// The command name, like `save`.
    pub name: String,
    /// What the palette shows, like `File: Save`.
    pub title: String,
    /// The chords bound to the command, like `ctrl+s`.
    pub keys: Vec<String>,
}

/// Which end of the status line a [`Segment`] goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// After the file name.
    Left,
    /// Before the cursor position.
    Right,
}

/// A piece of text other layers add to the status line for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The pieces of text and how to draw each.
    pub parts: Vec<(String, Style)>,
    /// Which end it goes on.
    pub side: Side,
}

impl Segment {
    /// Creates a segment of one piece of text.
    pub fn new(text: impl Into<String>, style: Style, side: Side) -> Self {
        Self {
            parts: vec![(text.into(), style)],
            side,
        }
    }
}

/// Something that happened since the last frame, for flair and sound to react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// A char was typed.
    Typed(char),
    /// Text was deleted.
    Deleted,
    /// A document was saved.
    Saved,
    /// A file was opened or focused.
    Opened,
    /// A key was pressed or the mouse was used.
    Activity,
    /// The number of problems in the focused document changed.
    Diagnostics {
        /// The number of errors.
        errors: usize,
        /// The number of warnings.
        warnings: usize,
    },
}

/// Where each part of the screen goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layout {
    /// The whole screen.
    pub screen: Rect,
    /// The tab bar, empty when hidden.
    pub tabs: Rect,
    /// The file explorer, empty when hidden.
    pub explorer: Rect,
    /// Room under the file explorer for flair, empty when hidden or unused.
    pub explorer_footer: Rect,
    /// The document text with its gutter, the left pane when split.
    pub editor: Rect,
    /// The right pane of a split, empty when not split.
    pub split: Rect,
    /// The minimap, empty when hidden.
    pub minimap: Rect,
    /// The AI chat panel, empty when hidden.
    pub chat: Rect,
    /// The terminal panel under the editor, empty when hidden.
    pub terminal: Rect,
    /// The status line.
    pub status: Rect,
}

/// State shared by every layer.
#[derive(Debug, Clone, Default)]
pub struct Ui {
    /// The live settings. Changing them here changes the editor right away.
    pub config: Config,
    /// What takes keyboard input.
    pub focus: Focus,
    /// The open popup, if any.
    pub overlay: Option<Overlay>,
    /// Bumped every time a popup opens so it can reset itself.
    pub overlay_generation: u64,
    /// The project folder, or the folder mog was started in.
    pub root: PathBuf,
    /// Every command the palette offers.
    pub commands: Vec<CommandInfo>,
    /// Every key binding as `(chord, command name)`.
    pub bindings: Vec<(String, String)>,
    /// Whether the terminal sends keys the old way, so some chords cannot reach mog.
    pub legacy_keys: bool,
    /// Whether a folder is open, so there is something to explore.
    pub has_explorer: bool,
    /// Whether the file explorer is shown.
    pub explorer_open: bool,
    /// Commands layers want the app to run after the current event.
    pub requests: Vec<Command>,
    /// Things that happened since the last frame.
    pub events: Vec<UiEvent>,
    /// Status line pieces added by layers this frame.
    pub segments: Vec<Segment>,
    /// Set when files changed on disk so the explorer reads its folders again.
    pub refresh_explorer: bool,
    /// The git status of changed files by absolute path.
    pub git_status: HashMap<PathBuf, FileStatus>,
    /// The text of open files in the last commit, used to mark changed lines.
    pub git_base: HashMap<PathBuf, String>,
    /// The checked out git branch, if the project is a repository.
    pub branch: Option<String>,
    /// Who last changed the cursor line, as `(file, line, description)`.
    pub blame: Option<(PathBuf, usize, String)>,
    /// The find and replace state.
    pub search: SearchState,
    /// Every flair as `(id, description)`, for the settings menu.
    pub flairs: Vec<(String, String)>,
    /// Settings changed in the menu that the app still has to apply and save.
    pub setting_changes: Vec<SettingKey>,
    /// Where the text cursor was drawn this frame, if it is on screen.
    pub cursor_screen: Option<Position>,
    /// The open completion menu.
    pub completion: Option<CompletionState>,
    /// Hover text and the char offset it is about.
    pub hover: Option<(String, usize)>,
    /// The signature of the call being typed.
    pub signature: Option<SignatureHint>,
    /// The symbols shown in the symbol pickers.
    pub symbols: Vec<SymbolEntry>,
    /// Bumped whenever `symbols` is replaced, so an open picker refills.
    pub symbols_version: u64,
    /// What was typed in the project symbol picker, for the app to search for.
    pub symbol_query: String,
    /// The open prompt.
    pub prompt: Option<Prompt>,
    /// A prompt that was answered and waits for the app to act on it.
    pub submitted: Option<Prompt>,
    /// The open right click menu.
    pub menu: Option<MenuState>,
    /// The AI chat.
    pub chat: ChatState,
    /// Found references as `(file, line, column, line text)`, lines and columns from 0.
    pub references: Vec<(PathBuf, usize, usize, String)>,
    /// An AI suggestion shown after the cursor.
    pub ghost: Option<Ghost>,
    /// The state of Copilot, if it is turned on.
    pub copilot: Option<CopilotState>,
    /// The split, if the editor is split.
    pub split: Option<SplitState>,
    /// Whether the terminal panel is shown.
    pub terminal_open: bool,
    /// Bytes waiting to be sent to the terminal, like pasted text.
    pub terminal_input: Vec<u8>,
    /// Set to have the terminal panel start a fresh shell.
    pub terminal_restart: bool,
    /// A key binding change waiting for the app, as `(command, new chord)`.
    ///
    /// A missing chord removes every binding of the command.
    pub rebind: Option<(String, Option<String>)>,
    /// The drawing on top of the screen.
    pub annotate: AnnotateState,
    /// The theme being edited in the theme editor, previewed live.
    pub theme_draft: Option<ThemeDraft>,
    /// The rows flair wants under the file explorer.
    pub explorer_footer: u16,
    /// The column just past the last tab, where the free part of the tab bar starts.
    pub tabs_end: u16,
    /// The release notes the release notes popup shows.
    pub release_notes: Option<ReleaseNotes>,
    /// Find and replace across the project.
    pub project_search: ProjectSearchState,
}

impl Ui {
    /// Creates the shared state for `config`.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Opens `overlay`, replacing any open one.
    pub fn open(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
        self.overlay_generation += 1;
    }

    /// Closes the open popup.
    pub fn close(&mut self) {
        self.overlay = None;
        self.prompt = None;
    }

    /// Opens a prompt asking for `kind`, starting with `text` typed.
    pub fn ask(
        &mut self,
        kind: PromptKind,
        title: impl Into<String>,
        text: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.open(Overlay::Prompt);
        self.prompt = Some(Prompt {
            kind,
            title: title.into(),
            text: text.into(),
            hint: hint.into(),
        });
    }

    /// Returns the palette title for the command called `name`, or the name itself.
    pub fn title_of<'a>(&'a self, name: &'a str) -> &'a str {
        self.commands
            .iter()
            .find(|info| info.name == name)
            .map_or(name, |info| info.title.as_str())
    }

    /// Asks the app to run `command` after the current event.
    pub fn request(&mut self, command: Command) {
        self.requests.push(command);
    }

    /// Returns whether the explorer is visible.
    pub fn explorer_visible(&self) -> bool {
        self.has_explorer && self.explorer_open
    }

    /// Splits `screen` into the parts of the editor.
    pub fn layout(&self, screen: Rect) -> Layout {
        let status_height = STATUS_HEIGHT.min(screen.height);
        let status = Rect {
            y: screen.bottom() - status_height,
            height: status_height,
            ..screen
        };
        let body = Rect {
            height: screen.height - status_height,
            ..screen
        };
        let explorer_width = if self.explorer_visible() {
            EXPLORER_MAX_WIDTH.min(body.width / 3)
        } else {
            0
        };
        let footer_height = if explorer_width > 0 {
            self.explorer_footer.min(body.height / 2)
        } else {
            0
        };
        let explorer = Rect {
            width: explorer_width,
            height: body.height - footer_height,
            ..body
        };
        let explorer_footer = Rect {
            y: explorer.bottom(),
            height: footer_height,
            ..explorer
        };
        let rest = Rect {
            x: body.x + explorer_width,
            width: body.width - explorer_width,
            ..body
        };
        let chat_width = if self.chat.open {
            CHAT_MAX_WIDTH.min(rest.width / 2)
        } else {
            0
        };
        let chat = Rect {
            x: rest.right() - chat_width,
            width: chat_width,
            ..rest
        };
        let above = Rect {
            width: rest.width - chat_width,
            ..rest
        };
        let terminal_height = if self.terminal_open {
            (above.height * 2 / 5).min(above.height.saturating_sub(TERMINAL_MIN_EDITOR))
        } else {
            0
        };
        let terminal = Rect {
            y: above.bottom() - terminal_height,
            height: terminal_height,
            ..above
        };
        let right = Rect {
            height: above.height - terminal_height,
            ..above
        };
        let tabs_height = if self.config.ui.tabs {
            TABS_HEIGHT.min(right.height)
        } else {
            0
        };
        let tabs = Rect {
            height: tabs_height,
            ..right
        };
        let main = Rect {
            y: right.y + tabs_height,
            height: right.height - tabs_height,
            ..right
        };
        let minimap_width = if self.config.ui.minimap && main.width >= MINIMAP_MIN_EDITOR {
            MINIMAP_WIDTH
        } else {
            0
        };
        let both = Rect {
            width: main.width - minimap_width,
            ..main
        };
        let split_width = if self.split.is_some() {
            both.width / 2
        } else {
            0
        };
        let editor = Rect {
            width: both.width - split_width,
            ..both
        };
        let split = Rect {
            x: editor.right(),
            width: split_width,
            ..both
        };
        let minimap = Rect {
            x: both.right(),
            width: minimap_width,
            ..main
        };
        Layout {
            screen,
            tabs,
            explorer,
            explorer_footer,
            editor,
            split,
            minimap,
            chat,
            terminal,
            status,
        }
    }
}

#[cfg(test)]
/// Tests for [`Ui`].
mod tests {
    use ratatui::layout::Rect;

    use super::Ui;

    /// The explorer, tabs and minimap carve their space out of the editor.
    #[test]
    fn layout_splits_screen() {
        let ui = Ui {
            has_explorer: true,
            explorer_open: true,
            ..Ui::default()
        };
        let layout = ui.layout(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.explorer, Rect::new(0, 0, 30, 39));
        assert_eq!(layout.tabs, Rect::new(30, 0, 90, 1));
        assert_eq!(layout.editor, Rect::new(30, 1, 76, 38));
        assert_eq!(layout.minimap, Rect::new(106, 1, 14, 38));
        assert_eq!(layout.status, Rect::new(0, 39, 120, 1));
    }

    /// Flair under the explorer takes rows from it, but never more than half.
    #[test]
    fn layout_with_explorer_footer() {
        let mut ui = Ui {
            has_explorer: true,
            explorer_open: true,
            explorer_footer: 8,
            ..Ui::default()
        };
        let layout = ui.layout(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.explorer, Rect::new(0, 0, 30, 31));
        assert_eq!(layout.explorer_footer, Rect::new(0, 31, 30, 8));
        ui.explorer_footer = 100;
        let layout = ui.layout(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.explorer_footer.height, 19);
        ui.explorer_open = false;
        assert!(
            ui.layout(Rect::new(0, 0, 120, 40))
                .explorer_footer
                .is_empty()
        );
    }

    /// With everything hidden the editor gets the whole body.
    #[test]
    fn layout_without_extras() {
        let mut ui = Ui::default();
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        let layout = ui.layout(Rect::new(0, 0, 80, 24));
        assert_eq!(layout.editor, Rect::new(0, 0, 80, 23));
        assert!(layout.explorer.is_empty());
    }

    /// The terminal panel takes the bottom of the editor column.
    #[test]
    fn layout_with_terminal() {
        let mut ui = Ui {
            terminal_open: true,
            ..Ui::default()
        };
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        let layout = ui.layout(Rect::new(0, 0, 80, 41));
        assert_eq!(layout.terminal, Rect::new(0, 24, 80, 16));
        assert_eq!(layout.editor, Rect::new(0, 0, 80, 24));
    }
}
