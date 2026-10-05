//! Matrix rain after a while without input.

use std::time::{Duration, Instant};

use mog_tui::{UiEvent, theme::mix};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    flair::{Flair, FlairContext, Placement},
    rng::Rng,
};

/// How long without input before the rain starts.
pub const IDLE_TIME: Duration = Duration::from_secs(180);

/// What the rain is made of.
const GLYPHS: &[char] = &[
    '\u{ff71}', '\u{ff72}', '\u{ff73}', '\u{ff74}', '\u{ff75}', '\u{ff76}', '\u{ff77}', '\u{ff78}',
    '\u{ff79}', '\u{ff7a}', '\u{ff7b}', '\u{ff7c}', '\u{ff7d}', '\u{ff7e}', '0', '1', '2', '3',
    '7', '9', 'm', 'o', 'g', ':', '=', '*', '+',
];

/// The message in the middle of the rain.
const MESSAGE: &str = " mog is dreaming. press any key ";

/// One falling stream.
#[derive(Debug, Clone, Copy)]
struct Drop {
    /// The row of the head.
    head: f32,
    /// Rows per second.
    speed: f32,
    /// How many rows the tail covers.
    length: u16,
    /// Seed for the glyphs in this column.
    seed: u64,
}

/// Starts matrix rain after three minutes without input and stops on the next key.
#[derive(Debug)]
pub struct Screensaver {
    /// When input last happened.
    active_at: Instant,
    /// One stream per screen column, empty while not raining.
    drops: Vec<Drop>,
    /// Random numbers for the rain.
    rng: Rng,
    /// How long the rain has been falling.
    falling: Duration,
}

impl Default for Screensaver {
    fn default() -> Self {
        Self {
            active_at: Instant::now(),
            drops: Vec::new(),
            rng: Rng::default(),
            falling: Duration::ZERO,
        }
    }
}

impl Screensaver {
    /// Creates the screensaver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a new stream for a screen `height` rows tall.
    fn new_drop(&mut self, height: u16) -> Drop {
        Drop {
            head: -self.rng.range(0.0, f32::from(height)),
            speed: self.rng.range(6.0, 22.0),
            length: u16::try_from(4 + self.rng.below(usize::from(height / 2).max(1))).unwrap_or(4),
            seed: self.rng.next_u64(),
        }
    }
}

impl Flair for Screensaver {
    fn id(&self) -> &str {
        "screensaver"
    }

    fn moves(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        "Matrix rain after three minutes of doing nothing."
    }

    fn placement(&self) -> Placement {
        Placement::Screen
    }

    fn observe(&mut self, event: &UiEvent) {
        if matches!(event, UiEvent::Activity | UiEvent::Typed(_)) {
            self.active_at = Instant::now();
            self.drops.clear();
            self.falling = Duration::ZERO;
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.drops.is_empty() {
            return;
        }
        self.falling += dt;
        let secs = dt.as_secs_f32();
        for drop in &mut self.drops {
            drop.head += drop.speed * secs;
        }
    }

    fn is_animating(&self) -> bool {
        !self.drops.is_empty()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        if self.active_at.elapsed() < IDLE_TIME {
            return;
        }
        if self.drops.len() != usize::from(area.width) {
            self.drops = (0..area.width)
                .map(|_| self.new_drop(area.height))
                .collect();
        }
        let p = cx.theme.palette;
        let bg = Style::new().bg(p.bg).fg(p.bg);
        buf.set_style(area, bg);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_symbol(" ");
            }
        }
        let height = area.height;
        for column in 0..area.width {
            let index = usize::from(column);
            let drop = self.drops[index];
            if drop.head - f32::from(drop.length) > f32::from(height) {
                self.drops[index] = self.new_drop(height);
                continue;
            }
            let x = area.x + column;
            for back in 0..drop.length {
                let row = drop.head - f32::from(back);
                if row < 0.0 || row >= f32::from(height) {
                    continue;
                }
                // the checks above make the cast exact apart from the fraction
                let y = area.y + row as u16;
                let glyph_seed = drop.seed
                    ^ u64::from(y).wrapping_mul(0x9e37_79b9)
                    ^ (self.falling.as_millis() as u64 / 180);
                let glyph = GLYPHS[usize::try_from(glyph_seed % GLYPHS.len() as u64).unwrap_or(0)];
                let fade = f32::from(back) / f32::from(drop.length);
                let color = if back == 0 {
                    p.fg
                } else {
                    mix(p.accent2, p.bg, fade)
                };
                let style = Style::new().fg(color).bg(p.bg);
                let style = if back == 0 {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                };
                buf.set_string(x, y, glyph.to_string(), style);
            }
        }
        let width = u16::try_from(MESSAGE.width()).unwrap_or(0);
        if area.width > width {
            let x = area.x + (area.width - width) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(
                x,
                y,
                MESSAGE,
                Style::new()
                    .fg(p.bg)
                    .bg(p.accent)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}
