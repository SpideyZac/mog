//! Drawing on top of the whole screen, like a marker on a monitor.

use std::collections::HashMap;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    theme::Palette,
    ui::{Layout, Ui},
};

/// The braille dot bits as `[column][row]`.
const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// The first braille code point.
const BRAILLE: u32 = 0x2800;

/// Braille dots per cell across.
const DOTS_WIDE: i32 = 2;

/// Braille dots per cell down.
const DOTS_TALL: i32 = 4;

/// How many cells around the pointer the eraser clears in each direction.
const ERASER_RADIUS: i32 = 1;

/// How many strokes can be undone.
const UNDO_LIMIT: usize = 64;

/// The number of colors to pick from.
pub const COLOR_COUNT: usize = 8;

/// What dragging the left mouse button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// A thin braille line.
    #[default]
    Pen,
    /// A thick line of full blocks.
    Marker,
    /// Removes marks under the pointer.
    Eraser,
    /// Click to place a cursor, then type.
    Text,
}

impl Tool {
    /// Every tool in toolbar order.
    const ALL: [Self; 4] = [Self::Pen, Self::Marker, Self::Eraser, Self::Text];

    /// Returns the toolbar label.
    fn label(self) -> &'static str {
        match self {
            Self::Pen => "pen",
            Self::Marker => "marker",
            Self::Eraser => "eraser",
            Self::Text => "text",
        }
    }
}

/// What is drawn in one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// Braille dots, as the bits past [`BRAILLE`].
    Dots(u8),
    /// A plain char, from the marker or the text tool.
    Glyph(char),
}

/// The drawing, kept between frames and toggled from the keymap.
#[derive(Debug, Clone, Default)]
pub struct AnnotateState {
    /// Whether the mouse and keys draw instead of editing.
    pub active: bool,
    /// The tool the left button uses.
    pub tool: Tool,
    /// The color index, see [`color`].
    pub color: usize,
    /// What is drawn, by cell, with the color index it was drawn in.
    marks: HashMap<(u16, u16), (Mark, usize)>,
    /// Earlier drawings to go back to, newest last.
    undo: Vec<HashMap<(u16, u16), (Mark, usize)>>,
    /// The last cell of the stroke being drawn.
    last: Option<(u16, u16)>,
    /// Where typed text goes, as `(column the line started at, cursor)`.
    typing: Option<(u16, Position)>,
}

impl AnnotateState {
    /// Returns whether nothing is drawn.
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Turns drawing on or off.
    pub fn toggle(&mut self) {
        self.active = !self.active;
        self.last = None;
        self.typing = None;
    }

    /// Wipes the whole drawing, keeping it for undo.
    pub fn clear(&mut self) {
        if !self.marks.is_empty() {
            self.checkpoint();
            self.marks.clear();
        }
        self.typing = None;
    }

    /// Goes back to before the last stroke.
    pub fn undo(&mut self) {
        if let Some(marks) = self.undo.pop() {
            self.marks = marks;
        }
    }

    /// Remembers the drawing so the next change can be undone.
    fn checkpoint(&mut self) {
        if self.undo.len() == UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(self.marks.clone());
    }

    /// Starts a stroke at `cell`, erasing instead of drawing when `erase` is set.
    fn press(&mut self, cell: (u16, u16), erase: bool) {
        self.typing = None;
        if self.tool == Tool::Text && !erase {
            self.typing = Some((cell.0, Position::new(cell.0, cell.1)));
            return;
        }
        self.checkpoint();
        self.last = Some(cell);
        self.stroke(cell, cell, erase);
    }

    /// Continues the stroke to `cell`.
    fn drag(&mut self, cell: (u16, u16), erase: bool) {
        if let Some(last) = self.last {
            self.stroke(last, cell, erase);
            self.last = Some(cell);
        }
    }

