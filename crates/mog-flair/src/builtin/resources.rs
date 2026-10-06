//! A resource monitor under the file explorer, with real cpu, ram and gpu when it can find one,
//! and how much memory and time plugins take.

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
    gpu::{GpuMonitor, GpuReading},
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

/// How long a plugin answer can take before it shows as slow, like mog's slow plugin warning.
const SLOW: Duration = Duration::from_secs(1);

/// How long a plugin answer can take before it shows as getting slow.
const SLUGGISH: Duration = Duration::from_millis(100);

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
    /// The made up gpu usage in percent, for systems without a gpu to read.
    gpu: f32,
    /// Reads the real gpu, started the first time the monitor is drawn.
    monitor: Option<GpuMonitor>,
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
            monitor: None,
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
        if first {
            self.monitor = Some(GpuMonitor::start());
        }
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

/// Returns `took` in at most five columns, like ` 12ms` or ` 1.5s`.
fn latency(took: Duration) -> String {
    if took < Duration::from_secs(1) {
        format!("{:>3}ms", took.as_millis())
    } else {
        format!("{:>4.1}s", took.as_secs_f32().min(99.9))
    }
}

/// Returns the plugin row: a summary, its color and the slowest recent answer time.
fn plugin_row(cx: &FlairContext<'_>, width: usize) -> (String, Color, String) {
    let plugins = &cx.ui.plugin_health;
    let p = cx.theme.palette;
    if plugins.is_empty() {
        return (format!("{:<width$}", "none"), p.dim, String::new());
    }
    let memory: u64 = plugins.iter().filter_map(|plugin| plugin.memory).sum();
    let worst = plugins.iter().filter_map(|plugin| plugin.p95).max();
    let summary = format!("{} on {}MB", plugins.len(), memory >> 20);
    let color = match worst {
        _ if plugins.iter().any(|plugin| plugin.timeouts > 0) => p.red,
        Some(took) if took >= SLOW => p.red,
        Some(took) if took >= SLUGGISH => p.yellow,
        _ => p.green,
    };
    (
        format!("{summary:<width$.width$}"),
        color,
        worst.map(latency).unwrap_or_default(),
    )
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
        "A resource monitor under the file explorer, with plugins. The gpu is mining $MOG."
    }

    fn placement(&self) -> Placement {
        Placement::Sidebar { height: 5 }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &FlairContext<'_>) {
        // reading the system costs cpu, which an idle editor should not spend
        if let Some(monitor) = &self.monitor {
            monitor.set_paused(cx.ui.idle);
        }
        if !cx.ui.idle || self.refreshed.is_none() {
            self.refresh();
        }
        let theme = cx.theme;
        let inner = sidebar_frame(area, buf, theme, "system");
        let width = usize::from(inner.width);
        if width < LABEL + PERCENT + 2 || inner.height < 4 {
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
        let reading = self
            .monitor
            .as_ref()
            .map_or(GpuReading::Detecting, GpuMonitor::reading);
        // the gpu is always mining, only the number is real when there is one to read
        let gpu = match reading {
            GpuReading::Percent(busy) => busy / 100.0,
            GpuReading::Detecting | GpuReading::Missing => self.gpu / 100.0,
        };
        let mining = format!("{:<graph$}", "mining $MOG");
        row(
            buf,
            inner.y + 2,
            "gpu",
            &mining,
            theme.palette.accent,
            &percent(gpu),
        );
        let (plugins, color, slowest) = plugin_row(cx, graph);
        row(buf, inner.y + 3, "plug", &plugins, color, &slowest);
    }
}

#[cfg(test)]
/// Tests for the resource monitor.
mod tests {
    use std::time::Duration;

    use super::{latency, percent};

    /// Percentages are padded and clamped.
    #[test]
    fn formats_percent() {
        assert_eq!(percent(0.61), "  61%");
        assert_eq!(percent(2.0), " 100%");
        assert_eq!(percent(-1.0), "   0%");
    }

    /// Plugin answer times fit the percentage column.
    #[test]
    fn formats_latency() {
        assert_eq!(latency(Duration::from_millis(12)), " 12ms");
        assert_eq!(latency(Duration::from_millis(1500)), " 1.5s");
        assert_eq!(latency(Duration::from_secs(500)), "99.9s");
    }
}
