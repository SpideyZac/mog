//! The layer that shows the focused document.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Document, InlayHint, Severity, TokenKind, View, movement, view};
use mog_git::LineChange;
use mog_syntax::{Kind, Span, language_for};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    highlight::{DocumentKey, Done, Job, Worker, map_spans, to_edits},
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

/// How long a frame waits for fresh colors, enough for small files to never show stale ones.
const HIGHLIGHT_WAIT: Duration = Duration::from_millis(4);

/// Converts a cell count to a terminal coordinate, saturating on overflow.
fn cells(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// Things worked out in the background instead of every frame.
#[derive(Default)]
struct Cache {
    /// The document the cache is for, as `(index, path, colored)`.
    key: Option<(usize, Option<PathBuf>, bool)>,
    /// The document version the cached work lines up with.
    version: u64,
    /// Whether the work was carried over from an older version and needs redoing.
    stale: bool,
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

/// Returns how many cells the inlay hints before char column `col` of a line take up.
///
/// A hint at `col` itself sits before that char, so it counts only when `at` is set. The cursor
/// stays in front of such a hint, like in other editors.
fn hint_cells(hints: &[InlayHint], col: usize, at: bool) -> usize {
    hints
        .iter()
        .filter(|hint| hint.col < col || (at && hint.col == col))
        .map(|hint| hint.label.width())
        .sum()
}

/// Turns a visual cell on `line` into the cell it would be without inlay hints, so clicks land on
/// the text under the mouse. A click on a hint lands just before the code it is about.
fn cell_without_hints(document: &Document, line: usize, cell: usize, tab_width: usize) -> usize {
    let hints = document.inlay_hints(line);
    if hints.is_empty() {
        return cell;
    }
    let text = document.text();
    let start = text.line_to_char(line);
    let mut shift = 0;
    for hint in hints {
        let at = view::visual_col(text, start + hint.col, tab_width) + shift;
        let width = hint.label.width();
        if cell < at {
            break;
        }
        if cell < at + width {
            return at - shift;
        }
        shift += width;
    }
    cell - shift
}

/// Draws the inlay hint `label` starting at visual cell `col`, keeping to the visible cells.
fn draw_hint(
    buf: &mut Buffer,
    origin: Position,
    col: usize,
    label: &str,
    style: Style,
    scroll_col: usize,
    width: usize,
) {
    let mut cell = col;
    for ch in label.chars() {
        let cells_wide = ch.to_string().width().max(1);
        if cell >= scroll_col && cell + cells_wide <= scroll_col + width {
            let x = origin.x + cells(cell - scroll_col);
            buf.set_string(x, origin.y, ch.to_string(), style);
        }
        cell += cells_wide;
    }
}

/// Returns the style for a semantic token `kind`, or `None` to keep the syntax color.
fn token_style(theme: &Theme, kind: TokenKind) -> Option<Style> {
    Some(match kind {
        TokenKind::Namespace => theme.namespace,
        TokenKind::Type => theme.type_name,
        TokenKind::Function => theme.function,
        TokenKind::Macro => theme.attribute,
        TokenKind::Property => theme.property,
        TokenKind::EnumMember | TokenKind::Constant => theme.constant,
        TokenKind::Parameter => theme.parameter,
        TokenKind::Keyword => theme.keyword,
        TokenKind::String => theme.string,
        TokenKind::Number => theme.number,
        TokenKind::Comment => theme.comment,
        TokenKind::Operator => theme.operator,
        TokenKind::Variable => return None,
    })
}

/// Returns `count` if the `count` chars from `start` are plain ascii without tabs, which take
/// exactly one cell each, or 0 otherwise.
fn plain_prefix(document: &Document, start: usize, count: usize) -> usize {
    let plain = document
        .text()
        .slice(start..start + count)
        .chunks()
        .all(|chunk| chunk.is_ascii() && !chunk.bytes().any(|byte| byte == b'\t'));
    if plain { count } else { 0 }
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
    /// Works out colors, bracket depths and git changes in the background.
    worker: Worker,
    /// The document and version the worker was last given, so it can be sent just the edits.
    sent: Option<(DocumentKey, u64)>,
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

    /// Returns whether the gutter needs a column for breakpoints and the debugger arrow.
    fn debug_column(ui: &Ui) -> bool {
        ui.debug.active || ui.breakpoints.values().any(|lines| !lines.is_empty())
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
        let signs = usize::from(ui.config.ui.git_gutter) + usize::from(Self::debug_column(ui));
        let numbers = if ui.config.ui.line_numbers {
            lines.to_string().len()
        } else {
            0
        };
        cells(signs + numbers + GUTTER_PADDING)
    }

    /// Brings the cache up to date with the shown document.
    ///
    /// Edits move the cached colors along right away, and fresh ones are worked out in the
    /// background. Large files get no colors or git changes at all.
    fn refresh_cache(&mut self, cx: &Context<'_>, index: usize) {
        let document = &cx.editor.documents()[index];
        let large = document.is_large();
        let colored = cx.ui.config.ui.syntax_highlighting && !large;
        let key = (index, document.path().map(ToOwned::to_owned), colored);
        let version = document.version();
        let base = document
            .path()
            .filter(|_| !large)
            .and_then(|path| cx.ui.git_base.get(path));
        let base_len = base.map(String::len);
        if self.cache.key.as_ref() != Some(&key) {
            self.cache = Cache {
                key: Some(key),
                version,
                stale: true,
                ..Cache::default()
            };
        } else if self.cache.version != version {
            match document.changes_since(self.cache.version) {
                Some(changes) => map_spans(&mut self.cache.spans, &changes),
                None => self.cache.spans.clear(),
            }
            self.cache.version = version;
            self.cache.stale = true;
        }
        if self.cache.base_len != base_len {
            self.cache.base_len = base_len;
            self.cache.stale = true;
        }
        if let Some(done) = self.worker.take(Duration::ZERO) {
            self.accept(document, done);
        }
        if !self.cache.stale || self.worker.is_busy() {
            return;
        }
        let language = document.path().and_then(language_for).filter(|_| colored);
        if language.is_none() && base.is_none() {
            self.cache.spans.clear();
            self.cache.changes.clear();
            self.cache.stale = false;
            return;
        }
        let key: DocumentKey = (index, document.path().map(ToOwned::to_owned));
        let edits = self
            .sent
            .as_ref()
            .filter(|(sent, _)| *sent == key)
            .and_then(|(_, from)| {
                document
                    .changes_since(*from)
                    .map(|changes| (*from, to_edits(&changes)))
            });
        self.sent = Some((key.clone(), version));
        self.worker.send(Job {
            document: key,
            version,
            edits,
            language,
            text: document.text().to_string(),
            base: base.cloned(),
        });
        if let Some(done) = self.worker.take(HIGHLIGHT_WAIT) {
            self.accept(document, done);
        }
    }

    /// Takes background results, moving them along with edits made since they started.
    fn accept(&mut self, document: &Document, mut done: Done) {
        if done.version != self.cache.version {
            let Some(changes) = document.changes_since(done.version) else {
                return;
            };
            map_spans(&mut done.spans, &changes);
        }
        self.cache.stale = done.version != self.cache.version;
        self.cache.spans = done.spans;
        self.cache.depths = done.depths;
        self.cache.changes = done.changes;
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
            // inlay hints push the cursor right, past where the view thinks it is
            let cursor = Self::cursor_cell(cx);
            let view = cx.editor.view_mut();
            let right = view.scroll_col + usize::from(text_width);
            if text_width > 0 && cursor >= right {
                view.scroll_col = cursor + 1 - usize::from(text_width);
            }
            shown_view.scroll_col = cx.editor.view().scroll_col;
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
        let decorations = path.and_then(|path| cx.ui.plugin_decorations.get(path));
        let mut change = self.cache.changes.iter().peekable();
        let mut span_index = 0;
        let number_width = lines.to_string().len();
        let search = &cx.ui.search;
        let matches = if active { &search.matches[..] } else { &[] };
        let first_visible = text.line_to_char(scroll.scroll_line.min(lines - 1));
        let mut match_index = matches.partition_point(|(_, to)| *to <= first_visible);
        let mut guide_indent = 0;
        let mut ghost_below: Option<(u16, Vec<String>)> = None;

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
            let stopped_here = path.is_some_and(|path| {
                cx.ui
                    .debug
                    .stopped_at
                    .as_ref()
                    .is_some_and(|(at, at_line)| at == path && *at_line == line)
            });
            let line_style = match diagnostic.map(|d| d.severity) {
                _ if stopped_here => Some(theme.debug_line),
                Some(Severity::Error) if settings.error_lens => Some(theme.error_line),
                Some(Severity::Warning) if settings.error_lens => Some(theme.warning_line),
                _ if is_cursor_line && settings.cursor_line => Some(theme.cursor_line),
                _ => None,
            };
            if let Some(style) = line_style {
                buf.set_style(Rect::new(area.x, y, area.width, 1), style);
            }

            let mut gx = area.x;
            if Self::debug_column(cx.ui) {
                let breakpoint = path
                    .and_then(|path| cx.ui.breakpoints.get(path))
                    .is_some_and(|lines| lines.contains(&line));
                if stopped_here {
                    buf.set_string(gx, y, "\u{25b6}", theme.warning);
                } else if breakpoint {
                    buf.set_string(gx, y, "\u{25cf}", theme.breakpoint);
                }
                gx += 1;
            }
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
            let tokens = if settings.semantic_highlighting && settings.syntax_highlighting {
                document.semantic_tokens(line)
            } else {
                &[]
            };
            let mut token = 0;
            let right_edge = scroll.scroll_col + usize::from(text_width);
            let hints = if settings.inlay_hints {
                document.inlay_hints(line)
            } else {
                &[]
            };
            let mut hint = 0;
            let hint_origin = Position::new(text_x, y);
            let text_cells = usize::from(text_width);
            // a plain ascii start scrolled off to the left is one cell per char, so jump over it
            let skip = if hints.is_empty() {
                plain_prefix(document, start, scroll.scroll_col.min(len))
            } else {
                0
            };
            col += skip;
            // tabs line up on the columns of the text itself, hints aside
            let mut plain = skip;
            for (i, ch) in text.slice(start + skip..line_end).chars().enumerate() {
                let i = i + skip;
                while let Some(found) = hints.get(hint).filter(|found| found.col <= i) {
                    let label = &found.label;
                    let (scroll_col, style) = (scroll.scroll_col, theme.inlay_hint);
                    draw_hint(buf, hint_origin, col, label, style, scroll_col, text_cells);
                    col += label.width();
                    hint += 1;
                }
                // the rest of a very long line is off screen, so stop instead of walking it
                if col >= right_edge {
                    break;
                }
                let pos = start + i;
                let width = view::char_width(ch, plain, tab_width);
                plain += width;
                // chars scrolled off to the left only matter for bracket depth
                if col + width <= scroll.scroll_col {
                    if movement::BRACKETS.iter().any(|(open, _)| *open == ch) {
                        depth += 1;
                    } else if movement::BRACKETS.iter().any(|(_, close)| *close == ch) {
                        depth = depth.saturating_sub(1);
                    }
                    col += width;
                    continue;
                }
                while span < self.cache.spans.len() && self.cache.spans[span].to <= pos {
                    span += 1;
                }
                let kind = self
                    .cache
                    .spans
                    .get(span)
                    .filter(|s| s.from <= pos)
                    .map(|s| s.kind);
                while tokens.get(token).is_some_and(|t| t.to <= i) {
                    token += 1;
                }
                let semantic = tokens
                    .get(token)
                    .filter(|t| t.from <= i)
                    .and_then(|t| token_style(theme, t.kind));
                let mut style = match (semantic, kind) {
                    (Some(semantic), _) => theme.text.patch(semantic),
                    (None, Some(kind)) => theme.text.patch(kind_style(theme, kind)),
                    (None, None) => theme.text,
                };
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
            // hints after the last char, like a type at the end of a line
            for found in hints.get(hint..).unwrap_or_default() {
                if col >= right_edge {
                    break;
                }
                let (scroll_col, style) = (scroll.scroll_col, theme.inlay_hint);
                draw_hint(
                    buf,
                    hint_origin,
                    col,
                    &found.label,
                    style,
                    scroll_col,
                    text_cells,
                );
                col += found.label.width();
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
                let cell = view::visual_col(text, extra.head, tab_width)
                    + hint_cells(hints, extra.head - start, false);
                if visible(cell) {
                    let x = text_x + cells(cell - scroll.scroll_col);
                    let block = Style::new().bg(theme.palette.accent).fg(theme.palette.bg);
                    buf.set_style(Rect::new(x, y, 1, 1), block);
                }
            }

            let ghost = cx.ui.ghost.as_ref().filter(|ghost| {
                ghost.document == cx.editor.active()
                    && ghost.version == document_version
                    && ghost.pos == selection.head
                    && is_cursor_line
            });
            if let Some(ghost) = ghost {
                let tab = " ".repeat(tab_width.max(1));
                let mut suggestion = ghost
                    .text()
                    .split('\n')
                    .map(|line| line.replace('\t', &tab));
                let mut first = suggestion.next().unwrap_or_default();
                if ghost.items.len() > 1 {
                    first.push_str(&format!("  {}/{}", ghost.index + 1, ghost.items.len()));
                }
                let head_col = view::visual_col(text, selection.head, tab_width)
                    + hint_cells(hints, selection.head - start, false);
                if visible(head_col) {
                    let x = text_x + cells(head_col - scroll.scroll_col);
                    let room = usize::from(text_x + text_width).saturating_sub(usize::from(x));
                    buf.set_stringn(x, y, first, room, theme.ghost);
                }
                ghost_below = Some((y, suggestion.collect()));
                continue;
            }
            let inline = match diagnostic {
                Some(d) if settings.error_lens => {
                    let message = d.message.lines().next().unwrap_or_default();
                    Some((format!("\u{25cf} {message}"), severity_style(d.severity)))
                }
                _ => decorations
                    .and_then(|decorations| decorations.get(&line))
                    .map(|(text, color)| {
                        (
                            text.clone(),
                            theme.color_style(color.as_deref(), theme.inlay_hint),
                        )
                    })
                    .or_else(|| {
                        blame
                            .filter(|_| is_cursor_line && settings.git_blame)
                            .map(|blame| (blame.to_owned(), theme.blame))
                    }),
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

        // the rest of a multi line suggestion covers the lines below until it is accepted
        if let Some((y, rest)) = ghost_below {
            let text_x = area.x + self.gutter_width;
            let rows = usize::from(area.bottom().saturating_sub(y + 1));
            let shown = rest.len().min(rows);
            for (i, line) in rest.iter().take(shown).enumerate() {
                let row_y = y + 1 + u16::try_from(i).unwrap_or(u16::MAX);
                let mut line: String = line.chars().skip(scroll.scroll_col).collect();
                if i + 1 == shown && shown < rest.len() {
                    line.push_str(" \u{2026}");
                }
                let row = Rect::new(text_x, row_y, text_width, 1);
                buf.set_style(row, theme.text);
                for x in row.left()..row.right() {
                    buf[(x, row_y)].set_symbol(" ");
                }
                buf.set_stringn(text_x, row_y, line, usize::from(text_width), theme.ghost);
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
        let row = usize::from(event.row.saturating_sub(area.y));
        let col = {
            // clicks are on screen cells, which inlay hints push away from the text
            let scroll = cx.editor.view();
            let (scroll_col, scroll_line) = (scroll.scroll_col, scroll.scroll_line);
            let cell = usize::from(event.column.saturating_sub(text_x)) + scroll_col;
            let document = cx.editor.document();
            let line = (scroll_line + row).min(document.text().len_lines().saturating_sub(1));
            let tab_width = cx.editor.options().tab_width;
            cell_without_hints(document, line, cell, tab_width).saturating_sub(scroll_col)
        };
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
                let items = menu::editor_menu(cx.ui, cx.editor);
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

    fn is_animating(&self) -> bool {
        // keep drawing until the background work lands
        self.worker.is_busy()
    }
}

impl EditorView {
    /// Returns the visual cell of the cursor on its line, counting inlay hints in front of it.
    fn cursor_cell(cx: &Context<'_>) -> usize {
        let document = cx.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let line = text.char_to_line(head);
        let col = view::visual_col(text, head, cx.editor.options().tab_width);
        if cx.ui.config.ui.inlay_hints {
            col + hint_cells(
                document.inlay_hints(line),
                head - text.line_to_char(line),
                false,
            )
        } else {
            col
        }
    }

    /// Returns where the text cursor is on screen, if it is visible.
    fn cursor_position(&self, area: Rect, cx: &Context<'_>) -> Option<Position> {
        let gutter = Self::gutter_width_for(cx.editor.document().text().len_lines(), cx.ui);
        let gutter = gutter.min(area.width);
        let document = cx.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let scroll = cx.editor.view();
        let line = text.char_to_line(head).checked_sub(scroll.scroll_line)?;
        let col = Self::cursor_cell(cx).checked_sub(scroll.scroll_col)?;
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
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use mog_core::{
        Diagnostic, Editor, InlayHint, MemoryClipboard, Range, SemanticToken, Severity, TokenKind,
        Transaction,
    };
    use ratatui::{
        Terminal,
        backend::TestBackend,
        buffer::Buffer,
        layout::{Position, Rect},
    };

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

    /// Inlay hints follow the line and semantic tokens color what they cover.
    #[test]
    fn draws_hints_and_tokens() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let document = editor.document_mut();
        document.apply(Transaction::insert(0, "let x = 1;"), Range::point(0), false);
        document.set_inlay_hints(vec![(
            0,
            InlayHint {
                col: 5,
                label: ": i32".into(),
            },
        )]);
        document.set_semantic_tokens(vec![(
            0,
            SemanticToken {
                from: 4,
                to: 5,
                kind: TokenKind::Parameter,
            },
        )]);
        let buffer = draw(&mut editor, 30, 2);
        let row: String = (0..30).map(|x| buffer[(x, 0)].symbol()).collect();
        assert_eq!(row.trim_end(), "1 let x: i32 = 1;");
        assert_eq!(
            buffer[(6, 0)].fg,
            Theme::default().parameter.fg.expect("color")
        );
        assert_eq!(
            buffer[(8, 0)].fg,
            Theme::default().inlay_hint.fg.expect("color")
        );
    }

    /// The cursor skips over inlay hints and clicks past one land on the code after it.
    #[test]
    fn cursor_and_clicks_skip_hints() {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        let document = editor.document_mut();
        document.apply(Transaction::insert(0, "let x = 1;"), Range::point(0), false);
        document.set_inlay_hints(vec![(
            0,
            InlayHint {
                col: 5,
                label: ": i32".into(),
            },
        )]);
        // after `=` the cursor sits past the hint
        editor.select(7, 7);
        let mut compositor = Compositor::new();
        compositor.push(Box::new(EditorView::new()));
        let theme = Theme::default();
        let mut ui = Ui::default();
        ui.config.ui.tabs = false;
        ui.config.ui.minimap = false;
        ui.config.ui.git_gutter = false;
        let mut terminal = Terminal::new(TestBackend::new(30, 2)).expect("test terminal");
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
        assert_eq!(cursor, Some(Position::new(14, 0)));
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let mut cx = Context {
            editor: &mut editor,
            theme: &theme,
            ui: &mut ui,
        };
        compositor.handle_mouse(click, Rect::new(0, 0, 30, 2), &mut cx);
        assert_eq!(editor.document().selection().head, 8);
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
