//! Colors and styles.

use mog_git::FileStatus;
use ratatui::style::{Color, Modifier, Style};

/// The number of colors nested brackets cycle through.
pub const RAINBOW_LEN: usize = 6;

/// The handful of colors a theme is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The editor background.
    pub bg: Color,
    /// A slightly different background for panels.
    pub panel: Color,
    /// The background of the cursor line and hovered things.
    pub raised: Color,
    /// The selection background.
    pub select: Color,
    /// Plain text.
    pub fg: Color,
    /// Quiet text like line numbers and comments.
    pub dim: Color,
    /// The loudest color, used for the badge and focus.
    pub accent: Color,
    /// A second accent that goes with the first.
    pub accent2: Color,
    /// Errors and deletions.
    pub red: Color,
    /// Numbers and constants.
    pub orange: Color,
    /// Warnings and types.
    pub yellow: Color,
    /// Strings and additions.
    pub green: Color,
    /// Functions.
    pub cyan: Color,
    /// Keywords.
    pub blue: Color,
    /// Macros and attributes.
    pub purple: Color,
    /// Highlights and special things.
    pub pink: Color,
}

/// The built in palettes by name.
const PALETTES: &[(&str, Palette)] = &[
    (
        "mog",
        Palette {
            bg: Color::Rgb(22, 18, 32),
            panel: Color::Rgb(27, 22, 39),
            raised: Color::Rgb(36, 29, 52),
            select: Color::Rgb(74, 52, 120),
            fg: Color::Rgb(220, 214, 240),
            dim: Color::Rgb(98, 88, 128),
            accent: Color::Rgb(255, 92, 205),
            accent2: Color::Rgb(120, 230, 200),
            red: Color::Rgb(255, 85, 110),
            orange: Color::Rgb(255, 160, 100),
            yellow: Color::Rgb(255, 210, 110),
            green: Color::Rgb(150, 235, 140),
            cyan: Color::Rgb(110, 210, 255),
            blue: Color::Rgb(170, 140, 255),
            purple: Color::Rgb(215, 130, 255),
            pink: Color::Rgb(255, 120, 190),
        },
    ),
    (
        "synthwave",
        Palette {
            bg: Color::Rgb(38, 29, 53),
            panel: Color::Rgb(30, 22, 44),
            raised: Color::Rgb(52, 39, 72),
            select: Color::Rgb(90, 50, 110),
            fg: Color::Rgb(246, 238, 255),
            dim: Color::Rgb(132, 110, 160),
            accent: Color::Rgb(255, 126, 219),
            accent2: Color::Rgb(54, 249, 246),
            red: Color::Rgb(254, 68, 80),
            orange: Color::Rgb(255, 139, 57),
            yellow: Color::Rgb(254, 222, 93),
            green: Color::Rgb(114, 241, 184),
            cyan: Color::Rgb(54, 249, 246),
            blue: Color::Rgb(122, 162, 247),
            purple: Color::Rgb(194, 129, 255),
            pink: Color::Rgb(255, 126, 219),
        },
    ),
    (
        "matrix",
        Palette {
            bg: Color::Rgb(4, 10, 5),
            panel: Color::Rgb(6, 16, 8),
            raised: Color::Rgb(10, 28, 13),
            select: Color::Rgb(20, 70, 30),
            fg: Color::Rgb(150, 255, 160),
            dim: Color::Rgb(40, 110, 50),
            accent: Color::Rgb(0, 255, 70),
            accent2: Color::Rgb(200, 255, 200),
            red: Color::Rgb(255, 80, 80),
            orange: Color::Rgb(180, 255, 100),
            yellow: Color::Rgb(220, 255, 120),
            green: Color::Rgb(90, 230, 110),
            cyan: Color::Rgb(120, 255, 200),
            blue: Color::Rgb(60, 200, 90),
            purple: Color::Rgb(170, 255, 170),
            pink: Color::Rgb(230, 255, 230),
        },
    ),
    (
        "sunset",
        Palette {
            bg: Color::Rgb(32, 20, 26),
            panel: Color::Rgb(40, 24, 32),
            raised: Color::Rgb(52, 32, 42),
            select: Color::Rgb(100, 50, 70),
            fg: Color::Rgb(252, 230, 214),
            dim: Color::Rgb(140, 100, 110),
            accent: Color::Rgb(255, 120, 80),
            accent2: Color::Rgb(255, 200, 90),
            red: Color::Rgb(255, 80, 100),
            orange: Color::Rgb(255, 150, 90),
            yellow: Color::Rgb(255, 205, 100),
            green: Color::Rgb(200, 230, 120),
            cyan: Color::Rgb(255, 170, 150),
            blue: Color::Rgb(255, 110, 150),
            purple: Color::Rgb(220, 130, 200),
            pink: Color::Rgb(255, 140, 170),
        },
    ),
    (
        "ocean",
        Palette {
            bg: Color::Rgb(11, 21, 35),
            panel: Color::Rgb(14, 27, 44),
            raised: Color::Rgb(20, 38, 60),
            select: Color::Rgb(30, 70, 110),
            fg: Color::Rgb(205, 228, 245),
            dim: Color::Rgb(80, 110, 140),
            accent: Color::Rgb(80, 200, 255),
            accent2: Color::Rgb(100, 255, 210),
            red: Color::Rgb(255, 100, 120),
            orange: Color::Rgb(255, 170, 120),
            yellow: Color::Rgb(240, 220, 130),
            green: Color::Rgb(120, 230, 180),
            cyan: Color::Rgb(90, 220, 255),
            blue: Color::Rgb(120, 160, 255),
            purple: Color::Rgb(180, 150, 255),
            pink: Color::Rgb(255, 150, 200),
        },
    ),
    (
        "forest",
        Palette {
            bg: Color::Rgb(21, 28, 22),
            panel: Color::Rgb(25, 34, 26),
            raised: Color::Rgb(33, 45, 34),
            select: Color::Rgb(60, 85, 55),
            fg: Color::Rgb(218, 230, 205),
            dim: Color::Rgb(100, 120, 95),
            accent: Color::Rgb(160, 225, 110),
            accent2: Color::Rgb(240, 190, 100),
            red: Color::Rgb(230, 100, 90),
            orange: Color::Rgb(230, 150, 90),
            yellow: Color::Rgb(230, 210, 120),
            green: Color::Rgb(150, 210, 120),
            cyan: Color::Rgb(130, 200, 180),
            blue: Color::Rgb(120, 180, 140),
            purple: Color::Rgb(200, 160, 120),
            pink: Color::Rgb(230, 160, 150),
        },
    ),
    (
        "paper",
        Palette {
            bg: Color::Rgb(250, 247, 240),
            panel: Color::Rgb(240, 236, 226),
            raised: Color::Rgb(236, 231, 220),
            select: Color::Rgb(210, 220, 245),
            fg: Color::Rgb(40, 40, 52),
            dim: Color::Rgb(150, 145, 140),
            accent: Color::Rgb(205, 50, 120),
            accent2: Color::Rgb(30, 140, 130),
            red: Color::Rgb(200, 40, 50),
            orange: Color::Rgb(200, 100, 20),
            yellow: Color::Rgb(160, 120, 0),
            green: Color::Rgb(40, 130, 60),
            cyan: Color::Rgb(0, 120, 160),
            blue: Color::Rgb(60, 70, 200),
            purple: Color::Rgb(140, 50, 180),
            pink: Color::Rgb(200, 60, 140),
        },
    ),
];

