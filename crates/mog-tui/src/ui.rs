//! State shared by layers that is not part of the editing model.

use std::{collections::HashMap, path::PathBuf};

use mog_config::Config;
use mog_core::Command;
use mog_git::FileStatus;
use ratatui::{
    layout::{Position, Rect},
    style::Style,
};

use crate::{search::SearchState, settings::SettingKey, status_line::STATUS_HEIGHT};

/// The widest the file explorer gets, in cells.
const EXPLORER_MAX_WIDTH: u16 = 30;

/// The width of the minimap, in cells.
const MINIMAP_WIDTH: u16 = 14;

/// The narrowest the editor can be and still get a minimap.
const MINIMAP_MIN_EDITOR: u16 = 70;

/// The height of the tab bar.
const TABS_HEIGHT: u16 = 1;

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
    /// The go to line prompt.
    GotoLine,
    /// The project graph.
    Graph,
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
    /// The document text with its gutter.
    pub editor: Rect,
    /// The minimap, empty when hidden.
    pub minimap: Rect,
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
        let explorer = Rect {
            width: explorer_width,
            ..body
        };
        let right = Rect {
            x: body.x + explorer_width,
            width: body.width - explorer_width,
            ..body
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
        let editor = Rect {
            width: main.width - minimap_width,
            ..main
        };
        let minimap = Rect {
            x: editor.right(),
            width: minimap_width,
            ..main
        };
        Layout {
            screen,
            tabs,
            explorer,
            editor,
            minimap,
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
}
