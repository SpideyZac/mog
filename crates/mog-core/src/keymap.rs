//! Key chords and the bindings from chords to commands.

use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use crate::command::Command;

/// The default bindings as `(chord, command)` names.
const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("left", "move_left"),
    ("right", "move_right"),
    ("up", "move_up"),
    ("down", "move_down"),
    ("home", "move_line_start"),
    ("end", "move_line_end"),
    ("pageup", "move_page_up"),
    ("pagedown", "move_page_down"),
    ("ctrl+left", "move_word_left"),
    ("ctrl+right", "move_word_right"),
    ("ctrl+home", "move_doc_start"),
    ("ctrl+end", "move_doc_end"),
    ("shift+left", "select_left"),
    ("shift+right", "select_right"),
    ("shift+up", "select_up"),
    ("shift+down", "select_down"),
    ("shift+home", "select_line_start"),
    ("shift+end", "select_line_end"),
    ("shift+pageup", "select_page_up"),
    ("shift+pagedown", "select_page_down"),
    ("ctrl+shift+left", "select_word_left"),
    ("ctrl+shift+right", "select_word_right"),
    ("ctrl+shift+home", "select_doc_start"),
    ("ctrl+shift+end", "select_doc_end"),
    ("ctrl+up", "scroll:-1"),
    ("ctrl+down", "scroll:1"),
    ("enter", "insert_newline"),
    ("tab", "insert_tab"),
    ("backspace", "delete_backward"),
    ("delete", "delete_forward"),
    ("ctrl+backspace", "delete_word_backward"),
    ("ctrl+a", "select_all"),
    ("ctrl+/", "toggle_comment"),
    ("alt+/", "toggle_comment"),
    ("shift+alt+down", "duplicate_line"),
    ("ctrl+d", "duplicate_line"),
    ("ctrl+shift+k", "delete_line"),
    ("alt+up", "move_line_up"),
    ("alt+down", "move_line_down"),
    ("shift+tab", "outdent"),
    ("ctrl+z", "undo"),
    ("ctrl+y", "redo"),
    ("ctrl+shift+z", "redo"),
    ("ctrl+c", "copy"),
    ("ctrl+x", "cut"),
    ("ctrl+v", "paste"),
    ("ctrl+s", "save"),
    ("ctrl+shift+s", "file.save_as"),
    ("ctrl+n", "file.new"),
    ("ctrl+q", "quit"),
    ("ctrl+p", "finder.files"),
    ("ctrl+shift+p", "command_palette"),
    ("alt+p", "command_palette"),
    ("f1", "command_palette"),
    ("ctrl+k", "help.keys"),
    ("ctrl+,", "settings.open"),
    ("alt+g", "graph.toggle"),
    ("ctrl+space", "lsp.complete"),
    ("alt+h", "lsp.hover"),
    ("f12", "lsp.definition"),
    ("f2", "lsp.rename"),
    ("shift+alt+f", "lsp.format"),
    ("alt+,", "settings.open"),
    ("ctrl+g", "goto.prompt"),
    ("ctrl+f", "search.find"),
    ("ctrl+h", "search.replace"),
    ("f3", "search.next"),
    ("shift+f3", "search.prev"),
    ("esc", "ui.escape"),
    ("ctrl+pagedown", "next_tab"),
    ("ctrl+pageup", "prev_tab"),
    ("alt+right", "next_tab"),
    ("alt+left", "prev_tab"),
    ("ctrl+w", "close_tab"),
    ("ctrl+b", "explorer.toggle"),
    ("ctrl+shift+e", "explorer.focus"),
    ("alt+e", "ai.explain"),
];

/// A key on the keyboard, independent of any terminal library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable char. Letters are always stored lowercase.
    Char(char),
    /// The enter key.
    Enter,
    /// The tab key.
    Tab,
    /// The backspace key.
    Backspace,
    /// The delete key.
    Delete,
    /// The insert key.
    Insert,
    /// The escape key.
    Esc,
    /// The left arrow.
    Left,
    /// The right arrow.
    Right,
    /// The up arrow.
    Up,
    /// The down arrow.
    Down,
    /// The home key.
    Home,
    /// The end key.
    End,
    /// The page up key.
    PageUp,
    /// The page down key.
    PageDown,
    /// A function key such as `F5`.
    F(u8),
}

