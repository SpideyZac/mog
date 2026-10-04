//! The AI chat panel on the right.

use std::time::Duration;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    completion::wrap,
    compositor::{Context, EventResult, Layer},
    ui::{Focus, Layout, Ui},
};

/// The most lines the input box grows to.
const INPUT_LINES: usize = 4;

/// How long one frame of the thinking animation lasts.
const THINK_FRAME: Duration = Duration::from_millis(120);

/// The thinking animation.
const SPINNER: [&str; 8] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
];

/// The state of the chat panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatState {
    /// Whether the panel is shown.
    pub open: bool,
    /// The conversation as `(from_user, text)`.
    pub messages: Vec<(bool, String)>,
    /// What is being typed.
    pub input: String,
    /// Whether an answer is on its way.
    pub waiting: bool,
    /// How many lines the conversation is scrolled up from the bottom.
    pub scroll: usize,
}

/// Draws the conversation and takes typing when focused.
#[derive(Debug, Default)]
pub struct ChatPanel {
    /// Time spent waiting, for the spinner.
    waited: Duration,
    /// Whether the panel was waiting at the last render.
    waiting: bool,
    /// Where the text cursor goes.
    cursor: Option<Position>,
}

impl ChatPanel {
    /// Creates the panel.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Layer for ChatPanel {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.chat
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let p = theme.palette;
        let state = &cx.ui.chat;
        self.waiting = state.waiting;
        let focused = cx.ui.focus == Focus::Chat && cx.ui.overlay.is_none();
        buf.set_style(area, theme.sidebar);
        let border = if focused {
            theme.sidebar_title
        } else {
            theme.border
        };
        for y in area.top()..area.bottom() {
            buf.set_string(area.x, y, "\u{2502}", border);
        }
        let inner = Rect {
            x: area.x + 2,
            width: area.width.saturating_sub(3),
            ..area
        };
        if inner.width < 8 || inner.height < 6 {
            return;
        }
        let width = usize::from(inner.width);
        buf.set_string(inner.x, inner.y, "\u{2726} ai chat", theme.sidebar_title);
        let hint = "ctrl+l";
        buf.set_string(
            inner.right() - u16::try_from(hint.len()).unwrap_or(0),
            inner.y,
            hint,
            theme.popup_dim,
        );

        let input_lines = wrap(&state.input, width - 2, INPUT_LINES);
        let input_height = u16::try_from(input_lines.len().max(1)).unwrap_or(1);
        let input_top = inner.bottom() - input_height;
        let rule_y = input_top - 1;
        let rule = "\u{2500}".repeat(width);
        buf.set_string(inner.x, rule_y, rule, theme.border);
        buf.set_string(inner.x, input_top, "\u{276f}", theme.sidebar_title);
        if state.input.is_empty() {
            buf.set_string(inner.x + 2, input_top, "ask anything...", theme.popup_dim);
            self.cursor = Some(Position::new(inner.x + 2, input_top));
        } else {
            for (i, line) in input_lines.iter().enumerate() {
                let y = input_top + u16::try_from(i).unwrap_or(0);
                buf.set_stringn(inner.x + 2, y, line, width - 2, theme.sidebar);
            }
            let last = input_lines.last().map_or(0, |line| line.width());
            let y = input_top + input_height - 1;
            let x = (inner.x + 2 + u16::try_from(last).unwrap_or(0)).min(inner.right() - 1);
            self.cursor = Some(Position::new(x, y));
        }

        // lay the conversation out bottom up so the newest message is always visible
        let mut lines: Vec<(String, Style)> = Vec::new();
        for (from_user, text) in &state.messages {
            let (who, color) = if *from_user {
                ("you", p.accent)
            } else {
                ("mog", p.accent2)
            };
            lines.push((
                format!("{who} \u{203a}"),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ));
            for line in wrap(text, width, usize::MAX) {
                lines.push((line, Style::new().fg(p.fg)));
            }
            lines.push((String::new(), Style::new()));
        }
        if state.waiting {
            let frame =
                (self.waited.as_millis() / THINK_FRAME.as_millis()) as usize % SPINNER.len();
            lines.push((
                format!("{} thinking", SPINNER[frame]),
                Style::new().fg(p.accent2).add_modifier(Modifier::ITALIC),
            ));
        }
        if lines.is_empty() {
            lines.push((
                "ask about your code. alt+e explains a selection.".into(),
                theme.popup_dim,
            ));
        }
        let rows = usize::from(rule_y.saturating_sub(inner.y + 2));
        let max_scroll = lines.len().saturating_sub(rows);
        let scroll = state.scroll.min(max_scroll);
        let end = lines.len() - scroll;
        let start = end.saturating_sub(rows);
        for (i, (line, style)) in lines[start..end].iter().enumerate() {
            let y = inner.y + 2 + u16::try_from(i).unwrap_or(0);
            buf.set_stringn(inner.x, y, line, width, *style);
        }
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.focus != Focus::Chat || !cx.ui.chat.open || cx.ui.overlay.is_some() {
            return EventResult::Ignored;
        }
        let state = &mut cx.ui.chat;
        match chord.key {
            Key::Esc => cx.ui.focus = Focus::Editor,
            Key::Enter if chord.mods.alt || chord.mods.shift => state.input.push('\n'),
            Key::Enter => cx.ui.request(Command::Custom("ai.send".into())),
            Key::Backspace if chord.mods.ctrl => state.input.clear(),
            Key::Backspace => {
                state.input.pop();
            }
            Key::Up => state.scroll += 1,
            Key::Down => state.scroll = state.scroll.saturating_sub(1),
            Key::PageUp => state.scroll += 10,
            Key::PageDown => state.scroll = state.scroll.saturating_sub(10),
            _ => match chord.typed_char() {
                Some(ch) => state.input.push(ch),
                None => return EventResult::Ignored,
            },
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => cx.ui.focus = Focus::Chat,
            MouseEventKind::ScrollUp => cx.ui.chat.scroll += 3,
            MouseEventKind::ScrollDown => cx.ui.chat.scroll = cx.ui.chat.scroll.saturating_sub(3),
            _ => {}
        }
        EventResult::Consumed
    }

    fn cursor(&self, _area: Rect, cx: &Context<'_>) -> Option<Position> {
        (cx.ui.focus == Focus::Chat && cx.ui.overlay.is_none())
            .then_some(self.cursor)
            .flatten()
    }

    fn tick(&mut self, dt: Duration) {
        self.waited += dt;
    }

    fn is_animating(&self) -> bool {
        self.waiting
    }
}
