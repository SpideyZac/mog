//! Turning the loaded config into editor state.

use std::{env, sync::Arc};

use mog_ai::{AiProvider, Claude, Copilot, claude};
use mog_config::Config;
use mog_core::{Command, KeyChord, Keymap, Options};
use mog_flair::{FlairLayer, builtin};

/// Builds the editing options from `config`.
pub fn options(config: &Config) -> Options {
    Options {
        tab_width: config.editor.tab_width.max(1),
        insert_spaces: config.editor.insert_spaces,
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

/// Builds the enabled AI providers, Claude first.
///
/// Providers that are enabled but cannot work, like Claude without a key, are skipped and
/// described in the returned list of problems.
pub fn ai_providers(config: &Config) -> (Vec<Arc<dyn AiProvider>>, Vec<String>) {
    let mut providers: Vec<Arc<dyn AiProvider>> = Vec::new();
    let mut problems = Vec::new();
    let claude_config = &config.ai.claude;
    if claude_config.enabled {
        match env::var(&claude_config.api_key_env) {
            Ok(key) if !key.is_empty() => {
                let model = claude_config
                    .model
                    .clone()
                    .unwrap_or_else(|| claude::DEFAULT_MODEL.into());
                providers.push(Arc::new(Claude::new(key, model)));
            }
            _ => problems.push(format!(
                "claude is enabled but {} is not set",
                claude_config.api_key_env
            )),
        }
    }
    if config.ai.copilot.enabled {
        providers.push(Arc::new(Copilot::new()));
    }
    (providers, problems)
}
