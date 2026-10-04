//! Drawing helpers shared by popups.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

use crate::theme::Theme;

/// Returns a `width` by `height` box centered horizontally in `screen`, a little above the
/// middle, clamped to fit.
pub fn centered(screen: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(screen.width.saturating_sub(4)).max(1);
    let height = height.min(screen.height.saturating_sub(2)).max(1);
    let x = screen.x + (screen.width - width) / 2;
    let y = screen.y + (screen.height - height) / 4;
    Rect::new(x, y, width, height)
}

/// Clears `area`, draws a rounded border titled `title` and returns the inside.
pub fn frame(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str) -> Rect {
    Clear.render(area, buf);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(theme.popup_border)
        .style(theme.popup)
        .title(Line::from(Span::styled(
            format!(" {title} "),
            theme.popup_title,
        )));
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// Dims everything outside `area` so the popup stands out.
pub fn dim_around(screen: Rect, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let dim = theme.popup_dim;
    for y in screen.top()..screen.bottom() {
        for x in screen.left()..screen.right() {
            if !area.contains((x, y).into()) {
                buf[(x, y)].set_style(dim);
            }
        }
    }
}

#[cfg(test)]
/// Tests for popup helpers.
mod tests {
    use ratatui::layout::Rect;

    use super::centered;

    /// Boxes are centered and clamped to the screen.
    #[test]
    fn centers_and_clamps() {
        assert_eq!(
            centered(Rect::new(0, 0, 100, 40), 60, 20),
            Rect::new(20, 5, 60, 20)
        );
        assert_eq!(
            centered(Rect::new(0, 0, 20, 10), 60, 20),
            Rect::new(2, 0, 16, 8)
        );
    }
}
