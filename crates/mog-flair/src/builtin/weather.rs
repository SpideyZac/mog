//! The weather in your codebase, going by the diagnostics.

use std::time::Instant;

use mog_core::Severity;
use mog_tui::{Segment, Side};
use ratatui::style::Style;

use crate::flair::{Flair, FlairContext, Placement};

/// How bad things look in the focused file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sky {
    /// No problems at all.
    Clear,
    /// Only warnings.
    Cloudy,
    /// A few errors.
    Showers,
    /// Lots of errors.
    Storm,
}

/// Returns the sky for `errors` and `warnings`.
fn sky(errors: usize, warnings: usize) -> Sky {
    match (errors, warnings) {
        (0, 0) => Sky::Clear,
        (0, _) => Sky::Cloudy,
        (1..=3, _) => Sky::Showers,
        _ => Sky::Storm,
    }
}

impl Sky {
    /// Returns the symbol, base temperature in celsius and forecast.
    fn report(self) -> (&'static str, f32, &'static str) {
        match self {
            Self::Clear => ("\u{2600}", 24.0, "clear skies"),
            Self::Cloudy => ("\u{2601}", 17.0, "warning clouds"),
            Self::Showers => ("\u{2602}", 11.0, "bug showers"),
            Self::Storm => ("\u{2608}", 3.0, "error storm"),
        }
    }
}

/// Shows a little weather report in the tab bar.
#[derive(Debug)]
pub struct Weather {
    /// When mog started, so the temperature drifts a little over the day.
    started: Instant,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Weather {
    /// Creates the weather report.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for Weather {
    fn id(&self) -> &str {
        "weather"
    }

    fn description(&self) -> &str {
        "The weather in your codebase. Errors bring storms."
    }

    fn placement(&self) -> Placement {
        Placement::TabBar
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let diagnostics = cx.editor.document().diagnostics();
        let count = |severity| {
            diagnostics
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        let sky = sky(count(Severity::Error), count(Severity::Warning));
        let (symbol, base, forecast) = sky.report();
        let drift = (self.started.elapsed().as_secs_f32() / 600.0).sin() * 2.0;
        let p = cx.theme.palette;
        let color = match sky {
            Sky::Clear => p.yellow,
            Sky::Cloudy => p.fg,
            Sky::Showers => p.cyan,
            Sky::Storm => p.red,
        };
        let style = Style::new().bg(cx.theme.tab.bg.unwrap_or(p.panel));
        Some(Segment {
            parts: vec![
                (format!("{symbol} "), style.fg(color)),
                (format!("{:.0}\u{b0} ", base + drift), style.fg(p.fg)),
                (forecast.into(), style.fg(p.dim)),
            ],
            side: Side::Right,
        })
    }
}

#[cfg(test)]
/// Tests for the weather.
mod tests {
    use super::{Sky, sky};

    /// More problems make worse weather.
    #[test]
    fn worse_with_errors() {
        assert_eq!(sky(0, 0), Sky::Clear);
        assert_eq!(sky(0, 4), Sky::Cloudy);
        assert_eq!(sky(2, 0), Sky::Showers);
        assert_eq!(sky(9, 9), Sky::Storm);
    }
}
