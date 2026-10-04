//! The find and replace bar, and the search logic commands share with it.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{
    Change, Editor, Key, KeyChord,
    search::{self, Matcher, SearchOptions},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::{Clear, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    theme::Theme,
    ui::{Focus, Layout, Ui},
};

/// The widest the bar gets.
const BAR_WIDTH: u16 = 64;

/// The width of the option toggles, three labels with a space between.
pub const TOGGLES_WIDTH: u16 = 8;

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
    /// Whether matches have to be whole words.
    pub whole_word: bool,
    /// Whether the query is a regular expression.
    pub regex: bool,
    /// Why the query cannot be used, like a broken regex.
    pub error: Option<String>,
    /// The char ranges of matches in the focused document.
    pub matches: Vec<(usize, usize)>,
    /// The index of the match the cursor is on.
    pub current: Option<usize>,
    /// What the matches were computed for, as `(document, version, query, options)`.
    computed_for: Option<(usize, u64, String, SearchOptions)>,
}

impl SearchState {
    /// Returns whether matches are case sensitive for the current query.
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
            .unwrap_or_else(|| search::smart_case(&self.query))
    }

    /// Returns how the query matches right now.
    pub fn options(&self) -> SearchOptions {
        SearchOptions {
            case_sensitive: self.is_case_sensitive(),
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    /// Switches `toggle` on or off.
    pub fn flip(&mut self, toggle: Toggle) {
        match toggle {
            Toggle::Case => self.case_sensitive = Some(!self.is_case_sensitive()),
            Toggle::Word => self.whole_word = !self.whole_word,
            Toggle::Regex => self.regex = !self.regex,
        }
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
    let options = state.options();
    let document = editor.document();
    let key = (
        editor.active(),
        document.version(),
        state.query.clone(),
        options,
    );
    if state.computed_for.as_ref() == Some(&key) {
        return;
    }
    match search::find_matches(document.text(), &state.query, options) {
        Ok(matches) => {
            state.matches = matches;
            state.error = None;
        }
        Err(err) => {
            state.matches.clear();
            state.error = Some(err);
        }
    }
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
    let Ok(matcher) = Matcher::new(&ui.search.query, ui.search.options()) else {
        return;
    };
    let text = editor.document().text();
    let haystack = text.to_string();
    let replacement = matcher.replacement_for(
        &haystack,
        text.char_to_byte(range.0),
        &ui.search.replacement,
    );
    editor.replace_with(vec![Change {
        start: range.0,
        end: range.1,
        text: replacement,
    }]);
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
    let Ok(matcher) = Matcher::new(&ui.search.query, ui.search.options()) else {
        return;
    };
    let changes =
        search::replace_changes(editor.document().text(), &matcher, &ui.search.replacement);
    editor.replace_with(changes);
    editor.set_status(format!("replaced {count} match(es), mogged"));
    refresh(ui, editor);
}

/// A search option that can be switched on and off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Match case.
    Case,
    /// Whole words only.
    Word,
    /// Regular expressions.
    Regex,
}

impl Toggle {
    /// Every toggle, in the order they are drawn.
    pub const ALL: [Self; 3] = [Self::Case, Self::Word, Self::Regex];

    /// Returns the two cell label drawn for the toggle.
    fn label(self) -> &'static str {
        match self {
            Self::Case => "Aa",
            Self::Word => "ab",
            Self::Regex => ".*",
        }
    }

    /// Returns whether the toggle is on in `options`.
    fn is_on(self, options: SearchOptions) -> bool {
        match self {
            Self::Case => options.case_sensitive,
            Self::Word => options.whole_word,
            Self::Regex => options.regex,
        }
    }
}

