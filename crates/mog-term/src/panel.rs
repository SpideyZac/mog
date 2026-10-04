//! The terminal panel under the editor.

use std::{mem, time::Duration};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Key, KeyChord, Modifiers};
use mog_tui::{Context, EventResult, Focus, Layer, Layout, Theme, Ui};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;
use vt100::{Cell, Color as TermColor};

use crate::{keys, shell::Shell};

/// Commands whose keys still reach mog while the terminal has focus.
const PASSTHROUGH: &[&str] = &["terminal.toggle", "command_palette", "quit"];

/// How many lines one wheel step scrolls.
const WHEEL_LINES: usize = 3;

/// The button in the panel title that starts a fresh shell.
const RESTART: &str = " \u{27f3} restart ";

/// Returns the theme color for one of the 16 basic terminal colors.
fn basic_color(index: u8, theme: &Theme) -> Color {
    let p = theme.palette;
    match index % 8 {
        0 => p.raised,
        1 => p.red,
        2 => p.green,
        3 => p.yellow,
        4 => p.blue,
        5 => p.purple,
        6 => p.cyan,
        _ if index < 8 => p.dim,
        _ => p.fg,
    }
}

/// Returns the screen color for `color`, or `default` for the terminal default.
fn color(color: TermColor, default: Color, theme: &Theme) -> Color {
    match color {
        TermColor::Default => default,
        TermColor::Idx(index @ 0..16) => basic_color(index, theme),
        TermColor::Idx(index) => Color::Indexed(index),
        TermColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Returns the style a terminal cell is drawn with.
fn cell_style(cell: &Cell, theme: &Theme) -> Style {
    let p = theme.palette;
    let mut style = Style::new()
        .fg(color(cell.fgcolor(), p.fg, theme))
        .bg(color(cell.bgcolor(), p.bg, theme));
    for (on, modifier) in [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    style
}

/// A shell in a panel under the editor, opened with `ctrl+backtick` like VS Code.
#[derive(Default)]
pub struct TerminalPanel {
    /// The running shell, started the first time the panel opens.
    shell: Option<Shell>,
    /// Why the shell could not start.
    problem: Option<String>,
    /// How many lines the view is scrolled back.
    scroll: usize,
    /// The screen part of the panel at the last render.
    screen: Rect,
    /// Where the restart button was drawn at the last render.
    restart_button: Rect,
    /// Whether the panel was drawn since the last tick, so a hidden shell does not animate.
    shown: bool,
}

impl TerminalPanel {
    /// Creates the panel without starting a shell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether the panel takes keys right now.
    fn focused(ui: &Ui) -> bool {
        ui.terminal_open && ui.focus == Focus::Terminal && ui.overlay.is_none()
    }

    /// Starts the shell for a screen of `area` if it is not running.
    fn ensure_shell(&mut self, area: Rect, ui: &Ui) {
        if self.shell.is_some() || self.problem.is_some() || area.is_empty() {
            return;
        }
        let config = &ui.config.terminal;
        match Shell::spawn(
            &config.shell,
            &config.args,
            &ui.root,
            area.height,
            area.width,
        ) {
            Ok(shell) => self.shell = Some(shell),
            Err(err) => self.problem = Some(format!("could not start the shell: {err}")),
        }
    }

    /// Stops the shell so a fresh one starts on the next draw.
    fn restart(&mut self) {
        self.shell = None;
        self.problem = None;
        self.scroll = 0;
    }

    /// Sends a wheel step to a full screen program like `less` or `vim` as arrow keys.
    ///
    /// Returns `false` when the shell is on its normal screen, which mog scrolls itself.
    fn wheel_to_app(&self, up: bool) -> bool {
        let Some(shell) = &self.shell else {
            return false;
        };
        let (alternate, application) = {
            let parser = shell.parser();
            let screen = parser.screen();
            (screen.alternate_screen(), screen.application_cursor())
        };
        if !alternate {
            return false;
        }
        let key = if up { Key::Up } else { Key::Down };
        if let Some(bytes) =
            keys::chord_bytes(KeyChord::new(key, Modifiers::default()), application)
        {
            for _ in 0..WHEEL_LINES {
                shell.write(&bytes);
            }
        }
        true
    }

    /// Draws the shell screen into `area`.
    fn draw_screen(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some(shell) = &self.shell else {
            return;
        };
        let mut parser = shell.parser();
        // vt100 clamps this to the lines it kept
        parser.screen_mut().set_scrollback(self.scroll);
        self.scroll = parser.screen().scrollback();
        let screen = parser.screen();
        for y in 0..area.height {
            for x in 0..area.width {
                let Some(cell) = screen.cell(y, x) else {
                    continue;
                };
                let target = &mut buf[(area.x + x, area.y + y)];
                target.set_style(cell_style(cell, theme));
                if cell.is_wide_continuation() {
                    continue;
                }
                let text = cell.contents();
                target.set_symbol(if text.is_empty() { " " } else { text });
            }
        }
        parser.screen_mut().set_scrollback(0);
    }
}

impl Layer for TerminalPanel {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.terminal
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        if mem::take(&mut cx.ui.terminal_restart) {
            self.restart();
        }
        if self.shell.as_ref().is_some_and(Shell::has_exited) {
            // typing exit closes the panel like it does in vs code
            self.shell = None;
            self.scroll = 0;
            cx.ui.terminal_open = false;
            if cx.ui.focus == Focus::Terminal {
                cx.ui.focus = Focus::Editor;
            }
            return;
        }
        self.shown = true;
        let theme = cx.theme;
        let p = theme.palette;
        buf.set_style(area, Style::new().fg(p.fg).bg(p.bg));
        let focused = Self::focused(cx.ui);
        let border = if focused {
            theme.sidebar_title
        } else {
            theme.border
        };
        for x in area.left()..area.right() {
            buf.set_string(x, area.y, "\u{2500}", border);
        }
        self.screen = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        self.ensure_shell(self.screen, cx.ui);
        let title = match &self.shell {
            Some(shell) if self.scroll > 0 => {
                format!(" \u{276f} {} \u{2191}{} ", shell.name(), self.scroll)
            }
            Some(shell) => format!(" \u{276f} {} ", shell.name()),
            None => " \u{276f} terminal ".to_owned(),
        };
        buf.set_stringn(area.x + 1, area.y, title, usize::from(area.width), border);
        let width = u16::try_from(RESTART.width()).unwrap_or(0);
        let restart_x = area.right().saturating_sub(width + 1);
        self.restart_button = if restart_x > area.x + 20 {
            buf.set_string(restart_x, area.y, RESTART, border);
            Rect::new(restart_x, area.y, width, 1)
        } else {
            Rect::default()
        };
        if let Some(problem) = &self.problem {
            buf.set_stringn(
                self.screen.x + 1,
                self.screen.y,
                problem,
                usize::from(self.screen.width),
                theme.error,
            );
            return;
        }
        if let Some(shell) = &mut self.shell {
            shell.resize(self.screen.height, self.screen.width);
            let input = mem::take(&mut cx.ui.terminal_input);
            if !input.is_empty() {
                shell.write(&input);
                self.scroll = 0;
            }
        }
        self.draw_screen(self.screen, buf, theme);
    }

    fn handle_key(&mut self, chord: KeyChord, cx: &mut Context<'_>) -> EventResult {
        if !Self::focused(cx.ui) {
            return EventResult::Ignored;
        }
        let name = chord.to_string();
        let passthrough = cx
            .ui
            .bindings
            .iter()
            .any(|(keys, command)| *keys == name && PASSTHROUGH.contains(&command.as_str()));
        if passthrough {
            return EventResult::Ignored;
        }
        let page = usize::from(self.screen.height.max(1));
        match chord.key {
            Key::PageUp if chord.mods.shift => {
                self.scroll += page;
                return EventResult::Consumed;
            }
            Key::PageDown if chord.mods.shift => {
                self.scroll = self.scroll.saturating_sub(page);
                return EventResult::Consumed;
            }
            _ => {}
        }
        if let Some(shell) = &self.shell {
            let application = shell.parser().screen().application_cursor();
            if let Some(bytes) = keys::chord_bytes(chord, application) {
                shell.write(&bytes);
                self.scroll = 0;
            }
        }
        EventResult::Consumed
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let point = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if self.restart_button.contains(point) => {
                self.restart();
                cx.ui.focus = Focus::Terminal;
            }
            MouseEventKind::Down(MouseButton::Left) => cx.ui.focus = Focus::Terminal,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = event.kind == MouseEventKind::ScrollUp;
                if !self.wheel_to_app(up) {
                    self.scroll = if up {
                        self.scroll + WHEEL_LINES
                    } else {
                        self.scroll.saturating_sub(WHEEL_LINES)
                    };
                }
            }
            _ => {}
        }
        EventResult::Consumed
    }

    fn cursor(&self, _area: Rect, cx: &Context<'_>) -> Option<Position> {
        if !Self::focused(cx.ui) || self.scroll > 0 {
            return None;
        }
        let parser = self.shell.as_ref()?.parser();
        let screen = parser.screen();
        if screen.hide_cursor() {
            return None;
        }
        let (row, col) = screen.cursor_position();
        let at = Position::new(self.screen.x + col, self.screen.y + row);
        self.screen.contains(at).then_some(at)
    }

    fn tick(&mut self, _dt: Duration) {
        self.shown = false;
    }

    fn is_animating(&self) -> bool {
        // output arrives on its own thread so keep redrawing while the shell is on screen
        self.shell.is_some() && self.shown
    }
}
