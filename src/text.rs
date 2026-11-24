//! Text model: rope helpers, selections, edits, IME composition and undo history.

use std::borrow::Cow;
use std::ops::Range;

use ropey::{LineType, RopeBuilder, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};

pub(crate) use ropey::Rope;

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
    pub(crate) fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub(crate) struct Selection {
    pub(crate) anchor: usize,
    pub(crate) head: usize,
    pub(crate) end_affinity: bool,
}

impl Selection {
    pub(crate) fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
            end_affinity: false,
        }
    }

    pub(crate) fn from_range(range: Range<usize>) -> Self {
        Self {
            anchor: range.start,
            head: range.end,
            end_affinity: false,
        }
    }

    pub(crate) fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub(crate) fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub(crate) fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub(crate) fn contains(&self, offset: usize) -> bool {
        offset >= self.start() && offset < self.end()
    }

    pub(crate) fn place(&mut self, offset: usize, end_affinity: bool) {
        self.anchor = offset;
        self.head = offset;
        self.end_affinity = end_affinity;
    }

    pub(crate) fn extend_to(
        &mut self,
        offset: usize,
        end_affinity: bool,
        anchor_word: Option<Range<usize>>,
        max_len: usize,
    ) {
        let offset = offset.min(max_len);
        self.anchor = self.anchor.min(max_len);
        self.head = offset;
        self.end_affinity = end_affinity;
        if let Some(word) = anchor_word {
            let ws = word.start.min(max_len);
            let we = word.end.min(max_len);
            if offset < ws {
                self.anchor = we;
                self.head = offset;
            } else if offset > we {
                self.anchor = ws;
                self.head = offset;
            } else {
                self.anchor = ws;
                self.head = we;
            }
        }
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

const BULK_REBUILD_THRESHOLD: usize = 64;
const SNAPSHOT_HISTORY_THRESHOLD: usize = 1024;

pub(crate) type ValidateFn = fn(&str) -> bool;

#[derive(Clone, Copy, Debug)]
pub(crate) struct EditPolicy {
    pub(crate) multi_line: bool,
    pub(crate) validate: Option<ValidateFn>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineChange {
    Splice {
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    },
    Full,
}

#[derive(Clone, Copy, Debug)]
enum SelectionAfter {
    AfterLastEdit,
    Remap,
    Explicit(Selection),
}

enum Record {
    Undoable(EditKind),
    CommitPreedit(Composition),
    Preedit {
        replaced_text: String,
        sel_before: Selection,
        cursor_byte: usize,
    },
    ResetHistory,
    Untracked,
}

enum EditSource {
    Changes(ChangeSet),
    ReplaceRope(Rope),
}

struct Edit {
    source: EditSource,
    selection: SelectionAfter,
    record: Record,
    validate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Composition {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) cursor: usize,
    replaced_text: String,
    sel_before: Selection,
}

pub(crate) struct TextModel {
    text: Rope,
    policy: EditPolicy,
    selection: Selection,
    composition: Option<Composition>,
    history: History,
    revision: u64,
}

impl TextModel {
    pub(crate) fn new(policy: EditPolicy) -> Self {
        Self {
            text: Rope::new(),
            policy,
            selection: Selection::default(),
            composition: None,
            history: History::new(),
            revision: 0,
        }
    }

    pub(crate) fn with_rope(mut self, rope: Rope) -> Self {
        self.text = rope;
        self
    }

    pub(crate) fn is_multi_line(&self) -> bool {
        self.policy.multi_line
    }

    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn selection(&self) -> Selection {
        self.selection
    }

    pub(crate) fn composition(&self) -> Option<&Composition> {
        self.composition.as_ref()
    }

    pub(crate) fn composition_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|c| c.start..c.end)
    }

    pub(crate) fn value(&self) -> String {
        self.text.to_string()
    }

    pub(crate) fn cursor(&self) -> usize {
        if let Some(comp) = &self.composition {
            return comp
                .start
                .saturating_add(comp.cursor)
                .clamp(comp.start, comp.end);
        }
        self.selection.head
    }

    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.selection.range()
    }

    pub(crate) fn selected_text(&self) -> RopeSlice<'_> {
        self.text.slice(self.selection.range())
    }

    pub(crate) fn caret_line_col(&self) -> (usize, usize) {
        let offset = self.cursor();
        let pt = self.text.offset_to_point(offset);
        let line_start = self.text.line_start_offset(pt.row);
        let col = self
            .text
            .byte_to_char_idx(offset)
            .saturating_sub(self.text.byte_to_char_idx(line_start));
        (pt.row + 1, col + 1)
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.history.is_clean() && self.composition.is_none()
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.commit_composition();
        self.history.begin_save()
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.history.mark_saved(version);
    }

    pub(crate) fn break_coalescing(&mut self) {
        self.history.break_coalescing();
    }

    pub(crate) fn snap_boundary(&self, offset: usize, bias: Bias) -> usize {
        snap_grapheme_boundary(&self.text, offset, bias)
    }

    fn normalize<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.policy.multi_line {
            if text.contains(['\n', '\r']) {
                Cow::Owned(text.replace(['\n', '\r'], ""))
            } else {
                Cow::Borrowed(text)
            }
        } else if text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        }
    }

    fn accepts(&self, candidate: &str) -> bool {
        candidate.is_empty() || self.policy.validate.is_none_or(|f| f(candidate))
    }

    fn debug_assert_invariants(&self) {
        debug_assert!(self.selection.start() <= self.selection.end());
        debug_assert!(self.selection.end() <= self.text.len());
        debug_assert!(self.text.is_char_boundary(self.selection.anchor));
        debug_assert!(self.text.is_char_boundary(self.selection.head));
        if let Some(comp) = &self.composition {
            debug_assert!(comp.start <= comp.end && comp.end <= self.text.len());
            debug_assert!(self.text.is_char_boundary(comp.start));
            debug_assert!(self.text.is_char_boundary(comp.end));
        }
    }

    fn commit(&mut self, edit: Edit) -> Option<LineChange> {
        let sel_before = self.selection;
        let before_rope = self.text.clone();

        let (lines, applied_changes, next_selection) = match edit.source {
            EditSource::ReplaceRope(rope) => {
                self.text = rope;
                let sel = match edit.selection {
                    SelectionAfter::Explicit(sel) => sel,
                    SelectionAfter::AfterLastEdit | SelectionAfter::Remap => {
                        Selection::caret(self.text.len())
                    }
                };
                (LineChange::Full, None, sel)
            }
            EditSource::Changes(raw) => {
                if raw.is_empty() {
                    return None;
                }
                let changes = ChangeSet::new(
                    raw.changes
                        .into_iter()
                        .map(|c| {
                            let start = self.text.clip_offset(c.range.start, Bias::Left);
                            let end = self.text.clip_offset(c.range.end, Bias::Right);
                            Change {
                                range: start.min(end)..start.max(end),
                                text: self.normalize(&c.text).into_owned(),
                            }
                        })
                        .collect(),
                );
                let first_start = changes.changes[0].range.start;
                let last_end = changes.changes.last().unwrap().range.end;
                let old_start_line = self.text.offset_to_point(first_start).row;
                let old_end_line = self.text.offset_to_point(last_end).row;

                if changes.len() > BULK_REBUILD_THRESHOLD {
                    self.text = rebuild_rope(&self.text, &changes.changes);
                } else {
                    for c in changes.changes.iter().rev() {
                        self.text.replace(c.range.clone(), &c.text);
                    }
                }

                if edit.validate
                    && !self.policy.multi_line
                    && self.policy.validate.is_some()
                    && !self.accepts(&self.text.to_string())
                    && self.accepts(&before_rope.to_string())
                {
                    self.text = before_rope;
                    return None;
                }

                let mut shift: isize = 0;
                let mut last_new_end = first_start;
                for c in &changes.changes {
                    last_new_end = (c.range.start as isize + shift) as usize + c.text.len();
                    shift += c.text.len() as isize - (c.range.end - c.range.start) as isize;
                }
                let lines = if matches!(edit.record, Record::ResetHistory) {
                    LineChange::Full
                } else {
                    let new_start_line = self.text.offset_to_point(first_start).row;
                    let new_end_line = self.text.offset_to_point(last_new_end).row;
                    LineChange::Splice {
                        old_lines: old_start_line..old_end_line + 1,
                        new_lines: new_start_line..new_end_line + 1,
                    }
                };
                let sel = match edit.selection {
                    SelectionAfter::AfterLastEdit => Selection::caret(last_new_end),
                    SelectionAfter::Remap => Selection {
                        anchor: changes.remap_offset(sel_before.anchor),
                        head: changes.remap_offset(sel_before.head),
                        end_affinity: false,
                    },
                    SelectionAfter::Explicit(sel) => sel,
                };
                (lines, Some(changes), sel)
            }
        };

        self.revision = self.revision.wrapping_add(1);
        let mut snapped = next_selection;
        snapped.anchor = self.snap_boundary(snapped.anchor, Bias::Left);
        snapped.head = self.snap_boundary(snapped.head, Bias::Left);
        self.selection = snapped;

        match edit.record {
            Record::Preedit {
                replaced_text,
                sel_before,
                cursor_byte,
            } => {
                let change = &applied_changes.as_ref().unwrap().changes[0];
                let start = change.range.start;
                let end = start + change.text.len();
                let caret = self.snap_boundary(start + cursor_byte, Bias::Left);
                self.composition = Some(Composition {
                    start,
                    end,
                    cursor: caret.saturating_sub(start),
                    replaced_text,
                    sel_before,
                });
                self.selection = Selection::caret(caret);
            }
            Record::CommitPreedit(comp) => {
                self.composition = None;
                let change = &applied_changes.as_ref().unwrap().changes[0];
                let orig_end = comp.start + comp.replaced_text.len();
                let new_end = comp.start + change.text.len();
                self.history.push(Transaction {
                    kind: EditKind::Atomic,
                    payload: TxPayload::Changes {
                        undo: ChangeSet::single(comp.start..new_end, comp.replaced_text),
                        redo: ChangeSet::single(comp.start..orig_end, change.text.clone()),
                    },
                    selection_before: comp.sel_before,
                    selection_after: self.selection,
                });
            }
            Record::Undoable(kind) => {
                self.composition = None;
                if let Some(changes) = applied_changes {
                    let payload = if changes.len() > SNAPSHOT_HISTORY_THRESHOLD {
                        let delta_bytes: usize = changes
                            .changes
                            .iter()
                            .map(|c| (c.range.end - c.range.start) + c.text.len())
                            .sum();
                        TxPayload::Snapshot {
                            before: before_rope,
                            after: self.text.clone(),
                            delta_bytes,
                        }
                    } else {
                        let undo = invert_changes(&before_rope, &changes);
                        TxPayload::Changes {
                            undo,
                            redo: changes,
                        }
                    };
                    self.history.push(Transaction {
                        kind,
                        payload,
                        selection_before: sel_before,
                        selection_after: self.selection,
                    });
                }
            }
            Record::ResetHistory => {
                self.composition = None;
                self.history.clear();
            }
            Record::Untracked => {
                self.composition = None;
            }
        }

        self.debug_assert_invariants();
        Some(lines)
    }

    pub(crate) fn replace_all(
        &mut self,
        ranges: &[Range<usize>],
        replacement: &str,
    ) -> Option<LineChange> {
        self.commit_composition();
        if ranges.is_empty() {
            return None;
        }
        let replacement = self.normalize(replacement).into_owned();
        let changes = ChangeSet::new(
            ranges
                .iter()
                .map(|r| Change {
                    range: r.clone(),
                    text: replacement.clone(),
                })
                .collect(),
        );
        let next_cursor = changes.remap_offset(self.cursor());
        self.commit(Edit {
            source: EditSource::Changes(changes),
            selection: SelectionAfter::Explicit(Selection::caret(next_cursor)),
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        })
    }

    pub(crate) fn set_value(&mut self, value: &str) -> Option<LineChange> {
        self.composition = None;
        let normalized = self.normalize(value).into_owned();
        let target_offset = if self.policy.multi_line {
            0
        } else {
            normalized.len()
        };
        let full = 0..self.text.len();
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(full, normalized)),
            selection: SelectionAfter::Explicit(Selection::caret(target_offset)),
            record: Record::ResetHistory,
            validate: false,
        })
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> Option<LineChange> {
        if let Some(comp) = self.composition.take() {
            let range = comp.start..comp.end;
            let res = self.commit(Edit {
                source: EditSource::Changes(ChangeSet::single(range, text)),
                selection: SelectionAfter::AfterLastEdit,
                record: Record::CommitPreedit(comp.clone()),
                validate: true,
            });
            if res.is_none() {
                self.composition = Some(comp);
            }
            return res;
        }

        let range = self.selected_range();
        let normalized = self.normalize(text).into_owned();
        let kind = if range.is_empty() && !normalized.is_empty() && !normalized.contains('\n') {
            EditKind::Insert
        } else {
            EditKind::Atomic
        };
        self.replace_range(range, normalized, kind)
    }

    pub(crate) fn replace_selection(&mut self, text: &str) -> Option<LineChange> {
        self.commit_composition();
        self.replace_range(self.selected_range(), text, EditKind::Atomic)
    }

    fn replace_range(
        &mut self,
        range: Range<usize>,
        text: impl Into<String>,
        kind: EditKind,
    ) -> Option<LineChange> {
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, text)),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Undoable(kind),
            validate: true,
        })
    }

    pub(crate) fn set_composition(&mut self, text: &str, cursor_utf16: usize) -> Option<LineChange> {
        if text.is_empty() {
            return self.cancel_composition();
        }
        let normalized = self.normalize(text).into_owned();
        if normalized.is_empty() {
            return self.cancel_composition();
        }
        let cursor_byte = utf16_offset_to_byte(&normalized, cursor_utf16);
        let (range, replaced_text, sel_before) = match &self.composition {
            Some(comp) => (comp.start..comp.end, comp.replaced_text.clone(), comp.sel_before),
            None => {
                let r = self.selected_range();
                let old = self.text.slice(r.clone()).to_string();
                (r, old, self.selection)
            }
        };
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, normalized)),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Preedit {
                replaced_text,
                sel_before,
                cursor_byte,
            },
            validate: true,
        })
    }

    pub(crate) fn commit_composition(&mut self) -> bool {
        let Some(comp) = self.composition.take() else {
            return false;
        };
        let committed = self.text.slice(comp.start..comp.end).to_string();
        self.selection = Selection::caret(self.snap_boundary(comp.end, Bias::Left));
        let orig_end = comp.start + comp.replaced_text.len();
        if committed != comp.replaced_text {
            self.history.push(Transaction {
                kind: EditKind::Atomic,
                payload: TxPayload::Changes {
                    undo: ChangeSet::single(comp.start..comp.end, comp.replaced_text),
                    redo: ChangeSet::single(comp.start..orig_end, committed),
                },
                selection_before: comp.sel_before,
                selection_after: self.selection,
            });
        }
        self.debug_assert_invariants();
        true
    }

    pub(crate) fn cancel_composition(&mut self) -> Option<LineChange> {
        let comp = self.composition.take()?;
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(comp.start..comp.end, comp.replaced_text)),
            selection: SelectionAfter::Explicit(comp.sel_before),
            record: Record::Untracked,
            validate: false,
        })
    }

    fn delete_with(
        &mut self,
        collapsed_kind: EditKind,
        collapsed_span: impl FnOnce(&Rope, usize) -> Range<usize>,
    ) -> Option<LineChange> {
        self.commit_composition();
        let (range, kind) = if self.selection.is_empty() {
            let span = collapsed_span(&self.text, self.selection.head);
            if span.start == span.end {
                return None;
            }
            (span.start.min(span.end)..span.start.max(span.end), collapsed_kind)
        } else {
            (self.selection.range(), EditKind::Atomic)
        };
        self.replace_range(range, "", kind)
    }

    pub(crate) fn delete_backward(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Backspace, |t, off| {
            prev_grapheme_boundary(t, off)..off
        })
    }

    pub(crate) fn delete_forward(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::DeleteForward, |t, off| {
            off..next_grapheme_boundary(t, off)
        })
    }

    pub(crate) fn delete_prev_word(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Atomic, |t, off| prev_word_start(t, off)..off)
    }

    pub(crate) fn delete_next_word(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Atomic, |t, off| off..next_word_end(t, off))
    }

    pub(crate) fn enter(&mut self) -> Option<LineChange> {
        self.commit_composition();
        if !self.policy.multi_line {
            return None;
        }
        self.replace_range(self.selected_range(), "\n", EditKind::Atomic)
    }

    pub(crate) fn indent(&mut self, block: bool) -> Option<Option<LineChange>> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let before = self.selection;
        let spans_multiple_lines = !before.is_empty()
            && self.text.offset_to_point(before.start()).row
                != self.text.offset_to_point(before.end()).row;
        if !block && !spans_multiple_lines {
            return Some(self.replace_range(before.range(), "\t", EditKind::Atomic));
        }
        let (start_row, end_row) = self.selected_line_bounds(before);
        let changes = ChangeSet::new(
            (start_row..=end_row)
                .map(|row| {
                    let start = self.text.line_start_offset(row);
                    Change {
                        range: start..start,
                        text: "\t".into(),
                    }
                })
                .collect(),
        );
        Some(self.commit(Edit {
            source: EditSource::Changes(changes),
            selection: SelectionAfter::Remap,
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        }))
    }

    pub(crate) fn outdent(&mut self) -> Option<Option<LineChange>> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let (start_row, end_row) = self.selected_line_bounds(self.selection);
        let changes: Vec<Change> = (start_row..=end_row)
            .filter_map(|row| {
                let start = self.text.line_start_offset(row);
                let remove = self.leading_outdent_bytes(start);
                (remove > 0).then(|| Change {
                    range: start..start + remove,
                    text: String::new(),
                })
            })
            .collect();
        if changes.is_empty() {
            return Some(None);
        }
        Some(self.commit(Edit {
            source: EditSource::Changes(ChangeSet::new(changes)),
            selection: SelectionAfter::Remap,
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        }))
    }

    fn selected_line_bounds(&self, sel: Selection) -> (usize, usize) {
        let start_row = self.text.offset_to_point(sel.start()).row;
        let end_pt = self.text.offset_to_point(sel.end());
        let end_row = if !sel.is_empty() && end_pt.column == 0 && end_pt.row > start_row {
            end_pt.row - 1
        } else {
            end_pt.row
        };
        (start_row, end_row)
    }

    fn leading_outdent_bytes(&self, line_start: usize) -> usize {
        if self.text.char_at(line_start) == Some('\t') {
            return 1;
        }
        self.text
            .slice(line_start..)
            .chars()
            .take(8)
            .take_while(|&ch| ch == ' ')
            .count()
    }

    pub(crate) fn undo(&mut self) -> Option<LineChange> {
        self.replay(true)
    }

    pub(crate) fn redo(&mut self) -> Option<LineChange> {
        self.replay(false)
    }

    fn replay(&mut self, is_undo: bool) -> Option<LineChange> {
        self.commit_composition();
        let replay = self.history.replay(is_undo)?;
        let source = match replay.payload {
            ReplayPayload::Changes(changes) => EditSource::Changes(changes),
            ReplayPayload::Snapshot(rope) => EditSource::ReplaceRope(rope),
        };
        self.commit(Edit {
            source,
            selection: SelectionAfter::Explicit(replay.selection),
            record: Record::Untracked,
            validate: false,
        })
    }

    pub(crate) fn select_all(&mut self) {
        self.commit_composition();
        self.selection = Selection::from_range(0..self.text.len());
        self.debug_assert_invariants();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.commit_composition();
        let end_bias = if range.start == range.end {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.snap_boundary(range.start, Bias::Left);
        let end = self.snap_boundary(range.end, end_bias);
        self.selection = Selection::from_range(start..end);
        self.debug_assert_invariants();
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.commit_composition();
        let offset = self.snap_boundary(self.text.line_start_offset(line), Bias::Left);
        self.selection = Selection::caret(offset);
        self.debug_assert_invariants();
    }

    pub(crate) fn place_caret(&mut self, offset: usize, end_affinity: bool) {
        self.commit_composition();
        let offset = self.snap_boundary(offset, Bias::Left);
        self.selection.place(offset, end_affinity);
        self.debug_assert_invariants();
    }

    pub(crate) fn extend_selection(
        &mut self,
        offset: usize,
        end_affinity: bool,
        anchor_word: Option<Range<usize>>,
    ) {
        let clamped = self.snap_boundary(offset, Bias::Left);
        self.selection
            .extend_to(clamped, end_affinity, anchor_word, self.text.len());
        self.debug_assert_invariants();
    }

    pub(crate) fn select_word_at(&mut self, offset: usize) -> Option<Range<usize>> {
        let range = word_range_at(&self.text, offset)?;
        self.selection = Selection::from_range(range.clone());
        self.debug_assert_invariants();
        Some(range)
    }

    pub(crate) fn select_line_at(&mut self, offset: usize) {
        let range = line_range_at(&self.text, offset);
        self.selection = Selection::from_range(range);
        self.debug_assert_invariants();
    }
}

