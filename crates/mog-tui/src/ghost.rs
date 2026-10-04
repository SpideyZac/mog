//! AI suggestions shown in gray after the cursor.

use mog_core::{Editor, Key, KeyChord};

/// An AI suggestion waiting after the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ghost {
    /// The document index it was made for.
    pub document: usize,
    /// The document version it was made for.
    pub version: u64,
    /// Where it goes.
    pub pos: usize,
    /// Every suggestion, never empty.
    pub items: Vec<String>,
    /// The suggestion shown.
    pub index: usize,
}

impl Ghost {
    /// Creates a ghost showing the first of `items`, or `None` if there are none.
    pub fn new(document: usize, version: u64, pos: usize, items: Vec<String>) -> Option<Self> {
        (!items.is_empty()).then_some(Self {
            document,
            version,
            pos,
            items,
            index: 0,
        })
    }

    /// Returns the suggestion shown.
    pub fn text(&self) -> &str {
        self.items.get(self.index).map_or("", String::as_str)
    }

    /// Returns `true` if the cursor and text of `editor` still match what it was made for.
    pub fn is_fresh(&self, editor: &Editor) -> bool {
        let document = editor.document();
        self.document == editor.active()
            && self.version == document.version()
            && self.pos == document.selection().head
    }
}

/// Handles the keys that act on a fresh suggestion: tab accepts it and escape dismisses it.
///
/// Returns `true` if the key was used. `menu_open` keeps tab for the completion menu.
pub fn handle_key(
    ghost: &mut Option<Ghost>,
    chord: KeyChord,
    editor: &mut Editor,
    menu_open: bool,
) -> bool {
    let Some(current) = ghost.as_ref().filter(|ghost| ghost.is_fresh(editor)) else {
        return false;
    };
    match chord.key {
        Key::Tab if !menu_open && !chord.mods.ctrl && !chord.mods.alt => {
            let (pos, text) = (current.pos, current.text().to_owned());
            *ghost = None;
            editor.replace_ranges(&[(pos, pos)], &text);
            true
        }
        Key::Esc => {
            *ghost = None;
            true
        }
        _ => false,
    }
}
