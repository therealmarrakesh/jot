use std::ops::{Range, RangeBounds};

use gpui::Pixels;

/// A selection in the text, represented by start and end byte indices.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}

impl Selection {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl From<Range<usize>> for Selection {
    fn from(value: Range<usize>) -> Self {
        Self::new(value.start, value.end)
    }
}

impl From<Selection> for Range<usize> {
    fn from(value: Selection) -> Self {
        value.start..value.end
    }
}

impl RangeBounds<usize> for Selection {
    fn start_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Included(&self.start)
    }

    fn end_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Excluded(&self.end)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub(super) struct CursorSelection {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) reversed: bool,
    pub(super) column_anchor: Option<(Pixels, usize)>,
}

impl CursorSelection {
    pub(super) fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            reversed: false,
            column_anchor: None,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub(super) fn clear(&mut self) {
        self.start = 0;
        self.end = 0;
        self.reversed = false;
        self.column_anchor = None;
    }

    pub(super) fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    pub(super) fn cursor_offset(&self) -> usize {
        if self.reversed { self.start } else { self.end }
    }

    pub(super) fn place_at(&mut self, offset: usize, column_anchor: Option<(Pixels, usize)>) {
        self.start = offset;
        self.end = offset;
        self.reversed = false;
        self.column_anchor = column_anchor;
    }
}

impl From<Range<usize>> for CursorSelection {
    fn from(value: Range<usize>) -> Self {
        Self::new(value.start, value.end)
    }
}

impl From<CursorSelection> for Range<usize> {
    fn from(value: CursorSelection) -> Self {
        value.start..value.end
    }
}

impl RangeBounds<usize> for CursorSelection {
    fn start_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Included(&self.start)
    }

    fn end_bound(&self) -> std::ops::Bound<&usize> {
        std::ops::Bound::Excluded(&self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::input::Position;
    use gpui::px;

    #[test]
    fn selection_keeps_its_public_range_api() {
        fn assert_eq<T: Eq>() {}

        assert_eq::<Selection>();
        let selection = Selection::new(2, 5);
        assert_eq!(selection, Selection { start: 2, end: 5 });
        assert_eq!(Range::<usize>::from(selection), 2..5);
    }

    #[test]
    fn test_line_column_from_to() {
        assert_eq!(
            Position::new(1, 2),
            Position {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn test_cursor_offset_reversed() {
        let mut sel = CursorSelection::new(5, 10);
        assert_eq!(sel.cursor_offset(), 10);
        sel.reversed = true;
        assert_eq!(sel.cursor_offset(), 5);
    }

    #[test]
    fn test_place_at() {
        let mut sel = CursorSelection::new(5, 10);
        sel.reversed = true;
        sel.place_at(7, Some((px(12.), 3)));
        assert_eq!(sel.start, 7);
        assert_eq!(sel.end, 7);
        assert!(sel.is_empty());
        assert!(!sel.reversed);
        assert_eq!(sel.column_anchor, Some((px(12.), 3)));
    }
}