/// The styles used to draw the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// The name of the palette the theme was built from.
    pub name: String,
    /// The palette the theme was built from, for flair that wants raw colors.
    pub palette: Palette,
    /// The background behind everything.
    pub background: Style,
    /// Plain document text.
    pub text: Style,
    /// Selected text.
    pub selection: Style,
    /// The line the cursor is on.
    pub cursor_line: Style,
    /// Line numbers.
    pub gutter: Style,
    /// The line number of the cursor line.
    pub gutter_active: Style,
    /// The status line background and text.
    pub status: Style,
    /// The highlighted badge at the start of the status line.
    pub status_badge: Style,
    /// Status messages.
    pub status_message: Style,
    /// Error counts and markers.
    pub error: Style,
    /// Warning counts and markers.
    pub warning: Style,
    /// Info markers.
    pub info: Style,
    /// Hint markers.
    pub hint: Style,
    /// The background tint of a line with an error.
    pub error_line: Style,
    /// The background tint of a line with a warning.
    pub warning_line: Style,
    /// The file explorer background and file names.
    pub sidebar: Style,
    /// The folder name at the top of the file explorer.
    pub sidebar_title: Style,
    /// The file in the explorer that is open in the editor.
    pub sidebar_active: Style,
    /// Folder names in the file explorer.
    pub directory: Style,
    /// The line between the file explorer and the editor.
    pub border: Style,
    /// Lines and files added since the last commit.
    pub git_added: Style,
    /// Lines and files changed since the last commit.
    pub git_modified: Style,
    /// Places where lines were removed since the last commit.
    pub git_removed: Style,
    /// Who last changed the cursor line.
    pub blame: Style,
    /// The thin lines marking indentation levels.
    pub indent_guide: Style,
    /// The colors nested brackets cycle through.
    pub rainbow: [Style; RAINBOW_LEN],
    /// The bracket matching the one at the cursor.
    pub matching_bracket: Style,
    /// Search matches.
    pub search_match: Style,
    /// The search match the cursor is on.
    pub search_current: Style,
    /// Popup backgrounds and text.
    pub popup: Style,
    /// Popup borders.
    pub popup_border: Style,
    /// Popup titles.
    pub popup_title: Style,
    /// The highlighted row in a popup list.
    pub popup_selected: Style,
    /// Secondary popup text like descriptions.
    pub popup_dim: Style,
    /// Chars in a popup row that match what was typed.
    pub popup_match: Style,
    /// Tabs of documents that are not focused.
    pub tab: Style,
    /// The tab of the focused document.
    pub tab_active: Style,
    /// The marker on tabs with unsaved changes.
    pub tab_modified: Style,
    /// The minimap.
    pub minimap: Style,
    /// The part of the minimap that is on screen.
    pub minimap_view: Style,
    /// Suggested text that is not inserted yet.
    pub ghost: Style,
    /// Language keywords.
    pub keyword: Style,
    /// Function names.
    pub function: Style,
    /// Type names.
    pub type_name: Style,
    /// String literals.
    pub string: Style,
    /// Number literals.
    pub number: Style,
    /// Constants and booleans.
    pub constant: Style,
    /// Comments.
    pub comment: Style,
    /// Operators.
    pub operator: Style,
    /// Brackets and separators.
    pub punctuation: Style,
    /// Macros and attributes.
    pub attribute: Style,
    /// Fields and properties.
    pub property: Style,
    /// Module and namespace names.
    pub namespace: Style,
    /// Markup headings, links and tags.
    pub markup: Style,
}

