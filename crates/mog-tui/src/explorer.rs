//! The file explorer shown on the left when a folder is open.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::FileTree;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{
    compositor::{Context, EventResult, Layer},
    status_line::STATUS_HEIGHT,
};

/// The widest the explorer gets, in cells.
const MAX_WIDTH: u16 = 30;

/// Rows above the file list, taken by the folder name.
const HEADER_HEIGHT: u16 = 1;

/// How many rows one wheel notch scrolls.
const WHEEL_ROWS: usize = 3;

/// Returns how wide the explorer is on a screen of `screen` size.
pub fn width(screen: Rect) -> u16 {
    MAX_WIDTH.min(screen.width / 3)
}

/// Shows a [`FileTree`] and opens files that are clicked.
#[derive(Debug)]
pub struct Explorer {
    /// The folder being shown.
    tree: FileTree,
    /// The index of the first entry shown.
    scroll: usize,
}

impl Explorer {
    /// Creates an explorer showing `tree`.
    pub fn new(tree: FileTree) -> Self {
        Self { tree, scroll: 0 }
    }

    /// Returns the largest useful scroll offset.
    fn max_scroll(&self) -> usize {
        self.tree.entries().len().saturating_sub(1)
    }
}

impl Layer for Explorer {
    fn area(&self, screen: Rect) -> Rect {
        Rect {
            width: width(screen),
            height: screen.height.saturating_sub(STATUS_HEIGHT),
            ..screen
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if area.is_empty() {
            return;
        }
        let theme = cx.theme;
        buf.set_style(area, theme.sidebar);
        let border_x = area.right() - 1;
        for y in area.top()..area.bottom() {
            buf.set_string(border_x, y, "\u{2502}", theme.border);
        }
        let content = usize::from(area.width - 1);
        buf.set_stringn(
            area.x,
            area.y,
            format!(" {}", self.tree.name().to_uppercase()),
            content,
            theme.sidebar_title,
        );

        self.scroll = self.scroll.min(self.max_scroll());
        let open = cx.editor.document().path();
        let rows = area.height.saturating_sub(HEADER_HEIGHT);
        let entries = self.tree.entries().iter().skip(self.scroll);
        for (row, entry) in (0..rows).zip(entries) {
            let y = area.y + HEADER_HEIGHT + row;
            let icon = match (entry.is_dir, entry.expanded) {
                (true, true) => "\u{25be} ",
                (true, false) => "\u{25b8} ",
                (false, _) => "  ",
            };
            let label = format!(" {}{icon}{}", "  ".repeat(entry.depth), entry.name);
            let style = if open == Some(entry.path.as_path()) {
                buf.set_style(
                    Rect::new(area.x, y, area.width - 1, 1),
                    theme.sidebar_active,
                );
                theme.sidebar_active
            } else if entry.is_dir {
                theme.directory
            } else {
                theme.sidebar
            };
            buf.set_stringn(area.x, y, label, content, style);
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, cx: &mut Context<'_>) -> EventResult {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let row = event.row.saturating_sub(area.y);
                let Some(row) = row.checked_sub(HEADER_HEIGHT) else {
                    // clicking the folder name picks up files changed outside the editor
                    self.tree.refresh();
                    return EventResult::Consumed;
                };
                let index = self.scroll + usize::from(row);
                let Some(entry) = self.tree.entries().get(index) else {
                    return EventResult::Consumed;
                };
                if entry.is_dir {
                    self.tree.toggle(index);
                } else if let Err(err) = cx.editor.open(entry.path.clone()) {
                    cx.editor
                        .set_status(format!("could not open {}: {err}", entry.name));
                }
            }
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(WHEEL_ROWS);
            }
            MouseEventKind::ScrollDown => {
                self.scroll = (self.scroll + WHEEL_ROWS).min(self.max_scroll());
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for [`Explorer`].
mod tests {
    use std::{env, fs, process};

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use mog_core::{Editor, FileTree, MemoryClipboard};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::Explorer;
    use crate::{
        compositor::{Compositor, Context},
        theme::Theme,
    };

    /// Clicking a file opens it and clicking a folder expands it.
    #[test]
    fn click_opens_files_and_expands_folders() {
        let dir = env::temp_dir().join(format!("mog-explorer-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("mkdir");
        fs::write(dir.join("src").join("main.rs"), "fn main() {}").expect("write");
        fs::write(dir.join("notes.txt"), "hi").expect("write");

        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let theme = Theme::default();
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Explorer::new(FileTree::new(&dir).expect("tree"))));
        let screen = Rect::new(0, 0, 60, 10);
        let mut click = |row| {
            let event = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row,
                modifiers: KeyModifiers::NONE,
            };
            let mut cx = Context {
                editor: &mut editor,
                theme: &theme,
            };
            compositor.handle_mouse(event, screen, &mut cx);
        };
        click(1);
        click(3);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(editor.document().name(), "notes.txt");

        let mut terminal = Terminal::new(TestBackend::new(60, 10)).expect("test terminal");
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
        let row = |y| {
            let text: String = (0..19).map(|x| buffer[(x, y)].symbol()).collect();
            text.trim_end().to_owned()
        };
        assert_eq!(row(1), " \u{25be} src");
        assert_eq!(row(2), "     main.rs");
    }
}