fn invert_changes(before: &Rope, forward: &ChangeSet) -> ChangeSet {
    let mut shift: isize = 0;
    let mut inverse = Vec::with_capacity(forward.len());
    for c in &forward.changes {
        let new_start = (c.range.start as isize + shift) as usize;
        let new_end = new_start + c.text.len();
        inverse.push(Change {
            range: new_start..new_end,
            text: before.slice(c.range.clone()).to_string(),
        });
        shift += c.text.len() as isize - (c.range.end - c.range.start) as isize;
    }
    ChangeSet::new(inverse)
}

fn rebuild_rope(text: &Rope, changes: &[Change]) -> Rope {
    let mut builder = RopeBuilder::new();
    let mut cursor = 0usize;
    for c in changes {
        if c.range.start > cursor {
            for chunk in text.slice(cursor..c.range.start).chunks() {
                builder.append(chunk);
            }
        }
        if !c.text.is_empty() {
            builder.append(&c.text);
        }
        cursor = c.range.end;
    }
    if cursor < text.len() {
        for chunk in text.slice(cursor..text.len()).chunks() {
            builder.append(chunk);
        }
    }
    builder.finish()
}

fn utf16_offset_to_byte(text: &str, target_utf16: usize) -> usize {
    let mut u16_pos = 0usize;
    for (byte_ix, ch) in text.char_indices() {
        if u16_pos >= target_utf16 {
            return byte_ix;
        }
        u16_pos += ch.len_utf16();
    }
    text.len()
}

