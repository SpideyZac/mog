//! The row of open documents along the top.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::Command;
use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    icons,
    ui::{Layout, Ui},
};

/// The glyph on a tab that closes it.
const CLOSE: &str = "\u{00d7}";

/// The glyph on a tab with unsaved changes.
const MODIFIED: &str = "\u{25cf}";

/// Where a tab was drawn, for mouse hit testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TabHit {
    /// The document index.
    index: usize,
    /// The first column of the tab.
    start: u16,
    /// The column just past the tab.
    end: u16,
    /// The column of the close button.
    close: u16,
}

/// Shows a tab per open document. Click to focus, click the cross or middle click to close.
#[derive(Debug, Default)]
pub struct Tabs {
    /// Where each visible tab was drawn in the last frame.
    hits: Vec<TabHit>,
}

impl Tabs {
    /// Creates the tab bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the label of a tab, without its padding and buttons.
    fn label(name: &str, icon: bool) -> String {
        if icon && icons::file_color(name).is_some() {
            format!("{} {name}", icons::FILE_MARKER)
        } else {
            name.to_owned()
        }
    }
}

impl Layer for Tabs {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.tabs
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        buf.set_style(area, theme.tab);
        self.hits.clear();
        let icon = cx.ui.config.ui.icons;
        let active = cx.editor.active();
        let widths: Vec<u16> = cx
            .editor
            .documents()
            .iter()
            .map(|document| {
                let label = Self::label(&document.name(), icon);
                // one space each side, the label, a space and the button
                u16::try_from(label.width() + 4).unwrap_or(u16::MAX)
            })
            .collect();
        // skip tabs on the left until the focused one fits
        let mut first = 0;
        while first < active && widths[first..=active].iter().sum::<u16>() > area.width {
            first += 1;
        }
        let mut x = area.x;
        for (index, document) in cx.editor.documents().iter().enumerate().skip(first) {
            if x >= area.right() {
                break;
            }
            let base = if index == active {
                theme.tab_active
            } else {
                theme.tab
            };
            let start = x;
            let width = widths[index].min(area.right() - x);
            buf.set_style(Rect::new(x, area.y, width, 1), base);
            let right = area.right();
            let put = |buf: &mut Buffer, x: u16, text: &str, style: Style| {
                let room = usize::from(right.saturating_sub(x));
                buf.set_stringn(x, area.y, text, room, base.patch(style)).0
            };
            x = put(buf, x, " ", Style::new());
            let name = document.name();
            if let Some(color) = icons::file_color(&name).filter(|_| icon) {
                let marker = format!("{} ", icons::FILE_MARKER);
                x = put(buf, x, &marker, Style::new().fg(color));
            }
            x = put(buf, x, &name, Style::new());
            x = put(buf, x, " ", Style::new());
            let close = x;
            x = if document.is_modified() {
                put(buf, x, MODIFIED, theme.tab_modified)
            } else {
                put(buf, x, CLOSE, Style::new())
            };
            x = put(buf, x, " ", Style::new());
            if index == active {
                let underline = Rect::new(start, area.y, x - start, 1);
                buf.set_style(
                    underline,
                    Style::new()
                        .fg(theme.palette.accent)
                        .patch(theme.tab_active),
                );
            }
            self.hits.push(TabHit {
                index,
                start,
                end: x,
                close,
            });
        }
        cx.ui.tabs_end = x;
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let hit = self
            .hits
            .iter()
            .find(|hit| (hit.start..hit.end).contains(&event.column))
            .copied();
        match (event.kind, hit) {
            (MouseEventKind::Down(MouseButton::Left), Some(hit)) => {
                cx.editor.focus(hit.index);
                if event.column == hit.close {
                    cx.ui.request(Command::CloseTab);
                }
            }
            (MouseEventKind::Down(MouseButton::Middle), Some(hit)) => {
                cx.editor.focus(hit.index);
                cx.ui.request(Command::CloseTab);
            }
            (MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft, _) => {
                cx.ui.request(Command::PrevTab);
            }
            (MouseEventKind::ScrollDown | MouseEventKind::ScrollRight, _) => {
                cx.ui.request(Command::NextTab);
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for [`Tabs`].
mod tests {
    use std::env;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use mog_core::{Command, Editor, MemoryClipboard};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::Tabs;
    use crate::{
        compositor::{Compositor, Context},
        theme::Theme,
        ui::Ui,
    };

    /// Clicking a tab focuses it and clicking its cross asks to close it.
    #[test]
    fn click_focuses_and_closes() {
        let dir = env::temp_dir();
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        editor.open(dir.join("mog-tab-a.txt")).expect("open");
        editor.open(dir.join("mog-tab-b.txt")).expect("open");
        let theme = Theme::default();
        let mut ui = Ui::default();
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Tabs::new()));
        let screen = Rect::new(0, 0, 80, 10);
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).expect("terminal");
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor: &mut editor,
                    theme: &theme,
                    ui: &mut ui,
                };
                compositor.render(frame, &mut cx);
            })
            .expect("draw");
        let mut click = |column| {
            let event = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 0,
                modifiers: KeyModifiers::NONE,
            };
            let mut cx = Context {
                editor: &mut editor,
                theme: &theme,
                ui: &mut ui,
            };
            compositor.handle_mouse(event, screen, &mut cx);
        };
        // the first tab is " mog-tab-a.txt x " so its cross is at column 15
        click(2);
        click(15);
        assert_eq!(editor.active(), 0);
        assert_eq!(ui.requests, [Command::CloseTab]);
    }
}
