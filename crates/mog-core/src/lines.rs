//! Edits that act on whole lines, like commenting, moving and indenting.
//!
//! Each function returns the [`Transaction`] to apply and the selection to have afterwards, so
//! they stay easy to test without an editor.

use ropey::Rope;

use crate::{
    movement,
    range::Range,
    transaction::{Change, Transaction},
};

/// A planned edit and the selection it leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    /// The changes to make.
    pub tx: Transaction,
    /// The selection afterwards.
    pub selection: Range,
}

/// Returns the line comment token for files with `extension`, if the language has one.
pub fn comment_token(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "hh" | "hxx" | "ino" | "js" | "jsx"
        | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "go" | "java" | "kt" | "kts" | "swift"
        | "cs" | "csx" | "zig" | "zon" | "dart" | "scss" | "jsonc" | "json5" | "scala" | "sc"
        | "sbt" | "php" | "glsl" | "proto" => "//",
        "py" | "pyi" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml" | "rb" | "rake"
        | "gemspec" | "pl" | "r" | "nix" | "conf" | "ps1" | "psm1" | "psd1" | "cmake"
        | "dockerfile" | "gitignore" | "ex" | "exs" | "mk" | "mak" | "jl" | "tf" => "#",
        "lua" | "sql" | "hs" | "elm" => "--",
        "vim" => "\"",
        "lisp" | "clj" | "scm" | "el" | "ini" | "asm" => ";",
        "tex" | "erl" => "%",
        _ => return None,
    })
}

/// Returns the first and last line touched by `selection`.
///
/// A selection ending at the very start of a line does not count that line.
pub fn selected_lines(text: &Rope, selection: Range) -> (usize, usize) {
    let first = text.char_to_line(selection.from());
    let mut last = text.char_to_line(selection.to());
    if last > first && text.line_to_char(last) == selection.to() {
        last -= 1;
    }
    (first, last)
}

/// Maps both ends of `selection` through `tx`.
fn map_selection(tx: &Transaction, selection: Range) -> Range {
    Range::new(tx.map_pos(selection.anchor), tx.map_pos(selection.head))
}

/// Returns the offset of the first non blank char of `line`, or `None` for blank lines.
fn first_non_blank(text: &Rope, line: usize) -> Option<usize> {
    let start = text.line_to_char(line);
    let len = movement::line_len(text, line);
    text.slice(start..start + len)
        .chars()
        .position(|ch| ch != ' ' && ch != '\t')
        .map(|i| start + i)
}

/// Comments the selected lines with `token`, or uncomments them if they all are commented.
pub fn toggle_comment(text: &Rope, selection: Range, token: &str) -> LineEdit {
    let (first, last) = selected_lines(text, selection);
    let lines: Vec<(usize, usize)> = (first..=last)
        .filter_map(|line| first_non_blank(text, line).map(|pos| (line, pos)))
        .collect();
    let token_len = token.chars().count();
    let commented = |pos: usize| {
        text.slice(pos..text.len_chars())
            .chars()
            .take(token_len)
            .eq(token.chars())
    };
    let all_commented = !lines.is_empty() && lines.iter().all(|&(_, pos)| commented(pos));
    let changes = if all_commented {
        lines
            .iter()
            .map(|&(_, pos)| {
                let space = text.get_char(pos + token_len) == Some(' ');
                Change {
                    start: pos,
                    end: pos + token_len + usize::from(space),
                    text: String::new(),
                }
            })
            .collect()
    } else {
        // comment at the shallowest indent so the markers line up
        let indent = lines
            .iter()
            .map(|&(line, pos)| pos - text.line_to_char(line))
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .map(|&(line, _)| {
                let at = text.line_to_char(line) + indent;
                Change {
                    start: at,
                    end: at,
                    text: format!("{token} "),
                }
            })
            .collect()
    };
    let tx = Transaction::new(changes);
    LineEdit {
        selection: map_selection(&tx, selection),
        tx,
    }
}

/// Copies the selected lines below themselves and selects the copy.
pub fn duplicate_lines(text: &Rope, selection: Range, ending: &str) -> LineEdit {
    let (first, last) = selected_lines(text, selection);
    let start = text.line_to_char(first);
    let end = movement::line_end(text, text.line_to_char(last));
    let block = text.slice(start..end).to_string();
    let inserted = format!("{ending}{block}");
    let shift = inserted.chars().count();
    LineEdit {
        tx: Transaction::insert(end, inserted),
        selection: Range::new(selection.anchor + shift, selection.head + shift),
    }
}

/// Deletes the selected lines including their line break.
pub fn delete_lines(text: &Rope, selection: Range) -> LineEdit {
    let (first, last) = selected_lines(text, selection);
    let mut start = text.line_to_char(first);
    let end = if last + 1 < text.len_lines() {
        text.line_to_char(last + 1)
    } else {
        // the last line has no break after it so take the one before instead
        let end = text.len_chars();
        if first > 0 {
            start = movement::line_end(text, text.line_to_char(first - 1));
        }
        end
    };
    LineEdit {
        tx: Transaction::delete(start, end),
        selection: Range::point(start.min(text.len_chars() - (end - start))),
    }
}