const MAX_HISTORY_ENTRIES: usize = 1000;
const MAX_CHANGES_PER_ENTRY: usize = 1000;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
}

impl Change {
    fn approx_bytes(&self) -> usize {
        self.text.len() + std::mem::size_of::<Self>()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChangeSet {
    changes: Vec<Change>,
}

impl ChangeSet {
    pub(crate) fn single(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            changes: vec![Change {
                range,
                text: text.into(),
            }],
        }
    }

    pub(crate) fn new(mut changes: Vec<Change>) -> Self {
        changes.sort_by_key(|c| c.range.start);
        debug_assert!(
            changes.windows(2).all(|w| w[0].range.end <= w[1].range.start),
            "ChangeSet ranges must be non-overlapping"
        );
        Self { changes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.changes.len()
    }

    fn approx_bytes(&self) -> usize {
        self.changes.iter().map(Change::approx_bytes).sum()
    }

    pub(crate) fn remap_offset(&self, offset: usize) -> usize {
        remap_offset(
            self.changes.iter().map(|c| (c.range.clone(), c.text.len())),
            offset,
        )
    }
}

fn remap_offset(
    changes: impl IntoIterator<Item = (Range<usize>, usize)>,
    offset: usize,
) -> usize {
    let mut out = offset;
    for (range, new_len) in changes {
        if range.end <= offset {
            out = out - (range.end - range.start) + new_len;
        } else if range.start < offset {
            let inside = offset - range.start;
            out = out - inside + inside.min(new_len);
            break;
        } else {
            break;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insert,
    Backspace,
    DeleteForward,
    Atomic,
}

#[derive(Debug)]
pub(crate) enum TxPayload {
    Changes { undo: ChangeSet, redo: ChangeSet },
    Snapshot { before: Rope, after: Rope, delta_bytes: usize },
}

impl TxPayload {
    fn approx_bytes(&self) -> usize {
        match self {
            Self::Changes { undo, redo } => undo.approx_bytes() + redo.approx_bytes(),
            Self::Snapshot { delta_bytes, .. } => (*delta_bytes).max(4096),
        }
    }

    fn is_noop(&self) -> bool {
        match self {
            Self::Changes { undo, redo } => undo.is_empty() || undo == redo,
            Self::Snapshot { .. } => false,
        }
    }
}

pub(crate) struct Transaction {
    pub(crate) kind: EditKind,
    pub(crate) payload: TxPayload,
    pub(crate) selection_before: Selection,
    pub(crate) selection_after: Selection,
}

#[derive(Debug)]
struct HistoryEntry {
    kind: EditKind,
    payload: TxPayload,
    merged_count: usize,
    approx_bytes: usize,
    selection_before: Selection,
    selection_after: Selection,
    version_before: u64,
    version_after: u64,
}

pub(crate) enum ReplayPayload {
    Changes(ChangeSet),
    Snapshot(Rope),
}

pub(crate) struct HistoryReplay {
    pub(crate) payload: ReplayPayload,
    pub(crate) selection: Selection,
}

#[derive(Debug)]
pub(crate) struct History {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    total_bytes: usize,
    coalesce_broken: bool,
    next_version: u64,
    current_version: u64,
    saved_version: u64,
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            total_bytes: 0,
            coalesce_broken: false,
            next_version: 1,
            current_version: 0,
            saved_version: 0,
        }
    }

    pub(crate) fn break_coalescing(&mut self) {
        self.coalesce_broken = true;
    }

    fn clear_redo(&mut self) {
        for entry in self.redo_stack.drain(..) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.approx_bytes);
        }
    }

