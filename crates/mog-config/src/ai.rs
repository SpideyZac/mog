//! AI provider settings.

use serde::Deserialize;

/// Settings for every AI provider.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    /// Whether the AI suggests code in gray after the cursor when typing pauses.
    pub ghost_text: bool,
    /// Claude settings.
    pub claude: ClaudeConfig,
    /// GitHub Copilot settings.
    pub copilot: CopilotConfig,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            ghost_text: true,
            claude: ClaudeConfig::default(),
            copilot: CopilotConfig::default(),
        }
    }
}

/// Settings for Claude.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    /// Whether Claude is used at all.
    pub enabled: bool,
    /// The model id, or the provider default when unset.
    pub model: Option<String>,
    /// The environment variable holding the API key. Keys are never stored in the config.
    pub api_key_env: String,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: None,
            api_key_env: "ANTHROPIC_API_KEY".into(),
        }
    }
}

/// Settings for GitHub Copilot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CopilotConfig {
    /// Whether Copilot is used at all.
    pub enabled: bool,
}
