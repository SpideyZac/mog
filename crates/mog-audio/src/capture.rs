//! Listening to whatever the computer is playing.
//!
//! On Windows and macOS this records the default output device, so it hears every app and not
//! just mog. Elsewhere it looks for a monitor input, which `PulseAudio` and `PipeWire` provide.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};

use rodio::cpal::{
    Device, SampleFormat, StreamConfig, default_host,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

/// How many samples are kept for analysis.
pub const WINDOW: usize = 2048;

/// How often the capture thread checks whether it should stop.
const STOP_POLL: Duration = Duration::from_millis(200);

/// Locks `mutex`, ignoring poisoning since the data stays usable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the capture thread and its handle share.
#[derive(Debug, Default)]
struct Shared {
    /// The newest mono samples, oldest first.
    samples: Mutex<VecDeque<f32>>,
    /// The sample rate of the recording, 0 until it starts.
    sample_rate: AtomicU32,
    /// Set when the handle is dropped.
    stop: AtomicBool,
    /// Why recording failed, if it did.
    problem: Mutex<Option<String>>,
}

/// Records system audio on a background thread until dropped.
#[derive(Debug)]
pub struct Loopback {
    /// State shared with the capture thread.
    shared: Arc<Shared>,
}

impl Loopback {
    /// Starts listening. Failures are reported by [`Loopback::problem`], never by panicking.
    pub fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        thread::spawn(move || {
            if let Err(problem) = record(&thread_shared) {
                *lock(&thread_shared.problem) = Some(problem);
            }
        });
        Self { shared }
    }

    /// Copies the newest [`WINDOW`] samples into a vector, padded with silence at the front.
    pub fn samples(&self) -> Vec<f32> {
        let samples = lock(&self.shared.samples);
        let mut out = vec![0.0; WINDOW - samples.len().min(WINDOW)];
        out.extend(samples.iter().copied());
        out
    }

    /// Returns the sample rate, or 0 before recording starts.
    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Ordering::Relaxed)
    }

    /// Returns why recording failed, if it did.
    pub fn problem(&self) -> Option<String> {
        lock(&self.shared.problem).clone()
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// Returns the device that carries what the computer is playing.
fn source_device() -> Result<Device, String> {
    let host = default_host();
    if cfg!(any(windows, target_os = "macos")) {
        // recording an output device records what it plays
        return host
            .default_output_device()
            .ok_or_else(|| "no output device".to_owned());
    }
    host.input_devices()
        .map_err(|err| err.to_string())?
        .find(|device| {
            device
                .description()
                .is_ok_and(|info| info.name().to_lowercase().contains("monitor"))
        })
        .ok_or_else(|| "no monitor input, desktop audio needs pulseaudio or pipewire".to_owned())
}

/// Adds interleaved `data` with `channels` channels to the window as mono.
fn push_frames<T: Copy>(shared: &Shared, data: &[T], channels: usize, to_f32: impl Fn(T) -> f32) {
    let mut samples = lock(&shared.samples);
    for frame in data.chunks(channels.max(1)) {
        let sum: f32 = frame.iter().map(|&s| to_f32(s)).sum();
        samples.push_back(sum / frame.len() as f32);
    }
    let excess = samples.len().saturating_sub(WINDOW);
    samples.drain(..excess);
}

/// Records into `shared` until it is told to stop.
fn record(shared: &Arc<Shared>) -> Result<(), String> {
    let device = source_device()?;
    let supported = if cfg!(any(windows, target_os = "macos")) {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .map_err(|err| err.to_string())?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let channels = usize::from(config.channels);
    let on_error = |_| {};
    let stream = match format {
        SampleFormat::F32 => {
            let shared = shared.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| push_frames(&shared, data, channels, |s| s),
                on_error,
                None,
            )
        }
        SampleFormat::I16 => {
            let shared = shared.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &_| {
                    push_frames(&shared, data, channels, |s| f32::from(s) / 32_768.0);
                },
                on_error,
                None,
            )
        }
        other => return Err(format!("cannot record {other} audio")),
    }
    .map_err(|err| err.to_string())?;
    stream.play().map_err(|err| err.to_string())?;
    shared
        .sample_rate
        .store(config.sample_rate, Ordering::Relaxed);
    // the stream records for as long as it lives, and it cannot move between threads
    while !shared.stop.load(Ordering::Relaxed) {
        thread::sleep(STOP_POLL);
    }
    Ok(())
}
