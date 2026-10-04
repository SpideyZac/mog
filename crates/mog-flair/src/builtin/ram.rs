//! A download of more RAM that is always almost done.

use std::time::{Duration, Instant};

use mog_tui::{Segment, Side};
use ratatui::style::{Color, Style};

use crate::flair::{Flair, FlairContext, Placement};

/// How long one download takes to reach 99 percent.
const CRAWL: Duration = Duration::from_secs(8 * 60);

/// How long it sits at 99 percent before finishing.
const STALL: Duration = Duration::from_secs(4 * 60);

/// How long the finished message shows before the next download starts.
const DONE: Duration = Duration::from_secs(20);

/// How much RAM the first download brings, in gigabytes.
const FIRST_GB: u32 = 16;

/// Where a download is at `elapsed` into its cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Downloading, with the percentage done.
    Downloading(u8),
    /// Finished and installed.
    Installed,
}

/// Returns the stage of a download `elapsed` into its cycle.
fn stage(elapsed: Duration) -> Stage {
    if elapsed >= CRAWL + STALL {
        return Stage::Installed;
    }
    let done = (elapsed.as_secs_f32() / CRAWL.as_secs_f32()).min(1.0);
    // fast at first and slower near the end, like every real progress bar
    let eased = 1.0 - (1.0 - done).powi(3);
    // the clamp keeps the cast lossless
    Stage::Downloading((eased * 99.0).clamp(0.0, 99.0) as u8)
}

/// Keeps downloading more RAM, doubling it every time it finishes.
#[derive(Debug)]
pub struct Ram {
    /// When the current download started.
    started: Instant,
    /// How many downloads finished.
    finished: u32,
}

impl Default for Ram {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            finished: 0,
        }
    }
}

impl Ram {
    /// Starts the first download.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Flair for Ram {
    fn id(&self) -> &str {
        "download_ram"
    }

    fn description(&self) -> &str {
        "Downloads more RAM in the background. Almost done, trust."
    }

    fn placement(&self) -> Placement {
        Placement::Status(Side::Left)
    }

    fn segment(&mut self, cx: &FlairContext<'_>) -> Option<Segment> {
        if self.started.elapsed() >= CRAWL + STALL + DONE {
            self.started = Instant::now();
            self.finished += 1;
        }
        let gb = FIRST_GB.saturating_mul(1 << self.finished.min(16));
        let p = cx.theme.palette;
        let bg = cx.theme.status.bg.unwrap_or(Color::Reset);
        let dim = Style::new().fg(p.dim).bg(bg);
        let parts = match stage(self.started.elapsed()) {
            Stage::Downloading(percent) => {
                let filled = usize::from(percent / 20);
                let bar: String = (0..5)
                    .map(|i| if i < filled { '\u{25b0}' } else { '\u{25b1}' })
                    .collect();
                vec![
                    (format!("\u{21e3} downloading {gb}gb ram "), dim),
                    (bar, Style::new().fg(p.cyan).bg(bg)),
                    (format!(" {percent}%"), dim),
                ]
            }
            Stage::Installed => vec![(
                format!("\u{2714} {gb}gb ram installed"),
                Style::new().fg(p.green).bg(bg),
            )],
        };
        Some(Segment {
            parts,
            side: Side::Left,
        })
    }
}

#[cfg(test)]
/// Tests for the RAM download.
mod tests {
    use std::time::Duration;

    use super::{CRAWL, STALL, Stage, stage};

    /// The bar crawls to 99, stalls there, then finishes.
    #[test]
    fn crawls_and_stalls() {
        assert_eq!(stage(Duration::ZERO), Stage::Downloading(0));
        assert_eq!(stage(CRAWL), Stage::Downloading(99));
        assert_eq!(stage(CRAWL + STALL / 2), Stage::Downloading(99));
        assert_eq!(stage(CRAWL + STALL), Stage::Installed);
    }
}
