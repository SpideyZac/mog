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

/// Handles the keys that act on a fresh suggestion.
///
/// Tab accepts it, `ctrl+right` accepts the next word, `alt+]` and `alt+[` cycle through the
/// suggestions and escape dismisses it. Returns `true` if the key was used. `menu_open` keeps
/// tab for the completion menu.
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
        Key::Right if chord.mods.ctrl && !chord.mods.alt => {
            let text = current.text();
            let word: String = text.chars().take(next_word_len(text)).collect();
            let rest = text[word.len()..].to_owned();
            let pos = current.pos;
            editor.replace_ranges(&[(pos, pos)], &word);
            let document = editor.document();
            *ghost = Ghost::new(
                editor.active(),
                document.version(),
                pos + word.chars().count(),
                Some(rest)
                    .filter(|rest| !rest.is_empty())
                    .into_iter()
                    .collect(),
            );
            true
        }
        Key::Char(ch @ (']' | '[')) if chord.mods.alt && !chord.mods.ctrl => {
            if let Some(ghost) = ghost.as_mut() {
                let len = ghost.items.len();
                ghost.index = if ch == ']' {
                    (ghost.index + 1) % len
                } else {
                    (ghost.index + len - 1) % len
                };
            }
            true
        }
        Key::Esc => {
            *ghost = None;
            true
        }
        _ => false,
    }
}

/// Returns how many chars of `text` make up its next word, with the whitespace before it.
///
/// A run of punctuation counts as one char at a time, so `foo(bar)` goes `foo`, `(`, `bar`.
fn next_word_len(text: &str) -> usize {
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut chars = text.chars().peekable();
    let mut len = 0;
    while chars.next_if(|ch| ch.is_whitespace()).is_some() {
        len += 1;
    }
    match chars.next() {
        Some(ch) if is_word(ch) => {
            len += 1;
            while chars.next_if(|&ch| is_word(ch)).is_some() {
                len += 1;
            }
        }
        Some(_) => len += 1,
        None => {}
    }
    len
}

#[cfg(test)]
/// Tests for accepting and cycling suggestions.
mod tests {
    use mog_core::{Document, Editor, KeyChord, MemoryClipboard, Range};

    use super::{Ghost, handle_key, next_word_len};

    /// Builds an editor holding `text` with the cursor at the end and a ghost of `items`.
    fn setup(text: &str, items: &[&str]) -> (Editor, Option<Ghost>) {
        let mut editor = Editor::new(Box::new(MemoryClipboard::default()));
        *editor.document_mut() = Document::from_text(text);
        let end = editor.document().text().len_chars();
        editor.document_mut().set_selection(Range::point(end));
        let version = editor.document().version();
        let items = items.iter().map(|item| (*item).to_owned()).collect();
        let ghost = Ghost::new(0, version, end, items);
        (editor, ghost)
    }

    /// Presses `chord` on the ghost.
    fn press(chord: &str, editor: &mut Editor, ghost: &mut Option<Ghost>) -> bool {
        let chord: KeyChord = chord.parse().expect("valid chord");
        handle_key(ghost, chord, editor, false)
    }

    /// Words, whitespace and punctuation are taken one step at a time.
    #[test]
    fn splits_words() {
        assert_eq!(next_word_len("foo(bar)"), 3);
        assert_eq!(next_word_len("(bar)"), 1);
        assert_eq!(next_word_len("  bar baz"), 5);
        assert_eq!(next_word_len("\n    let"), 8);
        assert_eq!(next_word_len(""), 0);
    }

    /// Accepting a word inserts it and keeps the rest as a fresh ghost.
    #[test]
    fn accepts_one_word() {
        let (mut editor, mut ghost) = setup("let x = ", &["vec_new(1)"]);
        assert!(press("ctrl+right", &mut editor, &mut ghost));
        assert_eq!(editor.document().text().to_string(), "let x = vec_new");
        let rest = ghost.expect("rest stays");
        assert_eq!(rest.text(), "(1)");
        assert!(rest.is_fresh(&editor));
    }

    /// Tab accepts the shown suggestion after cycling to it.
    #[test]
    fn cycles_then_accepts() {
        let (mut editor, mut ghost) = setup("x", &["1", "2", "3"]);
        assert!(press("alt+[", &mut editor, &mut ghost));
        assert_eq!(ghost.as_ref().map(Ghost::text), Some("3"));
        assert!(press("alt+]", &mut editor, &mut ghost));
        assert!(press("alt+]", &mut editor, &mut ghost));
        assert!(press("tab", &mut editor, &mut ghost));
        assert_eq!(editor.document().text().to_string(), "x2");
        assert!(ghost.is_none());
    }
}
