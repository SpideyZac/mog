//! The completion menu under the cursor and the hover box above it.

use std::mem;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Change, Command, Editor, Key, KeyChord, fuzzy_match};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::{Clear, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    ghost::{self, GhostKey},
    theme::Theme,
    ui::{Focus, Layout, SignatureHint, Ui},
};

/// The most rows the menu shows at once.
const MAX_ROWS: usize = 10;

/// The widest the menu gets.
const MAX_WIDTH: usize = 64;

/// The widest the hover box gets.
const HOVER_WIDTH: usize = 72;

/// The most lines the hover box shows.
const HOVER_LINES: usize = 14;

/// What kind of thing a completion is, for its icon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ItemKind {
    /// A function or method.
    Function,
    /// A variable or value.
    Variable,
    /// A field or property.
    Field,
    /// A type, struct, class or interface.
    Type,
    /// A module or namespace.
    Module,
    /// A keyword.
    Keyword,
    /// A constant or enum member.
    Constant,
    /// Anything else.
    #[default]
    Other,
}

impl ItemKind {
    /// Returns the icon and its style for the kind.
    fn icon(self, theme: &Theme) -> (&'static str, Style) {
        match self {
            Self::Function => ("\u{192}", theme.function),
            Self::Variable => ("\u{3bd}", theme.property),
            Self::Field => ("\u{25c7}", theme.property),
            Self::Type => ("\u{25a0}", theme.type_name),
            Self::Module => ("\u{25a3}", theme.namespace),
            Self::Keyword => ("\u{2318}", theme.keyword),
            Self::Constant => ("\u{3c0}", theme.constant),
            Self::Other => ("\u{b7}", theme.popup_dim),
        }
    }
}

/// One completion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionItem {
    /// What the menu shows.
    pub label: String,
    /// Extra text, usually the type or signature.
    pub detail: String,
    /// What it is.
    pub kind: ItemKind,
    /// The text to insert.
    pub insert: String,
    /// The text to filter by.
    pub filter: String,
    /// Changes elsewhere that come with it, like the import the item needs.
    pub extra: Vec<Change>,
    /// The item as the server sent it, when the server has more to say about it once picked.
    pub resolve: Option<String>,
    /// Where the server wants the replaced text to start, when it is before the typed word.
    pub start: Option<usize>,
}

/// An open completion menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionState {
    /// Every completion the server sent.
    pub items: Vec<CompletionItem>,
    /// Where the word being completed starts.
    pub anchor: usize,
    /// The document the menu belongs to.
    pub document: usize,
    /// The visible items as indexes into `items`, best first.
    pub filtered: Vec<usize>,
    /// The highlighted entry of `filtered`.
    pub selected: usize,
    /// Whether typing more should ask the server again, since it left some items out.
    pub incomplete: bool,
    /// The typed prefix `filtered` was computed for.
    prefix: Option<String>,
}

impl CompletionState {
    /// Creates a menu for `items` completing the word starting at `anchor`.
    pub fn new(items: Vec<CompletionItem>, anchor: usize, document: usize) -> Self {
        Self {
            items,
            anchor,
            document,
            filtered: Vec::new(),
            selected: 0,
            incomplete: false,
            prefix: None,
        }
    }

    /// Refilters for `prefix` if it changed.
    pub fn refilter(&mut self, prefix: &str) {
        if self.prefix.as_deref() == Some(prefix) {
            return;
        }
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| fuzzy_match(prefix, &item.filter).map(|found| (found.score, i)))
            .collect();
        scored.sort_by_key(|(score, _)| -score);
        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
        self.prefix = Some(prefix.to_owned());
    }

    /// Returns the highlighted item.
    pub fn current(&self) -> Option<&CompletionItem> {
        self.filtered.get(self.selected).map(|&i| &self.items[i])
    }
}

