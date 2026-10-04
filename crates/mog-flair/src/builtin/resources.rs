//! A resource monitor under the file explorer, with real cpu and ram and a very real gpu.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
use sysinfo::System;

use crate::{
    flair::{Flair, FlairContext, Placement, sidebar_frame},
    rng::Rng,
    spark::{bar, sparkline},
};

/// How often the numbers are read again.
const REFRESH: Duration = Duration::from_millis(1500);

/// How many cpu readings the graph keeps.
const HISTORY: usize = 40;

/// The width of the label column.
const LABEL: usize = 4;

/// The width of the percentage column.
const PERCENT: usize = 5;

/// Shows cpu, ram and gpu usage.
pub struct Resources {
    /// Where the readings come from.
    system: System,
    /// When the readings were last taken.
    refreshed: Option<Instant>,
    /// Recent cpu usage in percent, oldest first.
    cpu: VecDeque<f32>,
    /// Ram in use and in total, in bytes.
    ram: (u64, u64),
    /// The gpu usage in percent, which is always about to mine the next block.
    gpu: f32,
    /// Where the gpu numbers come from.
    rng: Rng,
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            system: System::new(),
            refreshed: None,
            cpu: VecDeque::new(),
            ram: (0, 0),
            gpu: 99.0,
            rng: Rng::default(),
        }
    }
}

impl Resources {
    /// Creates the monitor. Nothing is read until it is first drawn.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads new numbers if the last ones are old.
    fn refresh(&mut self) {
        if self.refreshed.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        let first = self.refreshed.is_none();
        self.refreshed = Some(Instant::now());
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.ram = (self.system.used_memory(), self.system.total_memory());
        // cpu usage is measured between two readings so the first one means nothing
        if first {
            return;
        }
        if self.cpu.len() == HISTORY {
            self.cpu.pop_front();
        }
        self.cpu.push_back(self.system.global_cpu_usage());
        self.gpu = self.rng.range(97.0, 100.0);
    }
}

/// Returns `fraction` as a short percentage like ` 61%`.
fn percent(fraction: f32) -> String {
    format!("{:>4.0}%", (fraction * 100.0).clamp(0.0, 100.0))
}

/// Returns the color for a load of `fraction`, from calm to on fire.
fn load_color(cx: &FlairContext<'_>, fraction: f32) -> Color {
    let p = cx.theme.palette;
    match fraction {
        f if f < 0.5 => p.green,
        f if f < 0.8 => p.yellow,
        _ => p.red,
    }
}

impl Flair for Resources {
    fn id(&self) -> &str {
        "resources"
    }

    fn description(&self) -> &str {
        "A resource monitor under the file explorer. The gpu is mining $MOG."
    }

    fn placement(&self) -> Placement {
        Placement::Sidebar { height: 4 }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        self.refresh();
        let theme = cx.theme;
        let inner = sidebar_frame(area, buf, theme, "system");
        let width = usize::from(inner.width);
        if width < LABEL + PERCENT + 2 || inner.height < 3 {
            return;
        }
        let graph = width - LABEL - PERCENT;
        let label = theme.sidebar.patch(Style::new().fg(theme.palette.dim));
        let row = |buf: &mut Buffer, y: u16, name: &str, chart: &str, color: Color, pct: &str| {
            let x = buf
                .set_stringn(inner.x, y, format!("{name:<LABEL$}"), LABEL, label)
                .0;
            let x = buf
                .set_stringn(x, y, chart, graph, theme.sidebar.fg(color))
                .0;
            buf.set_string(x, y, pct, theme.sidebar);
        };
        let cpu = self.cpu.back().copied().unwrap_or(0.0) / 100.0;
        let samples: Vec<f32> = self.cpu.iter().rev().take(graph).rev().copied().collect();
        let padded = format!(
            "{}{}",
            " ".repeat(graph - samples.len()),
            sparkline(&samples, 0.0, 100.0)
        );
        row(
            buf,
            inner.y,
            "cpu",
            &padded,
            load_color(cx, cpu),
            &percent(cpu),
        );
        let (used, total) = self.ram;
        let ram = if total == 0 {
            0.0
        } else {
            used as f32 / total as f32
        };
        row(
            buf,
            inner.y + 1,
            "ram",
            &bar(ram, graph),
            load_color(cx, ram),
            &percent(ram),
        );
        let mining = format!("{:<graph$}", "mining $MOG");
        let gpu = self.gpu / 100.0;
        row(
            buf,
            inner.y + 2,
            "gpu",
            &mining,
            theme.palette.accent,
            &percent(gpu),
        );
    }
}

#[cfg(test)]
/// Tests for the resource monitor.
mod tests {
    use super::percent;

    /// Percentages are padded and clamped.
    #[test]
    fn formats_percent() {
        assert_eq!(percent(0.61), "  61%");
        assert_eq!(percent(2.0), " 100%");
        assert_eq!(percent(-1.0), "   0%");
    }
}
