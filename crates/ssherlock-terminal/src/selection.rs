//! Text selection: how the user selects, and what is selected on the visible screen.
//!
//! A selection is anchored to the terminal's content, not the screen. It moves with the text as
//! output scrolls it, and survives scrolling back through history.

use std::ops::RangeInclusive;

/// How a selection grows from where the mouse went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// Character by character (drag).
    Simple,
    /// Whole words (double-click). Word boundaries are spaces and punctuation such as quotes,
    /// brackets and `|`, so paths and URLs select as one word.
    Word,
    /// Whole lines (triple-click). Wrapped lines count as one.
    Line,
    /// A rectangle of columns (Alt/Option-drag).
    Block,
}

/// Which half of a cell the pointer is over. A selection that starts on the right half of a
/// cell doesn't include that cell, as in every text editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellSide {
    /// The left half.
    Left,
    /// The right half.
    Right,
}

/// A cell on the visible screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridPoint {
    /// Row from the top of the visible screen.
    pub row: usize,
    /// Column from the left.
    pub column: usize,
}

impl GridPoint {
    /// The cell at `row`, `column`.
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// The part of a selection that is on the visible screen, start before end, both inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VisibleRange {
    /// First selected cell.
    pub start: GridPoint,
    /// Last selected cell.
    pub end: GridPoint,
    /// A rectangle: the same columns on every row, rather than running from line to line.
    pub is_block: bool,
}

impl VisibleRange {
    /// The selected columns on `row` of a screen `columns` wide, or `None` if nothing on that
    /// row is selected.
    pub fn columns_on(&self, row: usize, columns: usize) -> Option<RangeInclusive<usize>> {
        if row < self.start.row || row > self.end.row || columns == 0 {
            return None;
        }
        let last = columns - 1;
        let (first, end) = if self.is_block {
            let (a, b) = (self.start.column, self.end.column);
            (a.min(b), a.max(b))
        } else {
            let first = if row == self.start.row {
                self.start.column
            } else {
                0
            };
            let end = if row == self.end.row {
                self.end.column
            } else {
                last
            };
            (first, end)
        };
        (first <= end && first <= last).then(|| first..=end.min(last))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn columns_on_each_row() {
        let flowing = VisibleRange {
            start: GridPoint::new(1, 4),
            end: GridPoint::new(3, 2),
            is_block: false,
        };
        let block = VisibleRange {
            is_block: true,
            ..flowing
        };
        let one_row = VisibleRange {
            start: GridPoint::new(2, 3),
            end: GridPoint::new(2, 5),
            is_block: false,
        };
        let cases = [
            ("above", flowing, 0, None),
            ("first row from start", flowing, 1, Some(4..=9)),
            ("middle row whole", flowing, 2, Some(0..=9)),
            ("last row to end", flowing, 3, Some(0..=2)),
            ("below", flowing, 4, None),
            ("block uses both columns", block, 2, Some(2..=4)),
            ("block first row", block, 1, Some(2..=4)),
            ("single row", one_row, 2, Some(3..=5)),
        ];
        for (name, selection, row, expected) in cases {
            assert_eq!(selection.columns_on(row, 10), expected, "{name}");
        }
        let past_edge = VisibleRange {
            start: GridPoint::new(0, 12),
            end: GridPoint::new(0, 15),
            is_block: false,
        };
        assert_eq!(past_edge.columns_on(0, 10), None);
        assert_eq!(flowing.columns_on(2, 0), None);
    }
}
