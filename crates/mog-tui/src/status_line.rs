//! The bar at the bottom of the screen.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use mog_core::{Severity, view};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    compositor::{Context, EventResult, Layer},
    ui::{CopilotState, Layout, Segment, SegmentClick, Side, Ui},
};

/// The number of rows the status line takes.
pub const STATUS_HEIGHT: u16 = 1;

/// What clicking a part of the status line does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// Runs a command.
    Command(String),
    /// Tells the plugin whose segment it is, by the segment's name.
    Segment(String),
}

/// Shows the file name, status messages and the cursor position.
#[derive(Debug, Default)]
pub struct StatusLine {
    /// Where the clickable parts were drawn, as `(start, end, what)`.
    hits: Vec<(u16, u16, Hit)>,
}

/// Returns the name of a mouse button for plugins.
fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    }
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
        let mut left_hits: Vec<Option<Hit>> =
            vec![Some(Hit::Command("command_palette".into())), None];
        let mut flair_left_hits = Vec::new();
        let mut flair_right_hits = Vec::new();
        // plugins asked for their text on purpose, so it goes ahead of flair
        let plugins = cx.ui.plugin_segments.iter().map(|segment| {
            let mut style = theme.color_style(segment.color.as_deref(), theme.status);
            if let Some(bg) = segment.bg.as_deref().and_then(|name| theme.color(name)) {
                style = style.bg(bg);
            }
            if segment.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            let hit = match &segment.command {
                Some(command) => Hit::Command(command.clone()),
                None => Hit::Segment(segment.plugin.clone()),
            };
            (
                Segment::new(segment.text.clone(), style, segment.side),
                Some(hit),
            )
        });
        let others = cx.ui.segments.iter().map(|segment| (segment.clone(), None));
        for (segment, hit) in plugins.chain(others) {
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
            let count = spans.len();
            let hits: Vec<Option<Hit>> = (0..count)
                .map(|at| if at + 1 < count { hit.clone() } else { None })
                .collect();
            match segment.side {
                Side::Left => {
                    flair_left.extend(spans);
                    flair_left_hits.extend(hits);
                }
                Side::Right => {
                    flair_right.extend(spans);
                    flair_right_hits.extend(hits);
                }
            }
        }
        left.extend(flair_left);
        left_hits.extend(flair_left_hits);
        flair_right_hits.extend(right.iter().map(|_| None));
        flair_right.extend(right);
        let right = flair_right;
        let message_room = total.saturating_sub(width(&left) + width(&right) + 1);
        let message: String = message.chars().take(message_room).collect();
        left.push(Span::styled(message, theme.status_message));
        self.hits.clear();
        let mut x = area.x;
        for (span, hit) in left.iter().zip(&left_hits) {
            let span_width = u16::try_from(span.width()).unwrap_or(0);
            if let Some(hit) = hit {
                self.hits.push((x, x + span_width, hit.clone()));
            }
            x += span_width;
        }
        // problem counts open the problem list
        let right_width = u16::try_from(width(&right)).unwrap_or(0);
        let mut x = area.right().saturating_sub(right_width);
        for (span, hit) in right.iter().zip(&flair_right_hits) {
            let span_width = u16::try_from(span.width()).unwrap_or(0);
            let text = span.content.as_ref();
            let command = |name: &str| Some(Hit::Command(name.to_owned()));
            let hit = match hit {
                Some(hit) => Some(hit.clone()),
                None if (text.starts_with('E') || text.starts_with('W'))
                    && text[1..].trim().parse::<usize>().is_ok() =>
                {
                    command("problems.list")
                }
                None if text.starts_with("Ln ") => command("goto.prompt"),
                None if text.starts_with("copilot") => {
                    if cx.ui.copilot == Some(CopilotState::SignedOut) {
                        command("copilot.sign_in")
                    } else {
                        command("copilot.status")
                    }
                }
                None => None,
            };
            if let Some(hit) = hit {
                self.hits.push((x, x + span_width, hit));
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
        let MouseEventKind::Down(button) = event.kind else {
            return EventResult::Consumed;
        };
        let hit = self
            .hits
            .iter()
            .find(|(start, end, _)| (*start..*end).contains(&event.column))
            .map(|(_, _, hit)| hit.clone());
        match hit {
            Some(Hit::Command(name)) if button == MouseButton::Left => {
                if let Ok(command) = name.parse() {
                    cx.ui.request(command);
                }
            }
            Some(Hit::Segment(segment)) => cx.ui.segment_clicks.push(SegmentClick {
                segment,
                button: button_name(button),
            }),
            _ => {}
        }
        EventResult::Consumed
    }
}
