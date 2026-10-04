//! Spectrum bars for whatever the computer is playing.

use std::time::Instant;

use mog_audio::{Loopback, spectrum};
use mog_tui::theme::mix;
use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use crate::flair::{Corner, Flair, FlairContext, Placement};

/// The width of the box in cells.
const WIDTH: u16 = 36;

/// The height of the box in cells.
const HEIGHT: u16 = 6;

/// How far a bar falls per second, as a fraction of full height.
const FALL: f32 = 2.2;

/// How far a peak marker falls per second.
const PEAK_FALL: f32 = 0.5;

/// Bars below this count as silent.
const SILENT: f32 = 0.01;

/// The eighth blocks a bar is built from, empty first.
const BLOCKS: [&str; 9] = [
    " ", "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}", "\u{2586}", "\u{2587}",
    "\u{2588}",
];

/// Bouncing bars for all desktop audio, drawn in the bottom right corner.
#[derive(Debug, Default)]
pub struct Spectrum {
    /// The recording, started the first time the flair is drawn.
    capture: Option<Loopback>,
    /// The shown height of each bar from 0 to 1.
    levels: Vec<f32>,
    /// The falling peak marker of each bar.
    peaks: Vec<f32>,
    /// When the bars were last updated.
    updated: Option<Instant>,
}

impl Spectrum {
    /// Creates the flair without starting to record.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads new levels from the recording and lets bars fall toward them.
    fn update(&mut self, bars: usize) {
        let Some(capture) = &self.capture else {
            return;
        };
        let target = spectrum::bands(&capture.samples(), capture.sample_rate(), bars);
        let dt = self
            .updated
            .replace(Instant::now())
            .map_or(0.0, |at| at.elapsed().as_secs_f32());
        self.levels.resize(bars, 0.0);
        self.peaks.resize(bars, 0.0);
        for ((level, peak), target) in self.levels.iter_mut().zip(&mut self.peaks).zip(target) {
            *level = target.max(*level - FALL * dt);
            *peak = level.max(*peak - PEAK_FALL * dt);
        }
    }
}

impl Flair for Spectrum {
    fn id(&self) -> &str {
        "spectrum"
    }

    fn description(&self) -> &str {
        "Spectrum bars for any sound the computer plays, not just mog."
    }

    fn placement(&self) -> Placement {
        Placement::Corner {
            corner: Corner::BottomRight,
            width: WIDTH,
            height: HEIGHT,
        }
    }

    fn is_animating(&self) -> bool {
        self.peaks.iter().any(|&peak| peak > SILENT)
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        if self.capture.is_none() {
            self.capture = Some(Loopback::start());
        }
        let bars = usize::from(area.width.div_ceil(2));
        self.update(bars);
        let p = cx.theme.palette;
        let height = f32::from(area.height);
        for (index, (&level, &peak)) in self.levels.iter().zip(&self.peaks).enumerate() {
            let Ok(index) = u16::try_from(index) else {
                break;
            };
            let x = area.x + index * 2;
            if x >= area.right() {
                break;
            }
            // levels are clamped to 0..1 so the casts cannot overflow
            let eighths = (level * height * 8.0).round() as u16;
            let peak_row = (peak * height).floor() as u16;
            for row in 0..area.height {
                let y = area.bottom() - 1 - row;
                let fill = eighths.saturating_sub(row * 8).min(8);
                let color = mix(p.accent2, p.accent, f32::from(row) / height);
                if fill > 0 {
                    buf.set_string(x, y, BLOCKS[usize::from(fill)], Style::new().fg(color));
                } else if row == peak_row && peak > SILENT {
                    buf.set_string(x, y, "\u{2594}", Style::new().fg(p.fg));
                }
            }
        }
    }
}
