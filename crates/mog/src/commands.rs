//! The commands the palette offers and their titles.

use mog_core::{Command, Keymap};
use mog_tui::CommandInfo;

/// Every command in the palette as `(name, title)`, grouped by what they act on.
pub const CATALOG: &[(&str, &str)] = &[
    ("save", "File: Save"),
    ("file.save_as", "File: Save as"),
    ("file.new", "File: New untitled file"),
    ("file.create", "File: Create file in project"),
    ("finder.files", "File: Find a file"),
    ("close_tab", "File: Close tab"),
    ("next_tab", "File: Next tab"),
    ("prev_tab", "File: Previous tab"),
    ("quit", "File: Quit mog"),
    ("undo", "Edit: Undo"),
    ("redo", "Edit: Redo"),
    ("copy", "Edit: Copy"),
    ("cut", "Edit: Cut"),
    ("paste", "Edit: Paste"),
    ("select_all", "Edit: Select all"),
    ("toggle_comment", "Edit: Toggle comment"),
    ("duplicate_line", "Edit: Duplicate line"),
    ("delete_line", "Edit: Delete line"),
    ("move_line_up", "Edit: Move line up"),
    ("move_line_down", "Edit: Move line down"),
    ("outdent", "Edit: Outdent"),
    ("select_next_occurrence", "Cursors: Add next occurrence"),
    ("select_all_occurrences", "Cursors: Select all occurrences"),
    ("add_cursor_above", "Cursors: Add cursor above"),
    ("add_cursor_below", "Cursors: Add cursor below"),
    ("search.find", "Search: Find"),
    ("search.replace", "Search: Find and replace"),
    ("search.next", "Search: Next match"),
    ("search.prev", "Search: Previous match"),
    ("goto.prompt", "Go: Go to line"),
    ("lsp.definition", "Go: Go to definition"),
    ("problems.next", "Go: Next problem"),
    ("problems.prev", "Go: Previous problem"),
    ("problems.list", "View: Problems"),
    ("lsp.hover", "Code: Show hover info"),
    ("lsp.complete", "Code: Complete"),
    ("lsp.format", "Code: Format file"),
    ("lsp.rename", "Code: Rename symbol"),
    ("lsp.references", "Code: Find references"),
    ("lsp.actions", "Code: Quick fixes and refactors"),
    ("move_doc_start", "Go: Start of file"),
    ("move_doc_end", "Go: End of file"),
    ("graph.toggle", "View: Project graph"),
    ("split.toggle", "View: Split editor"),
    ("terminal.toggle", "View: Toggle terminal"),
    ("split.focus", "View: Focus other split"),
    ("explorer.toggle", "View: Toggle file explorer"),
    ("explorer.focus", "View: Focus file explorer"),
    ("settings.open", "Settings: Open settings"),
    ("config.open", "Settings: Open config file"),
    ("config.reload", "Settings: Reload config"),
    ("audio.toggle_music", "Sound: Toggle music"),
    ("audio.toggle_effects", "Sound: Toggle sound effects"),
    ("flair.toggle", "Flair: Toggle all flair"),
    ("theme.next", "Theme: Next theme"),
    ("command_palette", "Help: Command palette"),
    ("help.keys", "Help: Key bindings"),
    ("ai.explain", "AI: Explain the selection"),
    ("ai.chat", "AI: Toggle chat"),
    ("copilot.sign_in", "Copilot: Sign in"),
    ("copilot.sign_out", "Copilot: Sign out"),
    ("copilot.status", "Copilot: Status"),
];

/// Builds the palette entries with the chords bound to each command in `keymap`.
pub fn palette(keymap: &Keymap) -> Vec<CommandInfo> {
    CATALOG
        .iter()
        .map(|(name, title)| {
            let keys = name
                .parse::<Command>()
                .map(|command| keymap.chords_for(&command))
                .unwrap_or_default();
            CommandInfo {
                name: (*name).to_owned(),
                title: (*title).to_owned(),
                keys: keys.iter().map(ToString::to_string).collect(),
            }
        })
        .collect()
}

/// Returns every binding in `keymap` as `(chord, command name)`, hiding plain cursor keys.
pub fn bindings(keymap: &Keymap) -> Vec<(String, String)> {
    keymap
        .bindings()
        .into_iter()
        .map(|(chord, command)| (chord.to_string(), command.to_string()))
        .collect()
}

/// Formats every binding as an aligned table for `mog --keys`.
pub fn key_table(keymap: &Keymap) -> String {
    let palette = palette(keymap);
    let rows: Vec<(String, String, &str)> = bindings(keymap)
        .into_iter()
        .map(|(chord, name)| {
            let title = palette
                .iter()
                .find(|info| info.name == name)
                .map_or("", |info| info.title.as_str());
            (chord, name, title)
        })
        .collect();
    let chord_width = rows
        .iter()
        .map(|(chord, _, _)| chord.len())
        .max()
        .unwrap_or(0);
    let name_width = rows
        .iter()
        .map(|(_, name, _)| name.len())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(chord, name, title)| {
            format!("{chord:<chord_width$}  {name:<name_width$}  {title}")
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
/// Tests for the command catalog.
mod tests {
    use mog_core::{Command, Keymap};

    use super::{CATALOG, key_table, palette};

    /// Every catalog entry is a valid command name.
    #[test]
    fn catalog_parses() {
        for (name, _) in CATALOG {
            assert!(name.parse::<Command>().is_ok(), "{name}");
        }
    }

    /// Palette entries carry their key bindings.
    #[test]
    fn palette_has_keys() {
        let entries = palette(&Keymap::default());
        let save = entries
            .iter()
            .find(|info| info.name == "save")
            .expect("save");
        assert_eq!(save.keys, ["ctrl+s"]);
    }

    /// The key table has a row per binding with the command title.
    #[test]
    fn key_table_rows() {
        let table = key_table(&Keymap::default());
        assert!(
            table
                .lines()
                .any(|line| line.starts_with("ctrl+s") && line.ends_with("File: Save"))
        );
    }
}
