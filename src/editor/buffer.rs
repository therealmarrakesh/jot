use std::ops::{Bound, Range, RangeBounds};

use ropey::{LineType, Rope, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};

const WORD_SCAN_WINDOW_BYTES: usize = 256;
const GRAPHEME_SCAN_WINDOW_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Bias {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TextPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

impl TextPoint {
    #[inline]
    pub(crate) fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Position {
    pub(crate) line: u32,
    pub(crate) character: u32,
}

impl Position {
    #[inline]
    pub(crate) fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub(crate) struct Selection {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Selection {
    #[inline]
    pub(crate) fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl From<Range<usize>> for Selection {
    #[inline]
    fn from(range: Range<usize>) -> Self {
        Self::new(range.start, range.end)
    }
}

impl From<Selection> for Range<usize> {
    #[inline]
    fn from(sel: Selection) -> Self {
        sel.start..sel.end
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub(crate) struct CaretSelection {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) reversed: bool,
    pub(crate) preferred_x: Option<(f32, usize)>,
}

impl CaretSelection {
    #[inline]
    pub(crate) fn new(start: usize, end: usize) -> Self {
        Self {
            start,
            end,
            reversed: false,
            preferred_x: None,
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.start == self.end
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.start = 0;
        self.end = 0;
        self.reversed = false;
        self.preferred_x = None;
    }

    #[inline]
    pub(crate) fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    #[inline]
    pub(crate) fn head(&self) -> usize {
        if self.reversed { self.start } else { self.end }
    }

    #[inline]
    pub(crate) fn place_at(&mut self, offset: usize, preferred_x: Option<(f32, usize)>) {
        self.start = offset;
        self.end = offset;
        self.reversed = false;
        self.preferred_x = preferred_x;
    }

    pub(crate) fn extend_to(
        &mut self,
        offset: usize,
        anchor_word: Option<CaretSelection>,
        max_len: usize,
    ) {
        let offset = offset.min(max_len);
        self.start = self.start.min(max_len);
        self.end = self.end.min(max_len);
        if self.reversed {
            self.start = offset;
        } else {
            self.end = offset;
        }
        if self.end < self.start {
            self.reversed = !self.reversed;
            std::mem::swap(&mut self.start, &mut self.end);
        }
        if let Some(word) = anchor_word {
            self.start = self.start.min(word.start.min(max_len));
            self.end = self.end.max(word.end.min(max_len));
        }
    }
}

impl From<Range<usize>> for CaretSelection {
    #[inline]
    fn from(range: Range<usize>) -> Self {
        Self::new(range.start, range.end)
    }
}

impl From<CaretSelection> for Range<usize> {
    #[inline]
    fn from(sel: CaretSelection) -> Self {
        sel.start..sel.end
    }
}

impl RangeBounds<usize> for CaretSelection {
    fn start_bound(&self) -> Bound<&usize> {
        Bound::Included(&self.start)
    }

    fn end_bound(&self) -> Bound<&usize> {
        Bound::Excluded(&self.end)
    }
}

pub(crate) trait RopeExt {
    fn slice_line(&self, row: usize) -> RopeSlice<'_>;
    fn line_len(&self, row: usize) -> usize;
    fn line_start_offset(&self, row: usize) -> usize;
    fn line_end_offset(&self, row: usize) -> usize;
    fn lines_len(&self) -> usize;
    fn char_at(&self, offset: usize) -> Option<char>;
    fn offset_to_point(&self, offset: usize) -> TextPoint;
    fn point_to_offset(&self, point: TextPoint) -> usize;
    fn position_to_offset(&self, pos: &Position) -> usize;
    fn offset_utf16_to_offset(&self, utf16_offset: usize) -> usize;
    fn offset_to_offset_utf16(&self, byte_offset: usize) -> usize;
    fn clip_offset(&self, offset: usize, bias: Bias) -> usize;
    fn replace(&mut self, range: Range<usize>, text: &str);
}

impl RopeExt for Rope {
    fn slice_line(&self, row: usize) -> RopeSlice<'_> {
        if row >= self.lines_len() {
            return self.slice(0..0);
        }
        let line = self.line(row, LineType::LF);
        if line.len() > 0 && line.byte(line.len() - 1) == b'\n' {
            line.slice(..line.len() - 1)
        } else {
            line
        }
    }

    fn line_len(&self, row: usize) -> usize {
        self.slice_line(row).len()
    }

    fn line_start_offset(&self, row: usize) -> usize {
        self.point_to_offset(TextPoint::new(row, 0))
    }

    fn line_end_offset(&self, row: usize) -> usize {
        if row >= self.lines_len() {
            return self.len();
        }
        self.line_start_offset(row) + self.line_len(row)
    }

    fn lines_len(&self) -> usize {
        self.len_lines(LineType::LF)
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        (offset < self.len())
            .then(|| self.get_char(offset).ok())
            .flatten()
    }

    fn offset_to_point(&self, offset: usize) -> TextPoint {
        let clamped = self.clip_offset(offset, Bias::Left);
        let row = self.byte_to_line_idx(clamped, LineType::LF);
        let start = self.line_to_byte_idx(row, LineType::LF);
        TextPoint::new(row, clamped.saturating_sub(start))
    }

    fn point_to_offset(&self, point: TextPoint) -> usize {
        if point.row >= self.lines_len() {
            return self.len();
        }
        self.line_to_byte_idx(point.row, LineType::LF) + point.column
    }

    fn position_to_offset(&self, pos: &Position) -> usize {
        let row = pos.line as usize;
        let line = self.slice_line(row);
        let char_col = (pos.character as usize).min(line.len_chars());
        self.line_start_offset(row) + line.char_to_byte_idx(char_col)
    }

    #[inline]
    fn offset_utf16_to_offset(&self, utf16_offset: usize) -> usize {
        if utf16_offset >= self.len_utf16() {
            return self.len();
        }
        self.utf16_to_byte_idx(utf16_offset)
    }

    #[inline]
    fn offset_to_offset_utf16(&self, byte_offset: usize) -> usize {
        if byte_offset >= self.len() {
            return self.len_utf16();
        }
        self.byte_to_utf16_idx(byte_offset)
    }

    fn clip_offset(&self, offset: usize, bias: Bias) -> usize {
        if offset >= self.len() {
            return self.len();
        }
        if self.is_char_boundary(offset) {
            return offset;
        }
        match bias {
            Bias::Left => self.floor_char_boundary(offset),
            Bias::Right => self.ceil_char_boundary(offset),
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str) {
        let start = self.clip_offset(range.start, Bias::Left);
        let end = self.clip_offset(range.end, Bias::Right);
        self.remove(start..end);
        self.insert(start, text);
    }
}

pub(crate) fn line_range_at(text: &Rope, offset: usize) -> Range<usize> {
    let clamped = text.clip_offset(offset, Bias::Left);
    let row = text.offset_to_point(clamped).row;
    text.line_start_offset(row)..text.line_end_offset(row)
}

pub(crate) fn prev_grapheme_boundary(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let off = text.clip_offset(offset, Bias::Left);
    if off == 0 {
        return 0;
    }
    let prev_b = text.byte(off - 1);
    if (0x20..=0x7E).contains(&prev_b) && (off == 1 || text.byte(off - 2) < 0x80) {
        return off - 1;
    }
    let mut span = GRAPHEME_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(off.saturating_sub(span));
        let win_end = text.ceil_char_boundary(off.saturating_add(32).min(len));
        let chunk = text.slice(win_start..win_end).to_string();
        let mut cursor = GraphemeCursor::new(off, len, true);
        match cursor.prev_boundary(&chunk, win_start) {
            Ok(Some(pos)) => return pos,
            Ok(None) => return 0,
            Err(GraphemeIncomplete::PrevChunk | GraphemeIncomplete::PreContext(_))
                if win_start > 0 =>
            {
                span = span.saturating_mul(2);
            }
            Err(_) => return text.floor_char_boundary(off.saturating_sub(1)),
        }
    }
}

pub(crate) fn next_grapheme_boundary(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let off = text.clip_offset(offset, Bias::Right);
    if off >= len {
        return len;
    }
    let b = text.byte(off);
    if (0x20..=0x7E).contains(&b) && (off + 1 == len || text.byte(off + 1) < 0x80) {
        return off + 1;
    }
    let mut span = GRAPHEME_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(off.saturating_sub(span));
        let win_end = text.ceil_char_boundary(off.saturating_add(span).min(len));
        let chunk = text.slice(win_start..win_end).to_string();
        let mut cursor = GraphemeCursor::new(off, len, true);
        match cursor.next_boundary(&chunk, win_start) {
            Ok(Some(pos)) => return pos,
            Ok(None) => return len,
            Err(GraphemeIncomplete::NextChunk | GraphemeIncomplete::PreContext(_))
                if win_start > 0 || win_end < len =>
            {
                span = span.saturating_mul(2);
            }
            Err(_) => return text.ceil_char_boundary((off + 1).min(len)),
        }
    }
}

pub(crate) fn snap_grapheme_boundary(text: &Rope, offset: usize, bias: Bias) -> usize {
    let len = text.len();
    let off = text.clip_offset(offset, bias);
    if off == 0 || off >= len {
        return off;
    }
    let prev_b = text.byte(off - 1);
    let curr_b = text.byte(off);
    if prev_b < 0x80 && curr_b < 0x80 && !(prev_b == b'\r' && curr_b == b'\n') {
        return off;
    }
    let mut span = GRAPHEME_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text.floor_char_boundary(off.saturating_sub(span));
        let win_end = text.ceil_char_boundary(off.saturating_add(span).min(len));
        let chunk = text.slice(win_start..win_end).to_string();
        let mut cursor = GraphemeCursor::new(off, len, true);
        match cursor.is_boundary(&chunk, win_start) {
            Ok(true) => return off,
            Ok(false) => {
                return match bias {
                    Bias::Left => prev_grapheme_boundary(text, off),
                    Bias::Right => next_grapheme_boundary(text, off),
                };
            }
            Err(GraphemeIncomplete::PreContext(_)) if win_start > 0 => {
                span = span.saturating_mul(2);
            }
            Err(_) => return off,
        }
    }
}

#[inline]
fn is_horizontal_ws(ch: char) -> bool {
    ch.is_whitespace() && !matches!(ch, '\n' | '\r')
}

pub(crate) fn word_range_at(text: &Rope, offset: usize) -> Option<Range<usize>> {
    let line = line_range_at(text, offset);
    if line.is_empty() {
        return None;
    }
    let mut pos = text.clip_offset(offset, Bias::Left);
    if pos >= line.end {
        pos = prev_grapheme_boundary(text, line.end);
    }
    if pos < line.start || pos >= line.end {
        return None;
    }

    let ch = text.char_at(pos)?;
    if is_horizontal_ws(ch) {
        let mut left = pos;
        for prev in text.chars_at(pos).reversed() {
            if left <= line.start || !is_horizontal_ws(prev) {
                break;
            }
            left -= prev.len_utf8();
        }
        let mut right = pos + ch.len_utf8();
        for next in text.chars_at(right) {
            if right >= line.end || !is_horizontal_ws(next) {
                break;
            }
            right += next.len_utf8();
        }
        return Some(left..right);
    }

    let mut span = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text
            .floor_char_boundary(pos.saturating_sub(span))
            .max(line.start);
        let win_end = text
            .ceil_char_boundary(pos.saturating_add(span).min(line.end))
            .min(line.end);
        let chunk = text.slice(win_start..win_end).to_string();
        let rel_pos = pos - win_start;
        for (seg_start, seg) in UnicodeSegmentation::split_word_bound_indices(chunk.as_str()) {
            let seg_end = seg_start + seg.len();
            if rel_pos >= seg_start && rel_pos < seg_end {
                let touches_left = win_start > line.start && chunk[..seg_start].chars().count() < 4;
                let touches_right = win_end < line.end && chunk[seg_end..].chars().count() < 4;
                if !touches_left && !touches_right {
                    return Some((win_start + seg_start)..(win_start + seg_end));
                }
                break;
            }
        }
        if win_start == line.start && win_end == line.end {
            return Some(pos..(pos + ch.len_utf8()));
        }
        span = span.saturating_mul(2);
    }
}

pub(crate) fn prev_word_start(text: &Rope, offset: usize) -> usize {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    if offset == 0 {
        return 0;
    }
    if text.byte(offset - 1) == b'\n' {
        return offset - 1;
    }
    let row = text.offset_to_point(offset).row;
    let line_start = text.line_start_offset(row);

    let mut end = offset;
    for ch in text.chars_at(offset).reversed() {
        if end <= line_start || !is_horizontal_ws(ch) {
            break;
        }
        end -= ch.len_utf8();
    }
    if end <= line_start {
        return line_start;
    }

    let mut span = WORD_SCAN_WINDOW_BYTES;
    loop {
        let win_start = text
            .floor_char_boundary(end.saturating_sub(span))
            .max(line_start);
        let chunk = text.slice(win_start..end).to_string();
        if let Some((rel, _)) = UnicodeSegmentation::split_word_bound_indices(chunk.as_str())
            .rfind(|(_, piece)| !piece.chars().all(is_horizontal_ws))
            && (win_start == line_start || (rel > 0 && chunk[..rel].chars().count() >= 4))
        {
            return win_start + rel;
        }
        if win_start == line_start {
            return line_start;
        }
        span = span.saturating_mul(2);
    }
}

pub(crate) fn next_word_end(text: &Rope, offset: usize) -> usize {
    let len = text.len();
    let offset = text.floor_char_boundary(offset.min(len));
    if offset >= len {
        return len;
    }
    if text.byte(offset) == b'\n' {
        return offset + 1;
    }
    let row = text.offset_to_point(offset).row;
    let line_end = text.line_end_offset(row);
    if offset >= line_end {
        return (line_end + 1).min(len);
    }

    let mut start = offset;
    if text.char_at(start).is_some_and(is_horizontal_ws) {
        for ch in text.chars_at(start) {
            if start >= line_end || !is_horizontal_ws(ch) {
                break;
            }
            start += ch.len_utf8();
        }
        return start.min(line_end);
    }

    let mut span = WORD_SCAN_WINDOW_BYTES;
    let word_end = loop {
        let win_end = text
            .ceil_char_boundary(start.saturating_add(span).min(line_end))
            .min(line_end);
        let chunk = text.slice(start..win_end).to_string();
        if let Some((rel, piece)) = UnicodeSegmentation::split_word_bound_indices(chunk.as_str())
            .find(|(_, piece)| !piece.chars().all(is_horizontal_ws))
        {
            let end = rel + piece.len();
            if win_end == line_end || chunk[end..].chars().count() >= 4 {
                break start + end;
            }
        }
        if win_end == line_end {
            break line_end;
        }
        span = span.saturating_mul(2);
    };

    let mut pos = word_end.min(line_end);
    for ch in text.chars_at(pos) {
        if pos >= line_end || !is_horizontal_ws(ch) {
            break;
        }
        pos += ch.len_utf8();
    }
    pos.min(line_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_caret_basics() {
        let sel = Selection::new(2, 5);
        assert_eq!(Range::<usize>::from(sel), 2..5);
        assert!(!sel.is_empty());

        let mut caret = CaretSelection::new(5, 10);
        assert_eq!(caret.head(), 10);
        caret.reversed = true;
        assert_eq!(caret.head(), 5);
        caret.place_at(7, Some((12., 3)));
        assert!(caret.is_empty());
        assert!(!caret.reversed);
        assert_eq!(caret.preferred_x, Some((12., 3)));
    }

    #[test]
    fn rope_line_and_utf16_helpers_handle_multibyte_and_crlf() {
        let rope = Rope::from("Hello\nWorld\r\nThis is a test 中文🎉\nRope");
        assert_eq!(rope.lines_len(), 4);
        assert_eq!(rope.slice_line(0).to_string(), "Hello");
        assert_eq!(rope.slice_line(1).to_string(), "World\r");
        assert_eq!(rope.slice_line(2).to_string(), "This is a test 中文🎉");
        assert_eq!(rope.slice_line(3).to_string(), "Rope");
        assert_eq!(rope.slice_line(6).to_string(), "");

        assert_eq!(rope.line_start_offset(0), 0);
        assert_eq!(rope.line_end_offset(0), 5);
        assert_eq!(rope.line_start_offset(1), 6);
        assert_eq!(rope.line_end_offset(1), 12);
        assert_eq!(rope.char_at(28), Some('中'));
        assert_eq!(rope.char_at(34), Some('🎉'));
        assert_eq!(rope.char_at(99), None);

        let sample = Rope::from("hello 中文🎉 test\nRope");
        assert_eq!(sample.offset_to_offset_utf16("hello 中文🎉".len()), 10);
        assert_eq!(sample.offset_utf16_to_offset(10), "hello 中文🎉".len());
        assert_eq!(
            sample.position_to_offset(&Position::new(0, 9)),
            "hello 中文🎉".len()
        );
        assert_eq!(
            sample.offset_to_point("hello 中文🎉\nR".len()),
            TextPoint::new(0, "hello 中文🎉\nR".len())
        );

        let mut edit = Rope::from("中文");
        edit.replace(0..2, "New");
        assert_eq!(edit.to_string(), "New文");
        assert_eq!(sample.clip_offset(7, Bias::Left), 6);
        assert_eq!(sample.clip_offset(7, Bias::Right), 9);
    }

    #[test]
    fn word_and_line_ranges_match_expected_boundaries() {
        let rope = Rope::from(
            "test text:\nabcde 中文🎉 test\nhello[()]\ntest_connector ____\nRope\nrök\ngrande île\n",
        );
        let cases = [
            (0, 0, Some("test")),
            (0, 4, Some(" ")),
            (1, 0, Some("abcde")),
            (1, 6, Some("中")),
            (1, 13, Some("🎉")),
            (2, 5, Some("[")),
            (3, 5, Some("test_connector")),
            (3, 16, Some("____")),
            (5, 0, Some("rök")),
            (6, 8, Some("île")),
        ];
        for (line, col, expected) in cases {
            let offset = rope.line_start_offset(line) + col;
            let got = word_range_at(&rope, offset).map(|r| rope.slice(r).to_string());
            assert_eq!(got.as_deref(), expected, "line {line}, col {col}");
        }

        let lines = Rope::from("first line\nsecond line\nthird");
        assert_eq!(
            lines.slice(line_range_at(&lines, 5)).to_string(),
            "first line"
        );
        assert_eq!(
            lines
                .slice(line_range_at(&lines, lines.line_start_offset(2) + 1))
                .to_string(),
            "third"
        );
    }

    #[test]
    fn bounded_word_navigation_matches_full_scan_everywhere() {
        let long_word = "a".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let long_spaces = " ".repeat(WORD_SCAN_WINDOW_BYTES + 64);
        let sample = format!(
            "  hello, don't 1,234.56 foo_bar\n日本語🎉 test{long_spaces}{long_word}  tail"
        );
        let rope = Rope::from(sample.as_str());

        let nl = sample.find('\n').unwrap();
        assert_eq!(prev_word_start(&rope, nl + 1), nl);
        assert_eq!(next_word_end(&rope, nl), nl + 1);
        assert_eq!(next_word_end(&rope, nl - 3), nl);

        let long_start = sample.find(&long_word).unwrap();
        assert_eq!(
            word_range_at(&rope, long_start + 10),
            Some(long_start..long_start + long_word.len())
        );
        assert_eq!(
            prev_word_start(&rope, long_start + long_word.len()),
            long_start
        );
        assert_eq!(
            next_word_end(&rope, long_start),
            long_start + long_word.len() + 2
        );

        let emoji = Rope::from("a👍🏽b👨‍👩‍👧🇺🇸e\u{0301}z");
        let mut forward = vec![0];
        let mut cur = 0;
        while cur < emoji.len() {
            cur = next_grapheme_boundary(&emoji, cur);
            forward.push(cur);
        }
        assert_eq!(forward.len(), 8, "7 grapheme clusters + start");
        let mut backward = vec![emoji.len()];
        cur = emoji.len();
        while cur > 0 {
            cur = prev_grapheme_boundary(&emoji, cur);
            backward.push(cur);
        }
        backward.reverse();
        assert_eq!(forward, backward);
    }
}
