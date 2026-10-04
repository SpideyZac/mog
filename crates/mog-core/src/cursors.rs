//! Editing with several cursors at once.
//!
//! These functions take every cursor, the main one first, and return the transaction to apply
//! plus where the cursors end up, so the editor stays in charge of history and the view.

use ropey::Rope;

use crate::{
    range::Range,
    search,
    transaction::{Change, Transaction},
    view,
};

/// Builds one change per cursor with `change`, then returns the transaction and the cursors
/// after it, main cursor first.
///
/// Changes that would overlap an earlier one are skipped along with their cursor.
pub fn edit_all(ranges: &[Range], change: impl Fn(Range) -> Change) -> (Transaction, Vec<Range>) {
    let mut changes: Vec<(usize, Change)> = ranges
        .iter()
        .map(|&range| change(range))
        .enumerate()
        .collect();
    changes.sort_by_key(|(_, change)| change.start);
    let mut kept: Vec<(usize, Change)> = Vec::with_capacity(changes.len());
    for (i, change) in changes {
        if kept.last().is_none_or(|(_, last)| last.end <= change.start) {
            kept.push((i, change));
        }
    }
    let starts: Vec<(usize, usize)> = kept.iter().map(|(i, change)| (*i, change.start)).collect();
    let tx = Transaction::new(kept.into_iter().map(|(_, change)| change).collect());
    let mut after: Vec<(usize, Range)> = starts
        .into_iter()
        .map(|(i, start)| (i, Range::point(tx.map_pos(start))))
        .collect();
    after.sort_by_key(|(i, _)| *i);
    (
        tx,
        dedup(after.into_iter().map(|(_, range)| range).collect()),
    )
}

/// Removes cursors that sit on the same spot as an earlier one.
pub fn dedup(ranges: Vec<Range>) -> Vec<Range> {
    let mut seen: Vec<Range> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if !seen.iter().any(|other| other.head == range.head) {
            seen.push(range);
        }
    }
    seen
}

/// Returns the range of the next occurrence of the main selection's text after the last cursor,
/// wrapping around, or `None` if there is no other occurrence.
pub fn next_occurrence(text: &Rope, ranges: &[Range]) -> Option<Range> {
    let main = *ranges.first()?;
    if main.is_empty() {
        return None;
    }
    let needle = text.slice(main.from()..main.to()).to_string();
    let matches = search::find_all(text, &needle, true);
    let after = ranges.iter().map(Range::to).max().unwrap_or(0);
    let taken = |from: usize| ranges.iter().any(|range| range.from() == from);
    matches
        .iter()
        .filter(|(from, _)| *from >= after)
        .chain(matches.iter())
        .find(|(from, _)| !taken(*from))
        .map(|&(from, to)| Range::new(from, to))
}

/// Returns a cursor on the line above or below the outermost cursor, at the same visual column.
pub fn add_vertical(text: &Rope, ranges: &[Range], up: bool, tab_width: usize) -> Option<Range> {
    let edge = if up {
        ranges.iter().min_by_key(|range| range.head)?
    } else {
        ranges.iter().max_by_key(|range| range.head)?
    };
    let line = text.char_to_line(edge.head);
    let target = if up {
        line.checked_sub(1)?
    } else {
        (line + 1 < text.len_lines()).then_some(line + 1)?
    };
    let col = view::visual_col(text, edge.head, tab_width);
    Some(Range::point(view::pos_at_visual_col(
        text, target, col, tab_width,
    )))
}

#[cfg(test)]
/// Tests for multiple cursors.
mod tests {
    use ropey::Rope;

    use super::{add_vertical, edit_all, next_occurrence};
    use crate::{range::Range, transaction::Change};

    /// Typing at several cursors inserts at each and moves each cursor past its text.
    #[test]
    fn types_everywhere() {
        let mut text = Rope::from_str("a\nb\nc");
        let ranges = [Range::point(1), Range::point(3), Range::point(5)];
        let (tx, after) = edit_all(&ranges, |range| Change {
            start: range.from(),
            end: range.to(),
            text: "!".into(),
        });
        tx.apply(&mut text);
        assert_eq!(text.to_string(), "a!\nb!\nc!");
        assert_eq!(after, [Range::point(2), Range::point(5), Range::point(8)]);
    }

    /// Overlapping edits keep only the first so the transaction stays valid.
    #[test]
    fn skips_overlaps() {
        let ranges = [Range::new(0, 3), Range::new(2, 4)];
        let (tx, after) = edit_all(&ranges, |range| Change {
            start: range.from(),
            end: range.to(),
            text: String::new(),
        });
        assert_eq!(tx.changes().len(), 1);
        assert_eq!(after.len(), 1);
    }

    /// The next occurrence comes after the last cursor and wraps around.
    #[test]
    fn finds_next_occurrence() {
        let text = Rope::from_str("mog x mog y mog");
        let first = [Range::new(6, 9)];
        assert_eq!(next_occurrence(&text, &first), Some(Range::new(12, 15)));
        let two = [Range::new(6, 9), Range::new(12, 15)];
        assert_eq!(next_occurrence(&text, &two), Some(Range::new(0, 3)));
        let all = [Range::new(0, 3), Range::new(6, 9), Range::new(12, 15)];
        assert_eq!(next_occurrence(&text, &all), None);
    }

    /// Vertical cursors keep the column and stop at the edges.
    #[test]
    fn adds_vertical_cursors() {
        let text = Rope::from_str("abcd\nab\nabcd");
        let ranges = [Range::point(3)];
        assert_eq!(
            add_vertical(&text, &ranges, false, 4),
            Some(Range::point(7))
        );
        assert_eq!(add_vertical(&text, &ranges, true, 4), None);
    }
}
