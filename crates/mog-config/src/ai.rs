//! AI provider settings.

use serde::Deserialize;

/// Files that usually hold secrets, kept away from every AI unless the config says otherwise.
pub const DEFAULT_EXCLUDE: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "*.tfvars",
    "secrets.*",
    "credentials*",
];

/// Settings for every AI provider.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    /// Whether Copilot suggests code in gray after the cursor when typing pauses.
    pub ghost_text: bool,
    /// Files never sent to an AI, as gitignore style patterns relative to the project.
    pub exclude: Vec<String>,
    /// Claude settings.
    pub claude: ClaudeConfig,
    /// GitHub Copilot settings.
    pub copilot: CopilotConfig,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            ghost_text: true,
            exclude: DEFAULT_EXCLUDE
                .iter()
                .map(|&pattern| pattern.to_owned())
                .collect(),
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
    /// Whether Claude answers in the chat panel.
    pub chat: bool,
    /// Ignored, Claude only chats. Kept so configs from before 1.0 still load.
    #[doc(hidden)]
    pub ghost_text: Option<bool>,
    /// The model id, or the provider default when unset.
    pub model: Option<String>,
    /// The environment variable holding the API key. Keys are never stored in the config.
    pub api_key_env: String,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            chat: true,
            ghost_text: None,
            model: None,
            api_key_env: "ANTHROPIC_API_KEY".into(),
        }
    }
}

/// Settings for GitHub Copilot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CopilotConfig {
    /// Whether Copilot is used at all.
    pub enabled: bool,
    /// Ignored, Copilot only suggests ghost text. Kept so configs from before 1.0 still load.
    #[doc(hidden)]
    pub chat: Option<bool>,
    /// Whether Copilot suggests ghost text.
    pub ghost_text: bool,
    /// The Copilot language server program, from `npm i -g @github/copilot-language-server`.
    pub command: String,
    /// The arguments to pass to it.
    pub args: Vec<String>,
}

impl Default for CopilotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            chat: None,
            ghost_text: true,
            command: "copilot-language-server".into(),
            args: vec!["--stdio".into()],
        }
    }
}