    /// Draws or erases along the line from `from` to `to`.
    fn stroke(&mut self, from: (u16, u16), to: (u16, u16), erase: bool) {
        let tool = if erase { Tool::Eraser } else { self.tool };
        let start = (i32::from(from.0), i32::from(from.1));
        let end = (i32::from(to.0), i32::from(to.1));
        match tool {
            Tool::Pen => {
                // dot space line between cell centers keeps diagonals thin
                let center = |(x, y): (i32, i32)| (x * DOTS_WIDE, y * DOTS_TALL + 1);
                let dots = line(center(start), center(end));
                if from == to {
                    let (x, y) = center(start);
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        self.dot(x + dx, y + dy);
                    }
                }
                for (x, y) in dots {
                    self.dot(x, y);
                }
            }
            Tool::Marker => {
                for cell in line(start, end) {
                    self.put(cell, Mark::Glyph('\u{2588}'));
                }
            }
            Tool::Eraser => {
                for (x, y) in line(start, end) {
                    for dy in -ERASER_RADIUS..=ERASER_RADIUS {
                        for dx in -ERASER_RADIUS..=ERASER_RADIUS {
                            if let Some(cell) = to_cell(x + dx, y + dy) {
                                self.marks.remove(&cell);
                            }
                        }
                    }
                }
            }
            Tool::Text => {}
        }
    }

    /// Sets the braille dot at dot coordinates `x`, `y`.
    fn dot(&mut self, x: i32, y: i32) {
        let Some(cell) = to_cell(x.div_euclid(DOTS_WIDE), y.div_euclid(DOTS_TALL)) else {
            return;
        };
        // the rems are below 4 so the casts are lossless
        let bit = DOTS[x.rem_euclid(DOTS_WIDE) as usize][y.rem_euclid(DOTS_TALL) as usize];
        let entry = self
            .marks
            .entry(cell)
            .or_insert((Mark::Dots(0), self.color));
        entry.0 = match entry.0 {
            Mark::Dots(bits) => Mark::Dots(bits | bit),
            Mark::Glyph(_) => Mark::Dots(bit),
        };
        entry.1 = self.color;
    }

    /// Puts `mark` in the cell at `x`, `y`.
    fn put(&mut self, (x, y): (i32, i32), mark: Mark) {
        if let Some(cell) = to_cell(x, y) {
            self.marks.insert(cell, (mark, self.color));
        }
    }

    /// Types `ch` at the text cursor.
    fn type_char(&mut self, ch: char) {
        let Some((_, at)) = self.typing else {
            return;
        };
        self.checkpoint();
        self.marks
            .insert((at.x, at.y), (Mark::Glyph(ch), self.color));
        let width = u16::try_from(ch.to_string().width()).unwrap_or(1).max(1);
        if let Some((_, cursor)) = &mut self.typing {
            cursor.x = cursor.x.saturating_add(width);
        }
    }

    /// Removes the char before the text cursor.
    fn backspace(&mut self) {
        let Some((start, at)) = self.typing else {
            return;
        };
        if at.x <= start {
            return;
        }
        self.checkpoint();
        let x = at.x - 1;
        self.marks.remove(&(x, at.y));
        self.typing = Some((start, Position::new(x, at.y)));
    }

    /// Moves the text cursor to the start of the next line.
    fn newline(&mut self) {
        if let Some((start, at)) = self.typing {
            self.typing = Some((start, Position::new(start, at.y.saturating_add(1))));
        }
    }
}

/// Returns the cell at `x`, `y`, or `None` when it is off screen.
fn to_cell(x: i32, y: i32) -> Option<(u16, u16)> {
    Some((u16::try_from(x).ok()?, u16::try_from(y).ok()?))
}

/// Returns every point on the line from `from` to `to`, both ends included.
fn line(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = from;
    let dx = (to.0 - x).abs();
    let dy = -(to.1 - y).abs();
    let sx = if x < to.0 { 1 } else { -1 };
    let sy = if y < to.1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut points = vec![(x, y)];
    while (x, y) != to {
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        points.push((x, y));
    }
    points
}

/// Returns color number `index` from `p`.
pub fn color(p: &Palette, index: usize) -> Color {
    [
        p.accent, p.red, p.orange, p.yellow, p.green, p.cyan, p.blue, p.fg,
    ][index % COLOR_COUNT]
}

/// Something a click on the toolbar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarAction {
    /// Picks a tool.
    Tool(Tool),
    /// Picks a color.
    Color(usize),
    /// Undoes the last stroke.
    Undo,
    /// Wipes the drawing.
    Clear,
    /// Stops drawing.
    Done,
}