/// Returns where the identifier ending at `pos` starts.
pub fn word_start(editor: &Editor, pos: usize) -> usize {
    let text = editor.document().text();
    let mut start = pos;
    while start > 0 {
        let ch = text.char(start - 1);
        if ch.is_alphanumeric() || ch == '_' {
            start -= 1;
        } else {
            break;
        }
    }
    start
}

/// Returns the typed prefix of the menu, or `None` if the cursor left the word.
fn prefix(state: &CompletionState, editor: &Editor) -> Option<String> {
    if editor.active() != state.document {
        return None;
    }
    let document = editor.document();
    let head = document.selection().head;
    if head < state.anchor || !document.selection().is_empty() {
        return None;
    }
    let text = document.text().slice(state.anchor..head).to_string();
    text.chars()
        .all(|ch| ch.is_alphanumeric() || ch == '_')
        .then_some(text)
}

/// Inserts the highlighted completion and closes the menu.
pub fn accept(ui: &mut Ui, editor: &mut Editor) {
    let Some(state) = ui.completion.take() else {
        return;
    };
    let Some(item) = state.current() else {
        return;
    };
    let head = editor.document().selection().head;
    let start = item
        .start
        .map_or(state.anchor, |start| start.min(state.anchor));
    editor.complete(start, head, &item.insert, &item.extra);
    if let (true, Some(raw)) = (item.extra.is_empty(), &item.resolve) {
        ui.completion_resolve = Some(CompletionResolve {
            raw: raw.clone(),
            anchor: state.anchor,
        });
        ui.request(Command::Custom(RESOLVE_COMMAND.into()));
    }
}

/// The command the app runs to ask the server what else a picked completion needs.
pub const RESOLVE_COMMAND: &str = "lsp.completion_resolve";

/// A picked completion the server may have more to say about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResolve {
    /// The item as the server sent it.
    pub raw: String,
    /// Where the completed word started.
    pub anchor: usize,
}

