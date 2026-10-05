//! The debug panel on the right: where the program stopped, its call stack and its variables.

use std::path::PathBuf;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::Command;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{
    compositor::{Context, EventResult, Layer},
    theme::Theme,
    ui::{Layout, Ui},
};

/// The command that shows the frame picked in the call stack.
pub const FRAME_COMMAND: &str = "debug.frame";

/// How many console lines are kept.
const MAX_CONSOLE: usize = 500;

/// One frame of the call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameEntry {
    /// The function name.
    pub name: String,
    /// The file it is in, if it has source.
    pub path: Option<PathBuf>,
    /// The line, from 0.
    pub line: usize,
}

/// One variable, indented by how deep it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableEntry {
    /// The name, or a scope heading like `Locals`.
    pub name: String,
    /// The value, empty for a heading.
    pub value: String,
    /// How deep it is nested, 0 for headings.
    pub depth: usize,
}

/// What the debugger is doing.
#[derive(Debug, Clone, Default)]
pub struct DebugState {
    /// Whether the panel is shown.
    pub open: bool,
    /// Whether a debugging session is running.
    pub active: bool,
    /// Whether the program is stopped.
    pub paused: bool,
    /// A short description of the state, like `paused at a breakpoint`.
    pub status: String,
    /// The call stack, innermost first.
    pub frames: Vec<FrameEntry>,
    /// The frame whose variables are shown.
    pub frame: usize,
    /// The variables of that frame.
    pub variables: Vec<VariableEntry>,
    /// What the program and the debugger printed.
    pub console: Vec<String>,
    /// The file and line the program is stopped at.
    pub stopped_at: Option<(PathBuf, usize)>,
}

impl DebugState {
    /// Adds printed `text`, which may hold several lines.
    pub fn print(&mut self, text: &str) {
        for line in text.lines() {
            if self.console.len() == MAX_CONSOLE {
                self.console.remove(0);
            }
            self.console.push(line.to_owned());
        }
    }

    /// Forgets the stop, as the program runs again.
    pub fn resume(&mut self) {
        self.paused = false;
        self.frames.clear();
        self.variables.clear();
        self.stopped_at = None;
        self.status = "running".into();
    }
}

/// The debug panel layer.
#[derive(Debug, Default)]
pub struct DebugPanel {
    /// The screen rows of the call stack from the last frame, with their frame index.
    frame_rows: Vec<(u16, usize)>,
}

impl DebugPanel {
    /// Creates the panel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes a section heading at `y` and returns the next row.
    fn heading(area: Rect, y: u16, text: &str, buf: &mut Buffer, theme: &Theme) -> u16 {
        if y < area.bottom() {
            buf.set_stringn(
                area.x,
                y,
                text,
                usize::from(area.width),
                theme.sidebar_title,
            );
        }
        y + 1
    }
}

impl Layer for DebugPanel {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.debug
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let state = &cx.ui.debug;
        buf.set_style(area, theme.sidebar);
        for y in area.top()..area.bottom() {
            buf.set_string(area.x, y, "\u{2502}", theme.border);
        }
        let inner = Rect {
            x: area.x + 2,
            width: area.width.saturating_sub(3),
            ..area
        };
        let width = usize::from(inner.width);
        let (icon, style) = match (state.active, state.paused) {
            (false, _) => ("\u{25a0}", theme.popup_dim),
            (true, true) => ("\u{23f8}", theme.warning),
            (true, false) => ("\u{25b6}", theme.git_added),
        };
        let status = if state.active {
            state.status.clone()
        } else {
            "not debugging, f5 starts".to_owned()
        };
        buf.set_stringn(inner.x, inner.y, format!("{icon} {status}"), width, style);
        // stack, variables and console share what is left, the console gets the rest
        let rest = inner.height.saturating_sub(2);
        let stack_rows = u16::try_from(state.frames.len()).unwrap_or(0).min(rest / 3);
        let variable_rows = u16::try_from(state.variables.len())
            .unwrap_or(0)
            .min(rest.saturating_sub(stack_rows) / 2)
            .max(1);
        let mut y = inner.y + 2;
        y = Self::heading(inner, y, "CALL STACK", buf, theme);
        self.frame_rows.clear();
        for (index, frame) in state
            .frames
            .iter()
            .enumerate()
            .take(usize::from(stack_rows))
        {
            if y >= inner.bottom() {
                break;
            }
            let place = frame
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| format!("  {}:{}", name.to_string_lossy(), frame.line + 1))
                .unwrap_or_default();
            let row = if index == state.frame {
                theme.popup_selected
            } else {
                theme.sidebar
            };
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), row);
            let end = buf.set_stringn(inner.x, y, &frame.name, width, row).0;
            let room = usize::from(inner.right().saturating_sub(end));
            buf.set_stringn(end, y, &place, room, row.patch(theme.popup_dim));
            self.frame_rows.push((y, index));
            y += 1;
        }
        y += 1;
        y = Self::heading(inner, y, "VARIABLES", buf, theme);
        for variable in state.variables.iter().take(usize::from(variable_rows)) {
            if y >= inner.bottom() {
                break;
            }
            if variable.depth == 0 {
                buf.set_stringn(inner.x, y, &variable.name, width, theme.popup_dim);
            } else {
                let indent = "  ".repeat(variable.depth - 1);
                let name = format!("{indent}{} ", variable.name);
                let end = buf.set_stringn(inner.x, y, &name, width, theme.property).0;
                let room = usize::from(inner.right().saturating_sub(end));
                buf.set_stringn(end, y, format!("= {}", variable.value), room, theme.sidebar);
            }
            y += 1;
        }
        y += 1;
        y = Self::heading(inner, y, "CONSOLE", buf, theme);
        let rows = usize::from(inner.bottom().saturating_sub(y));
        let start = state.console.len().saturating_sub(rows);
        for line in &state.console[start..] {
            buf.set_stringn(inner.x, y, line.replace('\t', "  "), width, theme.sidebar);
            y += 1;
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        if let MouseEventKind::Down(MouseButton::Left) = event.kind
            && let Some(&(_, index)) = self.frame_rows.iter().find(|(y, _)| *y == event.row)
        {
            cx.ui.debug.frame = index;
            cx.ui.request(Command::Custom(FRAME_COMMAND.into()));
        }
        EventResult::Consumed
    }
}

#[cfg(test)]
/// Tests for the debug panel state.
mod tests {
    use super::DebugState;

    /// Printed text is split into lines and resuming forgets the stop.
    #[test]
    fn prints_and_resumes() {
        let mut state = DebugState::default();
        state.print("one\ntwo\n");
        assert_eq!(state.console, ["one", "two"]);
        state.paused = true;
        state.stopped_at = Some(("a.rs".into(), 3));
        state.resume();
        assert!(!state.paused);
        assert!(state.stopped_at.is_none());
    }
}
