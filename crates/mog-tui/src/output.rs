//! The output of the task that ran last, like a build, in a scrollable popup.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Command, Key, KeyChord};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};

use crate::{
    compositor::{Context, EventResult, Layer},
    popup,
    ui::{Focus, Layout, Overlay, Ui},
};

/// How many lines of output are kept.
const MAX_LINES: usize = 10_000;

/// How many lines a page key scrolls.
const PAGE: usize = 20;

/// What the panel says at the bottom.
const HINTS: &str = "up and down scroll  enter lists the problems  ctrl+c stops  esc closes";

/// The output of the newest task.
#[derive(Debug, Clone, Default)]
pub struct OutputState {
    /// The task name.
    pub title: String,
    /// The lines it printed.
    pub lines: Vec<String>,
    /// Whether it is still running.
    pub running: bool,
    /// How many lines up from the bottom the view is, 0 to follow new output.
    pub scroll: usize,
}

impl OutputState {
    /// Starts showing a new task called `title`.
    pub fn start(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.lines.clear();
        self.running = true;
        self.scroll = 0;
    }

    /// Adds a printed `line`, dropping the oldest ones past the limit.
    pub fn push(&mut self, line: String) {
        if self.lines.len() == MAX_LINES {
            self.lines.remove(0);
        }
        self.lines.push(line);
        // keep looking at the same lines while scrolled up
        if self.scroll > 0 {
            self.scroll += 1;
        }
    }
}

/// The task output popup.
#[derive(Debug, Default)]
pub struct OutputPanel {
    /// The popup from the last frame.
    area: Rect,
    /// How many lines fit, from the last frame.
    rows: usize,
}

impl OutputPanel {
    /// Creates the panel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scrolls up by `delta` lines, or down when it is negative.
    fn scroll(&self, ui: &mut Ui, delta: isize) {
        let output = &mut ui.output;
        let most = output.lines.len().saturating_sub(self.rows.max(1));
        output.scroll = output.scroll.saturating_add_signed(delta).min(most);
    }
}

impl Layer for OutputPanel {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.overlay == Some(Overlay::Output) {
            layout.screen
        } else {
            Rect::default()
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let output = &cx.ui.output;
        self.area = popup::centered(
            area,
            area.width.saturating_sub(8),
            area.height.saturating_sub(4),
        );
        popup::dim_around(area, self.area, buf, theme);
        let state = if output.running { "running" } else { "done" };
        let title = format!("\u{2699} {}, {state}", output.title);
        let inner = popup::frame(self.area, buf, theme, &title);
        if inner.height < 2 {
            return;
        }
        self.rows = usize::from(inner.height - 1);
        let end = output.lines.len().saturating_sub(output.scroll);
        let start = end.saturating_sub(self.rows);
        let width = usize::from(inner.width);
        for (offset, line) in output.lines[start..end].iter().enumerate() {
            let y = inner.y + u16::try_from(offset).unwrap_or(0);
            let lower = line.to_lowercase();
            let style = if lower.contains("error") || lower.contains("failed") {
                theme.popup.patch(theme.error)
            } else if lower.contains("warning") {
                theme.popup.patch(theme.warning)
            } else {
                theme.popup
            };
            buf.set_stringn(inner.x, y, line.replace('\t', "    "), width, style);
        }
        buf.set_stringn(inner.x, inner.bottom() - 1, HINTS, width, theme.popup_dim);
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if cx.ui.overlay != Some(Overlay::Output) {
            return EventResult::Ignored;
        }
        let page = PAGE.cast_signed();
        match chord.key {
            Key::Char('c') if chord.mods.ctrl => cx.ui.request(Command::Custom("task.stop".into())),
            _ if chord.mods.ctrl || chord.mods.alt => return EventResult::Ignored,
            Key::Up | Key::Char('k') => self.scroll(cx.ui, 1),
            Key::Down | Key::Char('j') => self.scroll(cx.ui, -1),
            Key::PageUp => self.scroll(cx.ui, page),
            Key::PageDown => self.scroll(cx.ui, -page),
            Key::Home => self.scroll(cx.ui, isize::MAX / 2),
            Key::End => cx.ui.output.scroll = 0,
            Key::Enter => cx.ui.open(Overlay::Problems),
            Key::Esc | Key::Char('q') => {
                cx.ui.close();
                cx.ui.focus = Focus::Editor;
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
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll(cx.ui, 3),
            MouseEventKind::ScrollDown => self.scroll(cx.ui, -3),
            MouseEventKind::Down(MouseButton::Left)
                if !self.area.contains(Position::new(event.column, event.row)) =>
            {
                cx.ui.close();
                cx.ui.focus = Focus::Editor;
            }
            _ => {}
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for task output.
mod tests {
    use super::OutputState;

    /// Scrolled up output stays on the same lines as more comes in.
    #[test]
    fn keeps_place_while_scrolled() {
        let mut output = OutputState::default();
        output.start("build");
        output.push("one".into());
        output.scroll = 1;
        output.push("two".into());
        assert_eq!(output.scroll, 2);
        output.scroll = 0;
        output.push("three".into());
        assert_eq!(output.scroll, 0);
    }
}
