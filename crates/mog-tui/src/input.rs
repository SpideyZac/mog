//! Translation from terminal input to editor input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mog_core::{Key, KeyChord, Modifiers};

/// Converts a terminal key event to a [`KeyChord`], or `None` for keys mog does not know.
pub fn key_chord(event: KeyEvent) -> Option<KeyChord> {
    let key = match event.code {
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Esc => Key::Esc,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };
    let mods = Modifiers {
        ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
        alt: event.modifiers.contains(KeyModifiers::ALT),
        shift: event.modifiers.contains(KeyModifiers::SHIFT) || event.code == KeyCode::BackTab,
    };
    Some(KeyChord::new(key, mods))
}

#[cfg(test)]
/// Tests for key translation.
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::key_chord;

    /// Control and shift chords map to their parsed forms.
    #[test]
    fn translates_modifiers() {
        let event = KeyEvent::new(
            KeyCode::Char('Z'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(key_chord(event), "ctrl+shift+z".parse().ok());
    }

    /// Back tab is shift plus tab.
    #[test]
    fn back_tab_is_shift_tab() {
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(key_chord(event), "shift+tab".parse().ok());
    }
}
