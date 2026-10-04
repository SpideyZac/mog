//! The right click menu.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    symbols::border,
    widgets::{Block, Borders, Clear, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    ui::{Layout, Overlay, PromptKind, Ui},
};

/// What picking a menu row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Runs a command.
    Run(Command),
    /// Opens a prompt as `(kind, title, text, hint)`.
    Ask(PromptKind, String, String, String),
}

/// One row of a menu. A row with an empty label is a separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// The text of the row.
    pub label: String,
    /// The key binding shown on the right.
    pub keys: String,
    /// What the row does.
    pub action: Option<MenuAction>,
}

impl MenuItem {
    /// Creates a row that runs the command called `name`, showing its first key binding.
    pub fn command(label: &str, name: &str, ui: &Ui) -> Self {
        let keys = ui
            .bindings
            .iter()
            .find(|(_, bound)| bound == name)
            .map(|(chord, _)| chord.clone())
            .unwrap_or_default();
        Self {
            label: label.to_owned(),
            keys,
            action: name.parse().ok().map(MenuAction::Run),
        }
    }

    /// Creates a row that opens a prompt.
    pub fn ask(label: &str, kind: PromptKind, title: &str, text: &str, hint: &str) -> Self {
        Self {
            label: label.to_owned(),
            keys: String::new(),
            action: Some(MenuAction::Ask(
                kind,
                title.to_owned(),
                text.to_owned(),
                hint.to_owned(),
            )),
        }
    }

    /// Creates a separator.
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            keys: String::new(),
            action: None,
        }
    }
}

/// An open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuState {
    /// Where the click was.
    pub at: Position,
    /// The rows.
    pub items: Vec<MenuItem>,
}

/// Opens a menu with `items` at `at`.
pub fn open_menu(ui: &mut Ui, at: Position, items: Vec<MenuItem>) {
    ui.open(Overlay::Menu);
    ui.menu = Some(MenuState { at, items });
}

/// Returns the editor menu.
pub fn editor_menu(ui: &Ui) -> Vec<MenuItem> {
    vec![
        MenuItem::command("Go to definition", "lsp.definition", ui),
        MenuItem::command("Show hover info", "lsp.hover", ui),
        MenuItem::command("Rename symbol", "lsp.rename", ui),
        MenuItem::command("Format file", "lsp.format", ui),
        MenuItem::separator(),
        MenuItem::command("Cut", "cut", ui),
        MenuItem::command("Copy", "copy", ui),
        MenuItem::command("Paste", "paste", ui),
        MenuItem::command("Select all", "select_all", ui),
        MenuItem::separator(),
        MenuItem::command("Toggle comment", "toggle_comment", ui),
        MenuItem::command("Find", "search.find", ui),
        MenuItem::command("Ask the AI to explain", "ai.explain", ui),
    ]
}

/// Draws the open menu and runs what is picked.
#[derive(Debug, Default)]
pub struct ContextMenu {
    /// The highlighted row.
    selected: Option<usize>,
    /// The menu box from the last render.
    area: Rect,
    /// The popup generation the highlight belongs to.
    generation: u64,
}

impl ContextMenu {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs the row at `index` and closes the menu.
    fn pick(index: usize, ui: &mut Ui) {
        let action = ui
            .menu
            .as_ref()
            .and_then(|menu| menu.items.get(index))
            .and_then(|item| item.action.clone());
        ui.close();
        ui.menu = None;
        match action {
            Some(MenuAction::Run(command)) => ui.request(command),
            Some(MenuAction::Ask(kind, title, text, hint)) => ui.ask(kind, title, text, hint),
            None => {}
        }
    }

    /// Moves the highlight by `delta`, skipping separators.
    fn step(&mut self, ui: &Ui, delta: isize) {
        let Some(menu) = &ui.menu else {
            return;
        };
        let count = menu.items.len();
        if count == 0 {
            return;
        }
        let mut at = self
            .selected
            .unwrap_or(if delta > 0 { count - 1 } else { 0 });
        for _ in 0..count {
            at = at.wrapping_add_signed(delta).rem_euclid(count);
            if menu.items[at].action.is_some() {
                self.selected = Some(at);
                return;
            }
        }
    }
}

