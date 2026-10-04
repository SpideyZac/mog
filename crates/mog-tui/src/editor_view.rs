//! The layer that shows the focused document.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Document, Severity, View, movement, view};
use mog_git::{LineChange, line_changes};
use mog_syntax::{Highlighter, Kind, Span, language_for};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};

use crate::{
    compositor::{Context, EventResult, Layer},
    menu,
    theme::{RAINBOW_LEN, Theme},
    ui::{Focus, Layout, Pane, Ui},
};

/// Blank cells between the line numbers and the text.
const GUTTER_PADDING: usize = 1;

/// The longest gap between clicks that still counts as a double or triple click.
const MULTI_CLICK_TIME: Duration = Duration::from_millis(400);

/// How many lines one wheel notch scrolls.
const WHEEL_LINES: isize = 3;

/// Blank cells between the end of a line and inline messages like diagnostics.
const INLINE_GAP: usize = 4;

/// How many lines ahead an empty line looks to decide its indent guides.
const GUIDE_LOOKAHEAD: usize = 64;

/// Converts a cell count to a terminal coordinate, saturating on overflow.
fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// Things worked out once per document version instead of every frame.
#[derive(Default)]
struct Cache {
    /// The document the cache is for, as `(index, path, version)`.
    key: Option<(usize, Option<PathBuf>, u64)>,
    /// The highlighted spans, sorted and not overlapping.
    spans: Vec<Span>,
    /// The bracket nesting depth at the start of each line.
    depths: Vec<usize>,
    /// The length of the git base the changes were computed against, to notice a new base.
    base_len: Option<usize>,
    /// The changed lines compared to the last commit.
    changes: Vec<(usize, LineChange)>,
}

/// Returns the syntax style for `kind`.
fn kind_style(theme: &Theme, kind: Kind) -> Style {
    match kind {
        Kind::Keyword => theme.keyword,
        Kind::Function => theme.function,
        Kind::Type => theme.type_name,
        Kind::String => theme.string,
        Kind::Number => theme.number,
        Kind::Constant => theme.constant,
        Kind::Comment => theme.comment,
        Kind::Operator => theme.operator,
        Kind::Punctuation => theme.punctuation,
        Kind::Attribute => theme.attribute,
        Kind::Property => theme.property,
        Kind::Namespace => theme.namespace,
        Kind::Markup => theme.markup,
    }
}

/// Returns the bracket depth at the start of every line, skipping strings and comments.
fn bracket_depths(document: &Document, spans: &[Span]) -> Vec<usize> {
    let text = document.text();
    let mut depths = Vec::with_capacity(text.len_lines());
    let mut depth = 0usize;
    let mut span = 0;
    depths.push(0);
    for (pos, ch) in text.chars().enumerate() {
        if ch == '\n' {
            depths.push(depth);
            continue;
        }
        while span < spans.len() && spans[span].to <= pos {
            span += 1;
        }
        let quoted = spans
            .get(span)
            .is_some_and(|s| s.from <= pos && matches!(s.kind, Kind::String | Kind::Comment));
        if quoted {
            continue;
        }
        if movement::BRACKETS.iter().any(|(open, _)| *open == ch) {
            depth += 1;
        } else if movement::BRACKETS.iter().any(|(_, close)| *close == ch) {
            depth = depth.saturating_sub(1);
        }
    }
    depths
}

/// Returns the leading whitespace width of `line` in cells, or `None` for blank lines.
fn indent_of(document: &Document, line: usize, tab_width: usize) -> Option<usize> {
    let text = document.text();
    let start = text.line_to_char(line);
    let len = movement::line_len(text, line);
    let mut col = 0;
    for ch in text.slice(start..start + len).chars() {
        match ch {
            ' ' | '\t' => col += view::char_width(ch, col, tab_width),
            _ => return Some(col),
        }
    }
    None
}

