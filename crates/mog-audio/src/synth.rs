//! Tiny chiptune synthesis: oscillators, envelopes and the sound effects built from them.

use std::f32::consts::TAU;

/// Samples per second for everything mog plays.
pub const SAMPLE_RATE: u32 = 44_100;

/// The shape of a tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    /// A square wave with a 50 percent duty cycle.
    Square,
    /// A thinner, buzzier square wave.
    Pulse,
    /// A soft triangle wave.
    Triangle,
    /// White noise.
    Noise,
}

/// A sound effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sfx {
    /// A char was typed.
    Key,
    /// Text was deleted.
    Delete,
    /// The file was saved.
    Save,
    /// The build broke.
    Error,
    /// The build is clean again.
    Fixed,
    /// A file was opened.
    Open,
}

/// One note of a sound effect, sliding from `from` to `to` hertz.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Note {
    /// The starting pitch in hertz.
    from: f32,
    /// The ending pitch in hertz.
    to: f32,
    /// The length in seconds.
    seconds: f32,
    /// The shape.
    wave: Wave,
    /// The loudness from 0 to 1.
    volume: f32,
}

/// Returns a note of steady pitch.
fn note(hz: f32, seconds: f32, wave: Wave, volume: f32) -> Note {
    Note {
        from: hz,
        to: hz,
        seconds,
        wave,
        volume,
    }
}

/// Returns a note that slides in pitch.
fn slide(from: f32, to: f32, seconds: f32, wave: Wave, volume: f32) -> Note {
    Note {
        from,
        to,
        seconds,
        wave,
        volume,
    }
}

/// Returns the frequency of a MIDI note number.
pub fn midi(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}

/// A cheap white noise generator.
#[derive(Debug, Clone)]
pub struct Noise {
    /// The xorshift state.
    state: u32,
}

impl Noise {
    /// Creates a generator from `seed`.
    pub fn new(seed: u32) -> Self {
        Self { state: seed.max(1) }
    }

    /// Returns the next sample in `-1.0..1.0`.
    pub fn next_sample(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

/// Returns the value of `wave` at `phase`, which runs from 0 to 1 per cycle.
pub fn oscillate(wave: Wave, phase: f32, noise: &mut Noise) -> f32 {
    match wave {
        Wave::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        Wave::Pulse => {
            if phase < 0.125 {
                1.0
            } else {
                -1.0
            }
        }
        Wave::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
        Wave::Noise => noise.next_sample(),
    }
}

/// Returns the loudness envelope at `t` seconds into a note `length` seconds long.
///
/// A quick attack avoids clicks and the release fades the tail out.
pub fn envelope(t: f32, length: f32) -> f32 {
    let attack = 0.004f32.min(length / 4.0);
    let release = (length * 0.6).min(0.12);
    if t < attack {
        t / attack
    } else if t > length - release {
        ((length - t) / release).max(0.0)
    } else {
        1.0
    }
}

/// Renders `notes` one after another into samples.
fn render(notes: &[Note], noise: &mut Noise) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let mut samples = Vec::new();
    for note in notes {
        let count = (note.seconds * rate) as usize;
        let mut phase = 0.0f32;
        for i in 0..count {
            let t = i as f32 / rate;
            let progress = t / note.seconds;
            let hz = note.from + (note.to - note.from) * progress;
            phase = (phase + hz / rate).fract();
            let value = oscillate(note.wave, phase, noise);
            samples.push(value * envelope(t, note.seconds) * note.volume);
        }
    }
    samples
}

/// Renders a sound effect at full volume. `seed` varies the little random bits.
pub fn render_sfx(sfx: Sfx, seed: u32) -> Vec<f32> {
    let mut noise = Noise::new(seed);
    // a little pitch wobble keeps typing from sounding like a machine gun
    let wobble = 1.0 + (seed % 7) as f32 * 0.03;
    let notes = match sfx {
        Sfx::Key => vec![
            note(0.0, 0.006, Wave::Noise, 0.35),
            note(1900.0 * wobble, 0.012, Wave::Pulse, 0.12),
        ],
        Sfx::Delete => vec![slide(700.0 * wobble, 400.0, 0.03, Wave::Pulse, 0.15)],
        Sfx::Save => [72, 76, 79, 84]
            .iter()
            .map(|&n| note(midi(n), 0.07, Wave::Triangle, 0.5))
            .collect(),
        Sfx::Error => vec![
            slide(311.0, 293.0, 0.22, Wave::Square, 0.18),
            slide(293.0, 277.0, 0.22, Wave::Square, 0.18),
            slide(277.0, 196.0, 0.6, Wave::Square, 0.18),
        ],
        Sfx::Fixed => [67, 72, 76, 79, 84]
            .iter()
            .map(|&n| note(midi(n), 0.08, Wave::Square, 0.15))
            .chain([note(midi(88), 0.3, Wave::Triangle, 0.5)])
            .collect(),
        Sfx::Open => vec![
            slide(0.0, 0.0, 0.12, Wave::Noise, 0.12),
            slide(500.0, 1500.0, 0.08, Wave::Triangle, 0.2),
        ],
    };
    render(&notes, &mut noise)
}

/// Converts a sine phase to a sample, used for the soft music pad.
pub fn sine(phase: f32) -> f32 {
    (phase * TAU).sin()
}

#[cfg(test)]
/// Tests for synthesis.
mod tests {
    use super::{SAMPLE_RATE, Sfx, envelope, midi, render_sfx};

    /// A4 is 440 hertz and octaves double.
    #[test]
    fn midi_pitches() {
        assert!((midi(69) - 440.0).abs() < 0.01);
        assert!((midi(81) - 880.0).abs() < 0.01);
    }

    /// Envelopes start and end silent and peak at one.
    #[test]
    fn envelope_shape() {
        assert_eq!(envelope(0.0, 1.0), 0.0);
        assert_eq!(envelope(0.5, 1.0), 1.0);
        assert!(envelope(1.0, 1.0) < 0.01);
    }

    /// Every effect renders a short, bounded clip.
    #[test]
    fn effects_are_short_and_bounded() {
        for sfx in [
            Sfx::Key,
            Sfx::Delete,
            Sfx::Save,
            Sfx::Error,
            Sfx::Fixed,
            Sfx::Open,
        ] {
            let samples = render_sfx(sfx, 3);
            assert!(!samples.is_empty(), "{sfx:?}");
            assert!(samples.len() < SAMPLE_RATE as usize * 2, "{sfx:?}");
            assert!(samples.iter().all(|s| s.abs() <= 1.0), "{sfx:?}");
        }
    }
}
