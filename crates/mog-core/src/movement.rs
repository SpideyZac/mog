//! Cursor motions over text.
//!
//! All positions are char offsets. A `\r\n` pair is treated as a single step.

use ropey::Rope;

/// Bracket pairs as `(open, close)`.
pub const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// How far bracket matching looks before giving up, in chars.
const BRACKET_SCAN_LIMIT: usize = 20_000;

/// A rough classification of chars used for word motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    /// Letters, digits and underscores.
    Word,
    /// Spaces, tabs and line breaks.
    Space,
    /// Everything else.
    Punct,
}

impl CharClass {
    /// Classifies `ch`.
    fn of(ch: char) -> Self {
        if ch.is_alphanumeric() || ch == '_' {
            Self::Word
        } else if ch.is_whitespace() {
            Self::Space
        } else {
            Self::Punct
        }
    }
}

/// Returns the number of chars on `line`, not counting its line break.
///
/// # Panics
///
/// Panics if `line` is past the last line.
pub fn line_len(text: &Rope, line: usize) -> usize {
    let slice = text.line(line);
    let mut len = slice.len_chars();
    if len > 0 && slice.char(len - 1) == '\n' {
        len -= 1;
        if len > 0 && slice.char(len - 1) == '\r' {
            len -= 1;
        }
    }
    len
}

/// Returns the column of `pos` within its line.
pub fn col_of(text: &Rope, pos: usize) -> usize {
    pos - text.line_to_char(text.char_to_line(pos))
}

/// Returns the offset of `col` on `line`, clamped to the end of the line.
///
/// # Panics
///
/// Panics if `line` is past the last line.
pub fn pos_at(text: &Rope, line: usize, col: usize) -> usize {
    text.line_to_char(line) + col.min(line_len(text, line))
}

/// Moves one char left, stepping over `\r\n` as one.
pub fn left(text: &Rope, pos: usize) -> usize {
    if pos >= 2 && text.char(pos - 1) == '\n' && text.char(pos - 2) == '\r' {
        pos - 2
    } else {
        pos.saturating_sub(1)
    }
}

/// Moves one char right, stepping over `\r\n` as one.
pub fn right(text: &Rope, pos: usize) -> usize {
    let len = text.len_chars();
    if pos + 1 < len && text.char(pos) == '\r' && text.char(pos + 1) == '\n' {
        pos + 2
    } else {
        (pos + 1).min(len)
    }
}

/// Moves `count` lines up or down, keeping `col` where possible.
///
/// Negative counts move up. Moving past the first or last line lands on the start or end of the
/// text.
pub fn vertical(text: &Rope, pos: usize, count: isize, col: usize) -> usize {
    let line = text.char_to_line(pos);
    let last = text.len_lines() - 1;
    match line.checked_add_signed(count) {
        None => 0,
        Some(target) if target > last => text.len_chars(),
        Some(target) => pos_at(text, target, col),
    }
}

/// Returns the start of the line `pos` is on.
pub fn line_start(text: &Rope, pos: usize) -> usize {
    text.line_to_char(text.char_to_line(pos))
}

/// Returns the end of the line `pos` is on, before its line break.
pub fn line_end(text: &Rope, pos: usize) -> usize {
    let line = text.char_to_line(pos);
    pos_at(text, line, usize::MAX)
}

/// Returns the first non blank char of the line, or the line start if already there.
///
/// This gives the usual "smart home" toggle.
pub fn smart_home(text: &Rope, pos: usize) -> usize {
    let start = line_start(text, pos);
    let end = line_end(text, pos);
    let indent = (start..end)
        .find(|&i| !text.char(i).is_whitespace())
        .unwrap_or(end);
    if pos == indent { start } else { indent }
}

/// Moves to the start of the previous word.
pub fn word_left(text: &Rope, pos: usize) -> usize {
    let mut pos = pos;
    while pos > 0 && CharClass::of(text.char(pos - 1)) == CharClass::Space {
        pos -= 1;
    }
    if pos == 0 {
        return 0;
    }
    let class = CharClass::of(text.char(pos - 1));
    while pos > 0 && CharClass::of(text.char(pos - 1)) == class {
        pos -= 1;
    }
    pos
}

/// Moves to the end of the next word.
pub fn word_right(text: &Rope, pos: usize) -> usize {
    let len = text.len_chars();
    let mut pos = pos;
    while pos < len && CharClass::of(text.char(pos)) == CharClass::Space {
        pos += 1;
    }
    if pos == len {
        return len;
    }
    let class = CharClass::of(text.char(pos));
    while pos < len && CharClass::of(text.char(pos)) == class {
        pos += 1;
    }
    pos
}

