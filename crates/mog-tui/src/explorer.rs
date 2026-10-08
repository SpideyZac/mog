//! The file explorer shown on the left when a folder is open.

use std::path::PathBuf;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{FileTree, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
};

use crate::{
    compositor::{Context, EventResult, Layer},
    icons,
    menu::{self, MenuItem},
    ui::{Focus, Layout, PromptKind, SidebarSide, Ui},
};

/// Rows above the file list, taken by the folder name.
const HEADER_HEIGHT: u16 = 1;

/// How many rows one wheel notch scrolls.
const WHEEL_ROWS: usize = 3;

/// Shows a [`FileTree`] and opens files that are clicked or picked with the keyboard.
#[derive(Debug)]
pub struct Explorer {
    /// The folder being shown.
    tree: FileTree,
    /// The index of the first entry shown.
    scroll: usize,
    /// The index of the highlighted entry.
    selected: usize,
    /// The number of entry rows that fit, from the last render.
    rows: usize,
}

impl Explorer {
    /// Creates an explorer showing `tree`.
    pub fn new(tree: FileTree) -> Self {
        Self {
            tree,
            scroll: 0,
            selected: 0,
            rows: 0,
        }
    }

    /// Returns the largest useful scroll offset.
    fn max_scroll(&self) -> usize {
        self.tree.entries().len().saturating_sub(1)
    }

    /// Moves the highlight by `delta` rows and scrolls to keep it visible.
    fn move_selection(&mut self, delta: isize) {
        let last = self.tree.entries().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.reveal_selection();
    }

    /// Scrolls so the highlighted entry is visible.
    fn reveal_selection(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.rows > 0 && self.selected >= self.scroll + self.rows {
            self.scroll = self.selected + 1 - self.rows;
        }
    }

    /// Opens the highlighted file or toggles the highlighted folder.
    fn activate(&mut self, cx: &mut Context<'_>) {
        let Some(entry) = self.tree.entries().get(self.selected) else {
            return;
        };
        if entry.is_dir {
            self.tree.toggle(self.selected);
            return;
        }
        match cx.editor.open(entry.path.clone()) {
            Ok(()) => cx.ui.focus = Focus::Editor,
            Err(err) => {
                cx.editor
                    .set_status(format!("could not open {}: {err}", entry.name));
            }
        }
    }

    /// Collapses the highlighted folder, or moves to the parent folder.
    fn collapse_or_parent(&mut self) {
        let entries = self.tree.entries();
        let Some(entry) = entries.get(self.selected) else {
            return;
        };
        if entry.is_dir && entry.expanded {
            self.tree.toggle(self.selected);
            return;
        }
        let depth = entry.depth;
        if let Some(parent) = entries[..self.selected]
            .iter()
            .rposition(|other| other.depth + 1 == depth)
        {
            self.selected = parent;
            self.reveal_selection();
        }
    }

    /// Expands the highlighted folder, or moves into it if already expanded.
    fn expand_or_child(&mut self) {
        let Some(entry) = self.tree.entries().get(self.selected) else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        if entry.expanded {
            self.move_selection(1);
        } else {
            self.tree.toggle(self.selected);
        }
    }

    /// Returns the folder new files go in: the highlighted folder, or the folder of the
    /// highlighted file.
    fn target_folder(&self) -> PathBuf {
        match self.tree.entries().get(self.selected) {
            Some(entry) if entry.is_dir => entry.path.clone(),
            Some(entry) => entry
                .path
                .parent()
                .map_or_else(|| self.tree.root().to_owned(), ToOwned::to_owned),
            None => self.tree.root().to_owned(),
        }
    }

    /// Asks for the name of a new file in the target folder.
    pub fn ask_new_file(&self, ui: &mut Ui) {
        let folder = self.target_folder();
        let shown = folder
            .strip_prefix(self.tree.root())
            .unwrap_or(&folder)
            .display()
            .to_string();
        let place = if shown.is_empty() {
            "the project folder".to_owned()
        } else {
            shown
        };
        let hint = format!("in {place}, end with / to make a folder");
        ui.ask(PromptKind::NewFile(folder), "\u{271a} new file", "", hint);
    }

    /// Returns the right click menu for the highlighted entry.
    fn menu_items(&self) -> Vec<MenuItem> {
        let folder = self.target_folder();
        let mut items = vec![MenuItem::ask(
            "New file",
            PromptKind::NewFile(folder),
            "\u{271a} new file",
            "",
            "end with / to make a folder",
        )];
        if let Some(entry) = self.tree.entries().get(self.selected) {
            items.push(MenuItem::ask(
                "Rename",
                PromptKind::RenameFile(entry.path.clone()),
                "\u{270e} rename",
                &entry.name,
                "a new name, or a path relative to the parent folder",
            ));
            items.push(MenuItem::separator());
            items.push(MenuItem::ask(
                "Delete",
                PromptKind::DeleteFile(entry.path.clone()),
                &format!("\u{2716} delete {}", entry.name),
                "",
                "type yes to delete it",
            ));
        }
        items
    }

