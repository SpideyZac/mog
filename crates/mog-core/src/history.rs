//! Undo and redo history.

use crate::{range::Range, transaction::Transaction};

/// One applied [`Transaction`] along with the transaction that undoes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The edit that was applied.
    pub forward: Transaction,
    /// The edit that reverts `forward`.
    pub inverse: Transaction,
}

/// A group of [`Step`]s that is undone and redone as one unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// The steps in the order they were applied.
    pub steps: Vec<Step>,
    /// The selection before the first step.
    pub before: Range,
    /// The selection after the last step.
    pub after: Range,
}

/// The undo and redo stacks of a document.
#[derive(Debug, Clone, Default)]
pub struct History {
    /// Revisions that can be undone, newest last.
    undo: Vec<Revision>,
    /// Revisions that can be redone, newest last.
    redo: Vec<Revision>,
}

impl History {
    /// Records a step that took the selection from `before` to `after`.
    ///
    /// If `merge` is set and there is a previous revision, the step joins it so both are undone
    /// together. This is how consecutive typing becomes a single undo. Any redo history is
    /// dropped.
    pub fn record(&mut self, step: Step, before: Range, after: Range, merge: bool) {
        self.redo.clear();
        match self.undo.last_mut() {
            Some(last) if merge => {
                last.steps.push(step);
                last.after = after;
            }
            _ => self.undo.push(Revision {
                steps: vec![step],
                before,
                after,
            }),
        }
    }

    /// Moves the newest revision to the redo stack and returns it so the caller can revert it.
    pub fn undo(&mut self) -> Option<&Revision> {
        let revision = self.undo.pop()?;
        self.redo.push(revision);
        self.redo.last()
    }

    /// Moves the newest undone revision back and returns it so the caller can reapply it.
    pub fn redo(&mut self) -> Option<&Revision> {
        let revision = self.redo.pop()?;
        self.undo.push(revision);
        self.undo.last()
    }
}

#[cfg(test)]
/// Tests for [`History`].
mod tests {
    use super::{History, Step};
    use crate::{range::Range, transaction::Transaction};

    /// Builds a step that inserts `text` at `pos` with a dummy inverse.
    fn step(pos: usize, text: &str) -> Step {
        Step {
            forward: Transaction::insert(pos, text),
            inverse: Transaction::delete(pos, pos + text.len()),
        }
    }

    /// Merged steps are undone together.
    #[test]
    fn merged_steps_undo_together() {
        let mut history = History::default();
        history.record(step(0, "a"), Range::point(0), Range::point(1), false);
        history.record(step(1, "b"), Range::point(1), Range::point(2), true);
        let revision = history.undo().expect("revision to undo");
        assert_eq!(revision.steps.len(), 2);
        assert_eq!(revision.before, Range::point(0));
        assert!(history.undo().is_none());
    }

    /// Recording a new step clears the redo stack.
    #[test]
    fn record_clears_redo() {
        let mut history = History::default();
        history.record(step(0, "a"), Range::point(0), Range::point(1), false);
        history.undo();
        history.record(step(0, "b"), Range::point(0), Range::point(1), false);
        assert!(history.redo().is_none());
    }
}