    fn enforce_budget(&mut self) {
        while self.undo_stack.len() > MAX_HISTORY_ENTRIES
            || (self.undo_stack.len() > 1 && self.total_bytes > MAX_HISTORY_BYTES)
        {
            let evicted = self.undo_stack.remove(0);
            self.total_bytes = self.total_bytes.saturating_sub(evicted.approx_bytes);
        }
    }

    pub(crate) fn push(&mut self, tx: Transaction) {
        let Transaction {
            kind,
            payload,
            selection_before,
            selection_after,
        } = tx;

        if payload.is_noop() {
            self.coalesce_broken = true;
            return;
        }

        self.clear_redo();

        if !self.coalesce_broken
            && kind != EditKind::Atomic
            && let Some(prev) = self.undo_stack.last_mut()
            && prev.kind == kind
            && prev.merged_count < MAX_CHANGES_PER_ENTRY
            && prev.selection_after == selection_before
            && let (
                TxPayload::Changes {
                    undo: prev_undo,
                    redo: prev_redo,
                },
                TxPayload::Changes {
                    undo: curr_undo,
                    redo: curr_redo,
                },
            ) = (&mut prev.payload, &payload)
            && try_coalesce(kind, prev_undo, prev_redo, curr_undo, curr_redo)
        {
            let old_bytes = prev.approx_bytes;
            let version = self.next_version;
            self.next_version += 1;
            prev.version_after = version;
            self.current_version = version;
            prev.merged_count += 1;
            prev.selection_after = selection_after;
            let new_bytes = prev.payload.approx_bytes();
            prev.approx_bytes = new_bytes;
            self.total_bytes = self.total_bytes.saturating_sub(old_bytes) + new_bytes;
            self.enforce_budget();
            return;
        }

        let version_before = self.current_version;
        let version_after = self.next_version;
        self.next_version += 1;
        self.current_version = version_after;
        let merged_count = match &payload {
            TxPayload::Changes { redo, .. } => redo.len(),
            TxPayload::Snapshot { .. } => 1,
        };
        let approx_bytes = payload.approx_bytes();
        self.total_bytes += approx_bytes;
        self.undo_stack.push(HistoryEntry {
            kind,
            payload,
            merged_count,
            approx_bytes,
            selection_before,
            selection_after,
            version_before,
            version_after,
        });
        self.coalesce_broken = kind == EditKind::Atomic;
        self.enforce_budget();
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.coalesce_broken = true;
        self.current_version
    }

