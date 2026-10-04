//! The find and replace bar, and the search logic commands share with it.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Editor, Key, KeyChord, search};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    ui::{Focus, Layout, Ui},
};

/// The widest the bar gets.
const BAR_WIDTH: u16 = 58;

/// The state of the find and replace bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchState {
    /// Whether the bar is shown.
    pub open: bool,
    /// Whether the replace field is shown.
    pub replacing: bool,
    /// Whether typing goes to the replace field.
    pub in_replacement: bool,
    /// What to find.
    pub query: String,
    /// What to replace matches with.
    pub replacement: String,
    /// Whether case must match, or `None` for smart case.
    pub case_sensitive: Option<bool>,
    /// The char ranges of matches in the focused document.
    pub matches: Vec<(usize, usize)>,
    /// The index of the match the cursor is on.
    pub current: Option<usize>,
    /// What the matches were computed for, as `(document, version, query, case)`.
    computed_for: Option<(usize, u64, String, bool)>,
}

impl SearchState {
    /// Returns whether matches are case sensitive for the current query.
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
            .unwrap_or_else(|| search::smart_case(&self.query))
    }
}

/// Opens the bar, prefilled with the selection if it is a short single line.
pub fn open(ui: &mut Ui, editor: &Editor, replacing: bool) {
    let document = editor.document();
    let selection = document.selection();
    if !selection.is_empty() {
        let text = document
            .text()
            .slice(selection.from()..selection.to())
            .to_string();
        if !text.contains('\n') && text.chars().count() <= 200 {
            ui.search.query = text;
        }
    }
    ui.search.open = true;
    ui.search.replacing = replacing;
    ui.search.in_replacement = false;
    ui.focus = Focus::Search;
    refresh(ui, editor);
}

/// Closes the bar and forgets its matches.
pub fn close(ui: &mut Ui) {
    ui.search.open = false;
    ui.search.matches.clear();
    ui.search.current = None;
    ui.search.computed_for = None;
    if ui.focus == Focus::Search {
        ui.focus = Focus::Editor;
    }
}

/// Recomputes the matches if the document or query changed since last time.
pub fn refresh(ui: &mut Ui, editor: &Editor) {
    let state = &mut ui.search;
    if !state.open {
        return;
    }
    let case = state.is_case_sensitive();
    let document = editor.document();
    let key = (
        editor.active(),
        document.version(),
        state.query.clone(),
        case,
    );
    if state.computed_for.as_ref() == Some(&key) {
        return;
    }
    state.matches = search::find_all(document.text(), &state.query, case);
    state.current = search::next_match(&state.matches, document.selection().from());
    state.computed_for = Some(key);
}

/// Moves to the next match, or the previous one if `forward` is not set.
pub fn step(ui: &mut Ui, editor: &mut Editor, forward: bool) {
    refresh(ui, editor);
    let selection = editor.document().selection();
    let index = if forward {
        search::next_match(&ui.search.matches, selection.from() + 1)
    } else {
        search::prev_match(&ui.search.matches, selection.from())
    };
    select(ui, editor, index);
}

/// Selects the match at `index` and makes it current.
fn select(ui: &mut Ui, editor: &mut Editor, index: Option<usize>) {
    ui.search.current = index;
    if let Some((from, to)) = index.and_then(|i| ui.search.matches.get(i).copied()) {
        editor.select(from, to);
    }
}

/// Replaces the current match and moves to the next one.
pub fn replace_one(ui: &mut Ui, editor: &mut Editor) {
    refresh(ui, editor);
    let Some(range) = ui
        .search
        .current
        .and_then(|i| ui.search.matches.get(i).copied())
    else {
        return;
    };
    let replacement = ui.search.replacement.clone();
    editor.replace_ranges(&[range], &replacement);
    refresh(ui, editor);
    let index = search::next_match(&ui.search.matches, editor.document().selection().head);
    select(ui, editor, index);
}

/// Replaces every match.
pub fn replace_all(ui: &mut Ui, editor: &mut Editor) {
    refresh(ui, editor);
    let count = ui.search.matches.len();
    if count == 0 {
        return;
    }
    let matches = ui.search.matches.clone();
    let replacement = ui.search.replacement.clone();
    editor.replace_ranges(&matches, &replacement);
    editor.set_status(format!("replaced {count} match(es), mogged"));
    refresh(ui, editor);
}

/// The find and replace bar in the top right of the editor.
#[derive(Debug, Default)]
pub struct SearchBar {
    /// Where the text cursor goes.
    cursor: Option<Position>,
}

