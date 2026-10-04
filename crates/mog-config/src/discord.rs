//! Settings for Discord Rich Presence.

use serde::Deserialize;

/// Settings for showing what you edit on your Discord profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscordConfig {
    /// Whether mog tells Discord what you are doing.
    pub enabled: bool,
    /// The id of the Discord application the status is shown as.
    pub client_id: String,
    /// The art asset key of the big picture, uploaded to the application.
    pub large_image: String,
    /// Whether the file name is shown, turn off to keep it secret.
    pub show_file: bool,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            large_image: "mog".into(),
            show_file: true,
        }
    }
}
