//! Aura points, gained and lost by how you code.

use std::time::Duration;

use mog_tui::{Segment, Side, UiEvent};
use ratatui::style::{Color, Modifier, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// How long a gain or loss is shown next to the total.
const FLASH: Duration = Duration::from_millis(1500);

/// Aura for saving.
const SAVE: i64 = 25;

/// Aura for every line written.
const NEW_LINE: i64 = 3;

/// Aura for fixing every error.
const FIXED: i64 = 500;

/// Aura lost per new error.
const BROKE: i64 = -150;

/// Formats `n` with thousands separators and a sign, like `+1,250`.
fn signed(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if n < 0 { '-' } else { '+' };
    format!("{sign}{grouped}")
}

/// Keeps an aura score that goes up for good habits and down for broken builds.
#[derive(Debug, Default)]
pub struct Aura {
    /// The total aura.
    total: i64,
    /// The error count last seen, to notice new and fixed errors.
    errors: usize,
    /// The newest change and how long it has been shown.
    flash: Option<(i64, Duration)>,
}

impl Aura {
    /// Starts at zero aura.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `delta` to the total and flashes it.
    fn gain(&mut self, delta: i64) {
        self.total += delta;
        let shown = match self.flash {
            Some((previous, _)) if previous.signum() == delta.signum() => previous + delta,
            _ => delta,
        };
        self.flash = Some((shown, Duration::ZERO));
    }
}

impl Flair for Aura {
    fn id(&self) -> &str {
        "aura"
    }

    fn description(&self) -> &str {
        "Aura points. Saving and fixing errors farms aura, breaking the build loses it."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Right)
    }

    fn observe(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Saved => self.gain(SAVE),
            UiEvent::Typed('\n') => self.gain(NEW_LINE),
            UiEvent::Diagnostics { errors, .. } => {
                let (before, now) = (self.errors, *errors);
                self.errors = now;
                if now == 0 && before > 0 {
                    self.gain(FIXED);
                } else if now > before {
                    let new = i64::try_from(now - before).unwrap_or(i64::MAX);
                    self.gain(new.saturating_mul(BROKE));
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if let Some((_, shown)) = &mut self.flash {
            *shown += dt;
            if *shown >= FLASH {
                self.flash = None;
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.flash.is_some()
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let p = cx.theme.palette;
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let color = if self.total < 0 { p.red } else { p.purple };
        let mut parts = vec![
            ("\u{2726} aura ".into(), Style::new().fg(color).bg(bg)),
            (
                signed(self.total),
                Style::new().fg(color).bg(bg).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some((delta, _)) = self.flash {
            let flash = if delta < 0 { p.red } else { p.green };
            parts.push((format!(" {}", signed(delta)), Style::new().fg(flash).bg(bg)));
        }
        Some(Segment {
            parts,
            side: Side::Right,
        })
    }
}

#[cfg(test)]
/// Tests for the aura counter.
mod tests {
    use mog_tui::UiEvent;

    use super::{Aura, signed};
    use crate::flair::Flair;

    /// Numbers get a sign and thousands separators.
    #[test]
    fn formats_numbers() {
        assert_eq!(signed(0), "+0");
        assert_eq!(signed(1250), "+1,250");
        assert_eq!(signed(-1_000_000), "-1,000,000");
    }

    /// Breaking the build costs aura and fixing it pays more back.
    #[test]
    fn errors_move_aura() {
        let mut aura = Aura::new();
        aura.observe(&UiEvent::Diagnostics {
            errors: 2,
            warnings: 0,
        });
        assert_eq!(aura.total, -300);
        aura.observe(&UiEvent::Diagnostics {
            errors: 0,
            warnings: 0,
        });
        assert_eq!(aura.total, 200);
    }
}
