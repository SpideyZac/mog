//! Translation from terminal input to editor input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mog_core::{Key, KeyChord, Modifiers};

/// Converts a terminal key event to a [`KeyChord`], or `None` for keys mog does not know.
///
/// With `alt_gr` set, symbols typed with `AltGr` come out as plain typed chars, see
/// [`KeyChord::is_alt_gr`].
pub fn key_chord(event: KeyEvent, alt_gr: bool) -> Option<KeyChord> {
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
    let chord = KeyChord::new(key, mods);
    Some(if alt_gr {
        chord.without_alt_gr()
    } else {
        chord
    })
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
        assert_eq!(key_chord(event, true), "ctrl+shift+z".parse().ok());
    }

    /// `AltGr` symbols type only when the setting is on.
    #[test]
    fn alt_gr_follows_the_setting() {
        let event = KeyEvent::new(
            KeyCode::Char('{'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(key_chord(event, true), "{".parse().ok());
        assert_eq!(key_chord(event, false), "ctrl+alt+{".parse().ok());
    }

    /// Back tab is shift plus tab.
    #[test]
    fn back_tab_is_shift_tab() {
        let event = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(key_chord(event, true), "shift+tab".parse().ok());
    }
}
