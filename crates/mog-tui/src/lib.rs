//! The terminal user interface of mog.
//!
//! Everything that draws to the screen or reads terminal input lives here.

pub mod annotate;
pub mod chat;
pub mod completion;
pub mod compositor;
pub mod editor_view;
pub mod explorer;
pub mod ghost;
pub mod git_panel;
pub mod highlight;
pub mod icons;
pub mod input;
pub mod menu;
pub mod minimap;
pub mod output;
pub mod picker;
pub mod popup;
pub mod popups;
pub mod project_search;
pub mod release_notes;
pub mod search;
pub mod settings;
pub mod status_line;
pub mod tabs;
pub mod theme;
pub mod theme_editor;
pub mod ui;

pub use annotate::Annotations;
pub use chat::ChatPanel;
pub use completion::CompletionMenu;
pub use compositor::{Compositor, Context, EventResult, Layer};
pub use editor_view::EditorView;
pub use explorer::Explorer;
pub use ghost::Ghost;
pub use git_panel::GitPanel;
pub use menu::ContextMenu;
pub use minimap::Minimap;
pub use output::OutputPanel;
pub use popups::Popups;
pub use project_search::ProjectSearchPanel;
pub use release_notes::ReleaseNotesPopup;
pub use search::SearchBar;
pub use settings::{SettingKey, SettingsPanel};
pub use status_line::StatusLine;
pub use tabs::Tabs;
pub use theme::Theme;
pub use theme_editor::ThemeEditor;
pub use ui::{
    CommandInfo, CopilotState, Focus, Layout, Overlay, Pane, Prompt, PromptKind, Segment, Side,
    SignatureHint, SplitState, SymbolEntry, Ui, UiEvent,
};
