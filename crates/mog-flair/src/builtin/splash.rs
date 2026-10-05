//! The welcome screen shown on an empty scratch buffer.

use std::time::Duration;

use mog_tui::theme::mix;
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

/// The big logo.
const LOGO: [&str; 6] = [
    "\u{2588}\u{2588}\u{2588}\u{2557}   \u{2588}\u{2588}\u{2588}\u{2557} \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2557}  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2557} ",
    "\u{2588}\u{2588}\u{2588}\u{2588}\u{2557} \u{2588}\u{2588}\u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2550}\u{2588}\u{2588}\u{2557}\u{2588}\u{2588}\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d} ",
    "\u{2588}\u{2588}\u{2554}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}   \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}  \u{2588}\u{2588}\u{2588}\u{2557}",
    "\u{2588}\u{2588}\u{2551}\u{255a}\u{2588}\u{2588}\u{2554}\u{255d}\u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}   \u{2588}\u{2588}\u{2551}\u{2588}\u{2588}\u{2551}   \u{2588}\u{2588}\u{2551}",
    "\u{2588}\u{2588}\u{2551} \u{255a}\u{2550}\u{255d} \u{2588}\u{2588}\u{2551}\u{255a}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}\u{255a}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2554}\u{255d}",
    "\u{255a}\u{2550}\u{255d}     \u{255a}\u{2550}\u{255d} \u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}  \u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d} ",
];

/// Taglines, one picked per start.
const TAGLINES: &[&str] = &[
    "the editor that mogs other editors",
    "now with 40% more critters",
    "your code, but it has aura",
    "certified organic, mostly",
    "it's not a phase, it's an editor",
    "vim users hate this one trick",
];

/// Shortcuts worth knowing, as `(keys, what)`.
const HINTS: &[(&str, &str)] = &[
    ("ctrl+p", "find a file"),
    ("ctrl+shift+p", "command palette"),
    ("ctrl+,", "settings"),
    ("ctrl+k", "every key binding"),
    ("ctrl+b", "file explorer"),
    ("alt+g", "project graph"),
    ("ctrl+q", "quit"),
];

/// Tips, one picked per start.
const TIPS: &[&str] = &[
    "every flair can be switched off in settings, but why would you",
    "the ai meter is never wrong. trust the meter.",
    "type fast enough and the combo counter gets excited",
    "the mogling falls asleep if you ignore it for two minutes",
    "mog --keys prints every key binding in your terminal",
    "error lens writes the error right where it happened",
    "music gets tense when the build is broken. turn it on in settings.",
];

/// How long one shimmer cycle of the logo takes.
const SHIMMER: Duration = Duration::from_secs(4);

/// The big logo, a tagline and some hints, shown when there is nothing to edit yet.
#[derive(Debug)]
pub struct Splash {
    /// Time into the shimmer cycle.
    elapsed: Duration,
    /// The tagline picked for this run.
    tagline: &'static str,
    /// The tip picked for this run.
    tip: &'static str,
    /// Whether the logo holds still for reduced motion.
    still: bool,
    /// Whether the welcome screen was shown in the last frame, so it only animates then.
    shown: bool,
}

impl Default for Splash {
    fn default() -> Self {
        let mut rng = Rng::default();
        Self {
            elapsed: Duration::ZERO,
            tagline: *rng.pick(TAGLINES),
            tip: *rng.pick(TIPS),
            still: false,
            shown: false,
        }
    }
}

impl Splash {
    /// Creates the welcome screen.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether the welcome screen should show for `cx`.
    fn wanted(cx: &FlairContext<'_>) -> bool {
        let editor = cx.editor;
        let document = editor.document();
        editor.documents().len() == 1
            && document.path().is_none()
            && !document.is_modified()
            && document.text().len_chars() == 0
            && cx.ui.overlay.is_none()
    }
}

impl Flair for Splash {
    fn id(&self) -> &str {
        "splash"
    }

    fn description(&self) -> &str {
        "The big shiny logo and some hints when nothing is open."
    }

    fn placement(&self) -> Placement {
        Placement::Overlay
    }

    fn set_reduced_motion(&mut self, on: bool) {
        self.still = on;
    }

    fn tick(&mut self, dt: Duration) {
        if self.still {
            return;
        }
        self.elapsed = (self.elapsed + dt)
            .checked_sub(SHIMMER)
            .unwrap_or(self.elapsed + dt);
    }

    fn is_animating(&self) -> bool {
        self.shown && !self.still
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        self.shown = Self::wanted(cx);
        if !self.shown {
            return;
        }
        let p = cx.theme.palette;
        let logo_width = u16::try_from(LOGO[0].width()).unwrap_or(0);
        let height = u16::try_from(LOGO.len() + 4 + HINTS.len() + 2).unwrap_or(0);
        if area.width < logo_width + 2 || area.height < height {
            return;
        }
        let x = area.x + (area.width - logo_width) / 2;
        let mut y = area.y + (area.height - height) / 2;
        let phase = self.elapsed.as_secs_f32() / SHIMMER.as_secs_f32();
        for (row, line) in LOGO.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                // a diagonal wave of light sweeps across the logo
                let t = (col as f32 / f32::from(logo_width) + row as f32 * 0.04 - phase)
                    .rem_euclid(1.0);
                let glow = (1.0 - (t - 0.5).abs() * 2.0).powi(3);
                let base = mix(p.accent, p.accent2, row as f32 / LOGO.len() as f32);
                let color = mix(base, p.fg, glow * 0.7);
                let px = x + u16::try_from(col).unwrap_or(0);
                buf.set_string(px, y, ch.to_string(), Style::new().fg(color));
            }
            y += 1;
        }
        y += 1;
        let centered = |text: &str| {
            let width = u16::try_from(text.width()).unwrap_or(0);
            area.x + area.width.saturating_sub(width) / 2
        };
        let tagline = format!("~ {} ~", self.tagline);
        buf.set_string(
            centered(&tagline),
            y,
            &tagline,
            Style::new().fg(p.accent2).add_modifier(Modifier::ITALIC),
        );
        y += 2;
        let hint_width = 34;
        let hx = area.x + area.width.saturating_sub(hint_width) / 2;
        for (keys, what) in HINTS {
            buf.set_string(hx, y, *what, Style::new().fg(p.fg));
            let key_x = hx + hint_width - u16::try_from(keys.width()).unwrap_or(0);
            buf.set_string(
                key_x,
                y,
                *keys,
                Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
            );
            y += 1;
        }
        y += 1;
        let tip = format!("tip: {}", self.tip);
        buf.set_stringn(
            centered(&tip),
            y,
            &tip,
            usize::from(area.width),
            Style::new().fg(p.dim),
        );
    }
}