/// Swaps the selected lines with the line above, or below if `up` is not set.
///
/// Returns `None` when there is no line to swap with.
pub fn move_lines(text: &Rope, selection: Range, up: bool) -> Option<LineEdit> {
    let (first, last) = selected_lines(text, selection);
    let lines = text.len_lines();
    let (other, block_first, block_last) = if up {
        (first.checked_sub(1)?, first, last)
    } else if last + 1 < lines {
        (last + 1, first, last)
    } else {
        return None;
    };
    let span = |from: usize, to: usize| {
        let start = text.line_to_char(from);
        let end = movement::line_end(text, text.line_to_char(to));
        (start, end, text.slice(start..end).to_string())
    };
    let (block_start, block_end, block) = span(block_first, block_last);
    let (other_start, other_end, other_text) = span(other, other);
    let (tx, moved) = if up {
        let gap = text.slice(other_end..block_start).to_string();
        let moved = other_text.chars().count() + gap.chars().count();
        let swapped = format!("{block}{gap}{other_text}");
        (Transaction::replace(other_start, block_end, swapped), moved)
    } else {
        let gap = text.slice(block_end..other_start).to_string();
        let moved = other_text.chars().count() + gap.chars().count();
        let swapped = format!("{other_text}{gap}{block}");
        (Transaction::replace(block_start, other_end, swapped), moved)
    };
    let selection = if up {
        Range::new(selection.anchor - moved, selection.head - moved)
    } else {
        Range::new(selection.anchor + moved, selection.head + moved)
    };
    Some(LineEdit { tx, selection })
}

/// Adds one level of indentation, `unit`, to the start of every selected line that is not blank.
pub fn indent_lines(text: &Rope, selection: Range, unit: &str) -> LineEdit {
    let (first, last) = selected_lines(text, selection);
    let changes = (first..=last)
        .filter(|&line| movement::line_len(text, line) > 0)
        .map(|line| {
            let at = text.line_to_char(line);
            Change {
                start: at,
                end: at,
                text: unit.to_owned(),
            }
        })
        .collect();
    let tx = Transaction::new(changes);
    LineEdit {
        selection: map_selection(&tx, selection),
        tx,
    }
}

/// Removes up to one level of indentation from every selected line.
pub fn outdent_lines(text: &Rope, selection: Range, tab_width: usize) -> LineEdit {
    let (first, last) = selected_lines(text, selection);
    let changes = (first..=last)
        .filter_map(|line| {
            let start = text.line_to_char(line);
            let len = movement::line_len(text, line);
            let mut width = 0;
            let mut count = 0;
            for ch in text.slice(start..start + len).chars() {
                match ch {
                    '\t' if count == 0 => {
                        count = 1;
                        break;
                    }
                    ' ' if width < tab_width.max(1) => {
                        width += 1;
                        count += 1;
                    }
                    _ => break,
                }
            }
            (count > 0).then(|| Change {
                start,
                end: start + count,
                text: String::new(),
            })
        })
        .collect();
    let tx = Transaction::new(changes);
    LineEdit {
        selection: map_selection(&tx, selection),
        tx,
    }
}

#[cfg(test)]
/// Tests for line edits.
mod tests {
    use ropey::Rope;

    use super::{
        LineEdit, delete_lines, duplicate_lines, indent_lines, move_lines, outdent_lines,
        toggle_comment,
    };
    use crate::range::Range;

    /// Applies `edit` to `text` and returns the result.
    fn apply(text: &str, edit: &LineEdit) -> String {
        let mut rope = Rope::from_str(text);
        edit.tx.apply(&mut rope);
        rope.to_string()
    }

    /// Commenting lines up markers at the shallowest indent and toggling undoes it.
    #[test]
    fn toggles_comments() {
        let text = "  a\n    b\n\n  c";
        let rope = Rope::from_str(text);
        let all = Range::new(0, rope.len_chars());
        let edit = toggle_comment(&rope, all, "//");
        let commented = apply(text, &edit);
        assert_eq!(commented, "  // a\n  //   b\n\n  // c");
        let rope = Rope::from_str(&commented);
        let back = toggle_comment(&rope, Range::new(0, rope.len_chars()), "//");
        assert_eq!(apply(&commented, &back), text);
    }

    /// Duplicating copies the line below and moves the cursor onto the copy.
    #[test]
    fn duplicates() {
        let rope = Rope::from_str("one\ntwo");
        let edit = duplicate_lines(&rope, Range::point(1), "\n");
        assert_eq!(apply("one\ntwo", &edit), "one\none\ntwo");
        assert_eq!(edit.selection, Range::point(5));
    }

    /// Moving swaps lines and keeps the cursor on the moved text.
    #[test]
    fn moves_lines() {
        let text = "a\nbb\nccc";
        let rope = Rope::from_str(text);
        let down = move_lines(&rope, Range::point(3), false).expect("can move");
        assert_eq!(apply(text, &down), "a\nccc\nbb");
        assert_eq!(down.selection, Range::point(7));
        let up = move_lines(&rope, Range::point(3), true).expect("can move");
        assert_eq!(apply(text, &up), "bb\na\nccc");
        assert_eq!(up.selection, Range::point(1));
        assert!(move_lines(&rope, Range::point(0), true).is_none());
        assert!(move_lines(&rope, Range::point(7), false).is_none());
    }

    /// Deleting removes the whole line, the last one included.
    #[test]
    fn deletes_lines() {
        let rope = Rope::from_str("a\nb\nc");
        assert_eq!(
            apply("a\nb\nc", &delete_lines(&rope, Range::point(2))),
            "a\nc"
        );
        assert_eq!(
            apply("a\nb\nc", &delete_lines(&rope, Range::point(4))),
            "a\nb"
        );
    }

    /// Indenting skips blank lines and outdenting removes one level.
    #[test]
    fn indents_and_outdents() {
        let text = "a\n\n      b";
        let rope = Rope::from_str(text);
        let all = Range::new(0, rope.len_chars());
        assert_eq!(
            apply(text, &indent_lines(&rope, all, "    ")),
            "    a\n\n          b"
        );
        assert_eq!(apply(text, &outdent_lines(&rope, all, 4)), "a\n\n  b");
    }
}