/// Named keys paired with their names.
const NAMED_KEYS: [(Key, &str); 15] = [
    (Key::Enter, "enter"),
    (Key::Tab, "tab"),
    (Key::Backspace, "backspace"),
    (Key::Delete, "delete"),
    (Key::Insert, "insert"),
    (Key::Esc, "esc"),
    (Key::Left, "left"),
    (Key::Right, "right"),
    (Key::Up, "up"),
    (Key::Down, "down"),
    (Key::Home, "home"),
    (Key::End, "end"),
    (Key::PageUp, "pageup"),
    (Key::PageDown, "pagedown"),
    (Key::Char(' '), "space"),
];

/// The modifier keys held during a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    /// Whether control is held.
    pub ctrl: bool,
    /// Whether alt is held.
    pub alt: bool,
    /// Whether shift is held.
    pub shift: bool,
}

/// A key together with its modifiers, like `ctrl+shift+z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// The key that was pressed.
    pub key: Key,
    /// The modifiers held with it.
    pub mods: Modifiers,
}

impl KeyChord {
    /// Creates a chord, normalizing it so equal presses always compare equal.
    ///
    /// Uppercase letters become lowercase with shift set. Other chars drop shift since the char
    /// itself already reflects it (`!` rather than `shift+1`).
    pub fn new(key: Key, mods: Modifiers) -> Self {
        let mut mods = mods;
        let key = match key {
            Key::Char(ch) if ch.is_alphabetic() => {
                mods.shift |= ch.is_uppercase();
                Key::Char(ch.to_lowercase().next().unwrap_or(ch))
            }
            Key::Char(ch) => {
                mods.shift = false;
                Key::Char(ch)
            }
            other => other,
        };
        Self { key, mods }
    }

    /// Returns the char this chord types, if it is plain text input.
    pub fn typed_char(&self) -> Option<char> {
        match self.key {
            Key::Char(ch) if !self.mods.ctrl && !self.mods.alt => Some(if self.mods.shift {
                ch.to_uppercase().next().unwrap_or(ch)
            } else {
                ch
            }),
            _ => None,
        }
    }
}

impl Display for KeyChord {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.mods.ctrl {
            f.write_str("ctrl+")?;
        }
        if self.mods.alt {
            f.write_str("alt+")?;
        }
        if self.mods.shift {
            f.write_str("shift+")?;
        }
        if let Some((_, name)) = NAMED_KEYS.iter().find(|(key, _)| *key == self.key) {
            return f.write_str(name);
        }
        match self.key {
            Key::Char(ch) => write!(f, "{ch}"),
            Key::F(n) => write!(f, "f{n}"),
            _ => Ok(()),
        }
    }
}

/// The error returned when a chord string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidChord(pub String);

impl Display for InvalidChord {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "invalid key chord `{}`", self.0)
    }
}

impl Error for InvalidChord {}

impl FromStr for KeyChord {
    type Err = InvalidChord;

    /// Parses chords like `ctrl+s`, `shift+pageup`, `alt+f4` or `+`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidChord(text.to_owned());
        let lower = text.to_lowercase();
        // a trailing + is the plus key itself
        let (prefix, key_name) = match lower.strip_suffix("++") {
            Some(prefix) => (prefix, "+"),
            None => lower.rsplit_once('+').unwrap_or(("", &lower)),
        };
        let mut mods = Modifiers::default();
        for part in prefix.split('+').filter(|part| !part.is_empty()) {
            match part {
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return Err(invalid()),
            }
        }
        let key = if let Some((key, _)) = NAMED_KEYS.iter().find(|(_, name)| *name == key_name) {
            *key
        } else if let Some(n) = key_name.strip_prefix('f').and_then(|n| n.parse().ok()) {
            Key::F(n)
        } else {
            let mut chars = key_name.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Key::Char(ch),
                _ => return Err(invalid()),
            }
        };
        Ok(Self::new(key, mods))
    }
}

/// The bindings from [`KeyChord`]s to [`Command`]s.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// The bound commands.
    bindings: HashMap<KeyChord, Command>,
}

