//! Selection ranges over a document.

/// A selection between two char offsets.
///
/// The `head` is where the cursor is drawn and moves. The `anchor` stays put while a selection is
/// extended. When both are equal the range is just a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    /// The fixed end of the selection.
    pub anchor: usize,
    /// The moving end of the selection, where the cursor sits.
    pub head: usize,
}

impl Range {
    /// Creates a range from `anchor` to `head`.
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// Creates an empty range, a plain cursor, at `pos`.
    pub fn point(pos: usize) -> Self {
        Self::new(pos, pos)
    }

    /// Returns the smaller end of the range.
    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// Returns the larger end of the range.
    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// Returns `true` if nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Moves the head to `pos`, keeping the anchor if `extend` is set and collapsing otherwise.
    pub fn put_head(self, pos: usize, extend: bool) -> Self {
        if extend {
            Self::new(self.anchor, pos)
        } else {
            Self::point(pos)
        }
    }
}

#[cfg(test)]
/// Tests for [`Range`].
mod tests {
    use super::Range;

    /// [`Range::from`] and [`Range::to`] return the ends in order.
    #[test]
    fn from_and_to_are_ordered() {
        let range = Range::new(5, 2);
        assert_eq!((range.from(), range.to()), (2, 5));
    }

    /// [`Range::put_head`] keeps the anchor only when extending.
    #[test]
    fn put_head_extends_or_collapses() {
        let range = Range::point(3);
        assert_eq!(range.put_head(7, true), Range::new(3, 7));
        assert_eq!(range.put_head(7, false), Range::point(7));
    }
}