/// Wraps `text` to `width` columns, keeping at most `max_lines` lines.
pub fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let raw = raw.trim_end();
        if raw.starts_with("```") {
            continue;
        }
        let mut line = String::new();
        for word in raw.split(' ') {
            if !line.is_empty() && line.width() + word.width() + 1 > width {
                lines.push(mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
        if lines.len() >= max_lines {
            break;
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.truncate(max_lines);
    lines
}

/// Draws the completion menu and the hover box.
#[derive(Debug, Default)]
pub struct CompletionMenu {
    /// The menu box from the last render.
    menu: Rect,
}

impl CompletionMenu {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the box for the menu with `rows` rows `width` wide, below the cursor if it fits.
    fn place(cursor: Position, editor: Rect, width: u16, rows: u16) -> Rect {
        let width = width.min(editor.width);
        let x = cursor
            .x
            .min(editor.right().saturating_sub(width))
            .max(editor.x);
        let below = cursor.y + 1;
        let y = if below + rows <= editor.bottom() {
            below
        } else {
            cursor.y.saturating_sub(rows).max(editor.y)
        };
        Rect::new(x, y, width, rows.min(editor.height))
    }
}

/// Draws the signature of the call being typed above the cursor, the current parameter bold.
fn render_signature(
    signature: &SignatureHint,
    cursor: Position,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
) {
    let doc = signature
        .documentation
        .as_deref()
        .and_then(|doc| doc.lines().find(|line| !line.trim().is_empty()))
        .map(str::trim);
    let label_width = signature.label.width() + 2;
    let doc_width = doc.map_or(0, |doc| doc.width() + 2);
    let width = label_width.max(doc_width).min(HOVER_WIDTH);
    let width = u16::try_from(width).unwrap_or(0).min(area.width);
    let height = if doc.is_some() { 2 } else { 1 };
    if cursor.y < area.y + height {
        return;
    }
    let rect = Rect::new(
        cursor.x.min(area.right().saturating_sub(width)).max(area.x),
        cursor.y - height,
        width,
        height,
    );
    Clear.render(rect, buf);
    buf.set_style(rect, theme.popup);
    buf.set_string(rect.x, rect.y, "\u{258c}", theme.popup_border);
    let mut x = rect.x + 1;
    for (i, ch) in signature.label.chars().enumerate() {
        if x >= rect.right() {
            break;
        }
        let active = signature
            .active
            .is_some_and(|(from, to)| (from..to).contains(&i));
        let style = if active {
            theme.popup_match.add_modifier(Modifier::UNDERLINED)
        } else {
            theme.popup
        };
        x = buf.set_stringn(x, rect.y, ch.to_string(), 2, style).0;
    }
    if let Some(doc) = doc {
        buf.set_string(rect.x, rect.y + 1, "\u{258c}", theme.popup_border);
        let room = usize::from(rect.width.saturating_sub(1));
        buf.set_stringn(rect.x + 1, rect.y + 1, doc, room, theme.popup_dim);
    }
}

impl Layer for CompletionMenu {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        let popup = ui.completion.is_some() || ui.hover.is_some() || ui.signature.is_some();
        if popup && ui.overlay.is_none() {
            layout.editor
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        self.menu = Rect::default();
        let Some(cursor) = cx.ui.cursor_screen else {
            return;
        };
        if let Some(signature) = cx.ui.signature.as_ref().filter(|_| cx.ui.hover.is_none()) {
            render_signature(signature, cursor, area, buf, theme);
        }
        if let Some((text, _)) = &cx.ui.hover {
            let lines = wrap(text, HOVER_WIDTH - 2, HOVER_LINES);
            let width = lines.iter().map(|line| line.width()).max().unwrap_or(0) + 2;
            let height = u16::try_from(lines.len()).unwrap_or(0);
            let width = u16::try_from(width.min(HOVER_WIDTH)).unwrap_or(0);
            let above = Rect::new(
                cursor.x.min(area.right().saturating_sub(width)).max(area.x),
                cursor.y.saturating_sub(height).max(area.y),
                width.min(area.width),
                height.min(cursor.y.saturating_sub(area.y)).max(1),
            );
            Clear.render(above, buf);
            buf.set_style(above, theme.popup);
            for (i, line) in lines.iter().enumerate().take(usize::from(above.height)) {
                let y = above.y + u16::try_from(i).unwrap_or(0);
                buf.set_string(above.x, y, "\u{258c}", theme.popup_border);
                buf.set_stringn(
                    above.x + 1,
                    y,
                    line,
                    usize::from(above.width - 1),
                    theme.popup,
                );
            }
        }
        let Some(state) = cx.ui.completion.as_mut() else {
            return;
        };
        let Some(prefix) = prefix(state, cx.editor) else {
            cx.ui.completion = None;
            return;
        };
        state.refilter(&prefix);
        if state.filtered.is_empty() {
            return;
        }
        let rows = state.filtered.len().min(MAX_ROWS);
        let width = state
            .filtered
            .iter()
            .map(|&i| state.items[i].label.width() + state.items[i].detail.width().min(30) + 6)
            .max()
            .unwrap_or(10)
            .min(MAX_WIDTH);
        let menu = Self::place(
            cursor,
            area,
            u16::try_from(width).unwrap_or(20),
            u16::try_from(rows).unwrap_or(1),
        );
        self.menu = menu;
        let scroll = state.selected.saturating_sub(rows - 1);
        Clear.render(menu, buf);
        for (row, &index) in state.filtered.iter().enumerate().skip(scroll).take(rows) {
            let item = &state.items[index];
            let y = menu.y + u16::try_from(row - scroll).unwrap_or(0);
            let base = if row == state.selected {
                theme.popup_selected
            } else {
                theme.popup
            };
            buf.set_style(Rect::new(menu.x, y, menu.width, 1), base);
            let (icon, style) = item.kind.icon(theme);
            buf.set_string(menu.x, y, format!(" {icon} "), base.patch(style));
            let label_room = usize::from(menu.width.saturating_sub(4));
            let (end, _) = buf.set_stringn(
                menu.x + 3,
                y,
                &item.label,
                label_room,
                base.add_modifier(Modifier::BOLD),
            );
            let detail_room = usize::from(menu.right().saturating_sub(end + 2));
            if detail_room > 3 && !item.detail.is_empty() {
                let detail: String = item.detail.lines().next().unwrap_or_default().to_owned();
                let detail_width = detail.width().min(detail_room);
                let dx = menu.right() - u16::try_from(detail_width).unwrap_or(0) - 1;
                buf.set_stringn(dx, y, &detail, detail_room, base.patch(theme.popup_dim));
            }
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.focus != Focus::Editor || cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        if cx.ui.hover.is_some() {
            cx.ui.hover = None;
            if chord.key == Key::Esc {
                return EventResult::Consumed;
            }
        }
        if cx.ui.signature.is_some() && chord.key == Key::Esc && cx.ui.completion.is_none() {
            cx.ui.signature = None;
            return EventResult::Consumed;
        }
        let menu_open = cx.ui.completion.is_some();
        match ghost::handle_key(&mut cx.ui.ghost, chord, cx.editor, menu_open) {
            GhostKey::Ignored => {}
            GhostKey::Used => return EventResult::Consumed,
            GhostKey::WantsMore => {
                cx.ui.request(Command::Custom(ghost::MORE_COMMAND.into()));
                return EventResult::Consumed;
            }
        }
        let Some(state) = cx.ui.completion.as_mut() else {
            return EventResult::Ignored;
        };
        if state.filtered.is_empty() || chord.mods.ctrl || chord.mods.alt {
            return EventResult::Ignored;
        }
        let last = state.filtered.len() - 1;
        match chord.key {
            Key::Up => state.selected = state.selected.checked_sub(1).unwrap_or(last),
            Key::Down => {
                state.selected = if state.selected >= last {
                    0
                } else {
                    state.selected + 1
                }
            }
            Key::PageUp => state.selected = state.selected.saturating_sub(MAX_ROWS),
            Key::PageDown => state.selected = (state.selected + MAX_ROWS).min(last),
            Key::Enter | Key::Tab => accept(cx.ui, cx.editor),
            Key::Esc => cx.ui.completion = None,
            _ => return EventResult::Ignored,
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let inside = self.menu.contains(Position::new(event.column, event.row));
        if !inside {
            return EventResult::Ignored;
        }
        if let (MouseEventKind::Down(MouseButton::Left), Some(state)) =
            (event.kind, cx.ui.completion.as_mut())
        {
            let scroll = state.selected.saturating_sub(MAX_ROWS - 1);
            state.selected = scroll + usize::from(event.row - self.menu.y);
            accept(cx.ui, cx.editor);
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for completion.
mod tests {
    use mog_core::{Document, Editor, MemoryClipboard, Range};

    use super::{CompletionItem, CompletionState, ItemKind, accept, word_start, wrap};
    use crate::ui::Ui;

    /// Creates a completion called `label`.
    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            kind: ItemKind::Function,
            insert: label.into(),
            filter: label.into(),
            ..CompletionItem::default()
        }
    }

    /// Accepting replaces the typed prefix with the best match.
    #[test]
    fn accepts_best_match() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        *editor.document_mut() = Document::from_text("let x = ve");
        editor.document_mut().set_selection(Range::point(10));
        let anchor = word_start(&editor, 10);
        assert_eq!(anchor, 8);
        let mut state = CompletionState::new(vec![item("print"), item("vec_new")], anchor, 0);
        state.refilter("ve");
        let mut ui = Ui {
            completion: Some(state),
            ..Ui::default()
        };
        accept(&mut ui, &mut editor);
        assert_eq!(editor.document().text().to_string(), "let x = vec_new");
        assert!(ui.completion.is_none());
    }

    /// Hover text wraps on words and drops code fences.
    #[test]
    fn wraps_hover() {
        let lines = wrap("```rust\nfn main()\n```\nhello there world", 11, 10);
        assert_eq!(lines, ["fn main()", "hello there", "world"]);
    }
}
