//! The [`Flair`] trait every widget implements.

use std::time::Duration;

use mog_core::Editor;
use mog_tui::{Layout, Segment, Side, Theme, Ui, UiEvent};
use ratatui::{buffer::Buffer, layout::Rect};

/// Which corner of the editor area a flair sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    /// The top left corner.
    TopLeft,
    /// The top right corner.
    TopRight,
    /// The bottom left corner, just above the status line.
    BottomLeft,
    /// The bottom right corner, just above the status line.
    BottomRight,
}

/// Where on screen a flair is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// A fixed size box in a corner of the editor area.
    Corner {
        /// The corner to sit in.
        corner: Corner,
        /// The width of the box in cells.
        width: u16,
        /// The height of the box in cells.
        height: u16,
    },
    /// The whole editor area, for things that wander over the text. Only cells the flair draws
    /// are changed.
    Overlay,
    /// The whole screen, for things like screensavers.
    Screen,
    /// A piece of the status line, see [`Flair::segment`].
    Status(Side),
}

impl Placement {
    /// Returns the area this placement covers on a screen split like `layout`.
    pub fn area(self, layout: &Layout) -> Rect {
        let editor_area = layout.editor;
        match self {
            Self::Overlay => editor_area,
            Self::Screen => layout.screen,
            Self::Status(_) => layout.status,
            Self::Corner {
                corner,
                width,
                height,
            } => {
                let width = width.min(editor_area.width);
                let height = height.min(editor_area.height);
                let x = match corner {
                    Corner::TopLeft | Corner::BottomLeft => editor_area.x,
                    Corner::TopRight | Corner::BottomRight => editor_area.right() - width,
                };
                let y = match corner {
                    Corner::TopLeft | Corner::TopRight => editor_area.y,
                    Corner::BottomLeft | Corner::BottomRight => editor_area.bottom() - height,
                };
                Rect::new(x, y, width, height)
            }
        }
    }
}

/// What a flair can look at while drawing.
pub struct FlairContext<'a> {
    /// The editor state, read only.
    pub editor: &'a Editor,
    /// The active theme.
    pub theme: &'a Theme,
    /// The shared ui state, read only.
    pub ui: &'a Ui,
    /// Where everything is on screen.
    pub layout: Layout,
}

/// A silly or cool widget drawn on top of the editor.
///
/// Flairs are purely decorative. They never take input and never change the editor, but they
/// can react to what happens through [`Flair::observe`].
pub trait Flair {
    /// Returns a unique name, used to turn the flair off in the config.
    fn id(&self) -> &str;

    /// Returns a short sentence about what the flair does, for the settings menu.
    fn description(&self) -> &str {
        ""
    }

    /// Returns where the flair is drawn.
    fn placement(&self) -> Placement;

    /// Reacts to something that happened, like typing or saving.
    fn observe(&mut self, _event: &UiEvent) {}

    /// Advances animations by `dt`.
    fn tick(&mut self, _dt: Duration) {}

    /// Returns `true` while the flair needs [`Flair::tick`] calls and redraws.
    fn is_animating(&self) -> bool {
        false
    }

    /// Draws the flair into `buf` within `area`. Not called for status line flairs.
    fn render(&mut self, _area: Rect, _buf: &mut Buffer, _cx: &FlairContext<'_>) {}

    /// Returns the status line piece of a [`Placement::Status`] flair.
    fn segment(&mut self, _cx: &FlairContext<'_>) -> Option<Segment> {
        None
    }
}

#[cfg(test)]
/// Tests for [`Placement`].
mod tests {
    use mog_tui::Layout;
    use ratatui::layout::Rect;

    use super::{Corner, Placement};

    /// Corner boxes hug the right edges and get clamped to the area.
    #[test]
    fn corner_area() {
        let layout = Layout {
            editor: Rect::new(0, 0, 80, 20),
            ..Layout::default()
        };
        let placement = Placement::Corner {
            corner: Corner::BottomRight,
            width: 10,
            height: 3,
        };
        assert_eq!(placement.area(&layout), Rect::new(70, 17, 10, 3));
        let tiny = Layout {
            editor: Rect::new(0, 0, 4, 2),
            ..Layout::default()
        };
        assert_eq!(placement.area(&tiny), Rect::new(0, 0, 4, 2));
    }
}
