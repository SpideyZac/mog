//! Writing single settings back to the config file.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use toml_edit::{Array, DocumentMut, Item, table, value};

use crate::{CONFIG_FILE, ConfigError, config_dir};

/// A value that can be written to the config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    /// A boolean.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A string.
    Text(String),
    /// A list of strings.
    List(Vec<String>),
}

impl SettingValue {
    /// Converts the value to a TOML item.
    fn to_item(&self) -> Item {
        match self {
            Self::Bool(flag) => value(*flag),
            Self::Int(n) => value(*n),
            Self::Text(text) => value(text.as_str()),
            Self::List(items) => value(items.iter().map(String::as_str).collect::<Array>()),
        }
    }
}

/// Returns the path of the config file, or `None` if there is no config directory.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(CONFIG_FILE))
}

/// Sets the setting at `keys`, like `["ui", "theme"]`, in the TOML `text`.
///
/// Comments and formatting of everything else are kept.
///
/// # Errors
///
/// Returns an error if `text` is not valid TOML.
pub fn set_in(text: &str, keys: &[&str], setting: &SettingValue) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|err| format!("{err}"))?;
    let Some((last, tables)) = keys.split_last() else {
        return Ok(text.to_owned());
    };
    let mut item = doc.as_item_mut();
    for key in tables {
        item = &mut item[key];
        if item.is_none() {
            *item = table();
        }
    }
    item[last] = setting.to_item();
    Ok(doc.to_string())
}

/// Sets the setting at `keys` in the config file at `path`, creating the file if needed.
///
/// # Errors
///
/// Returns an error if the file cannot be read, parsed or written.
pub fn save_setting_at(
    path: &Path,
    keys: &[&str],
    setting: &SettingValue,
) -> Result<(), ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let updated = set_in(&text, keys, setting).map_err(|message| ConfigError::Edit {
        path: path.to_owned(),
        message,
    })?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|source| ConfigError::Write {
            path: path.to_owned(),
            source,
        })?;
    }
    fs::write(path, updated).map_err(|source| ConfigError::Write {
        path: path.to_owned(),
        source,
    })
}

/// Sets the setting at `keys` in the config file in [`config_dir`].
///
/// # Errors
///
/// Returns an error if there is no config directory or the file cannot be updated.
pub fn save_setting(keys: &[&str], setting: &SettingValue) -> Result<(), ConfigError> {
    let path = config_path().ok_or(ConfigError::NoConfigDir)?;
    save_setting_at(&path, keys, setting)
}

#[cfg(test)]
/// Tests for saving settings.
mod tests {
    use super::{SettingValue, set_in};

    /// Setting a value keeps comments and other keys.
    #[test]
    fn keeps_comments() {
        let text = "# hi\n[ui]\ntheme = \"mog\" # cool\ntabs = true\n";
        let out = set_in(text, &["ui", "tabs"], &SettingValue::Bool(false)).expect("valid");
        assert_eq!(out, "# hi\n[ui]\ntheme = \"mog\" # cool\ntabs = false\n");
    }

    /// Missing tables are created.
    #[test]
    fn creates_tables() {
        let out = set_in("", &["audio", "volume"], &SettingValue::Int(30)).expect("valid");
        assert_eq!(out, "[audio]\nvolume = 30\n");
        let list = SettingValue::List(vec!["badge".into()]);
        let out = set_in(&out, &["flair", "disabled"], &list).expect("valid");
        assert!(out.contains("[flair]\ndisabled = [\"badge\"]\n"));
    }
}
