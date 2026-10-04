//! Little sparks that fly out of the cursor while typing.

use std::{mem, time::Duration};

use mog_tui::UiEvent;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Style},
};

use crate::{
    flair::{Flair, FlairContext, Placement},
    rng::Rng,
};

/// The most sparks alive at once.
const MAX_SPARKS: usize = 240;

/// Downward pull in cells per second squared.
const GRAVITY: f32 = 30.0;

/// What sparks look like.
const SYMBOLS: [&str; 6] = ["*", "\u{b7}", "\u{2726}", "+", "\u{2022}", "\u{2727}"];

/// One spark.
#[derive(Debug, Clone, Copy)]
struct Spark {
    /// Column on screen.
    x: f32,
    /// Row on screen.
    y: f32,
    /// Sideways speed in cells per second.
    vx: f32,
    /// Vertical speed in cells per second, negative is up.
    vy: f32,
    /// Seconds left to live.
    life: f32,
    /// Which symbol to draw.
    symbol: usize,
    /// Which palette color to use.
    color: usize,
}

/// Throws sparks from the cursor on every keystroke. The faster you type, the more sparks.
#[derive(Debug, Default)]
pub struct Sparks {
    /// The live sparks.
    sparks: Vec<Spark>,
    /// Keystrokes that have not spawned sparks yet.
    pending: u32,
    /// Quick keystrokes in a row, which makes bigger bursts.
    streak: u32,
    /// Time since the last keystroke.
    since_typed: Duration,
    /// Random numbers for the sparks.
    rng: Rng,
}

impl Sparks {
    /// Creates the flair.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns a burst at `at`.
    fn burst(&mut self, at: Position) {
        let count = 2 + (self.streak / 8).min(10) as usize;
        for _ in 0..count {
            if self.sparks.len() >= MAX_SPARKS {
                self.sparks.remove(0);
            }
            let spark = Spark {
                x: f32::from(at.x),
                y: f32::from(at.y),
                vx: self.rng.range(-14.0, 14.0),
                vy: self.rng.range(-12.0, -2.0),
                life: self.rng.range(0.35, 0.8),
                symbol: self.rng.below(SYMBOLS.len()),
                color: self.rng.below(5),
            };
            self.sparks.push(spark);
        }
    }
}

impl Flair for Sparks {
    fn id(&self) -> &str {
        "sparks"
    }

    fn description(&self) -> &str {
        "Power mode. Sparks fly out of the cursor while you type."
    }

    fn placement(&self) -> Placement {
        Placement::Screen
    }

    fn observe(&mut self, event: &UiEvent) {
        if let UiEvent::Typed(_) = event {
            self.pending += 1;
            self.streak = if self.since_typed < Duration::from_millis(800) {
                self.streak + 1
            } else {
                0
            };
            self.since_typed = Duration::ZERO;
        }
    }

    fn tick(&mut self, dt: Duration) {
        let secs = dt.as_secs_f32();
        self.since_typed += dt;
        for spark in &mut self.sparks {
            spark.vy += GRAVITY * secs;
            spark.x += spark.vx * secs;
            spark.y += spark.vy * secs;
            spark.life -= secs;
        }
        self.sparks.retain(|spark| spark.life > 0.0);
    }

    fn is_animating(&self) -> bool {
        !self.sparks.is_empty() || self.pending > 0
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        if let Some(cursor) = cx.ui.cursor_screen {
            for _ in 0..mem::take(&mut self.pending) {
                self.burst(cursor);
            }
        } else {
            self.pending = 0;
        }
        let p = cx.theme.palette;
        let colors: [Color; 5] = [p.accent, p.accent2, p.yellow, p.pink, p.cyan];
        let editor = cx.layout.editor;
        for spark in &self.sparks {
            if spark.x < 0.0 || spark.y < 0.0 {
                continue;
            }
            // the checks above make the casts safe, they only drop the fraction
            let (x, y) = (spark.x as u16, spark.y as u16);
            let at = Position::new(x, y);
            if !area.contains(at) || !editor.contains(at) {
                continue;
            }
            let style = Style::new().fg(colors[spark.color]);
            buf.set_string(x, y, SYMBOLS[spark.symbol], style);
        }
    }
}
