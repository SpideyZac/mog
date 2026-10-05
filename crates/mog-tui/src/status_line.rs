//! The bar at the bottom of the screen.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Severity, view};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    ui::{CopilotState, Layout, Segment, Side, Ui},
};

/// The number of rows the status line takes.
pub const STATUS_HEIGHT: u16 = 1;

/// Shows the file name, status messages and the cursor position.
#[derive(Debug, Default)]
pub struct StatusLine {
    /// Where the clickable parts were drawn, as `(start, end, command)`.
    hits: Vec<(u16, u16, String)>,
}

impl StatusLine {
    /// Creates the status line.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Layer for StatusLine {
    fn area(&self, layout: &Layout, _ui: &Ui) -> Rect {
        layout.status
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context<'_>) {
        let theme = cx.theme;
        let document = cx.editor.document();
        let text = document.text();
        let head = document.selection().head;
        let line = text.char_to_line(head) + 1;
        let col = view::visual_col(text, head, cx.editor.options().tab_width) + 1;
        let modified = if document.is_modified() { " [+]" } else { "" };

        buf.set_style(area, theme.status);
        let mut left = vec![
            Span::styled(" mog ", theme.status_badge),
            Span::raw(format!(" {}{modified} ", document.name())),
        ];
        let mut right = Vec::new();
        let count = |severity| {
            document
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.severity == severity)
                .count()
        };
        for (severity, label, style) in [
            (Severity::Error, "E", theme.error),
            (Severity::Warning, "W", theme.warning),
        ] {
            let n = count(severity);
            if n > 0 {
                right.push(Span::styled(format!("{label}{n} "), style));
            }
        }
        if let Some(state) = cx.ui.copilot {
            let (text, style) = match state {
                CopilotState::Starting => ("copilot \u{2026} ", theme.status_message),
                CopilotState::Ready => ("copilot \u{2713} ", theme.status_message),
                CopilotState::SignedOut => ("copilot: sign in ", theme.warning),
                CopilotState::Problem => ("copilot \u{2716} ", theme.error),
            };
            right.push(Span::styled(text, style));
        }
        if let Some(branch) = &cx.ui.branch {
            right.push(Span::styled(
                format!("\u{2387} {branch} "),
                theme.status_message,
            ));
        }
        right.push(Span::raw(format!("Ln {line}, Col {col} ")));

        // flair only gets the room left after the essentials and the status message
        let width = |spans: &[Span<'_>]| spans.iter().map(Span::width).sum::<usize>();
        let message = cx.editor.status().unwrap_or_default();
        let essentials = width(&left) + width(&right);
        let total = usize::from(area.width);
        let mut room = total
            .saturating_sub(essentials)
            .saturating_sub(message.width().min(total / 2) + 1);
        let mut flair_left = Vec::new();
        let mut flair_right = Vec::new();
        // plugins asked for their text on purpose, so it goes ahead of flair
        let plugins: Vec<Segment> = cx
            .ui
            .plugin_segments
            .iter()
            .map(|segment| {
                let style = theme.color_style(segment.color.as_deref(), theme.status);
                Segment::new(segment.text.clone(), style, Side::Right)
            })
            .collect();
        let clickable: Vec<(&str, &str)> = cx
            .ui
            .plugin_segments
            .iter()
            .filter_map(|segment| Some((segment.text.as_str(), segment.command.as_deref()?)))
            .collect();
        for segment in plugins.iter().chain(&cx.ui.segments) {
            let spans: Vec<Span<'_>> = segment
                .parts
                .iter()
                .map(|(text, style)| Span::styled(text.clone(), *style))
                .chain([Span::raw(" ")])
                .collect();
            let needed = width(&spans);
            if needed > room {
                continue;
            }
            room -= needed;
            match segment.side {
                Side::Left => flair_left.extend(spans),
                Side::Right => flair_right.extend(spans),
            }
        }
        left.extend(flair_left);
        flair_right.extend(right);
        let right = flair_right;
        let message_room = total.saturating_sub(width(&left) + width(&right) + 1);
        let message: String = message.chars().take(message_room).collect();
        left.push(Span::styled(message, theme.status_message));
        self.hits.clear();
        // the badge opens the palette, problem counts open the problem list
        self.hits
            .push((area.x, area.x + 5, "command_palette".into()));
        let right_width = u16::try_from(width(&right)).unwrap_or(0);
        let mut x = area.right().saturating_sub(right_width);
        for span in &right {
            let span_width = u16::try_from(span.width()).unwrap_or(0);
            let text = span.content.as_ref();
            if text.starts_with('E') || text.starts_with('W') {
                if text[1..].trim().parse::<usize>().is_ok() {
                    self.hits.push((x, x + span_width, "problems.list".into()));
                }
            } else if text.starts_with("Ln ") {
                self.hits.push((x, x + span_width, "goto.prompt".into()));
            } else if text.starts_with("copilot") {
                let command = if cx.ui.copilot == Some(CopilotState::SignedOut) {
                    "copilot.sign_in"
                } else {
                    "copilot.status"
                };
                self.hits.push((x, x + span_width, command.into()));
            } else if let Some((_, command)) =
                clickable.iter().find(|(segment, _)| *segment == text)
            {
                self.hits.push((x, x + span_width, (*command).to_owned()));
            }
            x += span_width;
        }
        Line::from(left).render(area, buf);
        Line::from(right).right_aligned().render(area, buf);
    }

    fn handle_mouse(
        &mut self,
        event: MouseEvent,
        _area: Rect,
        cx: &mut Context<'_>,
    ) -> EventResult {
        if let MouseEventKind::Down(MouseButton::Left) = event.kind
            && let Some((_, _, name)) = self
                .hits
                .iter()
                .find(|(start, end, _)| (*start..*end).contains(&event.column))
            && let Ok(command) = name.parse()
        {
            cx.ui.request(command);
        }
        EventResult::Consumed
    }
}
