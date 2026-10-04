//! The mogling, a little face in the status line that has feelings about your code.

use std::time::Duration;

use mog_tui::{Segment, Side, UiEvent};
use ratatui::style::{Color, Modifier, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// How long the mogling looks proud after a save.
const PROUD_TIME: Duration = Duration::from_secs(3);

/// How long the mogling looks shocked after new errors show up.
const SHOCK_TIME: Duration = Duration::from_secs(2);

/// How long without input before the mogling falls asleep.
const SLEEP_TIME: Duration = Duration::from_secs(120);

/// How often the mogling blinks.
const BLINK_EVERY: Duration = Duration::from_millis(4700);

/// How long a blink lasts.
const BLINK_TIME: Duration = Duration::from_millis(180);

/// How many quick keystrokes count as hyped.
const HYPE_KEYS: u32 = 15;

/// The gap between keystrokes that keeps the hype going.
const HYPE_GAP: Duration = Duration::from_millis(900);

/// How the mogling feels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mood {
    /// All is well.
    Happy,
    /// Blinking while happy.
    Blink,
    /// Just saved.
    Proud,
    /// New errors appeared.
    Shocked,
    /// There are errors.
    Sad,
    /// There are warnings.
    Worried,
    /// Typing fast.
    Hyped,
    /// Nobody has touched the keyboard in a while.
    Sleepy,
}

impl Mood {
    /// Returns the face for the mood.
    fn face(self) -> &'static str {
        match self {
            Self::Happy => "(\u{25d5}\u{203f}\u{25d5})",
            Self::Blink => "(-\u{203f}-)",
            Self::Proud => "(\u{2310}\u{25a0}_\u{25a0})",
            Self::Shocked => "(\u{b0}o\u{b0})",
            Self::Sad => "(;_;)",
            Self::Worried => "(-_-;)",
            Self::Hyped => "(\u{15d2}\u{15e8}\u{15d5})",
            Self::Sleepy => "(-.-) zZ",
        }
    }
}

/// A face in the status line that reacts to saves, errors, typing and idling.
#[derive(Debug, Default)]
pub struct Pet {
    /// Time since the mogling was born.
    clock: Duration,
    /// When the last save happened.
    saved_at: Option<Duration>,
    /// When the error count last went up.
    shocked_at: Option<Duration>,
    /// When the last input happened.
    active_at: Duration,
    /// When the last char was typed.
    typed_at: Option<Duration>,
    /// Quick keystrokes in a row.
    streak: u32,
    /// The current error and warning counts.
    problems: (usize, usize),
}

impl Pet {
    /// Creates the mogling.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `at` happened less than `window` ago.
    fn within(&self, at: Option<Duration>, window: Duration) -> bool {
        at.is_some_and(|at| self.clock.saturating_sub(at) < window)
    }

    /// Works out how the mogling feels right now.
    fn mood(&self) -> Mood {
        if self.within(self.shocked_at, SHOCK_TIME) {
            Mood::Shocked
        } else if self.within(self.saved_at, PROUD_TIME) {
            Mood::Proud
        } else if self.problems.0 > 0 {
            Mood::Sad
        } else if self.streak >= HYPE_KEYS && self.within(self.typed_at, HYPE_GAP) {
            Mood::Hyped
        } else if self.problems.1 > 0 {
            Mood::Worried
        } else if self.clock.saturating_sub(self.active_at) > SLEEP_TIME {
            Mood::Sleepy
        } else if self.clock.as_millis() % BLINK_EVERY.as_millis() < BLINK_TIME.as_millis() {
            Mood::Blink
        } else {
            Mood::Happy
        }
    }
}

impl Flair for Pet {
    fn id(&self) -> &str {
        "pet"
    }

    fn description(&self) -> &str {
        "The mogling. A tiny face that gets sad about errors and proud when you save."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Left)
    }

    fn observe(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Saved => self.saved_at = Some(self.clock),
            UiEvent::Diagnostics { errors, warnings } => {
                if *errors > self.problems.0 {
                    self.shocked_at = Some(self.clock);
                }
                self.problems = (*errors, *warnings);
            }
            UiEvent::Typed(_) => {
                self.streak = if self.within(self.typed_at, HYPE_GAP) {
                    self.streak + 1
                } else {
                    1
                };
                self.typed_at = Some(self.clock);
                self.active_at = self.clock;
            }
            _ => self.active_at = self.clock,
        }
    }

    fn tick(&mut self, dt: Duration) {
        self.clock += dt;
    }

    fn is_animating(&self) -> bool {
        true
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let p = cx.theme.palette;
        let mood = self.mood();
        let color = match mood {
            Mood::Sad | Mood::Shocked => p.red,
            Mood::Worried => p.yellow,
            Mood::Proud | Mood::Hyped => p.accent,
            Mood::Sleepy => p.dim,
            Mood::Happy | Mood::Blink => p.accent2,
        };
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let style = Style::new().fg(color).bg(bg).add_modifier(Modifier::BOLD);
        Some(Segment::new(mood.face(), style, Side::Left))
    }
}

#[cfg(test)]
/// Tests for the mogling.
mod tests {
    use std::time::Duration;

    use mog_tui::UiEvent;

    use super::{Mood, PROUD_TIME, Pet, SLEEP_TIME};
    use crate::flair::Flair;

    /// Saving makes it proud for a while, errors make it sad and idling makes it sleepy.
    #[test]
    fn moods_follow_events() {
        let mut pet = Pet::new();
        pet.tick(Duration::from_secs(1));
        assert_eq!(pet.mood(), Mood::Happy);
        pet.observe(&UiEvent::Saved);
        assert_eq!(pet.mood(), Mood::Proud);
        pet.tick(PROUD_TIME);
        pet.observe(&UiEvent::Diagnostics {
            errors: 2,
            warnings: 0,
        });
        assert_eq!(pet.mood(), Mood::Shocked);
        pet.tick(Duration::from_secs(3));
        assert_eq!(pet.mood(), Mood::Sad);
        pet.observe(&UiEvent::Diagnostics {
            errors: 0,
            warnings: 0,
        });
        pet.tick(SLEEP_TIME + Duration::from_secs(1));
        assert_eq!(pet.mood(), Mood::Sleepy);
    }
}
