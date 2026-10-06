//! What plugins add without running: themes, key bindings and highlight queries, and the tools
//! they offer the AI chat.

use std::{fs, mem};

use mog_config::ThemeConfig;
use mog_core::{Command, KeyChord};
use mog_syntax::set_extra_highlights;

use crate::{ai::ChatTool, app::App, settings};

/// The most chars a tool name the model sees may have.
const MAX_TOOL_NAME: usize = 64;

/// Returns the name the model calls `tool` of `plugin` by, unique across plugins.
fn tool_name(plugin: &str, tool: &str) -> String {
    let name: String = format!("{plugin}_{tool}")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    name.chars().take(MAX_TOOL_NAME).collect()
}

impl App {
    /// Adds the themes and highlight queries of every plugin, replacing what plugins added
    /// before. Themes in the config win over plugin themes with the same name.
    ///
    /// Returns a message for each file that could not be used.
    pub(in crate::app) fn apply_plugin_contributions(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        // a theme the user changed in the meantime is theirs now, so it stays
        for (name, theme) in mem::take(&mut self.plugin_state.themes) {
            if self.ui.config.themes.get(&name) == Some(&theme) {
                self.ui.config.themes.remove(&name);
            }
        }
        let mut queries = Vec::new();
        for (plugin, contributes) in self.plugins.contributions() {
            for path in &contributes.themes {
                let Some(name) = path
                    .file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
                else {
                    continue;
                };
                if self.ui.config.themes.contains_key(&name) {
                    continue;
                }
                let theme = fs::read_to_string(path)
                    .map_err(|err| err.to_string())
                    .and_then(|text| {
                        toml::from_str::<ThemeConfig>(&text).map_err(|err| err.to_string())
                    });
                match theme {
                    Ok(theme) => {
                        self.ui.config.themes.insert(name.clone(), theme.clone());
                        self.plugin_state.themes.insert(name, theme);
                    }
                    Err(err) => {
                        problems.push(format!("plugin {plugin}: theme {}: {err}", path.display()));
                    }
                }
            }
            for (language, path) in &contributes.highlights {
                match fs::read_to_string(path) {
                    Ok(query) => queries.push((language.clone(), query)),
                    Err(err) => {
                        problems.push(format!("plugin {plugin}: {}: {err}", path.display()));
                    }
                }
            }
        }
        problems.extend(
            set_extra_highlights(&queries)
                .into_iter()
                .map(|problem| format!("plugin {problem}")),
        );
        let (theme, problem) = settings::theme(&self.ui.config);
        self.theme = theme;
        problems.extend(problem);
        problems
    }

    /// Binds the keys plugins suggest, for their commands and in their manifests, where neither
    /// mog nor the user bound them to something else.
    pub(in crate::app) fn bind_plugin_keys(&mut self) {
        let mut wanted: Vec<(String, String)> = self
            .plugins
            .palette()
            .into_iter()
            .flat_map(|(name, _, keys)| keys.into_iter().map(move |key| (key, name.clone())))
            .collect();
        for (_, contributes) in self.plugins.contributions() {
            wanted.extend(contributes.keys);
        }
        for (key, name) in wanted {
            let (Ok(chord), Ok(command)) = (key.parse::<KeyChord>(), name.parse::<Command>())
            else {
                continue;
            };
            let configured = self
                .ui
                .config
                .keys
                .keys()
                .any(|other| other.parse::<KeyChord>().ok() == Some(chord));
            let free = self
                .keymap
                .resolve(&chord)
                .is_none_or(|bound| bound.to_string() == name);
            if free && !configured {
                self.keymap.bind(chord, command);
            }
        }
    }

    /// Returns the tools running plugins offer the AI chat, named for the model.
    pub(in crate::app) fn plugin_chat_tools(&self) -> Vec<ChatTool> {
        self.plugins
            .tools()
            .into_iter()
            .map(|(plugin, tool, handle)| ChatTool {
                name: tool_name(&plugin, &tool.name),
                tool,
                plugin: handle,
            })
            .collect()
    }
}

#[cfg(test)]
/// Tests for plugin contributions.
mod tests {
    use super::tool_name;

    /// Tool names are unique per plugin and only use what the model allows.
    #[test]
    fn names_tools() {
        assert_eq!(tool_name("todo", "count"), "todo_count");
        assert_eq!(tool_name("my plugin", "x.y"), "my_plugin_x_y");
        assert_eq!(tool_name(&"a".repeat(80), "b").len(), 64);
    }
}
