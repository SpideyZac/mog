//! The terminal user interface of mog.
//!
//! Everything that draws to the screen or reads terminal input lives here.

pub mod compositor;
pub mod editor_view;
pub mod explorer;
pub mod icons;
pub mod input;
pub mod status_line;
pub mod theme;
pub mod ui;

pub use compositor::{Compositor, Context, EventResult, Layer};
pub use editor_view::EditorView;
pub use explorer::Explorer;
pub use status_line::StatusLine;
pub use theme::Theme;
pub use ui::{Focus, Layout, Overlay, Segment, Side, Ui, UiEvent};
