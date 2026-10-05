//! Notes a language server pins to lines, like inlay hints and semantic tokens.
//!
//! Servers answer for one version of the text, which may be old by the time the answer arrives.
//! Each line keeps the text it had then, so marks on lines edited since are left out instead of
//! being drawn in the wrong place.

use std::collections::HashMap;

use ropey::Rope;

/// A short note shown after a line, like a type or a parameter name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    /// The char column in the line the note belongs to.
    pub col: usize,
    /// The note text.
    pub label: String,
}

/// What a language server says a run of text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A module or namespace.
    Namespace,
    /// A type, class, trait or type parameter.
    Type,
    /// A function or method.
    Function,
    /// A macro or attribute.
    Macro,
    /// A field or property.
    Property,
    /// A variant of an enum.
    EnumMember,
    /// A constant or static.
    Constant,
    /// A local variable.
    Variable,
    /// A function parameter.
    Parameter,
    /// A keyword.
    Keyword,
    /// A string.
    String,
    /// A number.
    Number,
    /// A comment.
    Comment,
    /// An operator.
    Operator,
}

/// A run of text a language server classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticToken {
    /// The char column in the line it starts at.
    pub from: usize,
    /// The char column in the line it ends before.
    pub to: usize,
    /// What it is.
    pub kind: TokenKind,
}

/// Marks of one kind for each line, with the line text they were made for.
#[derive(Debug, Clone)]
pub struct LineMarks<T> {
    /// The marks by line, with the text of the line when they were made.
    lines: HashMap<usize, (String, Vec<T>)>,
}

impl<T> Default for LineMarks<T> {
    fn default() -> Self {
        Self {
            lines: HashMap::new(),
        }
    }
}

impl<T> LineMarks<T> {
    /// Groups `marks`, given as `(line, mark)`, by line and remembers each line of `text`.
    pub fn new(text: &Rope, marks: impl IntoIterator<Item = (usize, T)>) -> Self {
        let mut lines: HashMap<usize, (String, Vec<T>)> = HashMap::new();
        for (line, mark) in marks {
            if line >= text.len_lines() {
                continue;
            }
            lines
                .entry(line)
                .or_insert_with(|| (text.line(line).to_string(), Vec::new()))
                .1
                .push(mark);
        }
        Self { lines }
    }

    /// Returns the marks of `line`, or none if the line changed since they were made.
    pub fn on_line(&self, text: &Rope, line: usize) -> &[T] {
        match self.lines.get(&line) {
            Some((was, marks)) if line < text.len_lines() && text.line(line) == was.as_str() => {
                marks
            }
            _ => &[],
        }
    }

    /// Returns `true` if there are no marks at all.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[cfg(test)]
/// Tests for [`LineMarks`].
mod tests {
    use ropey::Rope;

    use super::LineMarks;

    /// Marks stay on lines that did not change and vanish from ones that did.
    #[test]
    fn drops_marks_on_changed_lines() {
        let before = Rope::from_str("let a = 1;\nlet b = 2;\n");
        let marks = LineMarks::new(&before, [(0, "a"), (1, "b"), (1, "c"), (9, "gone")]);
        assert_eq!(marks.on_line(&before, 1), ["b", "c"]);
        let after = Rope::from_str("let a = 1;\nlet b = 22;\n");
        assert_eq!(marks.on_line(&after, 0), ["a"]);
        assert!(marks.on_line(&after, 1).is_empty());
        assert!(marks.on_line(&after, 7).is_empty());
    }
}
