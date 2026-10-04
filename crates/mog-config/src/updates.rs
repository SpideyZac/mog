//! Settings for updating mog from GitHub releases.

use serde::Deserialize;

/// Settings for finding and installing new versions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    /// Whether mog looks for a new release when it starts.
    pub check: bool,
    /// Whether a new release is downloaded and installed by itself, ready on the next start.
    pub install: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check: true,
            install: true,
        }
    }
}