/// Draws the focused document with line numbers, syntax colors, diagnostics and git changes.
#[derive(Default)]
pub struct EditorView {
    /// The gutter width used in the last render, needed to map mouse clicks to text.
    gutter_width: u16,
    /// The time, cell and count of the last left click, used to detect multi clicks.
    last_click: Option<(Instant, Position, u8)>,
    /// The syntax highlighter.
    highlighter: Highlighter,
    /// Per document work kept between frames.
    cache: Cache,
    /// Which pane this view draws.
    pane: Pane,
}

impl EditorView {
    /// Creates the editor view for the main pane.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates the editor view for the right pane of a split.
    pub fn side() -> Self {
        Self {
            pane: Pane::Side,
            ..Self::default()
        }
    }

    /// Returns whether this pane shows the focused document.
    fn is_active(&self, ui: &Ui) -> bool {
        ui.split
            .as_ref()
            .is_none_or(|split| split.focused == self.pane)
    }

    /// Returns which document this pane shows and how it is scrolled.
    fn shown(&self, cx: &Context<'_>) -> (usize, View) {
        match cx.ui.split.as_ref().filter(|_| !self.is_active(cx.ui)) {
            Some(split) => (
                split
                    .other_document
                    .min(cx.editor.documents().len().saturating_sub(1)),
                split.other_view.clone(),
            ),
            None => (cx.editor.active(), cx.editor.view().clone()),
        }
    }

    /// Records a left click at `at` and returns whether it is a single, double or triple click.
    fn click_count(&mut self, at: Position) -> u8 {
        let now = Instant::now();
        let count = match self.last_click {
            Some((time, pos, count)) if pos == at && now - time < MULTI_CLICK_TIME => count % 3 + 1,
            _ => 1,
        };
        self.last_click = Some((now, at, count));
        count
    }

    /// Returns the gutter width for a document with `lines` lines under the current settings.
    fn gutter_width_for(lines: usize, ui: &Ui) -> u16 {
        let signs = usize::from(ui.config.ui.git_gutter);
        let numbers = if ui.config.ui.line_numbers {
            lines.to_string().len()
        } else {
            0
        };
        cells(signs + numbers + GUTTER_PADDING)
    }

    /// Brings the cache up to date with the focused document.
    fn refresh_cache(&mut self, cx: &Context<'_>, index: usize) {
        let document = &cx.editor.documents()[index];
        let key = (
            index,
            document.path().map(ToOwned::to_owned),
            document.version(),
        );
        let base = document.path().and_then(|path| cx.ui.git_base.get(path));
        let base_len = base.map(String::len);
        let stale = self.cache.key.as_ref() != Some(&key);
        if stale {
            let language = document.path().and_then(language_for);
            self.cache.spans = match language {
                Some(language) if cx.ui.config.ui.syntax_highlighting => self
                    .highlighter
                    .highlight(language, &document.text().to_string()),
                _ => Vec::new(),
            };
            self.cache.depths = bracket_depths(document, &self.cache.spans);
            self.cache.key = Some(key);
        }
        if stale || self.cache.base_len != base_len {
            self.cache.changes = base.map_or_else(Vec::new, |base| {
                line_changes(base, &document.text().to_string())
            });
            self.cache.base_len = base_len;
        }
    }
}

impl Layer for EditorView {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        match self.pane {
            Pane::Main => layout.editor,
            Pane::Side => layout.split,
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let active = self.is_active(cx.ui);
        let (index, mut shown_view) = self.shown(cx);
        self.refresh_cache(cx, index);
        let lines = cx.editor.documents()[index].text().len_lines();
        self.gutter_width = Self::gutter_width_for(lines, cx.ui).min(area.width);
        let text_width = area.width - self.gutter_width;
        shown_view.resize(usize::from(text_width), usize::from(area.height));
        if active {
            cx.editor
                .view_mut()
                .resize(usize::from(text_width), usize::from(area.height));
            cx.ui.cursor_screen = self.cursor_position(area, cx);
        } else if let Some(split) = cx.ui.split.as_mut() {
            split.other_view = shown_view.clone();
        }
        let theme = cx.theme;
        let settings = &cx.ui.config.ui;
        // a split edge keeps the two panes apart
        if self.pane == Pane::Side {
            for y in area.top()..area.bottom() {
                buf.set_string(area.x, y, "\u{258f}", theme.border);
            }
        }

