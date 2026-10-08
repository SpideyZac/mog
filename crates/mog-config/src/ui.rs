//! Settings for how the editor looks.

use serde::Deserialize;

/// Settings for what the editor shows.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// The name of the color theme.
    pub theme: String,
    /// Whether open files are shown as tabs along the top.
    pub tabs: bool,
    /// Whether line numbers are shown.
    pub line_numbers: bool,
    /// Whether line numbers count from the cursor line instead of the top.
    pub relative_line_numbers: bool,
    /// Whether the cursor line is highlighted.
    pub cursor_line: bool,
    /// Whether thin guides mark each indentation level.
    pub indent_guides: bool,
    /// Whether nested brackets get different colors.
    pub rainbow_brackets: bool,
    /// Whether code is colored by syntax.
    pub syntax_highlighting: bool,
    /// Whether colors from the language server refine the syntax colors.
    pub semantic_highlighting: bool,
    /// Whether types and parameter names from the language server are shown after lines.
    pub inlay_hints: bool,
    /// Whether a zoomed out view of the file is shown on the right.
    pub minimap: bool,
    /// Whether diagnostics are written out at the end of their line.
    pub error_lens: bool,
    /// Whether changed lines are marked next to the line numbers.
    pub git_gutter: bool,
    /// Whether who last changed the cursor line is shown at its end.
    pub git_blame: bool,
    /// Whether the file explorer starts open when a folder is opened.
    pub explorer: bool,
    /// Whether the sidebar with buttons for the file explorer, source control and plugin views
    /// is shown.
    pub sidebar: bool,
    /// Whether the sidebar is on the right instead of the left.
    pub sidebar_right: bool,
    /// Whether files get little colored icons.
    pub icons: bool,
    /// How solid backgrounds are, from 0 to 100. Below 100 the terminal shows through, if it
    /// is see through itself.
    pub opacity: u8,
    /// Whether serious mode is on, which turns off every flair, sound and music.
    pub serious: bool,
    /// Whether flashing and moving flair like sparks and matrix rain stays still or hidden.
    pub reduced_motion: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "mog".into(),
            tabs: true,
            line_numbers: true,
            relative_line_numbers: false,
            cursor_line: true,
            indent_guides: true,
            rainbow_brackets: true,
            syntax_highlighting: true,
            semantic_highlighting: true,
            inlay_hints: true,
            minimap: true,
            error_lens: true,
            git_gutter: true,
            git_blame: true,
            explorer: true,
            sidebar: false,
            sidebar_right: false,
            icons: true,
            opacity: 100,
            serious: false,
            reduced_motion: false,
        }
    }
}
