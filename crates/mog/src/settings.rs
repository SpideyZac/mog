//! Turning the loaded config into editor state.

use mog_config::Config;
use mog_core::{Command, KeyChord, Keymap, Options};

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
