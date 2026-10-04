//! Settings for sound effects and music.

use serde::Deserialize;

/// Settings for everything that makes noise.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioConfig {
    /// Whether typing, saving and errors make sounds.
    pub sound_effects: bool,
    /// Whether background music plays. It gets tense when there are errors.
    pub music: bool,
    /// The volume from 0 to 100.
    pub volume: u8,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sound_effects: false,
            music: false,
            volume: 50,
        }
    }
}