    /// Asks for a new name for the highlighted entry.
    fn ask_rename(&self, ui: &mut Ui) {
        if let Some(entry) = self.tree.entries().get(self.selected) {
            ui.ask(
                PromptKind::RenameFile(entry.path.clone()),
                "\u{270e} rename",
                entry.name.clone(),
                "a new name, or a path relative to the parent folder",
            );
        }
    }

    /// Asks to confirm deleting the highlighted entry.
    fn ask_delete(&self, ui: &mut Ui) {
        if let Some(entry) = self.tree.entries().get(self.selected) {
            let what = if entry.is_dir {
                "folder and everything in it"
            } else {
                "file"
            };
            ui.ask(
                PromptKind::DeleteFile(entry.path.clone()),
                format!("\u{2716} delete {}", entry.name),
                "",
                format!("type yes to delete this {what}"),
            );
        }
    }

    /// Jumps to the next entry whose name starts with `ch`.
    fn jump_to(&mut self, ch: char) {
        let entries = self.tree.entries();
        let count = entries.len();
        let wanted = ch.to_lowercase().next().unwrap_or(ch);
        let found = (1..=count)
            .map(|step| (self.selected + step) % count)
            .find(|&i| entries[i].name.to_lowercase().starts_with(wanted));
        if let Some(index) = found {
            self.selected = index;
            self.reveal_selection();
        }
    }
}

impl Layer for Explorer {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.explorer
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if cx.ui.refresh_explorer {
            cx.ui.refresh_explorer = false;
            self.tree.refresh();
        }
        let theme = cx.theme;
        let focused = cx.ui.focus == Focus::Explorer && cx.ui.overlay.is_none();
        buf.set_style(area, theme.sidebar);
        // the rule faces the editor, which is on the other side when the sidebar is on the right
        let on_right = cx.ui.sidebar.open && cx.ui.sidebar.side == SidebarSide::Right;
        let (border_x, left) = if on_right {
            (area.x, area.x + 1)
        } else {
            (area.right() - 1, area.x)
        };
        let border = if focused {
            theme.sidebar_title
        } else {
            theme.border
        };
        for y in area.top()..area.bottom() {
            buf.set_string(border_x, y, "\u{2502}", border);
        }
        let content = usize::from(area.width - 1);
        buf.set_stringn(
            left,
            area.y,
            format!(" \u{25c6} {}", self.tree.name().to_uppercase()),
            content,
            theme.sidebar_title,
        );

