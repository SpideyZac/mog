//! Comparing the buffer against the last commit, line by line.

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

#[cfg(test)]
/// Tests for [`line_changes`].
mod tests {
    use super::{LineChange, line_changes};

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