impl Default for Theme {
    /// Returns the built in theme, a dark purple one with loud accents.
    fn default() -> Self {
        Self::from_palette("mog", PALETTES[0].1)
    }
}

impl Theme {
    /// Returns the names of every built in theme.
    pub fn names() -> impl Iterator<Item = &'static str> {
        PALETTES.iter().map(|(name, _)| *name)
    }

    /// Returns the built in theme called `name`.
    pub fn named(name: &str) -> Option<Self> {
        PALETTES
            .iter()
            .find(|(other, _)| other.eq_ignore_ascii_case(name))
            .map(|(name, palette)| Self::from_palette(name, *palette))
    }

    /// Builds every style from the colors in `p`.
    pub fn from_palette(name: &str, p: Palette) -> Self {
        let fg = |color| Style::new().fg(color);
        let bold = Modifier::BOLD;
        Self {
            name: name.to_owned(),
            palette: p,
            background: Style::new().bg(p.bg),
            text: fg(p.fg),
            selection: Style::new().bg(p.select),
            cursor_line: Style::new().bg(p.raised),
            gutter: fg(p.dim),
            gutter_active: fg(p.accent).add_modifier(bold),
            status: fg(p.fg).bg(p.raised),
            status_badge: fg(p.bg).bg(p.accent).add_modifier(bold),
            status_message: fg(p.accent2),
            error: fg(p.red),
            warning: fg(p.yellow),
            info: fg(p.cyan),
            hint: fg(p.dim),
            error_line: Style::new().bg(mix(p.bg, p.red, 0.14)),
            warning_line: Style::new().bg(mix(p.bg, p.yellow, 0.10)),
            sidebar: fg(p.fg).bg(p.panel),
            sidebar_title: fg(p.accent).add_modifier(bold),
            sidebar_active: fg(p.accent).bg(p.raised),
            directory: fg(p.blue),
            border: fg(mix(p.panel, p.fg, 0.15)),
            git_added: fg(p.green),
            git_modified: fg(p.yellow),
            git_removed: fg(p.red),
            blame: fg(p.dim).add_modifier(Modifier::ITALIC),
            indent_guide: fg(mix(p.bg, p.fg, 0.12)),
            rainbow: [p.yellow, p.pink, p.cyan, p.green, p.purple, p.orange].map(fg),
            matching_bracket: fg(p.accent)
                .bg(mix(p.bg, p.accent, 0.25))
                .add_modifier(bold),
            search_match: Style::new().bg(mix(p.bg, p.yellow, 0.30)),
            search_current: fg(p.bg).bg(p.yellow),
            popup: fg(p.fg).bg(p.panel),
            popup_border: fg(p.accent),
            popup_title: fg(p.accent).add_modifier(bold),
            popup_selected: fg(p.fg).bg(p.select),
            popup_dim: fg(p.dim),
            popup_match: fg(p.accent2).add_modifier(bold),
            tab: fg(p.dim).bg(p.panel),
            tab_active: fg(p.fg).bg(p.bg).add_modifier(bold),
            tab_modified: fg(p.accent),
            minimap: fg(mix(p.bg, p.fg, 0.30)),
            minimap_view: Style::new().bg(p.raised),
            ghost: fg(p.dim).add_modifier(Modifier::ITALIC),
            keyword: fg(p.blue).add_modifier(bold),
            function: fg(p.cyan),
            type_name: fg(p.yellow),
            string: fg(p.green),
            number: fg(p.orange),
            constant: fg(p.orange),
            comment: fg(p.dim).add_modifier(Modifier::ITALIC),
            operator: fg(p.pink),
            punctuation: fg(mix(p.fg, p.dim, 0.4)),
            attribute: fg(p.purple),
            property: fg(mix(p.fg, p.cyan, 0.35)),
            namespace: fg(p.purple),
            markup: fg(p.accent).add_modifier(bold),
        }
    }

    /// Returns the style for a file with git `status`.
    pub fn git_status(&self, status: FileStatus) -> Style {
        match status {
            FileStatus::Added | FileStatus::Untracked => self.git_added,
            FileStatus::Modified | FileStatus::Renamed => self.git_modified,
            FileStatus::Deleted => self.git_removed,
            FileStatus::Conflicted => self.error,
        }
    }
}

