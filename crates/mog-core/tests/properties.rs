//! Property tests for the invariants everything else in mog leans on: transactions and undo.

use mog_core::{Change, Document, Range, Rope, Transaction};
use proptest::{collection::vec, prelude::*};

/// Text with some multi byte chars and line breaks.
fn text() -> impl Strategy<Value = String> {
    "[a-c\n\u{e9}\u{1f600} ]{0,40}"
}

/// Changes that fit in a text of `len` chars and never overlap.
fn changes(len: usize) -> impl Strategy<Value = Vec<Change>> {
    vec((0..=len, 0..=len, "[x-z\n\u{e9}]{0,4}"), 0..6).prop_map(|raw| {
        let mut changes: Vec<Change> = raw
            .into_iter()
            .map(|(a, b, text)| Change {
                start: a.min(b),
                end: a.max(b),
                text,
            })
            .collect();
        changes.sort_by_key(|change| change.start);
        let mut kept: Vec<Change> = Vec::new();
        for change in changes {
            if kept.last().is_none_or(|last| last.end <= change.start) {
                kept.push(change);
            }
        }
        kept
    })
}

/// A text with changes that fit it.
fn text_and_changes() -> impl Strategy<Value = (String, Vec<Change>)> {
    text().prop_flat_map(|text| {
        let len = text.chars().count();
        (Just(text), changes(len))
    })
}

/// Applies `changes` the slow and obvious way, back to front.
fn naive(text: &str, changes: &[Change]) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for change in changes.iter().rev() {
        chars.splice(change.start..change.end, change.text.chars());
    }
    chars.into_iter().collect()
}

proptest! {
    /// A transaction does what applying each change by hand does, and its inverse undoes it.
    #[test]
    fn apply_matches_naive_and_inverts((text, changes) in text_and_changes()) {
        let tx = Transaction::try_new(changes.clone()).expect("valid changes");
        let mut rope = Rope::from_str(&text);
        tx.check_bounds(rope.len_chars()).expect("in bounds");
        let inverse = tx.apply(&mut rope);
        prop_assert_eq!(rope.to_string(), naive(&text, &changes));
        inverse.apply(&mut rope);
        prop_assert_eq!(rope.to_string(), text);
    }

    /// Mapped positions keep their order and stay inside the new text.
    #[test]
    fn map_pos_is_monotonic((text, changes) in text_and_changes()) {
        let len = text.chars().count();
        let tx = Transaction::try_new(changes.clone()).expect("valid changes");
        let new_len = naive(&text, &changes).chars().count();
        let mut last = 0;
        for pos in 0..=len {
            let mapped = tx.map_pos(pos);
            prop_assert!(mapped >= last);
            prop_assert!(mapped <= new_len);
            last = mapped;
        }
    }

    /// Building a transaction fails exactly when changes overlap or run backwards, and never
    /// panics.
    #[test]
    fn try_new_rejects_bad_changes(raw in vec((0usize..20, 0usize..20), 0..5)) {
        let changes: Vec<Change> = raw
            .iter()
            .map(|&(start, end)| Change { start, end, text: String::new() })
            .collect();
        let mut sorted = raw.clone();
        sorted.sort_by_key(|&(start, _)| start);
        let valid = sorted.iter().all(|&(start, end)| start <= end)
            && sorted.windows(2).all(|pair| pair[0].1 <= pair[1].0);
        prop_assert_eq!(Transaction::try_new(changes).is_ok(), valid);
    }

    /// Undoing every edit gets the first text back and redoing them all gets the last one.
    #[test]
    fn undo_and_redo_round_trip(
        (text, edits) in text().prop_flat_map(|text| {
            let len = text.chars().count();
            (Just(text), vec(changes(len + 20), 1..6))
        })
    ) {
        let mut document = Document::from_text(&text);
        let mut versions = vec![text.clone()];
        for changes in edits {
            let len = document.text().len_chars();
            let changes: Vec<Change> = changes
                .into_iter()
                .filter(|change| change.end <= len)
                .collect();
            let tx = Transaction::try_new(changes).expect("valid changes");
            if tx.is_empty() {
                continue;
            }
            document.apply(tx, Range::point(0), false);
            versions.push(document.text().to_string());
        }
        let last = versions.last().cloned().unwrap_or_default();
        while document.undo() {}
        prop_assert_eq!(document.text().to_string(), text);
        while document.redo() {}
        prop_assert_eq!(document.text().to_string(), last);
    }
}
