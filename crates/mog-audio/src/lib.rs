//! Sound effects and music for mog.
//!
//! Everything is synthesized on the fly so there are no audio files to ship. Playback runs on its
//! own thread because audio devices are not always safe to move between threads. If there is no
//! audio device, every call quietly does nothing.

pub mod music;
pub mod synth;

use std::{
    num::NonZero,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

pub use music::Mood;
use rodio::{DeviceSinkBuilder, buffer::SamplesBuffer};
pub use synth::Sfx;

use crate::{
    music::{Music, MusicControl},
    synth::{SAMPLE_RATE, render_sfx},
};

/// A message to the audio thread.
enum Message {
    /// Play a sound effect.
    Play(Sfx),
    /// Start the music if it is not playing yet.
    StartMusic,
}

/// A handle to the audio thread.
#[derive(Debug)]
pub struct Audio {
    /// Where messages for the audio thread go.
    sender: Sender<Message>,
    /// The music settings, shared with the music source.
    music: Arc<MusicControl>,
    /// The sound effect volume from 0 to 100, shared with the audio thread.
    volume: Arc<AtomicU8>,
}

impl Audio {
    /// Starts the audio thread. Never fails: without a device, sounds are just dropped.
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::channel();
        let music = Arc::new(MusicControl::default());
        let volume = Arc::new(AtomicU8::new(50));
        let thread_music = Arc::clone(&music);
        let thread_volume = Arc::clone(&volume);
        let _ = thread::Builder::new()
            .name("mog-audio".into())
            .spawn(move || run(&receiver, &thread_music, &thread_volume));
        Self {
            sender,
            music,
            volume,
        }
    }

    /// Plays a sound effect.
    pub fn play(&self, sfx: Sfx) {
        let _ = self.sender.send(Message::Play(sfx));
    }

    /// Turns the music on or off. It fades instead of cutting.
    pub fn set_music(&self, enabled: bool) {
        self.music.enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            let _ = self.sender.send(Message::StartMusic);
        }
    }

    /// Sets the mood of the music.
    pub fn set_mood(&self, mood: Mood) {
        self.music.mood.store(mood as u8, Ordering::Relaxed);
    }

    /// Sets the volume of everything from 0 to 100.
    pub fn set_volume(&self, volume: u8) {
        let volume = volume.min(100);
        self.volume.store(volume, Ordering::Relaxed);
        self.music.volume.store(volume, Ordering::Relaxed);
    }
}

/// The audio thread: opens the device and plays whatever it is told to.
fn run(receiver: &Receiver<Message>, music: &Arc<MusicControl>, volume: &AtomicU8) {
    let Ok(mut sink) = DeviceSinkBuilder::open_default_sink() else {
        // no device, so drain messages until the editor quits
        while receiver.recv().is_ok() {}
        return;
    };
    // the default would print to stderr on drop, right over the terminal
    sink.log_on_drop(false);
    let mut music_started = false;
    let mut seed = 1u32;
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Play(sfx) => {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let gain = f32::from(volume.load(Ordering::Relaxed)) / 100.0;
                let samples: Vec<f32> = render_sfx(sfx, seed)
                    .into_iter()
                    .map(|s| s * gain)
                    .collect();
                let rate = NonZero::new(SAMPLE_RATE).unwrap_or(NonZero::<u32>::MIN);
                sink.mixer()
                    .add(SamplesBuffer::new(NonZero::<u16>::MIN, rate, samples));
            }
            Message::StartMusic if !music_started => {
                music_started = true;
                sink.mixer().add(Music::new(Arc::clone(music)));
            }
            Message::StartMusic => {}
        }
    }
}
