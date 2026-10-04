//! A zoomed out view of the focused document on the right.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Severity, movement};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use crate::{
    compositor::{Context, EventResult, Layer},
    ui::{Layout, Ui},
};

/// Document lines per minimap row, one per braille dot row.
const LINES_PER_ROW: usize = 4;

/// Document chars per minimap column, one per braille dot column.
const CHARS_PER_COL: usize = 2;

/// The braille dot bits as `[column][row]`.
const DOTS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// The first braille code point.
const BRAILLE: u32 = 0x2800;

/// Returns the first document line shown at the top of a minimap `rows` tall.
///
/// When the document is taller than the minimap, the minimap scrolls in proportion to the view.
fn first_line(lines: usize, rows: usize, scroll: usize, view_height: usize) -> usize {
    let capacity = rows * LINES_PER_ROW;
    if lines <= capacity {
        return 0;
    }
    let max_scroll = lines.saturating_sub(view_height).max(1);
    let max_first = lines - capacity;
    max_first * scroll.min(max_scroll) / max_scroll
}

/// Draws the minimap.
#[derive(Debug, Default)]
pub struct Minimap {
    /// The first document line shown, from the last render.
    first: usize,
}

impl Minimap {
    /// Creates the minimap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scrolls the editor so the line under minimap row `row` is centered.
    fn jump(&self, row: u16, area: Rect, cx: &mut Context<'_>) {
        let row = usize::from(row.saturating_sub(area.y));
        let target = self.first + row * LINES_PER_ROW;
        let view = cx.editor.view();
        let wanted = target.saturating_sub(view.height / 2);
        let delta = isize::try_from(wanted).unwrap_or(isize::MAX)
            - isize::try_from(view.scroll_line).unwrap_or(isize::MAX);
        cx.editor.execute(Command::Scroll(delta));
    }
}

impl Layer for Minimap {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.minimap
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let document = cx.editor.document();
        let text = document.text();
        let lines = text.len_lines();
        let view = cx.editor.view();
        let rows = usize::from(area.height);
        self.first = first_line(lines, rows, view.scroll_line, view.height);
        let view_lines = view.scroll_line..view.scroll_line + view.height;
        let error_lines: Vec<(usize, Severity)> = document
            .diagnostics()
            .iter()
            .map(|d| (text.char_to_line(d.from.min(text.len_chars())), d.severity))
            .collect();
        let match_lines: Vec<usize> = cx
            .ui
            .search
            .matches
            .iter()
            .map(|(from, _)| text.char_to_line(*from))
            .collect();
        let cols = usize::from(area.width.saturating_sub(1));
        for row in 0..rows {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            let top = self.first + row * LINES_PER_ROW;
            let row_lines = top..top + LINES_PER_ROW;
            let in_view = row_lines
                .clone()
                .any(|line| line < lines && view_lines.contains(&line));
            let base = if in_view {
                theme.minimap.patch(theme.minimap_view)
            } else {
                theme.minimap.patch(theme.background)
            };
            buf.set_style(Rect::new(area.x, y, area.width, 1), base);
            let worst = error_lines
                .iter()
                .filter(|(line, _)| row_lines.contains(line))
                .map(|(_, severity)| *severity)
                .min();
            let found = match_lines.iter().any(|line| row_lines.contains(line));
            let fg = match worst {
                Some(Severity::Error) => theme.error,
                Some(Severity::Warning) => theme.warning,
                _ if found => theme
                    .search_current
                    .bg
                    .map_or(theme.minimap, |c| Style::new().fg(c)),
                _ => Style::new(),
            };
            // a thin edge marks the rows the view covers
            let edge = if in_view { "\u{258f}" } else { " " };
            buf.set_string(area.x, y, edge, base.patch(theme.gutter_active));
            for col in 0..cols {
                let mut bits = 0;
                for (dot_row, line) in row_lines.clone().enumerate() {
                    if line >= lines {
                        break;
                    }
                    let start = text.line_to_char(line);
                    let len = movement::line_len(text, line);
                    for (dot_col, bit) in DOTS.iter().enumerate() {
                        let at = col * CHARS_PER_COL + dot_col;
                        let filled = at < len
                            && text
                                .get_char(start + at)
                                .is_some_and(|ch| !ch.is_whitespace());
                        if filled {
                            bits |= bit[dot_row];
                        }
                    }
                }
                if bits == 0 {
                    continue;
                }
                let symbol = char::from_u32(BRAILLE + bits).unwrap_or(' ');
                let x = area.x + 1 + u16::try_from(col).unwrap_or(u16::MAX);
                buf.set_string(x, y, symbol.to_string(), base.patch(fg));
            }
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, cx: &mut Context<'_>) -> EventResult {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                self.jump(event.row, area, cx);
            }
            MouseEventKind::ScrollUp => {
                cx.editor.execute(Command::Scroll(-6));
            }
            MouseEventKind::ScrollDown => {
                cx.editor.execute(Command::Scroll(6));
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for the minimap.
mod tests {
    use super::first_line;

    /// Short documents start at the top and long ones scroll with the view.
    #[test]
    fn scrolls_in_proportion() {
        assert_eq!(first_line(50, 20, 10, 30), 0);
        assert_eq!(first_line(1000, 20, 0, 40), 0);
        assert_eq!(first_line(1000, 20, 960, 40), 920);
        assert_eq!(first_line(1000, 20, 480, 40), 460);
    }
}