impl SearchBar {
    /// Creates the bar.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Layer for SearchBar {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if !ui.search.open {
            return Rect::default();
        }
        let editor = layout.editor;
        let width = BAR_WIDTH.min(editor.width);
        let height = if ui.search.replacing { 2 } else { 1 };
        Rect::new(
            editor.right() - width,
            editor.y,
            width,
            height.min(editor.height),
        )
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        refresh(cx.ui, cx.editor);
        let theme = cx.theme;
        let state = &cx.ui.search;
        let focused = cx.ui.focus == Focus::Search;
        Clear.render(area, buf);
        buf.set_style(area, theme.popup);
        let count = match (state.current, state.matches.len()) {
            (_, 0) if state.query.is_empty() => String::new(),
            (_, 0) => "no results".to_owned(),
            (Some(i), n) => format!("{}/{n}", i + 1),
            (None, n) => format!("?/{n}"),
        };
        let case = if state.is_case_sensitive() {
            "Aa"
        } else {
            "aa"
        };
        let right = format!(" {count}  {case} ");
        let right_width = u16::try_from(right.width()).unwrap_or(0);
        let field_width = usize::from(area.width.saturating_sub(right_width + 3));
        let rows = [
            ("\u{2315} ", &state.query, !state.in_replacement),
            ("\u{21bb} ", &state.replacement, state.in_replacement),
        ];
        self.cursor = None;
        for (row, (icon, text, active)) in rows.iter().enumerate().take(usize::from(area.height)) {
            let y = area.y + u16::try_from(row).unwrap_or(0);
            buf.set_string(area.x, y, "\u{2590}", theme.popup_border);
            buf.set_string(area.x + 1, y, icon, theme.popup_title);
            // keep the end of long queries visible
            let shown: String = {
                let chars: Vec<char> = text.chars().collect();
                let skip = chars.len().saturating_sub(field_width.saturating_sub(1));
                chars[skip..].iter().collect()
            };
            let style = if *active && focused {
                theme.popup.patch(theme.popup_match)
            } else {
                theme.popup
            };
            buf.set_stringn(area.x + 3, y, &shown, field_width, style);
            if *active && focused {
                let x = area.x + 3 + u16::try_from(shown.width()).unwrap_or(0);
                self.cursor = Some(Position::new(x.min(area.right() - 1), y));
            }
        }
        let rx = area.right().saturating_sub(right_width);
        buf.set_string(rx, area.y, &right, theme.popup_dim);
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if !cx.ui.search.open || cx.ui.focus != Focus::Search || cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        let replacing = cx.ui.search.replacing;
        let in_replacement = cx.ui.search.in_replacement;
        match chord.key {
            Key::Esc => close(cx.ui),
            Key::Enter if chord.mods.alt || (chord.mods.ctrl && replacing) => {
                replace_all(cx.ui, cx.editor);
            }
            Key::Enter if in_replacement => replace_one(cx.ui, cx.editor),
            Key::Enter => step(cx.ui, cx.editor, !chord.mods.shift),
            Key::Down => step(cx.ui, cx.editor, true),
            Key::Up => step(cx.ui, cx.editor, false),
            Key::Tab if replacing => cx.ui.search.in_replacement = !in_replacement,
            Key::Char('c') if chord.mods.alt => {
                let sensitive = cx.ui.search.is_case_sensitive();
                cx.ui.search.case_sensitive = Some(!sensitive);
                refresh(cx.ui, cx.editor);
            }
            Key::Backspace => {
                let state = &mut cx.ui.search;
                let field = if in_replacement {
                    &mut state.replacement
                } else {
                    &mut state.query
                };
                if chord.mods.ctrl {
                    field.clear();
                } else {
                    field.pop();
                }
                self.after_typing(cx);
            }
            _ => match chord.typed_char() {
                Some(ch) => {
                    if in_replacement {
                        cx.ui.search.replacement.push(ch);
                    } else {
                        cx.ui.search.query.push(ch);
                    }
                    self.after_typing(cx);
                }
                None if chord.mods.ctrl || chord.mods.alt => return EventResult::Ignored,
                None => {}
            },
        }
        EventResult::Consumed
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, cx: &mut Context<'_>) -> EventResult {
        if let MouseEventKind::Down(MouseButton::Left) = event.kind {
            cx.ui.focus = Focus::Search;
            cx.ui.search.in_replacement = event.row > area.y;
        }
        EventResult::Consumed
    }

    fn cursor(&self, _area: Rect, cx: &Context<'_>) -> Option<Position> {
        (cx.ui.focus == Focus::Search && cx.ui.overlay.is_none())
            .then_some(self.cursor)
            .flatten()
    }
}

impl SearchBar {
    /// Jumps to the nearest match after the query changed.
    fn after_typing(&self, cx: &mut Context<'_>) {
        if cx.ui.search.in_replacement {
            return;
        }
        refresh(cx.ui, cx.editor);
        let index = cx.ui.search.current;
        select(cx.ui, cx.editor, index);
    }
}

#[cfg(test)]
/// Tests for searching through the ui.
mod tests {
    use mog_core::{Document, Editor, MemoryClipboard, Range};

    use super::{open, replace_all, replace_one, step};
    use crate::ui::Ui;

    /// Creates an editor holding `text` with the cursor at the start.
    fn editor_with(text: &str) -> Editor {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        *editor.document_mut() = Document::from_text(text);
        editor.document_mut().set_selection(Range::point(0));
        editor.view_mut().resize(80, 24);
        editor
    }

    /// Stepping cycles through matches and selects them.
    #[test]
    fn steps_through_matches() {
        let mut editor = editor_with("mog x mog y mog");
        let mut ui = Ui::default();
        ui.search.query = "mog".into();
        open(&mut ui, &editor, false);
        assert_eq!(ui.search.matches.len(), 3);
        step(&mut ui, &mut editor, true);
        assert_eq!(editor.document().selection(), Range::new(6, 9));
        step(&mut ui, &mut editor, false);
        assert_eq!(editor.document().selection(), Range::new(0, 3));
        step(&mut ui, &mut editor, false);
        assert_eq!(editor.document().selection(), Range::new(12, 15));
    }

    /// Replacing one or all matches edits the text.
    #[test]
    fn replaces() {
        let mut editor = editor_with("a-a-a");
        let mut ui = Ui::default();
        ui.search.query = "a".into();
        ui.search.replacement = "bb".into();
        open(&mut ui, &editor, true);
        replace_one(&mut ui, &mut editor);
        assert_eq!(editor.document().text().to_string(), "bb-a-a");
        replace_all(&mut ui, &mut editor);
        assert_eq!(editor.document().text().to_string(), "bb-bb-bb");
    }
}
