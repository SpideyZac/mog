//! One liners in the status line when something happens.

use std::time::Duration;

use mog_tui::{Segment, Side, UiEvent};
use ratatui::style::{Color, Modifier, Style};

use crate::{
    flair::{Flair, FlairContext, Placement},
    rng::Rng,
};

/// How long a quip stays up.
const SHOW_TIME: Duration = Duration::from_secs(4);

/// Said after a save.
const SAVED: &[&str] = &[
    "saved. you're mogging.",
    "saved. no cap.",
    "saved. ship it.",
    "saved (the file, not your soul)",
    "saved. future you says thanks.",
    "saved. that one was a banger.",
    "saved. ctrl+s gang.",
];

/// Said when errors show up in a clean file.
const BROKE: &[&str] = &[
    "skill issue detected",
    "the compiler is crying",
    "red squiggles, red flags",
    "it was working five minutes ago",
    "have you tried turning it off and on",
];

/// Said when the last error goes away.
const FIXED: &[&str] = &[
    "clean build. mog approves.",
    "0 errors. we're so back.",
    "it compiles. don't touch it.",
    "no errors. touch grass to celebrate.",
];

/// Shows a short message for a few seconds after saves and build changes.
#[derive(Debug, Default)]
pub struct Quips {
    /// The message being shown and how long it has been up.
    showing: Option<(&'static str, Duration)>,
    /// The error count at the last update.
    errors: usize,
    /// Picks which quip to say.
    rng: Rng,
}

impl Quips {
    /// Creates the flair.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts showing a random line from `lines`.
    fn say(&mut self, lines: &[&'static str]) {
        self.showing = Some((*self.rng.pick(lines), Duration::ZERO));
    }
}

impl Flair for Quips {
    fn id(&self) -> &str {
        "quips"
    }

    fn description(&self) -> &str {
        "Little one liners when you save, break or fix the build."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Left)
    }

    fn observe(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Saved => self.say(SAVED),
            UiEvent::Diagnostics { errors, .. } => {
                if self.errors == 0 && *errors > 0 {
                    self.say(BROKE);
                } else if self.errors > 0 && *errors == 0 {
                    self.say(FIXED);
                }
                self.errors = *errors;
            }
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if let Some((_, age)) = &mut self.showing {
            *age += dt;
            if *age > SHOW_TIME {
                self.showing = None;
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.showing.is_some()
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        let (text, _) = self.showing?;
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let style = Style::new()
            .fg(cx.theme.palette.accent2)
            .bg(bg)
            .add_modifier(Modifier::ITALIC);
        Some(Segment::new(text, style, Side::Left))
    }
}
