//! A stock ticker for `$MOG` that pumps when you save and dumps when the build breaks.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use mog_tui::{Segment, Side, UiEvent};
use ratatui::style::{Modifier, Style};

use crate::{
    flair::{Flair, FlairContext, Placement},
    rng::Rng,
    spark::sparkline,
};

/// What `$MOG` opens at.
const OPEN: f32 = 420.69;

/// The lowest it goes, since mog never truly hits zero.
const FLOOR: f32 = 1.0;

/// How often the price wiggles on its own.
const STEP: Duration = Duration::from_secs(2);

/// How many prices the sparkline shows.
const HISTORY: usize = 8;

/// The biggest random wiggle per step, as a fraction of the price.
const WIGGLE: f32 = 0.006;

/// Trades `$MOG` on your coding performance.
#[derive(Debug)]
pub struct Stonks {
    /// The current price.
    price: f32,
    /// Recent prices, oldest first.
    history: VecDeque<f32>,
    /// When the price last wiggled on its own.
    stepped: Instant,
    /// The error count at the last diagnostics event.
    errors: usize,
    /// Where wiggles come from.
    rng: Rng,
}

impl Default for Stonks {
    fn default() -> Self {
        Self {
            price: OPEN,
            history: VecDeque::from([OPEN]),
            stepped: Instant::now(),
            errors: 0,
            rng: Rng::default(),
        }
    }
}

impl Stonks {
    /// Opens the market.
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the price by `factor` and records it.
    fn trade(&mut self, factor: f32) {
        self.price = (self.price * factor).max(FLOOR);
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.price);
    }

    /// Returns the change since the open, in percent.
    fn change(&self) -> f32 {
        (self.price / OPEN - 1.0) * 100.0
    }
}

impl Flair for Stonks {
    fn id(&self) -> &str {
        "stonks"
    }

    fn description(&self) -> &str {
        "A $MOG stock ticker in the tab bar. Saving pumps it, errors dump it."
    }

    fn placement(&self) -> Placement {
        Placement::TabBar
    }

    fn observe(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Saved => self.trade(1.03),
            UiEvent::Typed(_) => self.price *= 1.0004,
            UiEvent::Diagnostics { errors, .. } => {
                let errors = *errors;
                if errors > self.errors {
                    let fresh = i32::try_from((errors - self.errors).min(5)).unwrap_or(5);
                    self.trade(0.92_f32.powi(fresh));
                } else if errors == 0 && self.errors > 0 {
                    self.trade(1.08);
                }
                self.errors = errors;
            }
            _ => {}
        }
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        // catch up on the wiggles missed while nothing was drawn, but not forever
        let mut steps = 0;
        while self.stepped.elapsed() >= STEP && steps < HISTORY {
            self.stepped += STEP;
            steps += 1;
            let wiggle = self.rng.range(-WIGGLE, WIGGLE);
            self.trade(1.0 + wiggle);
        }
        if self.stepped.elapsed() >= STEP {
            self.stepped = Instant::now();
        }
        let p = cx.theme.palette;
        let bg = cx.theme.tab.bg.unwrap_or(p.panel);
        let change = self.change();
        let (arrow, color) = if change >= 0.0 {
            ("\u{25b2}", p.green)
        } else {
            ("\u{25bc}", p.red)
        };
        let history: Vec<f32> = self.history.iter().copied().collect();
        let lo = history.iter().copied().fold(f32::MAX, f32::min);
        let hi = history.iter().copied().fold(f32::MIN, f32::max);
        let style = Style::new().bg(bg);
        Some(Segment {
            parts: vec![
                (
                    "$MOG ".into(),
                    style.fg(p.accent).add_modifier(Modifier::BOLD),
                ),
                (format!("{:.2} ", self.price), style.fg(p.fg)),
                (format!("{arrow}{:.1}% ", change.abs()), style.fg(color)),
                (sparkline(&history, lo, hi), style.fg(color)),
            ],
            side: Side::Right,
        })
    }
}

#[cfg(test)]
/// Tests for the ticker.
mod tests {
    use mog_tui::UiEvent;

    use super::{FLOOR, OPEN, Stonks};
    use crate::flair::Flair;

    /// Saving pumps, new errors dump and fixing them pumps again.
    #[test]
    fn reacts_to_events() {
        let mut stonks = Stonks::new();
        stonks.observe(&UiEvent::Saved);
        assert!(stonks.price > OPEN);
        let before = stonks.price;
        stonks.observe(&UiEvent::Diagnostics {
            errors: 3,
            warnings: 0,
        });
        assert!(stonks.price < before);
        let crashed = stonks.price;
        stonks.observe(&UiEvent::Diagnostics {
            errors: 0,
            warnings: 0,
        });
        assert!(stonks.price > crashed);
    }

    /// The price never hits zero.
    #[test]
    fn has_a_floor() {
        let mut stonks = Stonks::new();
        for _ in 0..500 {
            stonks.trade(0.5);
        }
        assert!((stonks.price - FLOOR).abs() < f32::EPSILON);
    }
}
