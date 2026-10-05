//! Comparing the buffer against the last commit, line by line.

use std::ops::Range;

use similar::{DiffTag, TextDiff};

/// How a line differs from the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineChange {
    /// The line is new.
    Added,
    /// The line replaced one or more old lines.
    Modified,
    /// Lines were removed just above this line.
    RemovedAbove,
}

/// Returns the changed lines of `current` compared to `base` as `(line, change)` pairs.
///
/// Lines are counted from 0 and sorted.
pub fn line_changes(base: &str, current: &str) -> Vec<(usize, LineChange)> {
    let diff = TextDiff::from_lines(base, current);
    let mut changes = Vec::new();
    for op in diff.ops() {
        let new = op.new_range();
        match op.tag() {
            DiffTag::Equal => {}
            DiffTag::Insert => changes.extend(new.map(|line| (line, LineChange::Added))),
            DiffTag::Replace => changes.extend(new.map(|line| (line, LineChange::Modified))),
            DiffTag::Delete => changes.push((new.start, LineChange::RemovedAbove)),
        }
    }
    changes
}

/// A run of changed lines, as the lines it replaced and the lines that replaced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The replaced lines of the old text, counted from 0.
    pub old: Range<usize>,
    /// The lines of the new text that replaced them, counted from 0.
    pub new: Range<usize>,
}

impl Hunk {
    /// Returns whether `line` of the new text belongs to the hunk.
    ///
    /// A hunk that only removed lines sits between two lines, so it counts the line right after
    /// the gap and the one right before it.
    pub fn touches(&self, line: usize) -> bool {
        if self.new.is_empty() {
            line == self.new.start || line + 1 == self.new.start
        } else {
            self.new.contains(&line)
        }
    }
}

/// Splits `text` into lines that keep their line breaks.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Returns the runs of lines that differ between `old` and `new`.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let mut hunks: Vec<Hunk> = Vec::new();
    for op in diff.ops() {
        if op.tag() == DiffTag::Equal {
            continue;
        }
        let (old_range, new_range) = (op.old_range(), op.new_range());
        // ops of one change come back to back, like a delete then an insert
        match hunks.last_mut() {
            Some(last) if last.old.end == old_range.start && last.new.end == new_range.start => {
                last.old.end = old_range.end;
                last.new.end = new_range.end;
            }
            _ => hunks.push(Hunk {
                old: old_range,
                new: new_range,
            }),
        }
    }
    hunks
}

/// Returns `old` with the lines `hunk` replaced swapped for its lines from `new`.
pub fn apply_hunk(old: &str, new: &str, hunk: &Hunk) -> String {
    let (old_lines, new_lines) = (lines(old), lines(new));
    let mut out = String::with_capacity(old.len());
    let keep = |range: Range<usize>, lines: &[&str], out: &mut String| {
        for line in lines.get(range).unwrap_or_default() {
            out.push_str(line);
        }
    };
    keep(0..hunk.old.start, &old_lines, &mut out);
    keep(hunk.new.clone(), &new_lines, &mut out);
    keep(hunk.old.end..old_lines.len(), &old_lines, &mut out);
    out
}

/// Returns `new` with the lines of `hunk` put back to what they were in `old`.
pub fn revert_hunk(old: &str, new: &str, hunk: &Hunk) -> String {
    let flipped = Hunk {
        old: hunk.new.clone(),
        new: hunk.old.clone(),
    };
    apply_hunk(new, old, &flipped)
}

/// Returns the line of `old` that `line` of `new` lines up with.
pub fn map_line(old: &str, new: &str, line: usize) -> usize {
    let diff = TextDiff::from_lines(old, new);
    for op in diff.ops() {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        if new_range.contains(&line) || (new_range.is_empty() && new_range.start == line) {
            let offset = (line - new_range.start).min(old_range.len().saturating_sub(1));
            return old_range.start + offset;
        }
    }
    line
}

#[cfg(test)]
/// Tests for [`line_changes`] and hunks.
mod tests {
    use super::{Hunk, LineChange, apply_hunk, hunks, line_changes, map_line, revert_hunk};

    /// Changes next to each other are one hunk, ones apart are two.
    #[test]
    fn groups_hunks() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\nf\n";
        let found = hunks(old, new);
        assert_eq!(
            found,
            [
                Hunk {
                    old: 1..2,
                    new: 1..2
                },
                Hunk {
                    old: 4..5,
                    new: 4..6
                },
            ]
        );
        assert!(found[1].touches(5));
        assert!(!found[1].touches(3));
    }

    /// Applying one hunk takes only that change, reverting puts it back.
    #[test]
    fn applies_and_reverts_one_hunk() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\nf\n";
        let found = hunks(old, new);
        assert_eq!(apply_hunk(old, new, &found[1]), "a\nb\nc\nd\nE\nf\n");
        assert_eq!(revert_hunk(old, new, &found[0]), "a\nb\nc\nd\nE\nf\n");
        let removed = hunks("a\nb\nc\n", "a\nc\n");
        assert!(removed[0].touches(1) && removed[0].touches(0));
        assert_eq!(apply_hunk("a\nb\nc\n", "a\nc\n", &removed[0]), "a\nc\n");
    }

    /// Lines of the new text map back past inserted lines.
    #[test]
    fn maps_lines_back() {
        assert_eq!(map_line("a\nb\n", "a\nnew\nb\n", 2), 1);
        assert_eq!(map_line("a\nb\n", "a\nb\n", 1), 1);
    }

    /// Inserted, replaced and deleted lines are told apart.
    #[test]
    fn detects_each_kind() {
        let base = "a\nb\nc\nd\n";
        let current = "a\nB\nc\nnew\n";
        assert_eq!(
            line_changes(base, current),
            [(1, LineChange::Modified), (3, LineChange::Modified)]
        );
        assert_eq!(line_changes("a\n", "a\nb\n"), [(1, LineChange::Added)]);
        assert_eq!(
            line_changes("a\nb\nc\n", "a\nc\n"),
            [(1, LineChange::RemovedAbove)]
        );
    }

    /// Identical text has no changes.
    #[test]
    fn identical_is_clean() {
        assert!(line_changes("same\n", "same\n").is_empty());
    }
}
