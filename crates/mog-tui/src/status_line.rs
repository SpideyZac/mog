//! The bar at the bottom of the screen.

use mog_core::{Severity, view};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

use crate::{
    compositor::{Context, Layer},
    ui::{Layout, Side, Ui},
};

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
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.status
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
        let mut left = vec![
            Span::styled(" mog ", theme.status_badge),
            Span::raw(format!(" {}{modified} ", document.name())),
        ];
        let mut right = Vec::new();
        for segment in &cx.ui.segments {
            let span = Span::styled(format!("{} ", segment.text), segment.style);
            match segment.side {
                Side::Left => left.push(span),
                Side::Right => right.push(span),
            }
        }
        left.push(Span::styled(
            cx.editor.status().unwrap_or_default().to_owned(),
            theme.status_message,
        ));
        Line::from(left).render(area, buf);

        let count = |severity| {
            document
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.severity == severity)
                .count()
        };
        for (severity, label, style) in [
            (Severity::Error, "E", theme.error),
            (Severity::Warning, "W", theme.warning),
        ] {
            let n = count(severity);
            if n > 0 {
                right.push(Span::styled(format!("{label}{n} "), style));
            }
        }
        if let Some(branch) = &cx.ui.branch {
            right.push(Span::styled(
                format!("\u{2387} {branch} "),
                theme.status_message,
            ));
        }
        right.push(Span::raw(format!("Ln {line}, Col {col} ")));
        Line::from(right).right_aligned().render(area, buf);
    }
}