        let tab_width = cx.editor.options().tab_width;
        let document = &cx.editor.documents()[index];
        let document_version = document.version();
        let text = document.text();
        let selection = document.selection();
        let extras = if active { document.cursors() } else { &[] };
        let cursor_line = text.char_to_line(selection.head);
        let scroll = &shown_view;
        let brackets = movement::matching_bracket(text, selection.head);
        let path = document.path();
        let blame = cx
            .ui
            .blame
            .as_ref()
            .filter(|(file, line, _)| Some(file.as_path()) == path && *line == cursor_line)
            .map(|(_, _, text)| text.as_str());
        let mut change = self.cache.changes.iter().peekable();
        let mut span_index = 0;
        let number_width = lines.to_string().len();
        let search = &cx.ui.search;
        let matches = if active { &search.matches[..] } else { &[] };
        let first_visible = text.line_to_char(scroll.scroll_line.min(lines - 1));
        let mut match_index = matches.partition_point(|(_, to)| *to <= first_visible);
        let mut guide_indent = 0;

        for row in 0..area.height {
            let line = scroll.scroll_line + usize::from(row);
            if line >= lines {
                break;
            }
            let y = area.y + row;
            let start = text.line_to_char(line);
            let len = movement::line_len(text, line);
            let line_end = start + len;
            let is_cursor_line = line == cursor_line;

            let diagnostic = document
                .diagnostics()
                .iter()
                .filter(|d| {
                    let first = text.char_to_line(d.from.min(text.len_chars()));
                    first == line
                })
                .min_by_key(|d| d.severity);
            let severity_style = |severity| match severity {
                Severity::Error => theme.error,
                Severity::Warning => theme.warning,
                Severity::Info => theme.info,
                Severity::Hint => theme.hint,
            };
            let line_style = match diagnostic.map(|d| d.severity) {
                Some(Severity::Error) if settings.error_lens => Some(theme.error_line),
                Some(Severity::Warning) if settings.error_lens => Some(theme.warning_line),
                _ if is_cursor_line && settings.cursor_line => Some(theme.cursor_line),
                _ => None,
            };
            if let Some(style) = line_style {
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
            }

            let mut gx = area.x;
            if settings.git_gutter {
                while change.peek().is_some_and(|(changed, _)| *changed < line) {
                    change.next();
                }
                let sign = change.peek().filter(|(changed, _)| *changed == line).map(
                    |(_, kind)| match kind {
                        LineChange::Added => ("\u{258e}", theme.git_added),
                        LineChange::Modified => ("\u{258e}", theme.git_modified),
                        LineChange::RemovedAbove => ("\u{2594}", theme.git_removed),
                    },
                );
                if let Some((symbol, style)) = sign {
                    buf.set_string(gx, y, symbol, style);
                }
                gx += 1;
            }
            if settings.line_numbers {
                let shown = if settings.relative_line_numbers && !is_cursor_line {
                    line.abs_diff(cursor_line)
                } else {
                    line + 1
                };
                let style = match diagnostic {
                    Some(d) if d.severity <= Severity::Warning => severity_style(d.severity),
                    _ if is_cursor_line => theme.gutter_active,
                    _ => theme.gutter,
                };
                let room = usize::from(area.right().saturating_sub(gx));
                buf.set_stringn(gx, y, format!("{shown:>number_width$} "), room, style);
            }

            let text_x = area.x + self.gutter_width;
            let visible = |cell: usize| {
                cell >= scroll.scroll_col && cell - scroll.scroll_col < usize::from(text_width)
            };

            if settings.indent_guides && tab_width > 0 {
                let indent = indent_of(document, line, tab_width).unwrap_or_else(|| {
                    (line + 1..lines.min(line + GUIDE_LOOKAHEAD))
                        .find_map(|next| indent_of(document, next, tab_width))
                        .map_or(guide_indent, |next| next.min(guide_indent.max(next)))
                });
                guide_indent = indent;
                for col in (tab_width..indent).step_by(tab_width) {
                    if visible(col) {
                        let x = text_x + cells(col - scroll.scroll_col);
                        buf.set_string(x, y, "\u{2502}", theme.indent_guide);
                    }
                }
            }

            span_index = self.cache.spans[span_index..].partition_point(|span| span.to <= start)
                + span_index;
            let mut span = span_index;
            let mut depth = self.cache.depths.get(line).copied().unwrap_or(0);
            let mut col = 0;
            for (i, ch) in text.slice(start..line_end).chars().enumerate() {
                let pos = start + i;
                let width = view::char_width(ch, col, tab_width);
                while span < self.cache.spans.len() && self.cache.spans[span].to <= pos {
                    span += 1;
                }
                let kind = self
                    .cache
                    .spans
                    .get(span)
                    .filter(|s| s.from <= pos)
                    .map(|s| s.kind);
                let mut style =
                    kind.map_or(theme.text, |kind| theme.text.patch(kind_style(theme, kind)));
                let quoted = matches!(kind, Some(Kind::String | Kind::Comment));
                if !quoted {
                    if movement::BRACKETS.iter().any(|(open, _)| *open == ch) {
                        if settings.rainbow_brackets {
                            style = style.patch(theme.rainbow[depth % RAINBOW_LEN]);
                        }
                        depth += 1;
                    } else if movement::BRACKETS.iter().any(|(_, close)| *close == ch) {
                        depth = depth.saturating_sub(1);
                        if settings.rainbow_brackets {
                            style = style.patch(theme.rainbow[depth % RAINBOW_LEN]);
                        }
                    }
                }
                if brackets.is_some_and(|(a, b)| a == pos || b == pos) {
                    style = style.patch(theme.matching_bracket);
                }
                while matches.get(match_index).is_some_and(|(_, to)| *to <= pos) {
                    match_index += 1;
                }
                if let Some(&(from, to)) = matches.get(match_index)
                    && (from..to).contains(&pos)
                {
                    style = if search.current == Some(match_index) {
                        style.patch(theme.search_current)
                    } else {
                        style.patch(theme.search_match)
                    };
                }
                let selected = (selection.from()..selection.to()).contains(&pos)
                    || extras.iter().any(|r| (r.from()..r.to()).contains(&pos));
                if selected {
                    style = style.patch(theme.selection);
                }
                if settings.error_lens
                    && let Some(d) = document
                        .diagnostics()
                        .iter()
                        .find(|d| (d.from..d.to.max(d.from + 1)).contains(&pos))
                {
                    let color = severity_style(d.severity).fg.unwrap_or_default();
                    style = style
                        .add_modifier(Modifier::UNDERLINED)
                        .underline_color(color);
                }
                for cell in col..col + width {
                    if !visible(cell) {
                        continue;
                    }
                    let x = text_x + cells(cell - scroll.scroll_col);
                    let symbol = if ch == '\t' || ch.is_control() {
                        " ".to_owned()
                    } else if cell == col {
                        ch.to_string()
                    } else {
                        // the trailing half of a wide char is drawn by its first cell
                        continue;
                    };
                    buf.set_string(x, y, symbol, style);
                }
                col += width;
            }

            // show selected line breaks as one highlighted cell like most editors
            if selection.from() <= line_end && line_end < selection.to() && visible(col) {
                let x = text_x + cells(col - scroll.scroll_col);
                buf.set_style(Rect::new(x, y, 1, 1), theme.selection);
            }

            // extra cursors are drawn as blocks since the terminal only has one real cursor
            for extra in extras
                .iter()
                .filter(|r| (start..=line_end).contains(&r.head))
            {
                let cell = view::visual_col(text, extra.head, tab_width);
                if visible(cell) {
                    let x = text_x + cells(cell - scroll.scroll_col);
                    let block = Style::new().bg(theme.palette.accent).fg(theme.palette.bg);
                    buf.set_style(Rect::new(x, y, 1, 1), block);
                }
            }

            let ghost = cx.ui.ghost.as_ref().filter(|(document, version, pos, _)| {
                *document == cx.editor.active()
                    && *version == document_version
                    && *pos == selection.head
                    && is_cursor_line
            });
            if let Some((_, _, _, suggestion)) = ghost {
                let first = suggestion.lines().next().unwrap_or_default();
                let more = if suggestion.lines().nth(1).is_some() {
                    " \u{2026}"
                } else {
                    ""
                };
                let head_col = view::visual_col(text, selection.head, tab_width);
                if visible(head_col) {
                    let x = text_x + cells(head_col - scroll.scroll_col);
                    let room = usize::from(text_x + text_width).saturating_sub(usize::from(x));
                    buf.set_stringn(x, y, format!("{first}{more}"), room, theme.ghost);
                }
                continue;
            }
            let inline = match diagnostic {
                Some(d) if settings.error_lens => {
                    let message = d.message.lines().next().unwrap_or_default();
                    Some((format!("\u{25cf} {message}"), severity_style(d.severity)))
                }
                _ => blame
                    .filter(|_| is_cursor_line && settings.git_blame)
                    .map(|blame| (blame.to_owned(), theme.blame)),
            };
            if let Some((message, style)) = inline {
                let cell = col + INLINE_GAP;
                if cell >= scroll.scroll_col {
                    let x = cell - scroll.scroll_col;
                    let room = usize::from(text_width).saturating_sub(x);
                    if room > 0 {
                        buf.set_stringn(text_x + cells(x), y, message, room, style);
                    }
                }
            }
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect, cx: &mut Context<'_>) -> EventResult {
        if !self.is_active(cx.ui) {
            match event.kind {
                MouseEventKind::Down(_) => {
                    cx.ui.request(Command::Custom("split.focus".into()));
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                    let lines = if event.kind == MouseEventKind::ScrollUp {
                        -WHEEL_LINES
                    } else {
                        WHEEL_LINES
                    };
                    let (index, _) = self.shown(cx);
                    let text = cx.editor.documents()[index].text().clone();
                    if let Some(split) = cx.ui.split.as_mut() {
                        split.other_view.scroll_by(lines, &text);
                    }
                }
                _ => {}
            }
            return EventResult::Consumed;
        }
        let text_x = area.x + self.gutter_width;
        let in_gutter = event.column < text_x;
        let col = usize::from(event.column.saturating_sub(text_x));
        let row = usize::from(event.row.saturating_sub(area.y));
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                cx.ui.focus = Focus::Editor;
                let count = self.click_count(Position::new(event.column, event.row));
                if in_gutter || count == 3 {
                    cx.editor.select_line_at(row);
                } else if count == 2 {
                    cx.editor.select_word_at(row, col);
                } else {
                    let extend = event.modifiers.contains(KeyModifiers::SHIFT);
                    if event.modifiers.contains(KeyModifiers::ALT) {
                        cx.editor.toggle_cursor_at(row, col);
                        return EventResult::Consumed;
                    }
                    cx.editor.click(row, col, extend);
                    if event.modifiers.contains(KeyModifiers::CONTROL) {
                        cx.ui.request(Command::Custom("lsp.definition".into()));
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // dragging past the edges scrolls so long selections are possible
                let row = if event.row < area.y {
                    cx.editor.execute(Command::Scroll(-1));
                    0
                } else if event.row >= area.bottom() {
                    cx.editor.execute(Command::Scroll(1));
                    usize::from(area.height.saturating_sub(1))
                } else {
                    row
                };
                cx.editor.click(row, col, true);
            }
            MouseEventKind::ScrollUp => {
                cx.editor.execute(Command::Scroll(-WHEEL_LINES));
            }
            MouseEventKind::ScrollDown => {
                cx.editor.execute(Command::Scroll(WHEEL_LINES));
            }
            MouseEventKind::Up(MouseButton::Left) => {}
            MouseEventKind::Down(MouseButton::Right) => {
                cx.ui.focus = Focus::Editor;
                // right clicking outside the selection moves the cursor there first
                let selection = cx.editor.document().selection();
                let at = cx.editor.view().pos_at_cell(
                    cx.editor.document().text(),
                    row,
                    col,
                    cx.editor.options().tab_width,
                );
                if !(selection.from()..selection.to()).contains(&at) {
                    cx.editor.click(row, col, false);
                }
                let items = menu::editor_menu(cx.ui);
                menu::open_menu(cx.ui, Position::new(event.column, event.row), items);
            }
            _ => return EventResult::Ignored,
        }
        EventResult::Consumed
    }

