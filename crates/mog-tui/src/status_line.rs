//! The bar at the bottom of the screen.

use mog_core::view;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::compositor::{Context, Layer};

/// The number of rows the status line takes.
pub const STATUS_HEIGHT: u16 = 1;

/// Shows the file name, status messages and the cursor position.
#[derive(Debug, Default)]
pub struct StatusLine;

impl StatusLine {
    /// Creates the status line.
    pub fn new() -> Self {
        Self
    }
}

impl Layer for StatusLine {
    fn area(&self, screen: Rect) -> Rect {
        let height = STATUS_HEIGHT.min(screen.height);
        Rect {
            y: screen.bottom() - height,
            height,
            ..screen
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let document = cx.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let line = text.char_to_line(head) + 1;
        let col = view::visual_col(text, head, cx.editor.options().tab_width) + 1;
        let modified = if document.is_modified() { " [+]" } else { "" };

        buf.set_style(area, theme.status);
        let left = Line::from(vec![
            Span::styled(" mog ", theme.status_badge),
            Span::raw(format!(" {}{modified} ", document.name())),
            Span::styled(
                cx.editor.status().unwrap_or_default().to_owned(),
                theme.status_message,
            ),
        ]);
        left.render(area, buf);
        Line::from(format!("Ln {line}, Col {col} "))
            .right_aligned()
            .render(area, buf);
    }
}
