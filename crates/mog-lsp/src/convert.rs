//! Conversions between editor and protocol types.

use std::{
    path::{Path, PathBuf},
    str::{self, FromStr},
};

use lsp_types::{Position, Uri};
use ropey::Rope;

/// The scheme prefix of file URIs.
const FILE_SCHEME: &str = "file://";

/// Returns `true` for bytes that can appear in a URI path without escaping.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte)
}

/// Converts an absolute path to a `file://` URI.
///
/// Returns `None` for relative paths.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    if !path.is_absolute() {
        return None;
    }
    let raw = path.to_string_lossy();
    // canonicalize on windows adds a verbatim prefix servers do not understand
    let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw).replace('\\', "/");
    let mut uri = String::from(FILE_SCHEME);
    if !raw.starts_with('/') {
        uri.push('/');
    }
    for byte in raw.bytes() {
        if is_unreserved(byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&uri).ok()
}

/// Converts a `file://` URI back to a path.
///
/// Returns `None` for other schemes or broken escapes.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix(FILE_SCHEME)?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hex = [iter.next()?, iter.next()?];
            let hex = str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    // windows uris look like /c:/dir so the leading slash has to go
    let is_drive = decoded.as_bytes().get(2) == Some(&b':');
    let path = if cfg!(windows) && is_drive {
        &decoded[1..]
    } else {
        decoded.as_str()
    };
    Some(PathBuf::from(path))
}

/// Converts a protocol position, which counts UTF-16 units, to a char offset.
///
/// Positions past the end of a line or the text are clamped.
pub fn position_to_char(text: &Rope, position: Position) -> usize {
    let line = usize::try_from(position.line).unwrap_or(usize::MAX);
    if line >= text.len_lines() {
        return text.len_chars();
    }
    let start = text.line_to_char(line);
    let slice = text.line(line);
    let breaks = slice
        .chars_at(slice.len_chars())
        .reversed()
        .take_while(|ch| *ch == '\n' || *ch == '\r')
        .count();
    let end = start + slice.len_chars() - breaks;
    let start_cu = text.char_to_utf16_cu(start);
    let unit = usize::try_from(position.character).unwrap_or(usize::MAX);
    let target = start_cu
        .saturating_add(unit)
        .min(text.char_to_utf16_cu(end));
    text.utf16_cu_to_char(target)
}

/// Converts a char offset to a protocol position.
pub fn char_to_position(text: &Rope, pos: usize) -> Position {
    let pos = pos.min(text.len_chars());
    let line = text.char_to_line(pos);
    let start = text.line_to_char(line);
    let character = text.char_to_utf16_cu(pos) - text.char_to_utf16_cu(start);
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(character).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
/// Tests for the conversions.
mod tests {
    use std::path::PathBuf;

    use lsp_types::Position;
    use ropey::Rope;

    use super::{char_to_position, path_to_uri, position_to_char, uri_to_path};

    /// Paths with spaces survive a round trip through a URI.
    #[test]
    fn path_uri_round_trip() {
        let path = if cfg!(windows) {
            PathBuf::from(r"D:\code\my proj\main.rs")
        } else {
            PathBuf::from("/code/my proj/main.rs")
        };
        let uri = path_to_uri(&path).expect("absolute path");
        assert!(uri.as_str().starts_with("file:///"));
        assert!(uri.as_str().ends_with("my%20proj/main.rs"));
        assert_eq!(uri_to_path(&uri), Some(path));
    }

    /// Relative paths have no URI.
    #[test]
    fn relative_path_has_no_uri() {
        assert!(path_to_uri(&PathBuf::from("main.rs")).is_none());
    }

    /// Characters outside the basic plane count as two UTF-16 units.
    #[test]
    fn positions_count_utf16() {
        let text = Rope::from_str("a\n\u{1F600}b");
        assert_eq!(position_to_char(&text, Position::new(1, 2)), 3);
        assert_eq!(char_to_position(&text, 3), Position::new(1, 2));
        assert_eq!(position_to_char(&text, Position::new(0, 99)), 1);
        assert_eq!(position_to_char(&text, Position::new(9, 0)), 4);
    }
}
