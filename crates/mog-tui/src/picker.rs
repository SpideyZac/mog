//! A filterable list with a query line, used by the palette, finder and key list.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Key, KeyChord, fuzzy_match};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// One row a picker can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    /// The main text, matched against the query.
    pub label: String,
    /// Quieter text after the label, also searchable.
    pub detail: String,
    /// Text pinned to the right edge, like a key binding.
    pub hint: String,
    /// An optional color for a marker before the label.
    pub marker: Option<Style>,
}

impl PickerItem {
    /// Creates an item with just a label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
            hint: String::new(),
            marker: None,
        }
    }

    /// Sets the detail text.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// Sets the right aligned hint.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Sets the marker style.
    #[must_use]
    pub fn marker(mut self, style: Style) -> Self {
        self.marker = Some(style);
        self
    }
}

/// What a key or click did to the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    /// Nothing worth reacting to happened, but the event was used.
    None,
    /// The item at this index of the original list was picked.
    Accept(usize),
    /// The picker should close without picking.
    Cancel,
    /// The event was not for the picker.
    Ignored,
}

/// A query line over a fuzzy filtered list.
#[derive(Debug, Default)]
pub struct Picker {
    /// What the user typed.
    query: String,
    /// Every item.
    items: Vec<PickerItem>,
    /// The visible items as `(item index, matched label char indexes)`, best first.
    filtered: Vec<(usize, Vec<usize>)>,
    /// The highlighted row of `filtered`.
    selected: usize,
    /// The first visible row of `filtered`.
    scroll: usize,
    /// The list area from the last render, for mouse hits.
    list_area: Rect,
    /// Where the query cursor was drawn.
    cursor: Option<Position>,
}

impl Picker {
    /// Replaces the items and clears the query.
    pub fn reset(&mut self, items: Vec<PickerItem>) {
        self.items = items;
        self.query.clear();
        self.refilter();
    }

    /// Replaces the items but keeps the query, for lists that change while typing.
    pub fn set_items(&mut self, items: Vec<PickerItem>) {
        self.items = items;
        self.refilter();
    }

    /// Returns the query text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns the number of items that match the query.
    pub fn match_count(&self) -> usize {
        self.filtered.len()
    }

