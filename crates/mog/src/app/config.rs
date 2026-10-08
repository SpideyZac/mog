//! Changing, reloading and opening the config, key bindings and themes.

use std::{fs, mem, path::Path};

use mog_config::{
    Config, SettingValue, ThemeConfig, config_path, project_config_path, save_setting,
    theme::COLOR_NAMES,
};
use mog_core::{Command, KeyChord};
use mog_tui::{
    Focus, Overlay, PromptKind, SidebarSide, Theme,
    settings::{SettingKey, change as settings_change, persisted},
    theme::to_hex,
};

use super::{App, NEW_CONFIG, NEW_PROJECT_CONFIG};
use crate::{
    ai::{Assistant, Exclusions},
    settings::{self},
};

impl App {
    /// Changes a setting by one step as if it was picked in the settings menu.
    pub(super) fn change_setting(&mut self, key: SettingKey) {
        settings_change(&mut self.ui.config, &key, 1);
        self.ui.setting_changes.push(key);
        self.apply_setting_changes();
    }

    /// Applies the key binding change the key list asked for and saves it to the config.
    ///
    /// The new chord replaces every chord the command had, and takes the chord away from
    /// whatever used it before.
    pub(super) fn apply_rebind(&mut self) {
        let Some((name, chord)) = self.ui.rebind.take() else {
            return;
        };
        let Ok(command) = name.parse::<Command>() else {
            return;
        };
        let chord = match chord.map(|chord| chord.parse::<KeyChord>()).transpose() {
            Ok(chord) => chord,
            Err(err) => {
                self.editor.set_status(err.to_string());
                return;
            }
        };
        let mut changes: Vec<(String, String)> = self
            .keymap
            .chords_for(&command)
            .into_iter()
            .filter(|old| Some(*old) != chord)
            .map(|old| (old.to_string(), String::new()))
            .collect();
        let title = self.ui.title_of(&name).to_owned();
        let message = match chord {
            Some(chord) => {
                changes.push((chord.to_string(), name.clone()));
                let taken = self
                    .keymap
                    .resolve(&chord)
                    .filter(|other| *other != command && chord.typed_char().is_none());
                match taken {
                    Some(other) => format!(
                        "{chord} now runs {title}, it was {}",
                        self.ui.title_of(&other.to_string())
                    ),
                    None => format!("{chord} now runs {title}"),
                }
            }
            None => format!("{title} has no keys now"),
        };
        for (chord, command) in changes {
            if let Err(err) = save_setting(&["keys", &chord], &SettingValue::Text(command.clone()))
            {
                self.editor
                    .set_status(format!("could not save the binding: {err}"));
                return;
            }
            self.ui.config.keys.insert(chord, command);
        }
        let (keymap, problems) = settings::keymap(&self.ui.config);
        self.keymap = keymap;
        self.refresh_commands();
        // reopening refills the list with the new keys
        self.ui.open(Overlay::Keys);
        self.editor.set_status(if problems.is_empty() {
            message
        } else {
            problems.join("; ")
        });
    }

    /// Saves and applies settings changed in the settings menu.
    pub(super) fn apply_setting_changes(&mut self) {
        let changes = mem::take(&mut self.ui.setting_changes);
        if changes.is_empty() {
            return;
        }
        for key in &changes {
            match key {
                SettingKey::Ui("sidebar") => self.ui.sidebar.open = self.ui.config.ui.sidebar,
                SettingKey::Ui("sidebar_right") => {
                    self.ui.sidebar.side = if self.ui.config.ui.sidebar_right {
                        SidebarSide::Right
                    } else {
                        SidebarSide::Left
                    };
                }
                _ => {}
            }
            let (path, value) = persisted(&self.ui.config, key);
            if let Err(err) = save_setting(&path, &value) {
                self.editor
                    .set_status(format!("could not save setting: {err}"));
            }
        }
        self.apply_config();
    }

    /// Applies the theme, editing options, sound and Discord from the live config.
    pub(super) fn apply_config(&mut self) {
        let (theme, problem) = settings::theme(&self.ui.config);
        self.theme = theme;
        if let Some(problem) = problem {
            self.editor.set_status(problem);
        }
        self.editor.set_options(settings::options(&self.ui.config));
        self.apply_audio_settings();
        self.apply_discord_settings();
        self.plugin_config_changed();
    }