impl Layer for ContextMenu {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::Menu) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if self.generation != cx.ui.overlay_generation {
            self.generation = cx.ui.overlay_generation;
            self.selected = None;
        }
        let theme = cx.theme;
        let Some(menu) = &cx.ui.menu else {
            return;
        };
        let label_width = menu
            .items
            .iter()
            .map(|item| item.label.width())
            .max()
            .unwrap_or(0);
        let keys_width = menu
            .items
            .iter()
            .map(|item| item.keys.width())
            .max()
            .unwrap_or(0);
        let width = u16::try_from(label_width + keys_width + 7).unwrap_or(30);
        let height = u16::try_from(menu.items.len() + 2).unwrap_or(10);
        let width = width.min(area.width);
        let height = height.min(area.height);
        // open towards the side with room, like most desktop menus
        let x = if menu.at.x + width <= area.right() {
            menu.at.x
        } else {
            menu.at.x.saturating_sub(width)
        };
        let y = if menu.at.y + height <= area.bottom() {
            menu.at.y
        } else {
            menu.at.y.saturating_sub(height)
        };
        self.area = Rect::new(x, y, width, height);
        Clear.render(self.area, buf);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(theme.popup_border)
            .style(theme.popup);
        let inner = block.inner(self.area);
        block.render(self.area, buf);
        for (i, item) in menu
            .items
            .iter()
            .enumerate()
            .take(usize::from(inner.height))
        {
            let y = inner.y + u16::try_from(i).unwrap_or(0);
            if item.action.is_none() {
                let rule = "\u{2500}".repeat(usize::from(inner.width));
                buf.set_string(inner.x, y, rule, theme.popup_dim);
                continue;
            }
            let style = if self.selected == Some(i) {
                theme.popup_selected
            } else {
                theme.popup
            };
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), style);
            buf.set_stringn(inner.x + 1, y, &item.label, usize::from(inner.width), style);
            let keys_x = inner
                .right()
                .saturating_sub(u16::try_from(item.keys.width() + 1).unwrap_or(0));
            buf.set_string(keys_x, y, &item.keys, style.patch(theme.popup_dim));
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::Menu) {
            return EventResult::Ignored;
        }
        match chord.key {
            Key::Esc => {
                cx.ui.close();
                cx.ui.menu = None;
            }
            Key::Up => self.step(cx.ui, -1),
            Key::Down | Key::Tab => self.step(cx.ui, 1),
            Key::Enter => {
                if let Some(index) = self.selected {
                    Self::pick(index, cx.ui);
                }
            }
            _ => {}
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let point = Position::new(event.column, event.row);
        let row = (self.area.contains(point) && event.row > self.area.y)
            .then(|| usize::from(event.row - self.area.y - 1));
        match event.kind {
            MouseEventKind::Moved => {
                self.selected = row.filter(|&i| {
                    cx.ui
                        .menu
                        .as_ref()
                        .and_then(|menu| menu.items.get(i))
                        .is_some_and(|item| item.action.is_some())
                });
            }
            MouseEventKind::Down(MouseButton::Left) => match row {
                Some(index) => Self::pick(index, cx.ui),
                None => {
                    cx.ui.close();
                    cx.ui.menu = None;
                }
            },
            MouseEventKind::Down(_) if row.is_none() => {
                cx.ui.close();
                cx.ui.menu = None;
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for the context menu.
mod tests {
    use mog_core::Command;
    use ratatui::layout::Position;

    use super::{ContextMenu, MenuItem, open_menu};
    use crate::ui::{Overlay, Ui};

    /// Picking a row closes the menu and queues its command.
    #[test]
    fn picks_commands() {
        let mut ui = Ui::default();
        let items = vec![
            MenuItem::command("Copy", "copy", &ui),
            MenuItem::separator(),
            MenuItem::command("Paste", "paste", &ui),
        ];
        open_menu(&mut ui, Position::new(3, 3), items);
        assert_eq!(ui.overlay, Some(Overlay::Menu));
        let mut menu = ContextMenu::new();
        menu.step(&ui, 1);
        menu.step(&ui, 1);
        assert_eq!(menu.selected, Some(2));
        ContextMenu::pick(2, &mut ui);
        assert_eq!(ui.requests, [Command::Paste]);
        assert_eq!(ui.overlay, None);
    }
}
