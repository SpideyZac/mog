//! A typing combo counter that shows up when you type fast.

use std::time::Duration;

use mog_tui::UiEvent;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Clear, Widget},
};

use crate::{
    builtin::badge::hue_to_color,
    flair::{Corner, Flair, FlairContext, Placement},
};

/// The gap between keystrokes that keeps a combo alive.
const WINDOW: Duration = Duration::from_millis(1500);

/// How long the counter stays up after the last keystroke.
const LINGER: Duration = Duration::from_millis(2500);

/// The smallest combo worth showing.
const MIN_SHOWN: u32 = 5;

/// How long a milestone shakes the counter.
const SHAKE_TIME: Duration = Duration::from_millis(400);

/// Combo sizes that get a title, smallest first.
const TIERS: &[(u32, &str)] = &[
    (5, "warming up"),
    (10, "nice"),
    (25, "spicy"),
    (50, "on fire"),
    (100, "unstoppable"),
    (200, "MOG MODE"),
    (500, "transcendent"),
];

/// Returns the title for a combo of `count`.
fn tier(count: u32) -> &'static str {
    TIERS
        .iter()
        .rev()
        .find(|(min, _)| count >= *min)
        .map_or("", |(_, name)| name)
}

/// Counts keystrokes in quick succession and cheers you on.
#[derive(Debug, Default)]
pub struct Combo {
    /// Time since the flair started.
    clock: Duration,
    /// The current combo.
    count: u32,
    /// The best combo this session.
    best: u32,
    /// When the last keystroke happened.
    typed_at: Option<Duration>,
    /// When the last milestone was hit.
    milestone_at: Option<Duration>,
}

impl Combo {
    /// Creates the counter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns how long ago the last keystroke was.
    fn since_typed(&self) -> Option<Duration> {
        self.typed_at.map(|at| self.clock.saturating_sub(at))
    }

    /// Returns whether the counter should be drawn.
    fn visible(&self) -> bool {
        self.count >= MIN_SHOWN && self.since_typed().is_some_and(|since| since < LINGER)
    }
}

impl Flair for Combo {
    fn id(&self) -> &str {
        "combo"
    }

    fn description(&self) -> &str {
        "A combo counter for fast typing, like a fighting game."
    }

    fn placement(&self) -> Placement {
        Placement::Corner {
            corner: Corner::TopRight,
            width: 20,
            height: 3,
        }
    }

    fn observe(&mut self, event: &UiEvent) {
        if let UiEvent::Typed(_) = event {
            let alive = self.since_typed().is_some_and(|since| since < WINDOW);
            self.count = if alive { self.count + 1 } else { 1 };
            self.best = self.best.max(self.count);
            self.typed_at = Some(self.clock);
            if TIERS.iter().skip(1).any(|(min, _)| *min == self.count) {
                self.milestone_at = Some(self.clock);
            }
        }
    }

    fn tick(&mut self, dt: Duration) {
        self.clock += dt;
    }

    fn is_animating(&self) -> bool {
        self.visible()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        if !self.visible() || area.width < 12 || area.height < 3 {
            return;
        }
        let p = cx.theme.palette;
        let shaking = self
            .milestone_at
            .is_some_and(|at| self.clock.saturating_sub(at) < SHAKE_TIME);
        // wiggle left and right every 50ms while shaking
        let offset = if shaking && (self.clock.as_millis() / 50).is_multiple_of(2) {
            1
        } else {
            0
        };
        let area = Rect {
            x: area.x.saturating_sub(offset + 1),
            ..area
        };
        Clear.render(area, buf);
        buf.set_style(area, cx.theme.background);
        let since = self.since_typed().unwrap_or(LINGER);
        let fading = since > WINDOW;
        let main = if fading { p.dim } else { p.accent };
        let title = format!(" COMBO \u{d7}{}", self.count);
        buf.set_string(
            area.x,
            area.y,
            &title,
            Style::new().fg(main).add_modifier(Modifier::BOLD),
        );
        // the bar shows how long until the combo drops
        let left = WINDOW.saturating_sub(since).as_millis();
        let cells = usize::from(area.width - 2);
        let filled = usize::try_from(left * cells as u128 / WINDOW.as_millis()).unwrap_or(0);
        let bar: String = (0..cells)
            .map(|i| if i < filled { '\u{2501}' } else { '\u{2504}' })
            .collect();
        buf.set_string(area.x + 1, area.y + 1, bar, Style::new().fg(p.accent2));
        let name = tier(self.count);
        let base_hue = self.clock.as_secs_f32() * 240.0;
        for (i, ch) in name.chars().enumerate() {
            let color = if fading {
                p.dim
            } else {
                hue_to_color(base_hue + i as f32 * 30.0)
            };
            let x = area.x + 1 + u16::try_from(i).unwrap_or(u16::MAX);
            if x < area.right() {
                buf.set_string(
                    x,
                    area.y + 2,
                    ch.to_string(),
                    Style::new()
                        .fg(color)
                        .add_modifier(Modifier::BOLD | Modifier::ITALIC),
                );
            }
        }
    }
}

#[cfg(test)]
/// Tests for the combo counter.
mod tests {
    use std::time::Duration;

    use mog_tui::UiEvent;

    use super::{Combo, WINDOW, tier};
    use crate::flair::Flair;

    /// Quick keystrokes build a combo and a pause resets it.
    #[test]
    fn counts_and_resets() {
        let mut combo = Combo::new();
        for _ in 0..12 {
            combo.observe(&UiEvent::Typed('x'));
            combo.tick(Duration::from_millis(100));
        }
        assert_eq!(combo.count, 12);
        assert!(combo.visible());
        combo.tick(WINDOW);
        combo.observe(&UiEvent::Typed('x'));
        assert_eq!(combo.count, 1);
        assert_eq!(combo.best, 12);
    }

    /// Titles go up with the combo.
    #[test]
    fn tiers_grow() {
        assert_eq!(tier(3), "");
        assert_eq!(tier(30), "spicy");
        assert_eq!(tier(999), "transcendent");
    }
}