impl Default for Keymap {
    /// Returns the built in modeless bindings.
    fn default() -> Self {
        let mut keymap = Self::empty();
        for (chord, command) in DEFAULT_BINDINGS {
            if let (Ok(chord), Ok(command)) = (chord.parse(), command.parse()) {
                keymap.bind(chord, command);
            }
        }
        keymap
    }
}

impl Keymap {
    /// Creates a keymap with no bindings.
    pub fn empty() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Binds `chord` to `command`, replacing any previous binding.
    pub fn bind(&mut self, chord: KeyChord, command: Command) {
        self.bindings.insert(chord, command);
    }

    /// Removes the binding for `chord`.
    pub fn unbind(&mut self, chord: &KeyChord) {
        self.bindings.remove(chord);
    }

    /// Returns every binding, sorted by command name and then chord.
    pub fn bindings(&self) -> Vec<(KeyChord, Command)> {
        let mut bindings: Vec<_> = self
            .bindings
            .iter()
            .map(|(chord, command)| (*chord, command.clone()))
            .collect();
        bindings.sort_by_key(|(chord, command)| (command.to_string(), chord.to_string()));
        bindings
    }

    /// Returns the chords bound to `command`, shortest first.
    pub fn chords_for(&self, command: &Command) -> Vec<KeyChord> {
        let mut chords: Vec<KeyChord> = self
            .bindings
            .iter()
            .filter(|(_, bound)| *bound == command)
            .map(|(chord, _)| *chord)
            .collect();
        chords.sort_by_key(|chord| {
            let text = chord.to_string();
            (text.len(), text)
        });
        chords
    }

    /// Returns the command for `chord`, falling back to typing the char for unbound text keys.
    pub fn resolve(&self, chord: &KeyChord) -> Option<Command> {
        self.bindings
            .get(chord)
            .cloned()
            .or_else(|| chord.typed_char().map(Command::InsertChar))
    }
}

#[cfg(test)]
/// Tests for chords and the keymap.
mod tests {
    use super::{DEFAULT_BINDINGS, Key, KeyChord, Keymap, Modifiers};
    use crate::command::Command;

    /// Every default binding parses.
    #[test]
    fn default_bindings_parse() {
        for (chord, command) in DEFAULT_BINDINGS {
            assert!(chord.parse::<KeyChord>().is_ok(), "{chord}");
            assert!(command.parse::<Command>().is_ok(), "{command}");
        }
    }

    /// Chords survive a trip through their string form.
    #[test]
    fn chords_round_trip() {
        for text in ["ctrl+shift+z", "alt+f4", "pageup", "ctrl++", "space"] {
            let chord: KeyChord = text.parse().expect("valid chord");
            assert_eq!(chord.to_string(), text);
        }
    }

    /// An uppercase char is the same chord as shift plus the lowercase char.
    #[test]
    fn uppercase_normalizes_to_shift() {
        let upper = KeyChord::new(Key::Char('Z'), Modifiers::default());
        assert_eq!(upper, "shift+z".parse().expect("valid chord"));
        assert_eq!(upper.typed_char(), Some('Z'));
    }

    /// Bindings can be listed and looked up by command.
    #[test]
    fn lists_bindings() {
        let keymap = Keymap::default();
        let chords = keymap.chords_for(&Command::Redo);
        let names: Vec<String> = chords.iter().map(ToString::to_string).collect();
        assert_eq!(names, ["ctrl+y", "ctrl+shift+z"]);
        assert!(
            keymap
                .bindings()
                .iter()
                .any(|(_, command)| *command == Command::Save)
        );
    }

    /// Unbound text keys type themselves and control chords do not.
    #[test]
    fn resolve_falls_back_to_typing() {
        let keymap = Keymap::default();
        let plain = KeyChord::new(Key::Char('m'), Modifiers::default());
        assert_eq!(keymap.resolve(&plain), Some(Command::InsertChar('m')));
        let ctrl = "ctrl+alt+f12".parse().expect("valid chord");
        assert_eq!(keymap.resolve(&ctrl), None);
        let save = "ctrl+s".parse().expect("valid chord");
        assert_eq!(keymap.resolve(&save), Some(Command::Save));
    }
}