/// Returns the span of same class chars around `pos`, used for double click selection.
pub fn word_at(text: &Rope, pos: usize) -> (usize, usize) {
    let len = text.len_chars();
    if len == 0 {
        return (0, 0);
    }
    let pos = pos.min(len - 1);
    let class = CharClass::of(text.char(pos));
    let mut start = pos;
    while start > 0 && CharClass::of(text.char(start - 1)) == class {
        start -= 1;
    }
    let mut end = pos;
    while end < len && CharClass::of(text.char(end)) == class {
        end += 1;
    }
    (start, end)
}

/// Returns the span of `line` including its line break, used for line selection.
///
/// # Panics
///
/// Panics if `line` is past the last line.
pub fn line_span(text: &Rope, line: usize) -> (usize, usize) {
    let start = text.line_to_char(line);
    (start, start + text.line(line).len_chars())
}

/// Finds the bracket matching one at or just before `pos`.
///
/// Returns `(bracket, partner)` offsets, or `None` when there is no bracket or no partner nearby.
pub fn matching_bracket(text: &Rope, pos: usize) -> Option<(usize, usize)> {
    let len = text.len_chars();
    let candidates = [Some(pos), pos.checked_sub(1)];
    for at in candidates.into_iter().flatten().filter(|&at| at < len) {
        let ch = text.char(at);
        for (open, close) in BRACKETS {
            if ch == open {
                return scan_bracket(text, at, open, close, true).map(|other| (at, other));
            }
            if ch == close {
                return scan_bracket(text, at, open, close, false).map(|other| (at, other));
            }
        }
    }
    None
}

/// Scans from the bracket at `from` for its partner, forward if `forward` is set.
fn scan_bracket(text: &Rope, from: usize, open: char, close: char, forward: bool) -> Option<usize> {
    let mut depth = 0usize;
    let (start, step_char) = if forward {
        (open, close)
    } else {
        (close, open)
    };
    let positions: Box<dyn Iterator<Item = usize>> = if forward {
        Box::new((from + 1..text.len_chars()).take(BRACKET_SCAN_LIMIT))
    } else {
        Box::new((0..from).rev().take(BRACKET_SCAN_LIMIT))
    };
    for at in positions {
        let ch = text.char(at);
        if ch == start {
            depth += 1;
        } else if ch == step_char {
            if depth == 0 {
                return Some(at);
            }
            depth -= 1;
        }
    }
    None
}

#[cfg(test)]
/// Tests for the motions.
mod tests {
    use ropey::Rope;

    use super::{
        left, line_end, line_len, matching_bracket, right, smart_home, vertical, word_at,
        word_left, word_right,
    };

    /// Brackets match across nesting in both directions.
    #[test]
    fn matches_brackets() {
        let text = Rope::from_str("f(a[1], (b))");
        assert_eq!(matching_bracket(&text, 1), Some((1, 11)));
        assert_eq!(matching_bracket(&text, 12), Some((11, 1)));
        assert_eq!(matching_bracket(&text, 3), Some((3, 5)));
        assert_eq!(matching_bracket(&text, 0), None);
        assert_eq!(matching_bracket(&Rope::from_str("(("), 0), None);
    }

    /// `\r\n` counts as one step both ways.
    #[test]
    fn crlf_is_one_step() {
        let text = Rope::from_str("a\r\nb");
        assert_eq!(right(&text, 1), 3);
        assert_eq!(left(&text, 3), 1);
    }

    /// Line length excludes the line break.
    #[test]
    fn line_len_skips_break() {
        let text = Rope::from_str("abc\r\nde\n");
        assert_eq!(line_len(&text, 0), 3);
        assert_eq!(line_len(&text, 1), 2);
        assert_eq!(line_len(&text, 2), 0);
        assert_eq!(line_end(&text, 0), 3);
    }

    /// Vertical motion clamps the column and the line.
    #[test]
    fn vertical_clamps() {
        let text = Rope::from_str("abcdef\nab\nabcd");
        assert_eq!(vertical(&text, 5, 1, 5), 9);
        assert_eq!(vertical(&text, 9, 1, 5), 14);
        assert_eq!(vertical(&text, 2, -1, 2), 0);
        assert_eq!(vertical(&text, 12, 5, 2), 14);
    }

    /// Word motions skip whitespace then a run of one class.
    #[test]
    fn word_motions() {
        let text = Rope::from_str("let foo = bar;");
        assert_eq!(word_right(&text, 0), 3);
        assert_eq!(word_right(&text, 3), 7);
        assert_eq!(word_left(&text, 7), 4);
        assert_eq!(word_left(&text, 4), 0);
        assert_eq!(word_at(&text, 5), (4, 7));
    }

    /// Smart home toggles between indent and line start.
    #[test]
    fn smart_home_toggles() {
        let text = Rope::from_str("    foo");
        assert_eq!(smart_home(&text, 6), 4);
        assert_eq!(smart_home(&text, 4), 0);
    }
}