/// Draws the toggles for `options` starting at `x`, and returns where each one went.
///
/// They take [`TOGGLES_WIDTH`] cells.
pub fn draw_toggles(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    options: SearchOptions,
    theme: &Theme,
) -> Vec<(Rect, Toggle)> {
    let mut x = x;
    let mut hits = Vec::new();
    for toggle in Toggle::ALL {
        let mut style = if toggle.is_on(options) {
            Style::new()
                .fg(theme.palette.bg)
                .bg(theme.palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            theme.popup_dim
        };
        if toggle == Toggle::Word {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        buf.set_string(x, y, toggle.label(), style);
        hits.push((Rect::new(x, y, 2, 1), toggle));
        x += 3;
    }
    hits
}

/// The find and replace bar in the top right of the editor.
#[derive(Debug, Default)]
pub struct SearchBar {
    /// Where the text cursor goes.
    cursor: Option<Position>,
    /// Where the option toggles were drawn.
    toggles: Vec<(Rect, Toggle)>,
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
        let height = u16::from(ui.search.replacing) + u16::from(ui.search.error.is_some()) + 1;
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
            _ if state.error.is_some() => "bad regex".to_owned(),
            (_, 0) if state.query.is_empty() => String::new(),
            (_, 0) => "no results".to_owned(),
            (Some(i), n) => format!("{}/{n}", i + 1),
            (None, n) => format!("?/{n}"),
        };
        let right = format!(" {count} ");
        let right_width = u16::try_from(right.width()).unwrap_or(0) + TOGGLES_WIDTH + 1;
        let field_width = usize::from(area.width.saturating_sub(right_width + 3));
        let rows = [
            ("\u{2315} ", &state.query, !state.in_replacement),
            ("\u{21bb} ", &state.replacement, state.in_replacement),
        ];
        let field_rows = if state.replacing { 2 } else { 1 };
        self.cursor = None;
        for (row, (icon, text, active)) in rows
            .iter()
            .enumerate()
            .take(field_rows.min(usize::from(area.height)))
        {
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
        let count_style = if state.error.is_some() {
            theme.error
        } else {
            theme.popup_dim
        };
        buf.set_string(rx, area.y, &right, count_style);
        let toggles_x = area.right().saturating_sub(TOGGLES_WIDTH + 1);
        self.toggles = draw_toggles(buf, toggles_x, area.y, state.options(), theme);
        // the bar grows a row to say what is wrong with the regex
        let error_row = if state.replacing { 2 } else { 1 };
        if let Some(error) = &state.error
            && area.height > error_row
        {
            let y = area.y + error_row;
            buf.set_string(area.x, y, "\u{2590}", theme.popup_border);
            let room = usize::from(area.width.saturating_sub(3));
            buf.set_stringn(area.x + 3, y, error, room, theme.error);
        }
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
            Key::Char(key @ ('c' | 'w' | 'r')) if chord.mods.alt && !chord.mods.ctrl => {
                let toggle = match key {
                    'c' => Toggle::Case,
                    'w' => Toggle::Word,
                    _ => Toggle::Regex,
                };
                cx.ui.search.flip(toggle);
                self.after_toggle(cx);
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
            let point = Position::new(event.column, event.row);
            if let Some(&(_, toggle)) = self.toggles.iter().find(|(rect, _)| rect.contains(point)) {
                cx.ui.search.flip(toggle);
                self.after_toggle(cx);
            } else if cx.ui.search.replacing {
                cx.ui.search.in_replacement = event.row == area.y + 1;
            }
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
    /// Searches again and jumps to the nearest match after an option changed.
    fn after_toggle(&self, cx: &mut Context<'_>) {
        refresh(cx.ui, cx.editor);
        let index = cx.ui.search.current;
        select(cx.ui, cx.editor, index);
    }

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

    use super::{Toggle, open, refresh, replace_all, replace_one, step};
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

    /// Whole words and regexes narrow matches, groups fill in replacements, bad regexes say so.
    #[test]
    fn options_and_regex_replace() {
        let mut editor = editor_with("let a = 1; let ab = 2; letter");
        let mut ui = Ui::default();
        ui.search.query = "let".into();
        open(&mut ui, &editor, true);
        assert_eq!(ui.search.matches.len(), 3);
        ui.search.flip(Toggle::Word);
        refresh(&mut ui, &editor);
        assert_eq!(ui.search.matches.len(), 2);
        ui.search.flip(Toggle::Word);
        ui.search.flip(Toggle::Regex);
        ui.search.query = r"let (\w+) =".into();
        ui.search.replacement = "const $1 =".into();
        replace_all(&mut ui, &mut editor);
        assert_eq!(
            editor.document().text().to_string(),
            "const a = 1; const ab = 2; letter"
        );
        ui.search.query = "(".into();
        refresh(&mut ui, &editor);
        assert!(ui.search.error.is_some());
        assert!(ui.search.matches.is_empty());
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
