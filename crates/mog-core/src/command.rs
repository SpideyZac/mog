//! Things the editor can be told to do.

use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

/// A way of moving the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    /// One char left.
    Left,
    /// One char right.
    Right,
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// To the start of the previous word.
    WordLeft,
    /// To the end of the next word.
    WordRight,
    /// To the first non blank char of the line, or the line start.
    LineStart,
    /// To the end of the line.
    LineEnd,
    /// One screen up.
    PageUp,
    /// One screen down.
    PageDown,
    /// To the start of the document.
    DocStart,
    /// To the end of the document.
    DocEnd,
}

impl Motion {
    /// Every motion paired with its name, used to build command names.
    const ALL: [(Self, &'static str); 12] = [
        (Self::Left, "left"),
        (Self::Right, "right"),
        (Self::Up, "up"),
        (Self::Down, "down"),
        (Self::WordLeft, "word_left"),
        (Self::WordRight, "word_right"),
        (Self::LineStart, "line_start"),
        (Self::LineEnd, "line_end"),
        (Self::PageUp, "page_up"),
        (Self::PageDown, "page_down"),
        (Self::DocStart, "doc_start"),
        (Self::DocEnd, "doc_end"),
    ];

    /// Returns the name of the motion.
    fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(motion, _)| *motion == self)
            .map_or("", |(_, name)| name)
    }

    /// Looks up a motion by name.
    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(motion, _)| *motion)
    }
}

/// An action the editor can perform.
///
/// Keys, mouse input and the command palette all end up as commands. Every command has a
/// name (see [`Display`] and [`FromStr`]) so it can be bound in config files.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    /// Moves the cursor, extending the selection if `extend` is set.
    Move {
        /// How to move.
        motion: Motion,
        /// Whether to keep the anchor and grow the selection.
        extend: bool,
    },
    /// Types a char over the selection.
    InsertChar(char),
    /// Types a string over the selection, used for pastes.
    InsertText(String),
    /// Inserts a line break.
    InsertNewline,
    /// Inserts a tab or spaces.
    InsertTab,
    /// Deletes the selection or the char before the cursor.
    DeleteBackward,
    /// Deletes the selection or the char after the cursor.
    DeleteForward,
    /// Deletes back to the start of the previous word.
    DeleteWordBackward,
    /// Selects the whole document.
    SelectAll,
    /// Reverts the last edit.
    Undo,
    /// Reapplies the last undone edit.
    Redo,
    /// Copies the selection to the clipboard.
    Copy,
    /// Copies the selection to the clipboard and deletes it.
    Cut,
    /// Inserts the clipboard contents.
    Paste,
    /// Saves the current document.
    Save,
    /// Quits the editor.
    Quit,
    /// Opens the command palette.
    CommandPalette,
    /// Focuses the next open document.
    NextTab,
    /// Focuses the previous open document.
    PrevTab,
    /// Closes the focused document, asking first if it has unsaved changes.
    CloseTab,
    /// Scrolls the view without moving the cursor. Negative values scroll up.
    Scroll(isize),
    /// A namespaced command handled outside the core, like `ai.explain`.
    Custom(String),
}

/// Commands without arguments paired with their names.
const SIMPLE: [(Command, &str); 17] = [
    (Command::InsertNewline, "insert_newline"),
    (Command::InsertTab, "insert_tab"),
    (Command::DeleteBackward, "delete_backward"),
    (Command::DeleteForward, "delete_forward"),
    (Command::DeleteWordBackward, "delete_word_backward"),
    (Command::SelectAll, "select_all"),
    (Command::Undo, "undo"),
    (Command::Redo, "redo"),
    (Command::Copy, "copy"),
    (Command::Cut, "cut"),
    (Command::Paste, "paste"),
    (Command::Save, "save"),
    (Command::Quit, "quit"),
    (Command::CommandPalette, "command_palette"),
    (Command::NextTab, "next_tab"),
    (Command::PrevTab, "prev_tab"),
    (Command::CloseTab, "close_tab"),
];

impl Display for Command {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Move { motion, extend } => {
                let verb = if *extend { "select" } else { "move" };
                write!(f, "{verb}_{}", motion.name())
            }
            Self::InsertChar(ch) => write!(f, "insert_char:{ch}"),
            Self::InsertText(text) => write!(f, "insert_text:{text}"),
            Self::Scroll(lines) => write!(f, "scroll:{lines}"),
            Self::Custom(name) => f.write_str(name),
            simple => {
                let name = SIMPLE
                    .iter()
                    .find(|(command, _)| command == simple)
                    .map_or("", |(_, name)| name);
                f.write_str(name)
            }
        }
    }
}

/// The error returned when a command name is not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCommand(pub String);

impl Display for UnknownCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "unknown command `{}`", self.0)
    }
}

impl Error for UnknownCommand {}

impl FromStr for Command {
    type Err = UnknownCommand;

    /// Parses a command name such as `save`, `select_word_left` or `scroll:-3`.
    ///
    /// Names containing a `.` are treated as [`Command::Custom`].
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let unknown = || UnknownCommand(name.to_owned());
        if let Some((verb, arg)) = name.split_once(':') {
            return match verb {
                "insert_char" => {
                    let mut chars = arg.chars();
                    match (chars.next(), chars.next()) {
                        (Some(ch), None) => Ok(Self::InsertChar(ch)),
                        _ => Err(unknown()),
                    }
                }
                "insert_text" => Ok(Self::InsertText(arg.to_owned())),
                "scroll" => arg.parse().map(Self::Scroll).map_err(|_| unknown()),
                _ => Err(unknown()),
            };
        }
        if let Some(motion) = name.strip_prefix("move_").and_then(Motion::from_name) {
            return Ok(Self::Move {
                motion,
                extend: false,
            });
        }
        if let Some(motion) = name.strip_prefix("select_").and_then(Motion::from_name) {
            return Ok(Self::Move {
                motion,
                extend: true,
            });
        }
        if name.contains('.') {
            return Ok(Self::Custom(name.to_owned()));
        }
        SIMPLE
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(command, _)| command.clone())
            .ok_or_else(unknown)
    }
}

#[cfg(test)]
/// Tests for command names.
mod tests {
    use super::{Command, Motion};

    /// Every kind of command survives a trip through its name.
    #[test]
    fn names_round_trip() {
        let commands = [
            Command::Save,
            Command::Move {
                motion: Motion::WordLeft,
                extend: true,
            },
            Command::Move {
                motion: Motion::DocEnd,
                extend: false,
            },
            Command::InsertChar('x'),
            Command::Scroll(-3),
            Command::Custom("hello.wave".into()),
        ];
        for command in commands {
            assert_eq!(command.to_string().parse::<Command>(), Ok(command));
        }
    }

    /// Unknown names are rejected.
    #[test]
    fn rejects_unknown_names() {
        assert!("frobnicate".parse::<Command>().is_err());
        assert!("scroll".parse::<Command>().is_err());
    }
}
