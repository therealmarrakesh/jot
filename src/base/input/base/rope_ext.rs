use std::ops::Range;

use ropey::{LineType, Rope, RopeSlice};

/// Side bias when clamping a byte offset onto a UTF-8 char boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Bias {
    Left,
    Right,
}

/// Parser-independent byte/row/column position used for incremental edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Point {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

impl Point {
    pub(crate) fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// Zero-based line and UTF-16 character position within a text buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Position {
    pub(crate) line: u32,
    pub(crate) character: u32,
}

impl Position {
    pub(crate) fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// An extension trait for [`Rope`] to provide additional utility methods.
pub(crate) trait RopeExt {
    /// Start offset of the line at the given row (0-based) index.
    fn line_start_offset(&self, row: usize) -> usize;

    /// End offset (excluding `\n`) of the line at the given row (0-based) index.
    fn line_end_offset(&self, row: usize) -> usize;

    /// Return a line slice at the given row (0-based) index, excluding `\n`.
    fn slice_line(&self, row: usize) -> RopeSlice<'_>;

    /// Return the number of lines in the rope.
    fn lines_len(&self) -> usize;

    /// Return the byte length of the row (0-based), excluding `\n`.
    fn line_len(&self, row: usize) -> usize;

    /// Replace the text in the given byte range with new text.
    fn replace(&mut self, range: Range<usize>, new_text: &str);

    /// Get char at the given byte offset.
    fn char_at(&self, offset: usize) -> Option<char>;

    /// Get the byte offset from the given line, column [`Position`] (0-based).
    fn position_to_offset(&self, line_col: &Position) -> usize;

    /// Get point (row, column) from the given byte offset (column in bytes).
    fn offset_to_point(&self, offset: usize) -> Point;

    /// Get byte offset from the given point (row, column in bytes).
    fn point_to_offset(&self, point: Point) -> usize;

    /// Convert offset in UTF-16 to byte offset (0-based).
    fn offset_utf16_to_offset(&self, offset_utf16: usize) -> usize;

    /// Convert byte offset (0-based) to offset in UTF-16.
    fn offset_to_offset_utf16(&self, offset: usize) -> usize;

    /// Clamp a byte offset onto a UTF-8 char boundary.
    fn clip_offset(&self, offset: usize, bias: Bias) -> usize;
}

impl RopeExt for Rope {
    fn slice_line(&self, row: usize) -> RopeSlice<'_> {
        let total_lines = self.lines_len();
        if row >= total_lines {
            return self.slice(0..0);
        }

        let line = self.line(row, LineType::LF);
        if line.len() > 0 {
            let line_end = line.len() - 1;
            if line.is_char_boundary(line_end) && line.char(line_end) == '\n' {
                return line.slice(..line_end);
            }
        }

        line
    }

    fn line_len(&self, row: usize) -> usize {
        self.slice_line(row).len()
    }

    fn line_start_offset(&self, row: usize) -> usize {
        self.point_to_offset(Point::new(row, 0))
    }

    fn offset_to_point(&self, offset: usize) -> Point {
        let offset = self.clip_offset(offset, Bias::Left);
        let row = self.byte_to_line_idx(offset, LineType::LF);
        let line_start = self.line_to_byte_idx(row, LineType::LF);
        let column = offset.saturating_sub(line_start);
        Point::new(row, column)
    }

    fn point_to_offset(&self, point: Point) -> usize {
        if point.row >= self.lines_len() {
            return self.len();
        }

        let line_start = self.line_to_byte_idx(point.row, LineType::LF);
        line_start + point.column
    }

    fn position_to_offset(&self, pos: &Position) -> usize {
        let line = self.slice_line(pos.line as usize);
        // Clamp out-of-range columns, then use Ropey's index to avoid rescanning long lines.
        self.line_start_offset(pos.line as usize)
            + line.char_to_byte_idx((pos.character as usize).min(line.len_chars()))
    }

    fn line_end_offset(&self, row: usize) -> usize {
        if row > self.lines_len() {
            return self.len();
        }

        self.line_start_offset(row) + self.line_len(row)
    }

    fn lines_len(&self) -> usize {
        self.len_lines(LineType::LF)
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        if offset > self.len() {
            return None;
        }

        self.get_char(offset).ok()
    }