    fn cursor(&self, area: Rect, cx: &Context<'_>) -> Option<Position> {
        if cx.ui.focus != Focus::Editor || cx.ui.overlay.is_some() || !self.is_active(cx.ui) {
            return None;
        }
        self.cursor_position(area, cx)
    }
}

impl EditorView {
    /// Returns where the text cursor is on screen, if it is visible.
    fn cursor_position(&self, area: Rect, cx: &Context<'_>) -> Option<Position> {
        let gutter = Self::gutter_width_for(cx.editor.document().text().len_lines(), cx.ui);
        let gutter = gutter.min(area.width);
        let document = cx.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let scroll = cx.editor.view();
        let line = text.char_to_line(head).checked_sub(scroll.scroll_line)?;
        let col = view::visual_col(text, head, cx.editor.options().tab_width)
            .checked_sub(scroll.scroll_col)?;
        let x = usize::from(gutter) + col;
        if line >= usize::from(area.height) || x >= usize::from(area.width) {
            return None;
        }
        Some(Position::new(area.x + cells(x), area.y + cells(line)))
    }
}

#[cfg(test)]
/// Tests for [`EditorView`].
mod tests {
    use mog_core::{Diagnostic, Editor, MemoryClipboard, Range, Severity, Transaction};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Position};

    use super::EditorView;
    use crate::{
        compositor::{Compositor, Context},
        theme::Theme,
        ui::Ui,
    };