        self.rows = usize::from(area.height.saturating_sub(HEADER_HEIGHT));
        self.selected = self.selected.min(self.max_scroll());
        self.scroll = self.scroll.min(self.max_scroll());
        let open = cx.editor.document().path();
        let icons_on = cx.ui.config.ui.icons;
        let entries = self.tree.entries().iter().enumerate().skip(self.scroll);
        for (row, (index, entry)) in (0..self.rows).zip(entries) {
            let y = area.y + HEADER_HEIGHT + u16::try_from(row).unwrap_or(u16::MAX);
            let line = Rect::new(left, y, area.width - 1, 1);
            let is_open = open == Some(entry.path.as_path());
            let base = if focused && index == self.selected {
                theme.sidebar.patch(theme.selection)
            } else if is_open {
                theme.sidebar_active
            } else {
                theme.sidebar
            };
            buf.set_style(line, base);
            let indent = "  ".repeat(entry.depth);
            let mut x = left;
            let mut put = |text: &str, style: Style| {
                let room = usize::from(line.right().saturating_sub(x));
                let (end, _) = buf.set_stringn(x, y, text, room, base.patch(style));
                x = end;
            };
            put(&format!(" {indent}"), Style::new());
            if entry.is_dir {
                let arrow = if entry.expanded {
                    "\u{25be} "
                } else {
                    "\u{25b8} "
                };
                put(arrow, theme.directory);
                put(&entry.name, theme.directory);
            } else {
                match icons::file_color(&entry.name).filter(|_| icons_on) {
                    Some(color) => put(&format!("{} ", icons::FILE_MARKER), Style::new().fg(color)),
                    None => put("  ", Style::new()),
                }
                let style = cx
                    .ui
                    .git_status
                    .get(&entry.path)
                    .map_or(Style::new(), |status| theme.git_status(*status));
                put(&entry.name, style);
            }
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.focus != Focus::Explorer || cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        match (chord.key, chord.mods.ctrl) {
            (Key::Insert, _) | (Key::Char('n'), true) => {
                self.ask_new_file(cx.ui);
                return EventResult::Consumed;
            }
            (Key::F(2), _) => {
                self.ask_rename(cx.ui);
                return EventResult::Consumed;
            }
            (Key::Delete, _) => {
                self.ask_delete(cx.ui);
                return EventResult::Consumed;
            }
            (_, true) => return EventResult::Ignored,
            _ => {}
        }
        let page = isize::try_from(self.rows.max(1)).unwrap_or(isize::MAX);
        match chord.key {
            Key::Up => self.move_selection(-1),
            Key::Down => self.move_selection(1),
            Key::PageUp => self.move_selection(-page),
            Key::PageDown => self.move_selection(page),
            Key::Home => self.move_selection(isize::MIN),
            Key::End => self.move_selection(isize::MAX),
            Key::Enter | Key::Char(' ') => self.activate(cx),
            Key::Left => self.collapse_or_parent(),
            Key::Right => self.expand_or_child(),
            Key::Esc => cx.ui.focus = Focus::Editor,
            Key::Char(ch) if !chord.mods.alt => self.jump_to(ch),
            _ => return EventResult::Ignored,
        }
        EventResult::Consumed
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, cx: &mut Context<'_>) -> EventResult {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left | MouseButton::Middle) => {
                cx.ui.focus = Focus::Explorer;
                let row = event.row.saturating_sub(area.y);
                let Some(row) = row.checked_sub(HEADER_HEIGHT) else {
                    // clicking the folder name picks up files changed outside the editor
                    self.tree.refresh();
                    return EventResult::Consumed;
                };
                let index = self.scroll + usize::from(row);
                if index < self.tree.entries().len() {
                    self.selected = index;
                    self.activate(cx);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                cx.ui.focus = Focus::Explorer;
                let row = usize::from(event.row.saturating_sub(area.y + HEADER_HEIGHT));
                if event.row >= area.y + HEADER_HEIGHT
                    && self.scroll + row < self.tree.entries().len()
                {
                    self.selected = self.scroll + row;
                }
                let items = self.menu_items();
                menu::open_menu(cx.ui, Position::new(event.column, event.row), items);
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
    use std::{env, fs, path::PathBuf, process};

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use mog_core::{Editor, FileTree, Key, KeyChord, MemoryClipboard, Modifiers};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::Explorer;
    use crate::{
        compositor::{Compositor, Context},
        theme::Theme,
        ui::{Focus, Ui},
    };

    /// Creates a scratch project with `src/main.rs` and `notes.txt`.
    fn project(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mog-explorer-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("mkdir");
        fs::write(dir.join("src").join("main.rs"), "fn main() {}").expect("write");
        fs::write(dir.join("notes.txt"), "hi").expect("write");
        dir
    }

    /// Creates shared state with the explorer open and nothing else on screen.
    fn ui() -> Ui {
        let mut ui = Ui {
            has_explorer: true,
            explorer_open: true,
            ..Ui::default()
        };
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        ui
    }

    /// Clicking a file opens it and clicking a folder expands it.
    #[test]
    fn click_opens_files_and_expands_folders() {
        let dir = project("click");
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let theme = Theme::default();
        let mut ui = ui();
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Explorer::new(FileTree::new(&dir).expect("tree"))));
        let screen = Rect::new(0, 0, 60, 10);
        let mut click = |row, ui: &mut Ui| {
            let event = MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row,
                modifiers: KeyModifiers::NONE,
            };
            let mut cx = Context {
                editor: &mut editor,
                theme: &theme,
                ui,
            };
            compositor.handle_mouse(event, screen, &mut cx);
        };
        click(1, &mut ui);
        click(3, &mut ui);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(editor.document().name(), "notes.txt");
        assert_eq!(ui.focus, Focus::Editor);

        let mut terminal = Terminal::new(TestBackend::new(60, 10)).expect("test terminal");
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
        let buffer = terminal.backend().buffer();
        let row = |y| {
            let text: String = (0..19).map(|x| buffer[(x, y)].symbol()).collect();
            text.trim_end().to_owned()
        };
        assert_eq!(row(1), " \u{25be} src");
        assert_eq!(row(2), "   \u{25cf} main.rs");
    }

    /// Middle clicking a file opens it like a left click does.
    #[test]
    fn middle_click_opens_files() {
        let dir = project("middle");
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let theme = Theme::default();
        let mut ui = ui();
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Explorer::new(FileTree::new(&dir).expect("tree"))));
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Middle),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        let mut cx = Context {
            editor: &mut editor,
            theme: &theme,
            ui: &mut ui,
        };
        compositor.handle_mouse(event, Rect::new(0, 0, 60, 10), &mut cx);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(editor.document().name(), "notes.txt");
    }

    /// The keyboard moves the highlight, expands folders and opens files.
    #[test]
    fn keyboard_navigation() {
        let dir = project("keys");
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let theme = Theme::default();
        let mut ui = ui();
        ui.focus = Focus::Explorer;
        let mut compositor = Compositor::new();
        compositor.push(Box::new(Explorer::new(FileTree::new(&dir).expect("tree"))));
        for key in [Key::Right, Key::Down, Key::Enter] {
            let mut cx = Context {
                editor: &mut editor,
                theme: &theme,
                ui: &mut ui,
            };
            compositor.handle_key(KeyChord::new(key, Modifiers::default()), &mut cx);
        }
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(editor.document().name(), "main.rs");
        assert_eq!(ui.focus, Focus::Editor);
    }
}
