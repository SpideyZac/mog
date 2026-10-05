//! Sound effects, music and Discord presence.

use mog_audio::{Audio, Mood, Sfx};
use mog_tui::UiEvent;

use super::App;
use crate::discord::{Presence, Status};

impl App {
    /// Starts, stops or adjusts sound to match the settings.
    pub(super) fn apply_audio_settings(&mut self) {
        let settings = &self.ui.config.audio;
        let serious = self.ui.config.ui.serious;
        let wanted = (settings.sound_effects || settings.music) && !serious;
        if wanted && self.audio.is_none() {
            self.audio = Some(Audio::start());
        }
        if let Some(audio) = &self.audio {
            audio.set_volume(settings.volume);
            audio.set_music(settings.music && !serious);
        }
    }

    /// Connects to or leaves Discord to match the settings.
    pub(super) fn apply_discord_settings(&mut self) {
        let settings = &self.ui.config.discord;
        if !settings.enabled {
            self.discord = None;
            return;
        }
        if settings.client_id.is_empty() {
            self.editor
                .set_status("discord needs [discord] client_id in the config, see the example");
            self.discord = None;
            return;
        }
        if self
            .discord
            .as_ref()
            .is_none_or(|presence| presence.client_id() != settings.client_id)
        {
            self.discord = Some(Presence::start(&settings.client_id, &settings.large_image));
            self.discord_status = None;
        }
        self.update_presence();
    }

    /// Tells Discord about the focused file if it changed.
    pub(super) fn update_presence(&mut self) {
        let Some(presence) = &self.discord else {
            return;
        };
        let document = self.editor.document();
        let details = if !self.ui.config.discord.show_file {
            "mogging something secret".to_owned()
        } else if document.path().is_some() {
            format!("mogging {}", document.name())
        } else {
            "mogging a blank file".to_owned()
        };
        let project = self
            .ui
            .root
            .file_name()
            .map_or_else(|| "mog".into(), |name| name.to_string_lossy());
        let state = match self.last_problems.0 {
            0 => format!("in {project}"),
            1 => format!("in {project}, 1 error"),
            errors => format!("in {project}, {errors} errors"),
        };
        let status = Status { details, state };
        if self.discord_status.as_ref() != Some(&status) {
            presence.update(status.clone());
            self.discord_status = Some(status);
        }
    }

    /// Plays sounds for what happened since the last frame.
    pub(super) fn play_sounds(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };
        let effects = self.ui.config.audio.sound_effects && !self.ui.config.ui.serious;
        for event in &self.ui.events {
            let sfx = match event {
                UiEvent::Typed(_) => Some(Sfx::Key),
                UiEvent::Deleted => Some(Sfx::Delete),
                UiEvent::Saved => Some(Sfx::Save),
                UiEvent::Opened => Some(Sfx::Open),
                UiEvent::Diagnostics { errors, .. } => {
                    let sound = match (self.sound_errors, *errors) {
                        (0, n) if n > 0 => Some(Sfx::Error),
                        (n, 0) if n > 0 => Some(Sfx::Fixed),
                        _ => None,
                    };
                    self.sound_errors = *errors;
                    let mood = if *errors > 0 { Mood::Tense } else { Mood::Calm };
                    audio.set_mood(mood);
                    sound
                }
                UiEvent::Activity => None,
            };
            if let Some(sfx) = sfx.filter(|_| effects) {
                audio.play(sfx);
            }
        }
    }
}