/// Draws the annotations over everything and takes the mouse while drawing.
#[derive(Debug, Default)]
pub struct Annotations {
    /// Where the toolbar buttons were drawn.
    toolbar: Vec<(Rect, ToolbarAction)>,
}

impl Annotations {
    /// Creates the drawing layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws the toolbar along the top of `area` and remembers its buttons.
    fn render_toolbar(&mut self, area: Rect, buf: &mut Buffer, cx: &Context<'_>) {
        let p = cx.theme.palette;
        let state = &cx.ui.annotate;
        let base = Style::new().fg(p.fg).bg(p.raised);
        let dim = base.fg(p.dim);
        let picked = Style::new()
            .fg(p.bg)
            .bg(p.accent)
            .add_modifier(Modifier::BOLD);
        let mut parts: Vec<(String, Style, Option<ToolbarAction>)> = vec![(
            " \u{270e} draw ".into(),
            base.fg(p.accent).add_modifier(Modifier::BOLD),
            None,
        )];
        for tool in Tool::ALL {
            let style = if tool == state.tool { picked } else { base };
            parts.push((" ".into(), base, None));
            parts.push((
                format!(" {} ", tool.label()),
                style,
                Some(ToolbarAction::Tool(tool)),
            ));
        }
        parts.push(("  ".into(), base, None));
        for index in 0..COLOR_COUNT {
            let symbol = if index == state.color {
                "\u{25c9}"
            } else {
                "\u{25cf}"
            };
            parts.push((
                format!("{symbol} "),
                base.fg(color(&p, index)),
                Some(ToolbarAction::Color(index)),
            ));
        }
        parts.push((" ".into(), base, None));
        parts.push((" undo ".into(), dim, Some(ToolbarAction::Undo)));
        parts.push((" clear ".into(), dim, Some(ToolbarAction::Clear)));
        parts.push((
            " done ".into(),
            base.fg(p.green).add_modifier(Modifier::BOLD),
            Some(ToolbarAction::Done),
        ));
        parts.push((" ".into(), base, None));
        let width: u16 = parts
            .iter()
            .map(|(text, _, _)| u16::try_from(text.width()).unwrap_or(0))
            .sum();
        let width = width.min(area.width);
        let mut x = area.x + (area.width - width) / 2;
        let y = area.y;
        self.toolbar.clear();
        for (text, style, action) in parts {
            let w = u16::try_from(text.width()).unwrap_or(0);
            if x + w > area.right() {
                break;
            }
            buf.set_string(x, y, &text, style);
            if let Some(action) = action {
                self.toolbar.push((Rect::new(x, y, w, 1), action));
            }
            x += w;
        }
    }

    /// Runs a toolbar `action`.
    fn run(action: ToolbarAction, cx: &mut Context<'_>) {
        let state = &mut cx.ui.annotate;
        match action {
            ToolbarAction::Tool(tool) => state.tool = tool,
            ToolbarAction::Color(index) => state.color = index,
            ToolbarAction::Undo => state.undo(),
            ToolbarAction::Clear => state.clear(),
            ToolbarAction::Done => state.toggle(),
        }
    }
}