    #[inline]
    fn offset_utf16_to_offset(&self, offset_utf16: usize) -> usize {
        if offset_utf16 > self.len_utf16() {
            return self.len();
        }

        self.utf16_to_byte_idx(offset_utf16)
    }

    #[inline]
    fn offset_to_offset_utf16(&self, offset: usize) -> usize {
        if offset > self.len() {
            return self.len_utf16();
        }

        self.byte_to_utf16_idx(offset)
    }

    fn replace(&mut self, range: Range<usize>, new_text: &str) {
        let range =
            self.clip_offset(range.start, Bias::Left)..self.clip_offset(range.end, Bias::Right);
        self.remove(range.clone());
        self.insert(range.start, new_text);
    }

    fn clip_offset(&self, offset: usize, bias: Bias) -> usize {
        if offset > self.len() {
            return self.len();
        }

        if self.is_char_boundary(offset) {
            return offset;
        }

        if bias == Bias::Left {
            self.floor_char_boundary(offset)
        } else {
            self.ceil_char_boundary(offset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bias, Point};
    use ropey::Rope;

    use crate::base::input::{Position, RopeExt};

    #[test]
    fn test_slice_line() {
        let rope = Rope::from("Hello\nWorld\r\nThis is a test 中文\nRope");
        assert_eq!(rope.slice_line(0).to_string(), "Hello");
        assert_eq!(rope.slice_line(1).to_string(), "World\r");
        assert_eq!(rope.slice_line(2).to_string(), "This is a test 中文");
        assert_eq!(rope.slice_line(3).to_string(), "Rope");
        assert_eq!(rope.slice_line(6).to_string(), "");

        let rope = Rope::from("Hello\r");
        assert_eq!(rope.slice_line(0).to_string(), "Hello\r");
        assert_eq!(rope.slice_line(1).to_string(), "");
    }

    #[test]
    fn test_lines_len() {
        let rope = Rope::from("Hello\nWorld\r\nThis is a test 中文\nRope");
        assert_eq!(rope.lines_len(), 4);
        assert_eq!(Rope::from("").lines_len(), 1);
        assert_eq!(Rope::from("Single line").lines_len(), 1);
        assert_eq!(Rope::from("Hello\r").lines_len(), 1);
    }

    #[test]
    fn test_line_start_end_offset() {
        let rope = Rope::from("Hello\nWorld\r\nThis is a test 中文\nRope");
        assert_eq!(rope.line_start_offset(0), 0);
        assert_eq!(rope.line_end_offset(0), 5);
        assert_eq!(rope.line_start_offset(1), 6);
        assert_eq!(rope.line_end_offset(1), 12);
        assert_eq!(rope.line_start_offset(2), 13);
        assert_eq!(rope.line_end_offset(2), 34);
        assert_eq!(rope.line_start_offset(3), 35);
        assert_eq!(rope.line_end_offset(3), 39);
        assert_eq!(rope.line_start_offset(4), 39);
        assert_eq!(rope.line_end_offset(4), 39);
    }

    #[test]
    fn test_position_to_offset() {
        let rope = Rope::from("a 中文🎉 test\nRope");
        assert_eq!(rope.position_to_offset(&Position::new(0, 3)), "a 中".len());
        assert_eq!(
            rope.position_to_offset(&Position::new(0, 5)),
            "a 中文🎉".len()
        );
        assert_eq!(
            rope.position_to_offset(&Position::new(1, 1)),
            "a 中文🎉 test\nR".len()
        );
        assert_eq!(
            rope.position_to_offset(&Position::new(0, u32::MAX)),
            "a 中文🎉 test".len()
        );
    }

    #[test]
    fn test_offset_to_point() {
        let rope = Rope::from("a 中文🎉 test\nRope");
        assert_eq!(rope.offset_to_point(0), Point::new(0, 0));
        assert_eq!(rope.offset_to_point(1), Point::new(0, 1));
        assert_eq!(rope.offset_to_point("a 中".len()), Point::new(0, 5));
        assert_eq!(rope.offset_to_point("a 中文🎉".len()), Point::new(0, 12));
        assert_eq!(
            rope.offset_to_point("a 中文🎉 test\nR".len()),
            Point::new(1, 1)
        );
    }

    #[test]
    fn test_point_to_offset() {
        let rope = Rope::from("a 中文🎉 test\nRope");
        assert_eq!(rope.point_to_offset(Point::new(0, 0)), 0);
        assert_eq!(rope.point_to_offset(Point::new(0, 1)), 1);
        assert_eq!(rope.point_to_offset(Point::new(0, 5)), "a 中".len());
        assert_eq!(rope.point_to_offset(Point::new(0, 12)), "a 中文🎉".len());
        assert_eq!(
            rope.point_to_offset(Point::new(1, 1)),
            "a 中文🎉 test\nR".len()
        );
    }

    #[test]
    fn test_char_at() {
        let rope = Rope::from("Hello\nWorld\r\nThis is a test 中文🎉\nRope");
        assert_eq!(rope.char_at(0), Some('H'));
        assert_eq!(rope.char_at(5), Some('\n'));
        assert_eq!(rope.char_at(13), Some('T'));
        assert_eq!(rope.char_at(28), Some('中'));
        assert_eq!(rope.char_at(34), Some('🎉'));
        assert_eq!(rope.char_at(38), Some('\n'));
        assert_eq!(rope.char_at(50), None);
    }

    #[test]
    fn test_offset_utf16_conversion() {
        let rope = Rope::from("hello 中文🎉 test\nRope");
        assert_eq!(rope.offset_to_offset_utf16("hello".len()), 5);
        assert_eq!(rope.offset_to_offset_utf16("hello 中".len()), 7);
        assert_eq!(rope.offset_to_offset_utf16("hello 中文".len()), 8);
        assert_eq!(rope.offset_to_offset_utf16("hello 中文🎉".len()), 10);
        assert_eq!(rope.offset_to_offset_utf16(100), 20);

        assert_eq!(rope.offset_utf16_to_offset(5), "hello".len());
        assert_eq!(rope.offset_utf16_to_offset(7), "hello 中".len());
        assert_eq!(rope.offset_utf16_to_offset(8), "hello 中文".len());
        assert_eq!(rope.offset_utf16_to_offset(10), "hello 中文🎉".len());
        assert_eq!(rope.offset_utf16_to_offset(100), rope.len());
    }

    #[test]
    fn test_replace() {
        let mut rope = Rope::from("Hello\nWorld\r\nThis is a test 中文\nRope");
        rope.replace(6..11, "Universe");
        assert_eq!(
            rope.to_string(),
            "Hello\nUniverse\r\nThis is a test 中文\nRope"
        );

        rope.replace(0..5, "Hi");
        assert_eq!(
            rope.to_string(),
            "Hi\nUniverse\r\nThis is a test 中文\nRope"
        );

        rope.replace(rope.len() - 4..rope.len(), "String");
        assert_eq!(
            rope.to_string(),
            "Hi\nUniverse\r\nThis is a test 中文\nString"
        );

        let mut rope = Rope::from("中文");
        rope.replace(0..1, "New");
        assert_eq!(rope.to_string(), "New文");
        let mut rope = Rope::from("中文");
        rope.replace(0..2, "New");
        assert_eq!(rope.to_string(), "New文");
        let mut rope = Rope::from("中文");
        rope.replace(0..3, "New");
        assert_eq!(rope.to_string(), "New文");
        let mut rope = Rope::from("中文");
        rope.replace(1..4, "New");
        assert_eq!(rope.to_string(), "New");
    }

    #[test]
    fn test_clip_offset() {
        let rope = Rope::from("Hello 中文🎉 test\nRope");
        assert_eq!(rope.clip_offset(5, Bias::Left), 5);
        assert_eq!(rope.clip_offset(7, Bias::Left), 6);
        assert_eq!(rope.clip_offset(7, Bias::Right), 9);
        assert_eq!(rope.clip_offset(9, Bias::Left), 9);

        assert_eq!(rope.clip_offset(13, Bias::Left), 12);
        assert_eq!(rope.clip_offset(13, Bias::Right), 16);
        assert_eq!(rope.clip_offset(16, Bias::Left), 16);

        assert_eq!(rope.clip_offset(5, Bias::Left), 5);
        assert_eq!(rope.clip_offset(5, Bias::Right), 5);

        assert_eq!(rope.clip_offset(26, Bias::Left), 26);
        assert_eq!(rope.clip_offset(100, Bias::Left), 26);
    }
}
