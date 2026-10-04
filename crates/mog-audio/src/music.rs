//! Endless procedural chiptune that changes mood with the state of the build.

use std::{
    num::NonZero,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};

use rodio::{ChannelCount, Sample, SampleRate, Source};

use crate::synth::{Noise, SAMPLE_RATE, Wave, envelope, midi, oscillate, sine};

/// The feel of the music.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mood {
    /// Everything compiles. Major key, lazy tempo.
    Calm = 0,
    /// Something is broken. Minor key, faster, with a kick drum.
    Tense = 1,
}

/// A chord progression as MIDI root notes and whether each chord is minor.
type Progression = [(i32, bool); 4];

/// The calm progression, a I V vi IV in C.
const CALM: Progression = [(48, false), (43, false), (45, true), (41, false)];

/// The tense progression, a i VI iv V in A minor.
const TENSE: Progression = [(45, true), (41, false), (38, true), (40, false)];

/// Arpeggio patterns as chord tone indexes, one per sixteenth note.
const ARPEGGIO: [usize; 8] = [0, 1, 2, 3, 2, 1, 0, 2];

/// Settings the music reads while it plays, shared with the controlling thread.
#[derive(Debug, Default)]
pub struct MusicControl {
    /// Whether music is audible.
    pub enabled: AtomicBool,
    /// The current [`Mood`] as a number.
    pub mood: AtomicU8,
    /// The volume from 0 to 100.
    pub volume: AtomicU8,
}

/// An endless chiptune [`Source`].
#[derive(Debug)]
pub struct Music {
    /// What to play and how loud.
    control: Arc<MusicControl>,
    /// Samples played so far.
    sample: u64,
    /// Oscillator phases for bass, arpeggio and pad.
    phases: [f32; 3],
    /// Noise for the drums.
    noise: Noise,
    /// The volume actually used, eased towards the requested one to avoid pops.
    gain: f32,
}

impl Music {
    /// Creates the music, controlled by `control`.
    pub fn new(control: Arc<MusicControl>) -> Self {
        Self {
            control,
            sample: 0,
            phases: [0.0; 3],
            noise: Noise::new(0x1234_5678),
            gain: 0.0,
        }
    }

    /// Returns the next sample of the song.
    fn next_value(&mut self) -> f32 {
        let tense = self.control.mood.load(Ordering::Relaxed) == Mood::Tense as u8;
        let enabled = self.control.enabled.load(Ordering::Relaxed);
        let volume = f32::from(self.control.volume.load(Ordering::Relaxed)) / 100.0;
        let target = if enabled { volume * 0.35 } else { 0.0 };
        self.gain += (target - self.gain) * 0.0005;
        let rate = SAMPLE_RATE as f32;
        let bpm = if tense { 138.0 } else { 92.0 };
        let sixteenth = rate * 60.0 / bpm / 4.0;
        let step = (self.sample as f32 / sixteenth) as u64;
        let into_step = (self.sample as f32 % sixteenth) / rate;
        let step_len = sixteenth / rate;
        let progression = if tense { TENSE } else { CALM };
        // one chord per bar of sixteen sixteenths
        let (root, minor) = progression[((step / 16) % 4) as usize];
        let third = if minor { 3 } else { 4 };
        let tones = [root, root + third, root + 7, root + 12];

        let bass_note = if step % 8 < 4 { root - 12 } else { root - 5 };
        let bass = self.voice(0, midi(bass_note), Wave::Triangle) * 0.5;
        let arp_note = tones[ARPEGGIO[(step % 8) as usize]] + 24;
        let arp_env = envelope(into_step, step_len) * (1.0 - into_step / step_len * 0.5);
        let arp_wave = if tense { Wave::Square } else { Wave::Pulse };
        let arp = self.voice(1, midi(arp_note), arp_wave) * arp_env * 0.18;
        self.phases[2] = (self.phases[2] + midi(tones[1] + 12) / rate).fract();
        let pad = sine(self.phases[2]) * 0.08;

        let beat = step % 4;
        let drum_t = into_step;
        let kick = if tense && beat == 0 {
            let hz = 120.0 - drum_t * 600.0;
            sine((drum_t * hz.max(40.0)).fract()) * (1.0 - drum_t / 0.12).max(0.0) * 0.6
        } else {
            0.0
        };
        let hat = if beat == 2 {
            self.noise.next_sample() * (1.0 - drum_t / 0.03).max(0.0) * 0.12
        } else {
            0.0
        };

        self.sample += 1;
        (bass + arp + pad + kick + hat) * self.gain
    }

    /// Advances oscillator `index` at `hz` and returns its value.
    fn voice(&mut self, index: usize, hz: f32, wave: Wave) -> f32 {
        self.phases[index] = (self.phases[index] + hz / SAMPLE_RATE as f32).fract();
        oscillate(wave, self.phases[index], &mut self.noise)
    }
}

impl Iterator for Music {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        Some(self.next_value().clamp(-1.0, 1.0))
    }
}

impl Source for Music {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        NonZero::<u16>::MIN
    }

    fn sample_rate(&self) -> SampleRate {
        NonZero::new(SAMPLE_RATE).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
/// Tests for the music.
mod tests {
    use std::sync::{Arc, atomic::Ordering};

    use super::{Mood, Music, MusicControl};
    use crate::synth::SAMPLE_RATE;

    /// Music stays in range and fades in when enabled.
    #[test]
    fn bounded_and_fades_in() {
        let control = Arc::new(MusicControl::default());
        control.enabled.store(true, Ordering::Relaxed);
        control.volume.store(100, Ordering::Relaxed);
        control.mood.store(Mood::Tense as u8, Ordering::Relaxed);
        let mut music = Music::new(Arc::clone(&control));
        let samples: Vec<f32> = music.by_ref().take(SAMPLE_RATE as usize).collect();
        assert!(samples.iter().all(|s| s.abs() <= 1.0));
        assert!(
            samples
                .iter()
                .skip(SAMPLE_RATE as usize / 2)
                .any(|s| s.abs() > 0.01)
        );
        control.enabled.store(false, Ordering::Relaxed);
        let quiet: Vec<f32> = music.take(SAMPLE_RATE as usize * 2).collect();
        assert!(quiet.iter().rev().take(100).all(|s| s.abs() < 0.01));
    }
}