impl Layer for Annotations {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.annotate.active || !ui.annotate.is_empty() {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let p = cx.theme.palette;
        for (&(x, y), &(mark, index)) in &cx.ui.annotate.marks {
            if !area.contains(Position::new(x, y)) {
                continue;
            }
            let symbol = match mark {
                Mark::Dots(bits) => char::from_u32(BRAILLE + u32::from(bits)).unwrap_or(' '),
                Mark::Glyph(ch) => ch,
            };
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(symbol).set_fg(color(&p, index));
            }
        }
        if cx.ui.annotate.active {
            self.render_toolbar(area, buf, cx);
        } else {
            self.toolbar.clear();
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        // popups opened while drawing get the mouse and keys
        if !cx.ui.annotate.active || cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        let cell = (event.column, event.row);
        let point = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(button) => {
                let hit = self
                    .toolbar
                    .iter()
                    .find(|(rect, _)| rect.contains(point))
                    .map(|(_, action)| *action);
                if let Some(action) = hit {
                    Self::run(action, cx);
                } else if button != MouseButton::Middle {
                    cx.ui.annotate.press(cell, button == MouseButton::Right);
                }
            }
            MouseEventKind::Drag(button) => {
                cx.ui.annotate.drag(cell, button == MouseButton::Right);
            }
            MouseEventKind::Up(_) => cx.ui.annotate.last = None,
            // scrolling still moves the text underneath
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => return EventResult::Ignored,
            MouseEventKind::Moved => {}
        }
        EventResult::Consumed
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        let state = &mut cx.ui.annotate;
        if !state.active {
            return EventResult::Ignored;
        }
        if chord.mods.ctrl && chord.key == Key::Char('z') {
            state.undo();
            return EventResult::Consumed;
        }
        if state.typing.is_some() {
            match chord.key {
                Key::Esc => state.typing = None,
                Key::Enter => state.newline(),
                Key::Backspace => state.backspace(),
                _ => match chord.typed_char() {
                    Some(ch) => state.type_char(ch),
                    None if chord.mods.ctrl || chord.mods.alt => return EventResult::Ignored,
                    None => {}
                },
            }
            return EventResult::Consumed;
        }
        // other ctrl and alt chords still run their commands, like the one that stops drawing
        if (chord.mods.ctrl || chord.mods.alt) && chord.typed_char().is_none() {
            return EventResult::Ignored;
        }
        match chord.key {
            Key::Esc => state.toggle(),
            Key::Char('p') => state.tool = Tool::Pen,
            Key::Char('m') => state.tool = Tool::Marker,
            Key::Char('e') => state.tool = Tool::Eraser,
            Key::Char('t') => state.tool = Tool::Text,
            Key::Char('u') => state.undo(),
            Key::Char('c') => state.clear(),
            Key::Char(ch @ '1'..='8') => state.color = usize::from(ch as u8 - b'1'),
            _ => {}
        }
        EventResult::Consumed
    }

    fn cursor(&self, _area: Rect, cx: &Context<'_>) -> Option<Position> {
        let state = &cx.ui.annotate;
        state
            .active
            .then_some(state.typing)
            .flatten()
            .map(|(_, at)| at)
    }
}

#[cfg(test)]
/// Tests for drawing.
mod tests {
    use super::{AnnotateState, Mark, Tool, line};

    /// Lines include both ends and step one cell at a time.
    #[test]
    fn lines_are_continuous() {
        assert_eq!(line((0, 0), (3, 0)), [(0, 0), (1, 0), (2, 0), (3, 0)]);
        assert_eq!(line((2, 2), (0, 0)), [(2, 2), (1, 1), (0, 0)]);
        assert_eq!(line((1, 1), (1, 1)), [(1, 1)]);
    }

    /// The pen sets braille dots and a drag fills the cells between.
    #[test]
    fn pen_draws_dots() {
        let mut state = AnnotateState::default();
        state.press((0, 0), false);
        state.drag((3, 0), false);
        for x in 0..=3 {
            assert!(
                matches!(state.marks.get(&(x, 0)), Some((Mark::Dots(_), _))),
                "cell {x} is empty"
            );
        }
    }

    /// The marker fills cells and the right button erases them.
    #[test]
    fn marker_and_eraser() {
        let mut state = AnnotateState {
            tool: Tool::Marker,
            ..AnnotateState::default()
        };
        state.press((5, 5), false);
        state.drag((9, 5), false);
        assert_eq!(state.marks.len(), 5);
        state.press((9, 5), true);
        assert_eq!(state.marks.len(), 3);
    }

    /// Text goes where the click was and backspace takes it away.
    #[test]
    fn text_tool_types() {
        let mut state = AnnotateState {
            tool: Tool::Text,
            ..AnnotateState::default()
        };
        state.press((2, 1), false);
        for ch in "hi".chars() {
            state.type_char(ch);
        }
        assert_eq!(state.marks.get(&(3, 1)), Some(&(Mark::Glyph('i'), 0)));
        state.backspace();
        assert!(!state.marks.contains_key(&(3, 1)));
    }

    /// Undo goes back one stroke and clear can be undone.
    #[test]
    fn undo_and_clear() {
        let mut state = AnnotateState::default();
        state.press((0, 0), false);
        state.press((5, 5), false);
        state.undo();
        assert!(!state.marks.contains_key(&(5, 5)));
        state.clear();
        assert!(state.is_empty());
        state.undo();
        assert!(!state.is_empty());
    }
}