    /// Recomputes which items match the query.
    fn refilter(&mut self) {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                if let Some(found) = fuzzy_match(&self.query, &item.label) {
                    return Some((found.score, index, found.indexes));
                }
                let both = format!("{} {}", item.label, item.detail);
                let found = fuzzy_match(&self.query, &both)?;
                let label_len = item.label.chars().count();
                let indexes = found
                    .indexes
                    .into_iter()
                    .filter(|&i| i < label_len)
                    .collect();
                Some((found.score - 8, index, indexes))
            })
            .collect();
        // stable so equal scores keep the given order
        scored.sort_by_key(|(score, _, _)| -score);
        self.filtered = scored.into_iter().map(|(_, i, idx)| (i, idx)).collect();
        self.selected = 0;
        self.scroll = 0;
    }

    /// Moves the highlight by `delta` rows.
    fn move_selection(&mut self, delta: isize) {
        let last = self.filtered.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Returns the item index under the highlight.
    fn current(&self) -> Option<usize> {
        self.filtered.get(self.selected).map(|(index, _)| *index)
    }

    /// Handles a key press.
    pub fn handle_key(&mut self, chord: KeyChord) -> PickerAction {
        let page = isize::try_from(self.list_area.height.max(1)).unwrap_or(1);
        match chord.key {
            Key::Esc => return PickerAction::Cancel,
            Key::Enter => {
                return self
                    .current()
                    .map_or(PickerAction::None, PickerAction::Accept);
            }
            Key::Up => self.move_selection(-1),
            Key::Down | Key::Tab => self.move_selection(1),
            Key::PageUp => self.move_selection(-page),
            Key::PageDown => self.move_selection(page),
            Key::Backspace if chord.mods.ctrl => {
                self.query.clear();
                self.refilter();
            }
            Key::Backspace => {
                self.query.pop();
                self.refilter();
            }
            _ => match chord.typed_char() {
                Some(ch) => {
                    self.query.push(ch);
                    self.refilter();
                }
                None if chord.mods.ctrl || chord.mods.alt => return PickerAction::Ignored,
                None => {}
            },
        }
        PickerAction::None
    }

    /// Handles a mouse event inside the popup.
    pub fn handle_mouse(&mut self, event: MouseEvent) -> PickerAction {
        let area = self.list_area;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if area.contains(Position::new(event.column, event.row)) {
                    self.selected = self.scroll + usize::from(event.row - area.y);
                    return self
                        .current()
                        .map_or(PickerAction::None, PickerAction::Accept);
                }
            }
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            _ => {}
        }
        PickerAction::None
    }

    /// Returns where the text cursor goes, from the last render.
    pub fn cursor(&self) -> Option<Position> {
        self.cursor
    }

    /// Draws the query line and the visible rows into `inner`.
    pub fn render(&mut self, inner: Rect, buf: &mut Buffer, theme: &Theme, placeholder: &str) {
        if inner.height == 0 || inner.width < 4 {
            return;
        }
        let prompt = "\u{276f} ";
        buf.set_string(inner.x, inner.y, prompt, theme.popup_title);
        let query_x = inner.x + 2;
        if self.query.is_empty() {
            buf.set_stringn(
                query_x,
                inner.y,
                placeholder,
                usize::from(inner.width - 2),
                theme.popup_dim,
            );
        } else {
            buf.set_stringn(
                query_x,
                inner.y,
                &self.query,
                usize::from(inner.width - 2),
                theme.popup,
            );
        }
        let count = format!("{}/{}", self.filtered.len(), self.items.len());
        let count_x = inner
            .right()
            .saturating_sub(u16::try_from(count.len()).unwrap_or(0));
        buf.set_string(count_x, inner.y, &count, theme.popup_dim);
        let query_width = u16::try_from(self.query.width()).unwrap_or(u16::MAX);
        self.cursor = Some(Position::new(
            (query_x + query_width).min(inner.right() - 1),
            inner.y,
        ));

        if inner.height < 3 {
            return;
        }
        let rule = "\u{2500}".repeat(usize::from(inner.width));
        buf.set_string(inner.x, inner.y + 1, rule, theme.popup_border);
        self.list_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        let rows = usize::from(self.list_area.height);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        let width = usize::from(inner.width);
        for (row, (index, matched)) in self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(rows)
        {
            let item = &self.items[*index];
            let y = self.list_area.y + u16::try_from(row - self.scroll).unwrap_or(u16::MAX);
            let base = if row == self.selected {
                theme.popup_selected
            } else {
                theme.popup
            };
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), base);
            let mut x = inner.x;
            let pointer = if row == self.selected {
                "\u{25b8} "
            } else {
                "  "
            };
            x = buf
                .set_stringn(x, y, pointer, width, base.patch(theme.popup_title))
                .0;
            if let Some(style) = item.marker {
                x = buf
                    .set_stringn(x, y, "\u{25cf} ", width, base.patch(style))
                    .0;
            }
            let hint_width = u16::try_from(item.hint.width()).unwrap_or(0);
            let limit = inner.right().saturating_sub(hint_width + 1);
            for (i, ch) in item.label.chars().enumerate() {
                if x >= limit {
                    break;
                }
                let style = if matched.contains(&i) {
                    base.patch(theme.popup_match)
                } else {
                    base
                };
                x = buf
                    .set_stringn(x, y, ch.to_string(), usize::from(limit - x), style)
                    .0;
            }
            if !item.detail.is_empty() && x + 2 < limit {
                let room = usize::from(limit - x - 2);
                buf.set_stringn(x + 2, y, &item.detail, room, base.patch(theme.popup_dim));
            }
            if hint_width > 0 {
                let hx = inner.right().saturating_sub(hint_width);
                buf.set_string(hx, y, &item.hint, base.patch(theme.popup_dim));
            }
        }
        if self.filtered.is_empty() {
            buf.set_stringn(
                inner.x + 2,
                self.list_area.y,
                "nothing matches, mog is confused",
                width,
                theme.popup_dim,
            );
        }
    }
}

#[cfg(test)]
/// Tests for [`Picker`].
mod tests {
    use mog_core::{Key, KeyChord, Modifiers};

    use super::{Picker, PickerAction, PickerItem};

    /// Presses `key` with no modifiers.
    fn press(picker: &mut Picker, key: Key) -> PickerAction {
        picker.handle_key(KeyChord::new(key, Modifiers::default()))
    }

    /// Typing filters and enter picks the best match by its original index.
    #[test]
    fn filters_and_accepts() {
        let mut picker = Picker::default();
        picker.reset(vec![
            PickerItem::new("Save file"),
            PickerItem::new("Toggle explorer"),
            PickerItem::new("Quit").detail("leave mog"),
        ]);
        for ch in "tex".chars() {
            press(&mut picker, Key::Char(ch));
        }
        assert_eq!(picker.match_count(), 1);
        assert_eq!(press(&mut picker, Key::Enter), PickerAction::Accept(1));
        press(&mut picker, Key::Backspace);
        press(&mut picker, Key::Backspace);
        press(&mut picker, Key::Backspace);
        for ch in "leave".chars() {
            press(&mut picker, Key::Char(ch));
        }
        assert_eq!(press(&mut picker, Key::Enter), PickerAction::Accept(2));
        assert_eq!(press(&mut picker, Key::Esc), PickerAction::Cancel);
    }
}
