//! The terminal user interface of mog.
//!
//! Everything that draws to the screen or reads terminal input lives here.

pub mod compositor;
pub mod editor_view;
pub mod explorer;
pub mod icons;
pub mod input;
pub mod picker;
pub mod popup;
pub mod popups;
pub mod search;
pub mod settings;
pub mod status_line;
pub mod tabs;
pub mod theme;
pub mod ui;

pub use compositor::{Compositor, Context, EventResult, Layer};
pub use editor_view::EditorView;
pub use explorer::Explorer;
pub use popups::Popups;
pub use search::SearchBar;
pub use settings::{SettingKey, SettingsPanel};
pub use status_line::StatusLine;
pub use tabs::Tabs;
pub use theme::Theme;
pub use ui::{CommandInfo, Focus, Layout, Overlay, Segment, Side, Ui, UiEvent};
