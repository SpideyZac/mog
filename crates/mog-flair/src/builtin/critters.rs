//! Little critters that wander through the empty parts of the editor.

use std::time::Duration;

use mog_tui::UiEvent;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    flair::{Flair, FlairContext, Placement},
    rng::Rng,
};

/// How long a critter hops after a save.
const HOP_TIME: Duration = Duration::from_millis(350);

/// How much faster critters run while there are errors.
const PANIC_SPEED: f32 = 2.5;

/// The kinds of critter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A cat.
    Cat,
    /// A duck.
    Duck,
    /// A fish.
    Fish,
    /// A ghost.
    Ghost,
}

impl Kind {
    /// Every kind.
    const ALL: [Self; 4] = [Self::Cat, Self::Duck, Self::Fish, Self::Ghost];

    /// Returns the sprite facing right or left.
    fn sprite(self, right: bool) -> &'static str {
        match (self, right) {
            (Self::Cat, true) => "\u{14da}\u{160f}\u{15e2}",
            (Self::Cat, false) => "\u{15e2}\u{160f}\u{14d7}",
            (Self::Duck, true) => "(')>",
            (Self::Duck, false) => "<(')",
            (Self::Fish, true) => "><>",
            (Self::Fish, false) => "<><",
            (Self::Ghost, _) => "\u{15e3}",
        }
    }

    /// Returns the walking speed in cells per second.
    fn speed(self) -> f32 {
        match self {
            Self::Cat => 3.0,
            Self::Duck => 2.0,
            Self::Fish => 4.5,
            Self::Ghost => 2.5,
        }
    }

    /// Returns the palette color index for the critter.
    fn color(self, colors: &[Color; 4]) -> Color {
        match self {
            Self::Cat => colors[0],
            Self::Duck => colors[1],
            Self::Fish => colors[2],
            Self::Ghost => colors[3],
        }
    }
}

/// One critter.
#[derive(Debug, Clone)]
struct Critter {
    /// What it is.
    kind: Kind,
    /// Column inside the editor area.
    x: f32,
    /// Row inside the editor area.
    row: u16,
    /// Whether it walks right.
    right: bool,
    /// Seconds left of standing still.
    resting: f32,
}

/// A few critters that walk through blank space and hide behind code.
#[derive(Debug, Default)]
pub struct Critters {
    /// The critters, spawned on the first render.
    critters: Vec<Critter>,
    /// Whether there are errors, which makes everyone panic.
    panicking: bool,
    /// Time left in the hop after a save.
    hop: Duration,
    /// Random numbers for wandering.
    rng: Rng,
    /// The editor area from the last render.
    area: Rect,
}

impl Critters {
    /// Creates the flair.
    pub fn new() -> Self {
        Self::default()
    }

    /// Places a critter of every kind at random spots in `area`.
    fn spawn(&mut self, area: Rect) {
        self.critters = Kind::ALL
            .iter()
            .map(|&kind| Critter {
                kind,
                x: self.rng.range(0.0, f32::from(area.width.max(1))),
                row: u16::try_from(self.rng.below(usize::from(area.height.max(1)))).unwrap_or(0),
                right: self.rng.below(2) == 0,
                resting: self.rng.range(0.0, 2.0),
            })
            .collect();
    }
}

impl Flair for Critters {
    fn id(&self) -> &str {
        "critters"
    }

    fn moves(&self) -> bool {
        true
    }

    fn description(&self) -> &str {
        "A cat, a duck, a fish and a ghost wander through the blank parts of your code."
    }

    fn placement(&self) -> Placement {
        Placement::Overlay
    }

    fn observe(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Saved => self.hop = HOP_TIME,
            UiEvent::Diagnostics { errors, .. } => self.panicking = *errors > 0,
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        let secs = dt.as_secs_f32();
        self.hop = self.hop.saturating_sub(dt);
        let width = f32::from(self.area.width);
        let height = self.area.height;
        if width < 8.0 || height == 0 {
            return;
        }
        let boost = if self.panicking { PANIC_SPEED } else { 1.0 };
        for critter in &mut self.critters {
            if critter.resting > 0.0 {
                critter.resting -= secs * boost;
                continue;
            }
            let step = critter.kind.speed() * boost * secs;
            critter.x += if critter.right { step } else { -step };
            let sprite_width = critter.kind.sprite(critter.right).width() as f32;
            if critter.x < 0.0 || critter.x + sprite_width > width {
                critter.right = !critter.right;
                critter.x = critter.x.clamp(0.0, width - sprite_width);
            }
            // now and then stop for a look around or wander to another row
            if self.rng.below(400) == 0 {
                critter.resting = self.rng.range(0.5, 3.0);
            }
            if self.rng.below(900) == 0 {
                critter.row = u16::try_from(self.rng.below(usize::from(height))).unwrap_or(0);
            }
        }
    }

    fn is_animating(&self) -> bool {
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        if self.area != area || self.critters.is_empty() {
            self.area = area;
            self.spawn(area);
        }
        let p = cx.theme.palette;
        let colors = [p.orange, p.yellow, p.cyan, p.purple];
        let hop = u16::from(!self.hop.is_zero());
        for critter in &self.critters {
            let sprite = critter.kind.sprite(critter.right);
            // the clamp in tick keeps x inside the area, this only drops the fraction
            let x = area.x + critter.x.max(0.0) as u16;
            let y = (area.y + critter.row.min(area.height - 1))
                .saturating_sub(hop)
                .max(area.y);
            let width = u16::try_from(sprite.width()).unwrap_or(0);
            let cells = (x..x + width).map(|cx| Position::new(cx, y));
            // only walk where there is nothing written, so code always wins
            let free = cells.clone().all(|cell| {
                area.contains(cell)
                    && buf[cell].symbol() == " "
                    && Some(cell) != cx.ui.cursor_screen
            });
            if free {
                let style = Style::new().fg(critter.kind.color(&colors));
                buf.set_string(x, y, sprite, style);
            }
        }
    }
}
