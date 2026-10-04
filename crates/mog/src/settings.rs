//! Turning the loaded config into editor state.

use std::{env, path::Path, sync::Arc};

use mog_ai::{AiProvider, Claude, Copilot, CopilotEvent, claude};
use mog_config::Config;
use mog_core::{Command, KeyChord, Keymap, Options};
use mog_flair::{FlairLayer, builtin};
use mog_tui::Theme;
use tokio::sync::mpsc::UnboundedReceiver;

/// Builds the editing options from `config`.
pub fn options(config: &Config) -> Options {
    Options {
        tab_width: config.editor.tab_width.max(1),
        insert_spaces: config.editor.insert_spaces,
        auto_close: config.editor.auto_close_brackets,
    }
}

/// Builds the keymap from the defaults plus the overrides in `config`.
///
/// Bad entries are skipped and described in the returned list of problems.
pub fn keymap(config: &Config) -> (Keymap, Vec<String>) {
    let mut keymap = Keymap::default();
    let mut problems = Vec::new();
    for (chord, command) in &config.keys {
        let chord = match chord.parse::<KeyChord>() {
            Ok(chord) => chord,
            Err(err) => {
                problems.push(err.to_string());
                continue;
            }
        };
        if command.is_empty() {
            keymap.unbind(&chord);
            continue;
        }
        match command.parse::<Command>() {
            Ok(command) => keymap.bind(chord, command),
            Err(err) => problems.push(err.to_string()),
        }
    }
    (keymap, problems)
}

/// Builds the flair layer with every built in flair, minus the ones `config` turns off.
pub fn flair_layer(config: &Config) -> FlairLayer {
    let mut layer = FlairLayer::new();
    for flair in builtin::all() {
        layer.register(flair);
    }
    layer.set_hidden(!config.flair.enabled);
    for id in &config.flair.disabled {
        layer.disable(id.clone());
    }
    layer
}

/// The enabled AI providers, split by what each one is allowed to do.
#[derive(Default)]
pub struct AiProviders {
    /// Providers that answer in the chat panel, preferred first.
    pub chat: Vec<Arc<dyn AiProvider>>,
    /// Providers that suggest ghost text, preferred first.
    pub ghost: Vec<Arc<dyn AiProvider>>,
    /// Copilot again, for signing in and out.
    pub copilot: Option<Arc<Copilot>>,
    /// What the Copilot server reports, like sign in changes.
    pub copilot_events: Option<UnboundedReceiver<CopilotEvent>>,
}

/// Builds the enabled AI providers for the project at `root`, Claude first for chat and Copilot
/// first for ghost text.
///
/// Providers that are enabled but cannot work, like Claude without a key, are skipped and
/// described in the returned list of problems.
pub fn ai_providers(config: &Config, root: &Path) -> (AiProviders, Vec<String>) {
    let mut providers = AiProviders::default();
    let mut problems = Vec::new();
    let claude_config = &config.ai.claude;
    if claude_config.enabled {
        match env::var(&claude_config.api_key_env) {
            Ok(key) if !key.is_empty() => {
                let model = claude_config
                    .model
                    .clone()
                    .unwrap_or_else(|| claude::DEFAULT_MODEL.into());
                let claude: Arc<dyn AiProvider> = Arc::new(Claude::new(key, model));
                if claude_config.chat {
                    providers.chat.push(Arc::clone(&claude));
                }
                if claude_config.ghost_text {
                    providers.ghost.push(claude);
                }
            }
            _ => problems.push(format!(
                "claude is enabled but {} is not set",
                claude_config.api_key_env
            )),
        }
    }
    let copilot_config = &config.ai.copilot;
    if copilot_config.enabled {
        match Copilot::start(&copilot_config.command, &copilot_config.args, root) {
            Ok((copilot, events)) => {
                let copilot = Arc::new(copilot);
                if copilot_config.chat {
                    providers
                        .chat
                        .push(Arc::clone(&copilot) as Arc<dyn AiProvider>);
                }
                if copilot_config.ghost_text {
                    // copilot is built for inline suggestions so it goes ahead of claude
                    providers
                        .ghost
                        .insert(0, Arc::clone(&copilot) as Arc<dyn AiProvider>);
                }
                providers.copilot = Some(copilot);
                providers.copilot_events = Some(events);
            }
            Err(err) => problems.push(format!(
                "copilot could not start `{}` ({err}), install it with npm i -g \
                 @github/copilot-language-server",
                copilot_config.command
            )),
        }
    }
    (providers, problems)
}

/// Builds the theme named in `config`, falling back to the default with a problem message.
pub fn theme(config: &Config) -> (Theme, Option<String>) {
    match Theme::named(&config.ui.theme) {
        Some(theme) => (theme, None),
        None => (
            Theme::default(),
            Some(format!(
                "unknown theme `{}`, try one of: {}",
                config.ui.theme,
                Theme::names().collect::<Vec<_>>().join(", ")
            )),
        ),
    }
}
