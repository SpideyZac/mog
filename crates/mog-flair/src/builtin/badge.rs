//! A small rainbow `mog` badge in the corner.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::flair::{Corner, Flair, FlairContext, Placement};

/// The text the badge shows.
const TEXT: &str = " ~mog~ ";

/// How long one full trip around the color wheel takes.
const CYCLE: Duration = Duration::from_secs(3);

/// The hue gap between neighbouring letters, in degrees.
const HUE_STEP: f32 = 24.0;

/// Converts a hue in degrees to a fully saturated color.
fn hue_to_color(hue: f32) -> Color {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u8 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    // the clamp makes the casts lossless
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::Rgb(channel(r), channel(g), channel(b))
}

/// A badge whose letters slowly cycle through the rainbow.
#[derive(Debug, Default)]
pub struct Badge {
    /// How far through the current color cycle the badge is.
    elapsed: Duration,
}

impl Badge {
    /// Creates the badge.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for Badge {
    fn id(&self) -> &str {
        "badge"
    }

    fn placement(&self) -> Placement {
        Placement::Corner {
            corner: Corner::TopRight,
            width: u16::try_from(TEXT.len()).unwrap_or(u16::MAX),
            height: 1,
        }
    }

    fn tick(&mut self, dt: Duration) {
        self.elapsed += dt;
        while self.elapsed >= CYCLE {
            self.elapsed -= CYCLE;
        }
    }

    fn is_animating(&self) -> bool {
        true
    }

    fn render(&self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        let base = self.elapsed.as_secs_f32() / CYCLE.as_secs_f32() * 360.0;
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        for (i, ch) in TEXT.chars().enumerate().take(usize::from(area.width)) {
            let hue = base + HUE_STEP * i as f32;
            let style = Style::new().fg(hue_to_color(hue)).bg(bg);
            let x = area.x + u16::try_from(i).unwrap_or(u16::MAX);
            buf.set_string(x, area.y, ch.to_string(), style);
        }
    }
}

#[cfg(test)]
/// Tests for [`Badge`].
mod tests {
    use std::time::Duration;

    use ratatui::style::Color;

    use super::{Badge, CYCLE, hue_to_color};
    use crate::flair::Flair;

    /// The primary hues map to pure colors.
    #[test]
    fn primary_hues() {
        assert_eq!(hue_to_color(0.0), Color::Rgb(255, 0, 0));
        assert_eq!(hue_to_color(120.0), Color::Rgb(0, 255, 0));
        assert_eq!(hue_to_color(240.0), Color::Rgb(0, 0, 255));
        assert_eq!(hue_to_color(360.0), Color::Rgb(255, 0, 0));
    }

    /// Ticking wraps around after a full cycle.
    #[test]
    fn tick_wraps() {
        let mut badge = Badge::new();
        badge.tick(CYCLE - Duration::from_millis(100));
        badge.tick(Duration::from_millis(300));
        assert_eq!(badge.elapsed, Duration::from_millis(200));
    }
}
