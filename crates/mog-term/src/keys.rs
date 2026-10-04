//! Turning key chords into the bytes a shell expects.

use mog_core::{Key, KeyChord, Modifiers};

/// Returns the xterm modifier parameter for `mods`, or `None` without modifiers.
fn modifier_param(mods: Modifiers) -> Option<u8> {
    let param = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
    (param > 1).then_some(param)
}

/// Returns the sequence for a cursor or function key ending in `last`, like `A` for up.
fn csi_key(last: char, mods: Modifiers, application: bool) -> Vec<u8> {
    match modifier_param(mods) {
        Some(param) => format!("\x1b[1;{param}{last}").into_bytes(),
        None if application => format!("\x1bO{last}").into_bytes(),
        None => format!("\x1b[{last}").into_bytes(),
    }
}

/// Returns the sequence for a key sent as `CSI number ~`, like delete.
fn tilde_key(number: u8, mods: Modifiers) -> Vec<u8> {
    match modifier_param(mods) {
        Some(param) => format!("\x1b[{number};{param}~").into_bytes(),
        None => format!("\x1b[{number}~").into_bytes(),
    }
}

/// Returns the bytes for a char typed with `mods`.
fn char_bytes(ch: char, mods: Modifiers) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if mods.alt {
        bytes.push(0x1b);
    }
    if mods.ctrl {
        let control = match ch {
            'a'..='z' => ch as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' | '/' => 0x1f,
            '8' | '?' => 0x7f,
            _ => return None,
        };
        bytes.push(control);
    } else {
        let ch = if mods.shift {
            ch.to_uppercase().next().unwrap_or(ch)
        } else {
            ch
        };
        let mut buf = [0; 4];
        bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    Some(bytes)
}

/// Returns the bytes a terminal sends for `chord`, or `None` for chords it cannot send.
///
/// `application` is whether the program asked for application cursor keys, which changes
/// what the arrows send.
pub fn chord_bytes(chord: KeyChord, application: bool) -> Option<Vec<u8>> {
    let mods = chord.mods;
    let bytes = match chord.key {
        Key::Char(ch) => return char_bytes(ch, mods),
        Key::Enter => b"\r".to_vec(),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace if mods.ctrl => b"\x08".to_vec(),
        Key::Backspace if mods.alt => b"\x1b\x7f".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Esc => b"\x1b".to_vec(),
        Key::Up => csi_key('A', mods, application),
        Key::Down => csi_key('B', mods, application),
        Key::Right => csi_key('C', mods, application),
        Key::Left => csi_key('D', mods, application),
        Key::Home => csi_key('H', mods, application),
        Key::End => csi_key('F', mods, application),
        Key::Insert => tilde_key(2, mods),
        Key::Delete => tilde_key(3, mods),
        Key::PageUp => tilde_key(5, mods),
        Key::PageDown => tilde_key(6, mods),
        Key::F(n @ 1..=4) => csi_key(char::from(b'P' + n - 1), mods, true),
        Key::F(n @ 5..=12) => {
            let number = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)];
            tilde_key(number, mods)
        }
        Key::F(_) => return None,
    };
    Some(bytes)
}

#[cfg(test)]
/// Tests for key encoding.
mod tests {
    use super::chord_bytes;

    /// Returns the bytes for the chord written as `text`.
    fn bytes(text: &str) -> Vec<u8> {
        chord_bytes(text.parse().expect("valid chord"), false).expect("sendable")
    }

    /// Plain and shifted letters type themselves and ctrl makes control codes.
    #[test]
    fn chars() {
        assert_eq!(bytes("a"), b"a");
        assert_eq!(bytes("shift+a"), b"A");
        assert_eq!(bytes("ctrl+c"), [3]);
        assert_eq!(bytes("alt+b"), b"\x1bb");
    }

    /// Arrows follow the cursor key mode and carry modifiers.
    #[test]
    fn arrows() {
        assert_eq!(bytes("up"), b"\x1b[A");
        assert_eq!(bytes("ctrl+left"), b"\x1b[1;5D");
        let up = "up".parse().expect("valid chord");
        assert_eq!(chord_bytes(up, true).expect("sendable"), b"\x1bOA");
    }

    /// Editing and function keys use their xterm sequences.
    #[test]
    fn special_keys() {
        assert_eq!(bytes("enter"), b"\r");
        assert_eq!(bytes("backspace"), b"\x7f");
        assert_eq!(bytes("delete"), b"\x1b[3~");
        assert_eq!(bytes("f1"), b"\x1bOP");
        assert_eq!(bytes("f5"), b"\x1b[15~");
        assert_eq!(bytes("shift+tab"), b"\x1b[Z");
    }
}