/// Blends `amount` of `b` into `a`. Non RGB colors are returned unchanged.
pub fn mix(a: Color, b: Color, amount: f32) -> Color {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        return a;
    };
    let t = amount.clamp(0.0, 1.0);
    // the clamp keeps every channel inside u8 so the casts are lossless
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::Rgb(channel(ar, br), channel(ag, bg), channel(ab, bb))
}

#[cfg(test)]
/// Tests for themes.
mod tests {
    use ratatui::style::Color;

    use super::{Theme, mix};

    /// Every built in theme can be looked up by name, ignoring case.
    #[test]
    fn every_theme_resolves() {
        for name in Theme::names() {
            assert_eq!(
                Theme::named(name).map(|theme| theme.name),
                Some(name.into())
            );
        }
        assert!(Theme::named("SYNTHWAVE").is_some());
        assert!(Theme::named("nope").is_none());
    }

    /// Mixing goes from one color to the other.
    #[test]
    fn mixes_colors() {
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);
        assert_eq!(mix(black, white, 0.0), black);
        assert_eq!(mix(black, white, 1.0), white);
        assert_eq!(mix(black, white, 0.5), Color::Rgb(128, 128, 128));
        assert_eq!(mix(Color::Red, white, 0.5), Color::Red);
    }
}
