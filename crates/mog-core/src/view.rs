//! The visible window onto a document.

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::movement;

/// How many lines are kept visible above and below the cursor when scrolling.
const SCROLL_MARGIN: usize = 3;

/// Returns how many cells `ch` takes when drawn at visual column `col`.
///
/// Tabs stretch to the next multiple of `tab_width`. Control chars are drawn as one cell.
pub fn char_width(ch: char, col: usize, tab_width: usize) -> usize {
    if ch == '\t' {
        tab_width - col % tab_width.max(1)
    } else {
        ch.width().unwrap_or(1).max(1)
    }
}

/// Returns the visual column of `pos` on its line.
pub fn visual_col(text: &Rope, pos: usize, tab_width: usize) -> usize {
    let start = movement::line_start(text, pos);
    text.slice(start..pos)
        .chars()
        .fold(0, |col, ch| col + char_width(ch, col, tab_width))
}

/// Returns the offset on `line` whose cell contains visual column `col`.
///
/// Columns past the end of the line land on the line end.
///
/// # Panics
///
/// Panics if `line` is past the last line.
pub fn pos_at_visual_col(text: &Rope, line: usize, col: usize, tab_width: usize) -> usize {
    let start = text.line_to_char(line);
    let len = movement::line_len(text, line);
    let mut cur = 0;
    for (i, ch) in text.slice(start..start + len).chars().enumerate() {
        let width = char_width(ch, cur, tab_width);
        if col < cur + width {
            // clicking the right half of a wide char feels like it should land after it
            return start + i + usize::from(col - cur >= width.div_ceil(2) && width > 1);
        }
        cur += width;
    }
    start + len
}

/// The scroll position and size of the area a document is shown in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// The first visible line.
    pub scroll_line: usize,
    /// The first visible visual column.
    pub scroll_col: usize,
    /// The number of text columns visible.
    pub width: usize,
    /// The number of lines visible.
    pub height: usize,
    /// The visual column vertical motions try to keep.
    pub preferred_col: Option<usize>,
}

impl View {
    /// Sets the visible size in cells.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
    }

    /// Scrolls by `lines`, negative for up, keeping at least one line of text on screen.
    pub fn scroll_by(&mut self, lines: isize, text: &Rope) {
        let max = text.len_lines().saturating_sub(1);
        self.scroll_line = self.scroll_line.saturating_add_signed(lines).min(max);
    }

    /// Scrolls just enough to show `pos` with a small margin around it.
    pub fn ensure_visible(&mut self, text: &Rope, pos: usize, tab_width: usize) {
        let line = text.char_to_line(pos);
        let margin = SCROLL_MARGIN.min(self.height.saturating_sub(1) / 2);
        if line < self.scroll_line + margin {
            self.scroll_line = line.saturating_sub(margin);
        } else if self.height > 0 && line + margin >= self.scroll_line + self.height {
            self.scroll_line = line + margin + 1 - self.height;
        }
        let col = visual_col(text, pos, tab_width);
        if col < self.scroll_col {
            self.scroll_col = col;
        } else if self.width > 0 && col >= self.scroll_col + self.width {
            self.scroll_col = col + 1 - self.width;
        }
    }

    /// Returns the offset shown at `row` and `col` relative to the top left of the view.
    ///
    /// Rows past the last line land on the end of the text.
    pub fn pos_at_cell(&self, text: &Rope, row: usize, col: usize, tab_width: usize) -> usize {
        let line = self.scroll_line + row;
        if line >= text.len_lines() {
            return text.len_chars();
        }
        pos_at_visual_col(text, line, self.scroll_col + col, tab_width)
    }
}

#[cfg(test)]
/// Tests for [`View`] and visual columns.
mod tests {
    use ropey::Rope;

    use super::{View, pos_at_visual_col, visual_col};

    /// Tabs expand to the next tab stop.
    #[test]
    fn tabs_expand_to_stops() {
        let text = Rope::from_str("a\tb");
        assert_eq!(visual_col(&text, 2, 4), 4);
        assert_eq!(pos_at_visual_col(&text, 0, 2, 4), 1);
        assert_eq!(pos_at_visual_col(&text, 0, 4, 4), 2);
        assert_eq!(pos_at_visual_col(&text, 0, 99, 4), 3);
    }

    /// The view scrolls down to keep the cursor and its margin visible.
    #[test]
    fn ensure_visible_scrolls_down() {
        let text = Rope::from_str(&"x\n".repeat(50));
        let mut view = View::default();
        view.resize(10, 10);
        view.ensure_visible(&text, text.line_to_char(20), 4);
        assert_eq!(view.scroll_line, 14);
        view.ensure_visible(&text, 0, 4);
        assert_eq!(view.scroll_line, 0);
    }

    /// Cells below the text map to the end.
    #[test]
    fn pos_at_cell_past_end() {
        let text = Rope::from_str("ab\ncd");
        let view = View::default();
        assert_eq!(view.pos_at_cell(&text, 1, 1, 4), 4);
        assert_eq!(view.pos_at_cell(&text, 9, 0, 4), 5);
    }
}