    pub(crate) fn mark_saved(&mut self, version: u64) {
        self.saved_version = version;
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.saved_version == self.current_version
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub(crate) fn replay(&mut self, is_undo: bool) -> Option<HistoryReplay> {
        let (from, to) = if is_undo {
            (&mut self.undo_stack, &mut self.redo_stack)
        } else {
            (&mut self.redo_stack, &mut self.undo_stack)
        };
        let entry = from.pop()?;
        let (version, selection) = if is_undo {
            (entry.version_before, entry.selection_before)
        } else {
            (entry.version_after, entry.selection_after)
        };
        let payload = match (&entry.payload, is_undo) {
            (TxPayload::Changes { undo: c, .. }, true) | (TxPayload::Changes { redo: c, .. }, false) => {
                ReplayPayload::Changes(c.clone())
            }
            (TxPayload::Snapshot { before: r, .. }, true)
            | (TxPayload::Snapshot { after: r, .. }, false) => ReplayPayload::Snapshot(r.clone()),
        };
        to.push(entry);
        self.current_version = version;
        self.coalesce_broken = true;
        Some(HistoryReplay { payload, selection })
    }
}

fn try_coalesce(
    kind: EditKind,
    prev_undo: &mut ChangeSet,
    prev_redo: &mut ChangeSet,
    curr_undo: &ChangeSet,
    curr_redo: &ChangeSet,
) -> bool {
    let ([pu], [pr], [cu], [cr]) = (
        prev_undo.changes.as_mut_slice(),
        prev_redo.changes.as_mut_slice(),
        curr_undo.changes.as_slice(),
        curr_redo.changes.as_slice(),
    ) else {
        return false;
    };
    match kind {
        EditKind::Insert => {
            if !pr.range.is_empty()
                || pr.text.contains(['\n', '\r'])
                || cr.text.contains(['\n', '\r'])
                || cr.range.start < pu.range.start
                || cr.range.end != pu.range.end
            {
                return false;
            }
            let cut = cr.range.start - pr.range.start;
            if cut > pr.text.len() || !pr.text.is_char_boundary(cut) {
                return false;
            }
            pr.text.truncate(cut);
            pr.text.push_str(&cr.text);
            pu.range.end = pr.range.start + pr.text.len();
            true
        }
        EditKind::Backspace => {
            if !pr.text.is_empty() || !cr.text.is_empty() || cr.range.end != pr.range.start {
                return false;
            }
            pr.range.start = cr.range.start;
            pu.range = cr.range.start..cr.range.start;
            pu.text.insert_str(0, &cu.text);
            true
        }
        EditKind::DeleteForward => {
            if !pr.text.is_empty() || !cr.text.is_empty() || cr.range.start != pr.range.start {
                return false;
            }
            pr.range.end += cr.range.end - cr.range.start;
            pu.text.push_str(&cu.text);
            true
        }
        EditKind::Atomic => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_caret_basics() {
        let mut sel = Selection::from_range(5..10);
        assert_eq!(sel.range(), 5..10);
        assert!(!sel.is_empty());
        assert_eq!(sel.head, 10);

        sel.extend_to(2, false, None, 20);
        assert_eq!(sel.range(), 2..5);
        assert_eq!(sel.head, 2);

        sel.place(7, true);
        assert!(sel.is_empty());
        assert!(sel.end_affinity);

        sel.extend_to(3, false, Some(5..8), 20);
        assert_eq!(sel.range(), 3..8);
        assert_eq!(sel.head, 3);
        sel.extend_to(12, false, Some(5..8), 20);
        assert_eq!(sel.range(), 5..12);
        assert_eq!(sel.head, 12);
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

    #[test]
    fn ime_preedit_does_not_pollute_history_and_commits_atomically() {
        let mut m = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        })
        .with_rope(Rope::from("hello world"));
        m.set_selected_range(6..11);

        m.set_composition("に", 1);
        assert_eq!(m.value(), "hello に");
        assert!(!m.can_undo());
        assert!(!m.is_clean());

        m.set_composition("にほん", 3);
        assert_eq!(m.value(), "hello にほん");
        assert!(!m.can_undo());

        // Cancelling restores original replaced text without adding an undo entry.
        m.cancel_composition();
        assert_eq!(m.value(), "hello world");
        assert_eq!(m.selected_range(), 6..11);
        assert!(!m.can_undo());
        assert!(m.is_clean());

        // Committing pushes one atomic undo entry restoring the replaced text.
        m.set_composition("に", 1);
        m.insert_text("日本");
        assert_eq!(m.value(), "hello 日本");
        assert!(m.can_undo());
        m.undo();
        assert_eq!(m.value(), "hello world");
        assert_eq!(m.selected_range(), 6..11);
        assert!(m.is_clean());
    }

    #[test]
    fn single_line_validation_and_indent_remapping_work() {
        let mut field = TextModel::new(EditPolicy {
            multi_line: false,
            validate: Some(|s| s.bytes().all(|b| b.is_ascii_digit())),
        });
        assert!(field.insert_text("123").is_some());
        assert!(field.insert_text("abc").is_none());
        assert_eq!(field.value(), "123");
        assert!(field.enter().is_none());

        let mut doc = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        })
        .with_rope(Rope::from("alpha\nbeta\ngamma"));
        doc.set_selected_range(2..9);
        doc.indent(true);
        assert_eq!(doc.value(), "\talpha\n\tbeta\ngamma");
        assert_eq!(doc.selected_range(), 3..11);
        doc.outdent();
        assert_eq!(doc.value(), "alpha\nbeta\ngamma");
        assert_eq!(doc.selected_range(), 2..9);
    }

    #[test]
    fn replace_all_remaps_cursor_and_round_trips_undo_redo() {
        let ranges = [0..3, 8..11];
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 0), 0);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 2), 1);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 0)), 2), 0);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 5)), 2), 2);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 3), 1);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 5), 3);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 10), 7);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 0)), 10), 5);
        assert_eq!(remap_offset(ranges.iter().map(|r| (r.clone(), 1)), 11), 7);

        let mut doc = TextModel::new(EditPolicy {
            multi_line: true,
            validate: None,
        })
        .with_rope(Rope::from("aa bb cc aa dd aa"));
        doc.place_caret(10, false);
        doc.replace_all(&[0..2, 9..11, 15..17], "XYZ");
        assert_eq!(doc.value(), "XYZ bb cc XYZ dd XYZ");
        assert_eq!(doc.cursor(), 11);

        doc.undo();
        assert_eq!(doc.value(), "aa bb cc aa dd aa");
        assert_eq!(doc.cursor(), 10);
        doc.redo();
        assert_eq!(doc.value(), "XYZ bb cc XYZ dd XYZ");
        assert_eq!(doc.cursor(), 11);
    }

    fn insert_tx(offset: usize, text: &str, kind: EditKind) -> Transaction {
        Transaction {
            kind,
            payload: TxPayload::Changes {
                undo: ChangeSet::single(offset..offset + text.len(), ""),
                redo: ChangeSet::single(offset..offset, text),
            },
            selection_before: Selection::caret(offset),
            selection_after: Selection::caret(offset + text.len()),
        }
    }

    fn undo_changes(h: &mut History) -> ChangeSet {
        match h.replay(true).unwrap().payload {
            ReplayPayload::Changes(c) => c,
            ReplayPayload::Snapshot(_) => panic!("expected changes"),
        }
    }

    #[test]
    fn adjacent_typing_coalesces_and_splits_at_limit() {
        let mut h = History::new();
        h.push(insert_tx(0, "a", EditKind::Insert));
        h.push(insert_tx(1, "b", EditKind::Insert));
        assert_eq!(undo_changes(&mut h), ChangeSet::single(0..2, ""));
        assert!(h.replay(true).is_none());

        let mut h = History::new();
        for i in 0..1_100 {
            h.push(insert_tx(i, "a", EditKind::Insert));
        }
        assert_eq!(
            undo_changes(&mut h),
            ChangeSet::single(MAX_CHANGES_PER_ENTRY..MAX_CHANGES_PER_ENTRY + 100, "")
        );
        assert_eq!(
            undo_changes(&mut h),
            ChangeSet::single(0..MAX_CHANGES_PER_ENTRY, "")
        );
        assert!(h.replay(true).is_none());
    }

    #[test]
    fn multi_edit_atomic_isolation_and_stack_cap() {
        let mut h = History::new();
        h.push(Transaction {
            kind: EditKind::Atomic,
            payload: TxPayload::Changes {
                undo: ChangeSet::new(vec![
                    Change {
                        range: 0..1,
                        text: String::new(),
                    },
                    Change {
                        range: 2..3,
                        text: String::new(),
                    },
                    Change {
                        range: 4..5,
                        text: String::new(),
                    },
                ]),
                redo: ChangeSet::new(vec![
                    Change {
                        range: 0..0,
                        text: "a".into(),
                    },
                    Change {
                        range: 1..1,
                        text: "b".into(),
                    },
                    Change {
                        range: 2..2,
                        text: "c".into(),
                    },
                ]),
            },
            selection_before: Selection::caret(0),
            selection_after: Selection::caret(5),
        });
        assert_eq!(undo_changes(&mut h).len(), 3);

        h.push(insert_tx(0, "x", EditKind::Atomic));
        h.push(insert_tx(1, "y", EditKind::Insert));
        assert_eq!(undo_changes(&mut h).len(), 1);
        assert_eq!(undo_changes(&mut h).len(), 1);

        let mut capped = History::new();
        for i in 0..1_100 {
            capped.push(insert_tx(i, "a", EditKind::Atomic));
        }
        for _ in 0..MAX_HISTORY_ENTRIES {
            assert!(capped.replay(true).is_some());
        }
        assert!(capped.replay(true).is_none());
    }

    #[test]
    fn clean_state_tracks_undo_redo_and_save_boundaries() {
        let mut h = History::new();
        assert!(h.is_clean());

        h.push(insert_tx(0, "a", EditKind::Insert));
        assert!(!h.is_clean());
        assert!(h.replay(true).is_some());
        assert!(h.is_clean());
        assert!(h.can_redo());
        assert!(h.replay(false).is_some());
        assert!(!h.is_clean());

        let saved = h.begin_save();
        h.mark_saved(saved);
        assert!(h.is_clean());
        h.push(insert_tx(1, "b", EditKind::Insert));
        assert!(!h.is_clean());
        assert_eq!(undo_changes(&mut h), ChangeSet::single(1..2, ""));
        assert!(h.is_clean());

        let snapshot = h.begin_save();
        h.push(insert_tx(1, "c", EditKind::Insert));
        h.mark_saved(snapshot);
        assert!(!h.is_clean());
        assert!(h.replay(true).is_some());
        assert!(h.is_clean());

        assert!(h.replay(true).is_some());
        h.push(insert_tx(0, "z", EditKind::Insert));
        assert!(!h.is_clean());
        assert!(h.replay(true).is_some());
        assert!(!h.is_clean());
    }
}
