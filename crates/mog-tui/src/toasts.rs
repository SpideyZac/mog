//! Notifications plugins show in the top right corner of the editor, with a progress bar and
//! buttons when they want them.

use std::{
    mem,
    time::{Duration, Instant},
};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    theme::Theme,
    ui::{Layout, Ui},
};

/// How wide a notification is, borders included.
const WIDTH: u16 = 44;

/// The most notifications shown at once, newest first.
const MAX_SHOWN: usize = 4;

/// The most lines of text a notification shows.
const MAX_LINES: usize = 4;

/// How fast the busy bar moves, in cells a second.
const BUSY_SPEED: f32 = 12.0;

/// How long the busy block is, in cells.
const BUSY_WIDTH: usize = 6;

/// How loud a notification is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastLevel {
    /// Worth knowing.
    #[default]
    Info,
    /// Something may be wrong.
    Warning,
    /// Something went wrong.
    Error,
}

/// How far along the work a notification is about is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastProgress {
    /// This many percent done.
    Percent(u8),
    /// Busy, with no idea how long it takes.
    Busy,
}

/// A button on a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastButton {
    /// What it says.
    pub title: String,
    /// The command it runs, or `None` to tell the plugin it was clicked.
    pub command: Option<String>,
}

/// A notification a plugin shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// The plugin that shows it.
    pub plugin: String,
    /// Its id inside the plugin.
    pub id: String,
    /// The first line, in bold.
    pub title: String,
    /// The message under the title.
    pub text: String,
    /// How loud it is.
    pub level: ToastLevel,
    /// How far along the work is, if it is about work.
    pub progress: Option<ToastProgress>,
    /// The buttons along the bottom.
    pub buttons: Vec<ToastButton>,
    /// When it was shown.
    pub shown_at: Instant,
    /// How long it stays, forever when `None`.
    pub timeout: Option<Duration>,
}

impl Toast {
    /// Returns whether it is past its time at `now`.
    fn expired(&self, now: Instant) -> bool {
        self.timeout
            .is_some_and(|timeout| now.saturating_duration_since(self.shown_at) >= timeout)
    }

    /// Returns its text broken into lines that fit `width` cells.
    fn lines(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in self.text.lines() {
            let mut line = String::new();
            for word in paragraph.split_whitespace() {
                if !line.is_empty() && line.width() + 1 + word.width() > width {
                    lines.push(mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
            }
            lines.push(line);
        }
        lines.truncate(MAX_LINES);
        lines
    }

    /// Returns how many rows it takes, borders included.
    fn height(&self, width: usize) -> u16 {
        let rows = 1
            + self.lines(width).len()
            + usize::from(self.progress.is_some())
            + usize::from(!self.buttons.is_empty());
        u16::try_from(rows + 2).unwrap_or(u16::MAX)
    }
}

/// A click on a notification button that has no command, for the app to pass on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastClick {
    /// The plugin that shows it.
    pub plugin: String,
    /// Its id inside the plugin.
    pub id: String,
    /// The button, counted from 0.
    pub button: usize,
}

/// What a part of a drawn notification does when clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// Closes the notification at this index.
    Close(usize),
    /// Presses this button of the notification at this index.
    Button(usize, usize),
    /// Nothing, but the click stays on the notification.
    Nothing,
}

/// Returns the bar for `progress` in `width` cells, `elapsed` after it was shown.
fn bar(progress: ToastProgress, width: usize, elapsed: Duration) -> String {
    match progress {
        ToastProgress::Percent(percent) => {
            let label = format!(" {percent:>3}%");
            let room = width.saturating_sub(label.len());
            let filled = room * usize::from(percent.min(100)) / 100;
            format!(
                "{}{}{label}",
                "\u{2588}".repeat(filled),
                "\u{2591}".repeat(room - filled)
            )
        }
        ToastProgress::Busy => {
            let travel = width.saturating_sub(BUSY_WIDTH).max(1);
            let step = (elapsed.as_secs_f32() * BUSY_SPEED) as usize % (travel * 2);
            let at = if step > travel {
                travel * 2 - step
            } else {
                step
            };
            let mut cells = vec!["\u{2591}"; width];
            for cell in cells.iter_mut().skip(at).take(BUSY_WIDTH) {
                *cell = "\u{2588}";
            }
            cells.concat()
        }
    }
}

/// Draws notifications and handles clicks on them.
#[derive(Debug, Default)]
pub struct Toasts {
    /// The parts drawn last frame, as `(area, what a click does)`.
    hits: Vec<(Rect, Hit)>,
    /// Whether a shown notification is busy or will run out.
    animating: bool,
}

