//! Atomic groups of text changes.

use ropey::Rope;

/// A replacement of the chars in `start..end` with `text`.
///
/// Offsets are char indices into the text the change is applied to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The first char offset that is replaced.
    pub start: usize,
    /// The char offset one past the last replaced char.
    pub end: usize,
    /// The text inserted in place of the replaced chars.
    pub text: String,
}

/// A set of non-overlapping [`Change`]s that are applied together as one edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transaction {
    /// The changes, sorted by `start` and never overlapping.
    changes: Vec<Change>,
}

impl Transaction {
    /// Creates a transaction from `changes` in any order.
    ///
    /// # Panics
    ///
    /// Panics if any two changes overlap or a change has `start > end`.
    pub fn new(mut changes: Vec<Change>) -> Self {
        changes.sort_by_key(|change| change.start);
        for change in &changes {
            assert!(change.start <= change.end, "change starts after it ends");
        }
        for pair in changes.windows(2) {
            assert!(pair[0].end <= pair[1].start, "changes overlap");
        }
        Self { changes }
    }

    /// Creates a transaction that inserts `text` at `pos`.
    pub fn insert(pos: usize, text: impl Into<String>) -> Self {
        Self::replace(pos, pos, text)
    }

    /// Creates a transaction that deletes the chars in `start..end`.
    pub fn delete(start: usize, end: usize) -> Self {
        Self::replace(start, end, "")
    }

    /// Creates a transaction that replaces the chars in `start..end` with `text`.
    ///
    /// # Panics
    ///
    /// Panics if `start > end`.
    pub fn replace(start: usize, end: usize, text: impl Into<String>) -> Self {
        Self::new(vec![Change {
            start,
            end,
            text: text.into(),
        }])
    }

    /// Returns the changes in order.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// Returns `true` if the transaction changes nothing.
    pub fn is_empty(&self) -> bool {
        self.changes
            .iter()
            .all(|change| change.start == change.end && change.text.is_empty())
    }

    /// Applies the transaction to `rope` and returns the transaction that undoes it.
    ///
    /// # Panics
    ///
    /// Panics if any change is out of bounds for `rope`.
    pub fn apply(&self, rope: &mut Rope) -> Transaction {
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut shift: isize = 0;
        for change in &self.changes {
            let start = offset(change.start, shift);
            let end = offset(change.end, shift);
            let removed = rope.slice(start..end).to_string();
            rope.remove(start..end);
            rope.insert(start, &change.text);
            let inserted = change.text.chars().count();
            inverse.push(Change {
                start,
                end: start + inserted,
                text: removed,
            });
            shift += signed(inserted) - signed(end - start);
        }
        Self { changes: inverse }
    }

    /// Returns where `pos` ends up after the transaction is applied.
    ///
    /// Positions inside a replaced span move to the end of its inserted text.
    pub fn map_pos(&self, pos: usize) -> usize {
        let mut shift: isize = 0;
        for change in &self.changes {
            if pos < change.start {
                break;
            }
            let inserted = signed(change.text.chars().count());
            if pos < change.end || (pos == change.start && change.start == change.end) {
                return offset(change.start, shift + inserted);
            }
            shift += inserted - signed(change.end - change.start);
        }
        offset(pos, shift)
    }
}

/// Converts a char count to a signed shift.
///
/// # Panics
///
/// Panics if `count` does not fit in an [`isize`], which a real document never reaches.
fn signed(count: usize) -> isize {
    isize::try_from(count).expect("char count fits in isize")
}

/// Applies a signed `shift` to `pos`.
///
/// # Panics
///
/// Panics if the result would be negative.
fn offset(pos: usize, shift: isize) -> usize {
    pos.checked_add_signed(shift)
        .expect("shifted offset is negative")
}

#[cfg(test)]
/// Tests for [`Transaction`].
mod tests {
    use ropey::Rope;

    use super::{Change, Transaction};

    /// Applying then applying the inverse restores the original text.
    #[test]
    fn inverse_undoes_apply() {
        let mut rope = Rope::from_str("hello world");
        let tx = Transaction::new(vec![
            Change {
                start: 6,
                end: 11,
                text: "mog".into(),
            },
            Change {
                start: 0,
                end: 0,
                text: ">> ".into(),
            },
        ]);
        let inverse = tx.apply(&mut rope);
        assert_eq!(rope.to_string(), ">> hello mog");
        inverse.apply(&mut rope);
        assert_eq!(rope.to_string(), "hello world");
    }

    /// Positions before, inside and after a change map correctly.
    #[test]
    fn map_pos_shifts_positions() {
        let tx = Transaction::replace(2, 4, "abc");
        assert_eq!(tx.map_pos(1), 1);
        assert_eq!(tx.map_pos(3), 5);
        assert_eq!(tx.map_pos(6), 7);
    }

    /// A cursor at an insertion point moves past the inserted text.
    #[test]
    fn map_pos_moves_past_insert() {
        assert_eq!(Transaction::insert(3, "xy").map_pos(3), 5);
    }

    /// Overlapping changes are rejected.
    #[test]
    #[should_panic(expected = "changes overlap")]
    fn overlapping_changes_panic() {
        Transaction::new(vec![
            Change {
                start: 0,
                end: 3,
                text: String::new(),
            },
            Change {
                start: 2,
                end: 4,
                text: String::new(),
            },
        ]);
    }
}
