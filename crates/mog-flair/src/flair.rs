//! The [`Flair`] trait every widget implements.

use std::time::Duration;

use mog_core::Editor;
use mog_tui::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

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
}

impl Placement {
    /// Returns the area this placement covers within `editor_area`.
    pub fn area(self, editor_area: Rect) -> Rect {
        match self {
            Self::Overlay => editor_area,
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
}

/// A silly or cool widget drawn on top of the editor.
///
/// Flairs are purely decorative. They never take input and never change the editor.
pub trait Flair {
    /// Returns a unique name, used to turn the flair off in the config.
    fn id(&self) -> &str;

    /// Returns where the flair is drawn.
    fn placement(&self) -> Placement;

    /// Advances animations by `dt`.
    fn tick(&mut self, _dt: Duration) {}

    /// Returns `true` while the flair needs [`Flair::tick`] calls and redraws.
    fn is_animating(&self) -> bool {
        false
    }

    /// Draws the flair into `buf` within `area`.
    fn render(&self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>);
}

#[cfg(test)]
/// Tests for [`Placement`].
mod tests {
    use ratatui::layout::Rect;

    use super::{Corner, Placement};

    /// Corner boxes hug the right edges and get clamped to the area.
    #[test]
    fn corner_area() {
        let area = Rect::new(0, 0, 80, 20);
        let placement = Placement::Corner {
            corner: Corner::BottomRight,
            width: 10,
            height: 3,
        };
        assert_eq!(placement.area(area), Rect::new(70, 17, 10, 3));
        let tiny = Rect::new(0, 0, 4, 2);
        assert_eq!(placement.area(tiny), Rect::new(0, 0, 4, 2));
    }
}
