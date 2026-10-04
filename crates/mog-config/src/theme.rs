//! Custom color themes.

use serde::Deserialize;

/// The names of the colors a theme is made of, in the order the theme editor lists them.
pub const COLOR_NAMES: [&str; 16] = [
    "bg", "panel", "raised", "select", "fg", "dim", "accent", "accent2", "red", "orange", "yellow",
    "green", "cyan", "blue", "purple", "pink",
];

/// A theme made in the config, as changes to a built in one.
///
/// Colors are hex strings like `"#ff5ccd"`. Missing colors come from `base`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// The built in theme to start from. Defaults to `mog`.
    pub base: Option<String>,
    /// The editor background.
    pub bg: Option<String>,
    /// A slightly different background for panels.
    pub panel: Option<String>,
    /// The background of the cursor line and hovered things.
    pub raised: Option<String>,
    /// The selection background.
    pub select: Option<String>,
    /// Plain text.
    pub fg: Option<String>,
    /// Quiet text like line numbers and comments.
    pub dim: Option<String>,
    /// The loudest color, used for the badge and focus.
    pub accent: Option<String>,
    /// A second accent that goes with the first.
    pub accent2: Option<String>,
    /// Errors and deletions.
    pub red: Option<String>,
    /// Numbers and constants.
    pub orange: Option<String>,
    /// Warnings and types.
    pub yellow: Option<String>,
    /// Strings and additions.
    pub green: Option<String>,
    /// Functions.
    pub cyan: Option<String>,
    /// Keywords.
    pub blue: Option<String>,
    /// Macros and attributes.
    pub purple: Option<String>,
    /// Highlights and special things.
    pub pink: Option<String>,
}

impl ThemeConfig {
    /// Returns the color called `name`, one of [`COLOR_NAMES`], if it is set.
    pub fn color(&self, name: &str) -> Option<&str> {
        let color = match name {
            "bg" => &self.bg,
            "panel" => &self.panel,
            "raised" => &self.raised,
            "select" => &self.select,
            "fg" => &self.fg,
            "dim" => &self.dim,
            "accent" => &self.accent,
            "accent2" => &self.accent2,
            "red" => &self.red,
            "orange" => &self.orange,
            "yellow" => &self.yellow,
            "green" => &self.green,
            "cyan" => &self.cyan,
            "blue" => &self.blue,
            "purple" => &self.purple,
            "pink" => &self.pink,
            _ => return None,
        };
        color.as_deref()
    }

    /// Returns the color called `name`, one of [`COLOR_NAMES`], to change it.
    pub fn color_mut(&mut self, name: &str) -> Option<&mut Option<String>> {
        Some(match name {
            "bg" => &mut self.bg,
            "panel" => &mut self.panel,
            "raised" => &mut self.raised,
            "select" => &mut self.select,
            "fg" => &mut self.fg,
            "dim" => &mut self.dim,
            "accent" => &mut self.accent,
            "accent2" => &mut self.accent2,
            "red" => &mut self.red,
            "orange" => &mut self.orange,
            "yellow" => &mut self.yellow,
            "green" => &mut self.green,
            "cyan" => &mut self.cyan,
            "blue" => &mut self.blue,
            "purple" => &mut self.purple,
            "pink" => &mut self.pink,
            _ => return None,
        })
    }
}

#[cfg(test)]
/// Tests for custom themes.
mod tests {
    use super::{COLOR_NAMES, ThemeConfig};

    /// Every listed color name can be read back.
    #[test]
    fn every_color_reads() {
        let theme = ThemeConfig {
            pink: Some("#ffffff".into()),
            ..ThemeConfig::default()
        };
        assert_eq!(theme.color("pink"), Some("#ffffff"));
        for name in COLOR_NAMES {
            assert!(name == "pink" || theme.color(name).is_none());
        }
        assert_eq!(theme.color("nope"), None);
    }
}
