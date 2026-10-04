//! The layer that shows the focused document.

use mog_core::{movement, view};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use crate::compositor::{Context, Layer};
use crate::status_line::STATUS_HEIGHT;

/// Blank cells between the line numbers and the text.
const GUTTER_PADDING: usize = 2;

/// Converts a cell count to a terminal coordinate, saturating on overflow.
fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// Draws the focused document with line numbers, selection and the cursor line.
#[derive(Debug, Default)]
pub struct EditorView {
    /// The gutter width used in the last render, needed to map mouse clicks to text.
    gutter_width: u16,
}

impl EditorView {
    /// Creates the editor view.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the width of the line number gutter for a document with `lines` lines.
    fn gutter_width_for(lines: usize) -> u16 {
        cells(lines.to_string().len() + GUTTER_PADDING)
    }
}

impl Layer for EditorView {
    fn area(&self, screen: Rect) -> Rect {
        Rect {
            height: screen.height.saturating_sub(STATUS_HEIGHT),
            ..screen
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let lines = cx.editor.document().text().len_lines();
        self.gutter_width = Self::gutter_width_for(lines).min(area.width);
        let text_width = area.width - self.gutter_width;
        cx.editor
            .view_mut()
            .resize(usize::from(text_width), usize::from(area.height));

        let tab_width = cx.editor.options().tab_width;
        let document = cx.editor.document();
        let text = document.text();
        let selection = document.selection();
        let cursor_line = text.char_to_line(selection.head);
        let scroll = cx.editor.view();

        for row in 0..area.height {
            let line = scroll.scroll_line + usize::from(row);
            if line >= lines {
                break;
            }
            let y = area.y + row;
            let is_cursor_line = line == cursor_line;
            if is_cursor_line {
                buf.set_style(Rect::new(area.x, y, area.width, 1), theme.cursor_line);
            }

            let number = format!(
                "{:>width$} ",
                line + 1,
                width = usize::from(self.gutter_width).saturating_sub(1)
            );
            let number_style = if is_cursor_line {
                theme.gutter_active
            } else {
                theme.gutter
            };
            buf.set_stringn(
                area.x,
                y,
                number,
                usize::from(self.gutter_width),
                number_style,
            );

            let text_x = area.x + self.gutter_width;
            let start = text.line_to_char(line);
            let len = movement::line_len(text, line);
            let mut col = 0;
            for (i, ch) in text.slice(start..start + len).chars().enumerate() {
                let width = view::char_width(ch, col, tab_width);
                let style = if (selection.from()..selection.to()).contains(&(start + i)) {
                    theme.text.patch(theme.selection)
                } else {
                    theme.text
                };
                for cell in col..col + width {
                    if cell < scroll.scroll_col {
                        continue;
                    }
                    let x = cell - scroll.scroll_col;
                    if x >= usize::from(text_width) {
                        break;
                    }
                    let symbol = if ch == '\t' || ch.is_control() {
                        " ".to_owned()
                    } else if cell == col {
                        ch.to_string()
                    } else {
                        // the trailing half of a wide char is drawn by its first cell
                        continue;
                    };
                    buf.set_string(text_x + cells(x), y, symbol, style);
                }
                col += width;
            }

            // show selected line breaks as one highlighted cell like most editors
            let line_end = start + len;
            if selection.from() <= line_end
                && line_end < selection.to()
                && col >= scroll.scroll_col
                && col - scroll.scroll_col < usize::from(text_width)
            {
                let x = text_x + cells(col - scroll.scroll_col);
                buf.set_style(Rect::new(x, y, 1, 1), theme.selection);
            }
        }
    }

    fn cursor(&self, area: Rect, cx: &Context<'_>) -> Option<Position> {
        let document = cx.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let scroll = cx.editor.view();
        let line = text.char_to_line(head).checked_sub(scroll.scroll_line)?;
        let col = view::visual_col(text, head, cx.editor.options().tab_width)
            .checked_sub(scroll.scroll_col)?;
        let x = usize::from(self.gutter_width) + col;
        if line >= usize::from(area.height) || x >= usize::from(area.width) {
            return None;
        }
        Some(Position::new(area.x + cells(x), area.y + cells(line)))
    }
}

#[cfg(test)]
/// Tests for [`EditorView`].
mod tests {
    use mog_core::{Editor, MemoryClipboard, Range, Transaction};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    use super::EditorView;
    use crate::compositor::{Compositor, Context};
    use crate::theme::Theme;

    /// Text is drawn after the gutter with tabs expanded and the cursor placed.
    #[test]
    fn renders_text_and_cursor() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let document = editor.document_mut();
        document.apply(Transaction::insert(0, "hi\n\tyo"), Range::point(6), false);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(12, 3)).expect("test terminal");
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor: &mut editor,
                    theme: &theme,
                };
                compositor.render(frame, &mut cx);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..2)
            .map(|y| (0..12).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert_eq!(rows, [" 1 hi       ", " 2     yo   "]);
        terminal
            .backend_mut()
            .assert_cursor_position(Position::new(9, 1));
    }
}