    /// Draws `editor` on a `width` by `height` screen with only the plain editor visible.
    fn draw(editor: &mut Editor, width: u16, height: u16) -> Buffer {
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        let theme = Theme::default();
        let mut ui = Ui::default();
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        ui.config.ui.git_gutter = false;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor,
                    theme: &theme,
                    ui: &mut ui,
                };
                let _ = compositor.render(frame, &mut cx);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// Error lens writes the diagnostic message after the line.
    #[test]
    fn error_lens_shows_message() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let document = editor.document_mut();
        document.apply(Transaction::insert(0, "let x"), Range::point(0), false);
        document.set_diagnostics(vec![Diagnostic {
            from: 4,
            to: 5,
            severity: Severity::Error,
            message: "unused\nmore".into(),
        }]);
        let buffer = draw(&mut editor, 30, 2);
        let row: String = (0..30).map(|x| buffer[(x, 0)].symbol()).collect();
        assert_eq!(row.trim_end(), "1 let x    \u{25cf} unused");
    }

    /// Text is drawn after the gutter with tabs expanded and the cursor placed.
    #[test]
    fn renders_text_and_cursor() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let document = editor.document_mut();
        document.apply(Transaction::insert(0, "hi\n\tyo"), Range::point(6), false);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        let theme = Theme::default();
        let mut ui = Ui::default();
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        ui.config.ui.git_gutter = false;
        ui.config.ui.indent_guides = false;
        let mut terminal = Terminal::new(TestBackend::new(12, 3)).expect("test terminal");
        let mut cursor = None;
        terminal
            .draw(|frame| {
                let mut cx = Context {
                    editor: &mut editor,
                    theme: &theme,
                    ui: &mut ui,
                };
                cursor = compositor.render(frame, &mut cx);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..2)
            .map(|y| (0..12).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert_eq!(rows, ["1 hi        ", "2     yo    "]);
        assert_eq!(cursor, Some(Position::new(8, 1)));
    }
}