    /// Reads the config file again and applies everything in it.
    ///
    /// A broken file is reported and the current config is kept.
    pub(super) fn reload_config(&mut self) {
        let mut config = match Config::load() {
            Ok(config) => config,
            Err(err) => {
                self.editor
                    .set_status(format!("config not reloaded: {err}"));
                return;
            }
        };
        let project = settings::apply_project(&mut config, &self.ui.root);
        let (keymap, mut problems) = settings::keymap(&config);
        // restarting the ai drops the copilot server, so only do it when its settings changed
        if config.ai != self.ui.config.ai {
            let (providers, ai_problems) = settings::ai_providers(&config, &self.ui.root);
            problems.extend(ai_problems);
            let exclusions = Exclusions::new(&self.ui.root, &config.ai.exclude);
            self.assistant = Assistant::new(providers, exclusions);
            self.ui.copilot = None;
        }
        self.keymap = keymap;
        self.refresh_commands();
        self.lsp.reconfigure(config.language_servers());
        self.ui.config = config;
        self.editor.set_status("config reloaded");
        problems.extend(self.plugins.configure(&self.ui.config.plugins));
        problems.extend(self.apply_plugin_contributions());
        self.refresh_commands();
        self.apply_config();
        if !problems.is_empty() {
            self.editor.set_status(problems.join("; "));
        }
        self.project_status(project);
    }

    /// Opens the project config in the editor, creating it first if there is none.
    pub(super) fn open_project_config(&mut self) {
        let path = project_config_path(&self.ui.root);
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::write(&path, NEW_PROJECT_CONFIG));
            if let Err(err) = created {
                self.editor
                    .set_status(format!("could not create {}: {err}", path.display()));
                return;
            }
        }
        self.ui.close();
        self.ui.focus = Focus::Editor;
        match self.editor.open(&path) {
            Ok(()) => self
                .editor
                .set_status("language server settings for this project only, saving applies them"),
            Err(err) => self
                .editor
                .set_status(format!("could not open {}: {err}", path.display())),
        }
    }

    /// Opens the config file in the editor, creating it first if there is none.
    pub(super) fn open_config(&mut self) {
        let Some(path) = config_path() else {
            self.editor
                .set_status("there is no config folder on this system");
            return;
        };
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::write(&path, NEW_CONFIG));
            if let Err(err) = created {
                self.editor
                    .set_status(format!("could not create {}: {err}", path.display()));
                return;
            }
        }
        self.ui.close();
        self.ui.focus = Focus::Editor;
        match self.editor.open(&path) {
            Ok(()) => self
                .editor
                .set_status("saving the config reloads it, or run Settings: Reload config"),
            Err(err) => self
                .editor
                .set_status(format!("could not open {}: {err}", path.display())),
        }
    }

    /// Returns `true` if `path` is the config file.
    pub(super) fn is_config(path: &Path) -> bool {
        let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        config_path().is_some_and(|config| canonical(&config) == canonical(path))
    }

    /// Saves the theme being edited as `[themes.<name>]` and switches to it.
    pub(super) fn save_theme(&mut self, name: &str) {
        let Some(draft) = self.ui.theme_draft.take() else {
            return;
        };
        let mut custom = ThemeConfig::default();
        for (index, color_name) in COLOR_NAMES.iter().enumerate() {
            let hex = to_hex(draft.palette.get(index));
            let saved = save_setting(
                &["themes", name, color_name],
                &SettingValue::Text(hex.clone()),
            );
            if let Err(err) = saved {
                self.editor
                    .set_status(format!("could not save the theme: {err}"));
                self.apply_config();
                return;
            }
            if let Some(slot) = custom.color_mut(color_name) {
                *slot = Some(hex);
            }
        }
        self.ui.config.themes.insert(name.to_owned(), custom);
        self.ui.config.ui.theme = name.to_owned();
        self.ui.setting_changes.push(SettingKey::Theme);
        self.apply_setting_changes();
        self.editor
            .set_status(format!("saved theme {name}, it mogs"));
    }

    /// Previews the theme being edited, or puts the saved theme back once editing stopped.
    pub(super) fn sync_theme_draft(&mut self) {
        let editing = self.ui.overlay == Some(Overlay::ThemeEditor)
            || self
                .ui
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.kind == PromptKind::SaveTheme);
        match &self.ui.theme_draft {
            Some(draft) if editing => {
                if self.theme.palette != draft.palette {
                    self.theme = Theme::from_palette(&draft.name, draft.palette);
                }
            }
            Some(_) => {
                self.ui.theme_draft = None;
                self.apply_config();
            }
            None => {}
        }
    }
}