impl Toasts {
    /// Creates the layer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws `toast` with its top left corner at `(x, y)` and records where its parts are.
    fn draw(&mut self, index: usize, toast: &Toast, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let inner_width = usize::from(area.width.saturating_sub(4));
        buf.set_style(area, theme.popup);
        let border = match toast.level {
            ToastLevel::Info => theme.popup_border,
            ToastLevel::Warning => theme.warning,
            ToastLevel::Error => theme.error,
        };
        let right = area.right() - 1;
        let bottom = area.bottom() - 1;
        for x in area.x..=right {
            buf.set_string(x, area.y, "\u{2500}", border);
            buf.set_string(x, bottom, "\u{2500}", border);
        }
        for y in area.y..=bottom {
            buf.set_string(area.x, y, "\u{2502}", border);
            buf.set_string(right, y, "\u{2502}", border);
        }
        buf.set_string(area.x, area.y, "\u{256d}", border);
        buf.set_string(right, area.y, "\u{256e}", border);
        buf.set_string(area.x, bottom, "\u{2570}", border);
        buf.set_string(right, bottom, "\u{256f}", border);
        let x = area.x + 2;
        let mut y = area.y + 1;
        let mark = match toast.level {
            ToastLevel::Info => ("\u{2022} ", theme.popup_title),
            ToastLevel::Warning => ("\u{26a0} ", theme.warning),
            ToastLevel::Error => ("\u{2716} ", theme.error),
        };
        let after = buf.set_stringn(x, y, mark.0, inner_width, mark.1).0;
        let room = inner_width.saturating_sub(4);
        buf.set_stringn(after, y, &toast.title, room, theme.popup_title);
        let close = Rect::new(right - 2, y, 1, 1);
        buf.set_string(close.x, close.y, "\u{00d7}", theme.popup_dim);
        self.hits.push((close, Hit::Close(index)));
        for line in toast.lines(inner_width) {
            y += 1;
            buf.set_stringn(x, y, line, inner_width, theme.popup);
        }
        if let Some(progress) = toast.progress {
            y += 1;
            let elapsed = Instant::now().saturating_duration_since(toast.shown_at);
            buf.set_stringn(
                x,
                y,
                bar(progress, inner_width, elapsed),
                inner_width,
                theme.popup_title,
            );
        }
        if !toast.buttons.is_empty() {
            y += 1;
            let mut bx = x;
            for (number, button) in toast.buttons.iter().enumerate() {
                let label = format!("[ {} ]", button.title);
                let width = u16::try_from(label.width()).unwrap_or(0);
                if bx + width > right {
                    break;
                }
                buf.set_string(bx, y, &label, theme.popup_selected);
                self.hits
                    .push((Rect::new(bx, y, width, 1), Hit::Button(index, number)));
                bx += width + 1;
            }
        }
        self.hits.push((area, Hit::Nothing));
    }
}

impl Layer for Toasts {
    fn area(&self, layout: &Layout, ui: &Ui) -> Rect {
        if ui.toasts.is_empty() {
            Rect::default()
        } else {
            layout.screen
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let now = Instant::now();
        cx.ui.toasts.retain(|toast| !toast.expired(now));
        let layout = cx.ui.layout(area);
        let editor = if layout.split.is_empty() {
            layout.editor
        } else {
            layout.editor.union(layout.split)
        };
        self.hits.clear();
        self.animating = !cx.ui.toasts.is_empty();
        let width = WIDTH.min(editor.width.saturating_sub(2));
        if width < 12 {
            return;
        }
        let x = editor.right() - width - 1;
        let mut y = editor.y + 1;
        for (index, toast) in cx.ui.toasts.iter().enumerate().rev().take(MAX_SHOWN) {
            let height = toast.height(usize::from(width.saturating_sub(4)));
            if y + height > editor.bottom() {
                break;
            }
            let drawn = Rect::new(x, y, width, height);
            self.draw(index, toast, drawn, buf, cx.theme);
            y += height;
        }
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        let point = Position::new(event.column, event.row);
        let Some((_, hit)) = self.hits.iter().find(|(area, _)| area.contains(point)) else {
            return EventResult::Ignored;
        };
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return EventResult::Consumed;
        }
        match hit.clone() {
            Hit::Close(index) => {
                if index < cx.ui.toasts.len() {
                    cx.ui.toasts.remove(index);
                }
            }
            Hit::Button(index, button) => {
                let Some(toast) = cx.ui.toasts.get(index).cloned() else {
                    return EventResult::Consumed;
                };
                match toast
                    .buttons
                    .get(button)
                    .and_then(|button| button.command.clone())
                {
                    Some(name) => {
                        if let Ok(command) = name.parse() {
                            cx.ui.request(command);
                        }
                    }
                    None => cx.ui.toast_clicks.push(ToastClick {
                        plugin: toast.plugin,
                        id: toast.id,
                        button,
                    }),
                }
                // a button is an answer, so the notification has done its job
                if toast.progress.is_none() {
                    cx.ui.toasts.remove(index);
                }
            }
            Hit::Nothing => {}
        }
        EventResult::Consumed
    }

    fn is_animating(&self) -> bool {
        self.animating
    }
}

#[cfg(test)]
/// Tests for notifications.
mod tests {
    use std::time::{Duration, Instant};

    use super::{Toast, ToastLevel, ToastProgress, bar};

    /// Progress bars fill with the percentage and a busy bar moves.
    #[test]
    fn draws_bars() {
        assert_eq!(
            bar(ToastProgress::Percent(50), 14, Duration::ZERO),
            "\u{2588}\u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}  50%"
        );
        let early = bar(ToastProgress::Busy, 20, Duration::ZERO);
        let later = bar(ToastProgress::Busy, 20, Duration::from_millis(500));
        assert_ne!(early, later);
        assert_eq!(early.chars().count(), 20);
    }

    /// Long text wraps and notifications run out after their time.
    #[test]
    fn wraps_and_expires() {
        let toast = Toast {
            plugin: "p".into(),
            id: "t".into(),
            title: "Indexing".into(),
            text: "one two three four five".into(),
            level: ToastLevel::Info,
            progress: None,
            buttons: Vec::new(),
            shown_at: Instant::now(),
            timeout: Some(Duration::ZERO),
        };
        assert_eq!(toast.lines(9), ["one two", "three", "four five"]);
        assert!(toast.expired(Instant::now()));
    }
}
