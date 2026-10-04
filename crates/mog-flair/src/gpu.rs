//! Reading how busy the GPU is, from whatever tool the system has.

use std::{
    collections::HashMap,
    fs,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// How often the GPU is read.
const INTERVAL: Duration = Duration::from_secs(2);

/// What is known about the GPU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GpuReading {
    /// Still looking for a way to read it.
    Detecting,
    /// Nothing on this system reports GPU usage.
    Missing,
    /// The GPU is this busy, in percent.
    Percent(f32),
}

/// A place the GPU usage can be read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// NVIDIA's `nvidia-smi`, on any system.
    NvidiaSmi,
    /// The `gpu_busy_percent` files Linux drivers like amdgpu write.
    LinuxSysfs,
    /// The accelerator statistics `ioreg` shows on macOS.
    Ioreg,
    /// The GPU engine counters Windows keeps, read with `typeperf`.
    Typeperf,
}

impl Source {
    /// Every source worth trying on this system, best first.
    fn candidates() -> Vec<Self> {
        let mut sources = vec![Self::NvidiaSmi];
        if cfg!(target_os = "linux") {
            sources.push(Self::LinuxSysfs);
        }
        if cfg!(target_os = "macos") {
            sources.push(Self::Ioreg);
        }
        if cfg!(windows) {
            sources.push(Self::Typeperf);
        }
        sources
    }

    /// Reads the GPU usage in percent, or `None` if this source does not work here.
    fn read(self) -> Option<f32> {
        match self {
            Self::NvidiaSmi => parse_nvidia(&run(
                "nvidia-smi",
                &[
                    "--query-gpu=utilization.gpu",
                    "--format=csv,noheader,nounits",
                ],
            )?),
            Self::LinuxSysfs => fs::read_dir("/sys/class/drm")
                .ok()?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    fs::read_to_string(entry.path().join("device/gpu_busy_percent")).ok()
                })
                .filter_map(|text| text.trim().parse::<f32>().ok())
                .reduce(f32::max),
            Self::Ioreg => parse_ioreg(&run(
                "ioreg",
                &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"],
            )?),
            Self::Typeperf => parse_typeperf(&run(
                "typeperf",
                &[
                    r"\GPU Engine(*engtype_3D)\Utilization Percentage",
                    "-sc",
                    "1",
                ],
            )?),
        }
    }
}

/// Runs `program` with `args` and returns what it printed, if it ran fine.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Returns the busiest GPU from `nvidia-smi` output, one percentage per line.
fn parse_nvidia(text: &str) -> Option<f32> {
    text.lines()
        .filter_map(|line| line.trim().parse::<f32>().ok())
        .reduce(f32::max)
}

/// Returns the busiest accelerator from `ioreg` output.
fn parse_ioreg(text: &str) -> Option<f32> {
    const KEY: &str = "\"Device Utilization %\"=";
    text.match_indices(KEY)
        .filter_map(|(at, _)| {
            let digits: String = text[at + KEY.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<f32>().ok()
        })
        .reduce(f32::max)
}

/// Returns the busiest 3D engine from `typeperf` CSV output, like Task Manager does.
///
/// Each column is one process on one engine, so columns are summed per engine first.
fn parse_typeperf(text: &str) -> Option<f32> {
    let mut lines = text.lines().filter(|line| line.starts_with('"'));
    let header = lines.next()?;
    let values = lines.next()?;
    let split = |line: &str| -> Vec<String> {
        line.split("\",\"")
            .map(|cell| cell.trim_matches('"').to_owned())
            .collect()
    };
    let mut engines: HashMap<String, f32> = HashMap::new();
    for (name, value) in split(header).iter().zip(split(values)).skip(1) {
        let engine = name.find("luid_").map_or(name.as_str(), |at| &name[at..]);
        let engine = engine.split("_engtype").next().unwrap_or(engine);
        *engines.entry(engine.to_owned()).or_default() +=
            value.trim().parse::<f32>().unwrap_or(0.0);
    }
    engines
        .into_values()
        .reduce(f32::max)
        .map(|busiest| busiest.clamp(0.0, 100.0))
}

/// Reads the GPU on its own thread, since some tools take a second to answer.
#[derive(Debug)]
pub struct GpuMonitor {
    /// The newest reading.
    reading: Arc<Mutex<GpuReading>>,
    /// Set when the monitor is dropped so the thread stops.
    stop: Arc<AtomicBool>,
}

impl GpuMonitor {
    /// Starts reading the GPU in the background.
    pub fn start() -> Self {
        let reading = Arc::new(Mutex::new(GpuReading::Detecting));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (reading, stop) = (reading.clone(), stop.clone());
            thread::spawn(move || watch(&reading, &stop));
        }
        Self { reading, stop }
    }

    /// Returns the newest reading.
    pub fn reading(&self) -> GpuReading {
        *self.reading.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for GpuMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Finds a source that works, then keeps `reading` fresh until `stop` is set.
fn watch(reading: &Mutex<GpuReading>, stop: &AtomicBool) {
    let set = |value| *reading.lock().unwrap_or_else(PoisonError::into_inner) = value;
    let found = Source::candidates()
        .into_iter()
        .find_map(|source| source.read().map(|percent| (source, percent)));
    let Some((source, percent)) = found else {
        set(GpuReading::Missing);
        return;
    };
    set(GpuReading::Percent(percent));
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(INTERVAL);
        if let Some(percent) = source.read() {
            set(GpuReading::Percent(percent));
        }
    }
}

#[cfg(test)]
/// Tests for reading the GPU.
mod tests {
    use super::{Source, parse_ioreg, parse_nvidia, parse_typeperf};

    /// Prints what this machine reports, run with `--ignored` to check a real gpu.
    #[test]
    #[ignore = "depends on the machine"]
    fn reads_this_machine() {
        let found: Vec<_> = Source::candidates()
            .into_iter()
            .map(|source| (source, source.read()))
            .collect();
        println!("{found:?}");
        assert!(found.iter().any(|(_, reading)| reading.is_some()));
    }

    /// The busiest of several NVIDIA GPUs wins.
    #[test]
    fn reads_nvidia() {
        assert_eq!(parse_nvidia("12\n57\n"), Some(57.0));
        assert_eq!(parse_nvidia("[N/A]\n"), None);
    }

    /// The utilization number is found in ioreg's wall of text.
    #[test]
    fn reads_ioreg() {
        let text = r#"| "PerformanceStatistics" = {"Device Utilization %"=42,"Renderer Utilization %"=40}"#;
        assert_eq!(parse_ioreg(text), Some(42.0));
        assert_eq!(parse_ioreg("nothing here"), None);
    }

    /// Processes on the same engine add up and the busiest engine wins.
    #[test]
    fn reads_typeperf() {
        let text = concat!(
            "\n",
            r#""(PDH-CSV 4.0)","\\PC\GPU Engine(pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D)\Utilization Percentage","\\PC\GPU Engine(pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D)\Utilization Percentage","\\PC\GPU Engine(pid_2_luid_0x0_0x1_phys_0_eng_1_engtype_3D)\Utilization Percentage""#,
            "\n",
            r#""10/04/2026 14:41:05.837","20.5","30.0","10.0""#,
            "\n",
            "Exiting, please wait...\n",
        );
        assert_eq!(parse_typeperf(text), Some(50.5));
        assert_eq!(parse_typeperf("garbage"), None);
    }
}
