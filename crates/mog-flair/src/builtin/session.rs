//! A session timer that eventually tells you to go outside.

use std::time::{Duration, Instant};

use mog_tui::{Segment, Side, UiEvent};
use ratatui::style::{Color, Modifier, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// How long a session goes before the timer suggests touching grass.
const GRASS_TIME: Duration = Duration::from_secs(90 * 60);

/// Formats a duration like `1h05m` or `42m`.
fn short(time: Duration) -> String {
    let minutes = time.as_secs() / 60;
    if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// Shows how long mog has been open and how many lines were written.
#[derive(Debug)]
pub struct Session {
    /// When the session started.
    started: Instant,
    /// Line breaks typed this session.
    lines: usize,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            lines: 0,
        }
    }
}

impl Session {
    /// Starts the session clock.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for Session {
    fn id(&self) -> &str {
        "session"
    }

    fn description(&self) -> &str {
        "How long you have been coding and how many lines, plus a nudge to touch grass."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Right)
    }

    fn observe(&mut self, event: &UiEvent) {
        if *event == UiEvent::Typed('\n') {
            self.lines += 1;
        }
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let elapsed = self.started.elapsed();
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let p = cx.theme.palette;
        let mut parts = vec![(
            format!("\u{23f1} {} +{}", short(elapsed), self.lines),
            Style::new().fg(p.dim).bg(bg),
        )];
        if elapsed >= GRASS_TIME {
            parts.push((
                " \u{2618} touch grass".into(),
                Style::new().fg(p.green).bg(bg).add_modifier(Modifier::BOLD),
            ));
        }
        Some(Segment {
            parts,
            side: Side::Right,
        })
    }
}

#[cfg(test)]
/// Tests for the session timer.
mod tests {
    use std::time::Duration;

    use super::short;

    /// Durations format in minutes and hours.
    #[test]
    fn formats_durations() {
        assert_eq!(short(Duration::from_secs(59)), "0m");
        assert_eq!(short(Duration::from_secs(42 * 60)), "42m");
        assert_eq!(short(Duration::from_secs(65 * 60)), "1h05m");
    }
}
