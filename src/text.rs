//! The text system: byte positions in a rope, grapheme and word navigation, selections, and the
//! edit model with its undo history.

use std::borrow::Cow;
use std::ops::Range;

use ropey::{LineType, RopeBuilder, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};

pub(crate) use ropey::Rope;

/// Which way to round a position that falls inside a character or grapheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Bias {
    Left,
    Right,
}

/// A row (`\n`-separated line) and a byte column within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TextPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

/// Line and position helpers on the rope. Rows are `\n`-separated; a `\r` before the `\n`
/// stays part of its row.
pub(crate) trait RopeExt {
    fn slice_line(&self, row: usize) -> RopeSlice<'_>;
    fn line_len(&self, row: usize) -> usize;
    fn line_start_offset(&self, row: usize) -> usize;
    fn line_end_offset(&self, row: usize) -> usize;
    fn lines_len(&self) -> usize;
    fn char_at(&self, offset: usize) -> Option<char>;
    fn offset_to_point(&self, offset: usize) -> TextPoint;
    fn clip_offset(&self, offset: usize, bias: Bias) -> usize;
    fn replace(&mut self, range: Range<usize>, text: &str);
}

impl RopeExt for Rope {
    /// Row `row` without its `\n`; empty past the last row.
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
        if row >= self.lines_len() {
            return self.len();
        }
        self.line_to_byte_idx(row, LineType::LF)
    }

    fn line_end_offset(&self, row: usize) -> usize {
        self.line_start_offset(row) + self.line_len(row)
    }

    fn lines_len(&self) -> usize {
        self.len_lines(LineType::LF)
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        self.get_char(offset).ok()
    }

    fn offset_to_point(&self, offset: usize) -> TextPoint {
        let offset = self.clip_offset(offset, Bias::Left);
        let row = self.byte_to_line_idx(offset, LineType::LF);
        TextPoint {
            row,
            column: offset - self.line_to_byte_idx(row, LineType::LF),
        }
    }

    /// `offset` clamped to the text and rounded onto a char boundary.
    fn clip_offset(&self, offset: usize, bias: Bias) -> usize {
        match bias {
            _ if offset >= self.len() => self.len(),
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

/// The row around `offset`, without its `\n`.
fn line_range_at(text: &Rope, offset: usize) -> Range<usize> {
    let row = text.offset_to_point(offset).row;
    text.line_start_offset(row)..text.line_end_offset(row)
}

/// Runs a grapheme cursor from `offset`, feeding it the rope's chunks as it asks for them.
fn grapheme_walk<T>(
    text: &Rope,
    offset: usize,
    step: impl Fn(&mut GraphemeCursor, &str, usize) -> Result<T, GraphemeIncomplete>,
) -> T {
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    let (mut chunk, mut start) = text.chunk(offset);
    loop {
        match step(&mut cursor, chunk, start) {
            Ok(found) => return found,
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (context, context_start) = text.chunk(end - 1);
                cursor.provide_context(&context[..end - context_start], context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => (chunk, start) = text.chunk(start - 1),
            Err(GraphemeIncomplete::NextChunk) => (chunk, start) = text.chunk(start + chunk.len()),
            Err(GraphemeIncomplete::InvalidOffset) => unreachable!("grapheme cursor left the text"),
        }
    }
}

pub(crate) fn prev_grapheme_boundary(text: &Rope, offset: usize) -> usize {
    match text.clip_offset(offset, Bias::Left) {
        0 => 0,
        at => grapheme_walk(text, at, GraphemeCursor::prev_boundary).unwrap_or(0),
    }
}

pub(crate) fn next_grapheme_boundary(text: &Rope, offset: usize) -> usize {
    match text.clip_offset(offset, Bias::Right) {
        at if at >= text.len() => text.len(),
        at => grapheme_walk(text, at, GraphemeCursor::next_boundary).unwrap_or(text.len()),
    }
}

/// `offset` if it's a grapheme boundary, otherwise the boundary on the `bias` side.
fn snap_grapheme_boundary(text: &Rope, offset: usize, bias: Bias) -> usize {
    let at = text.clip_offset(offset, bias);
    if at == 0 || at >= text.len() || grapheme_walk(text, at, GraphemeCursor::is_boundary) {
        return at;
    }
    match bias {
        Bias::Left => prev_grapheme_boundary(text, at),
        Bias::Right => next_grapheme_boundary(text, at),
    }
}

#[inline]
fn is_horizontal_ws(ch: char) -> bool {
    ch.is_whitespace() && !matches!(ch, '\n' | '\r')
}

/// The far end of the run of characters matching `keep` at `from`, stopping at `limit`
/// (scanning backwards when `limit` is before `from`).
fn skip_while(text: &Rope, mut from: usize, limit: usize, keep: fn(char) -> bool) -> usize {
    if limit < from {
        for ch in text.chars_at(from).reversed() {
            if from <= limit || !keep(ch) {
                break;
            }
            from -= ch.len_utf8();
        }
    } else {
        for ch in text.chars_at(from) {
            if from >= limit || !keep(ch) {
                break;
            }
            from += ch.len_utf8();
        }
    }
    from
}

/// No word spans a space or tab, so word segmentation (`unicode-segmentation`) only needs the
/// run of text between them. (A mark or format character right after a space belongs to the
/// space's segment; here it starts the run instead.)
fn in_run(ch: char) -> bool {
    !matches!(ch, ' ' | '\t')
}

/// Whether a word segment is a word rather than a run of horizontal whitespace.
fn is_word(segment: &str) -> bool {
    !segment.chars().all(is_horizontal_ws)
}

/// The word, or run of spaces, under `offset` (double-click selection).
fn word_range_at(text: &Rope, offset: usize) -> Option<Range<usize>> {
    let line = line_range_at(text, offset);
    let mut pos = text.clip_offset(offset, Bias::Left);
    if pos >= line.end {
        pos = prev_grapheme_boundary(text, line.end);
    }
    if !line.contains(&pos) {
        return None;
    }
    let run = |keep| skip_while(text, pos, line.start, keep)..skip_while(text, pos, line.end, keep);
    if text.char_at(pos).is_some_and(is_horizontal_ws) {
        return Some(run(is_horizontal_ws));
    }
    let run = run(in_run);
    let words: Cow<str> = text.slice(run.clone()).into();
    let mut words = words.split_word_bound_indices();
    let (i, word) = words.find(|(i, w)| run.start + i + w.len() > pos)?;
    Some(run.start + i..run.start + i + word.len())
}

/// Ctrl+Left: the start of the word before `offset`, skipping spaces; a line start steps back
/// over the line break.
pub(crate) fn prev_word_start(text: &Rope, offset: usize) -> usize {
    let offset = text.clip_offset(offset, Bias::Left);
    if offset == 0 || text.byte(offset - 1) == b'\n' {
        return offset.saturating_sub(1);
    }
    let line_start = text.line_start_offset(text.offset_to_point(offset).row);
    let end = skip_while(text, offset, line_start, is_horizontal_ws);
    let start = skip_while(text, end, line_start, in_run);
    let before: Cow<str> = text.slice(start..end).into();
    let mut words = before.split_word_bound_indices();
    words
        .rfind(|(_, w)| is_word(w))
        .map_or(line_start, |(i, _)| start + i)
}

/// Ctrl+Right: the end of the word after `offset` plus the spaces that follow it; a line end
/// steps over the line break.
pub(crate) fn next_word_end(text: &Rope, offset: usize) -> usize {
    let offset = text.clip_offset(offset, Bias::Left);
    if offset >= text.len() || text.byte(offset) == b'\n' {
        return (offset + 1).min(text.len());
    }
    let line_end = text.line_end_offset(text.offset_to_point(offset).row);
    if text.char_at(offset).is_some_and(is_horizontal_ws) {
        return skip_while(text, offset, line_end, is_horizontal_ws);
    }
    let end = skip_while(text, offset, line_end, in_run);
    let after: Cow<str> = text.slice(offset..end).into();
    let mut words = after.split_word_bound_indices();
    let word_end = words
        .find(|(_, w)| is_word(w))
        .map_or(end, |(i, w)| offset + i + w.len());
    skip_while(text, word_end, line_end, is_horizontal_ws)
}

/// A selection from `anchor` to `head` (the caret). `end_affinity` keeps a caret at a soft-wrap
/// point on the end of the earlier row.
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
        self.range().contains(&offset)
    }

    pub(crate) fn place(&mut self, offset: usize, end_affinity: bool) {
        *self = Self {
            anchor: offset,
            head: offset,
            end_affinity,
        };
    }

    /// Moves the head to `offset`. While extending a double-click (`anchor_word`), the whole
    /// word stays selected and the anchor moves to whichever end of it is away from the head.
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
            let (start, end) = (word.start.min(max_len), word.end.min(max_len));
            (self.anchor, self.head) = if offset < start {
                (end, offset)
            } else if offset > end {
                (start, offset)
            } else {
                (start, end)
            };
        }
    }
}

/// Edits with more changes than this rebuild the rope in one pass.
const BULK_REBUILD_THRESHOLD: usize = 64;
/// Edits with more changes than this are kept in the history as before/after snapshots.
const SNAPSHOT_HISTORY_THRESHOLD: usize = 1024;

pub(crate) type ValidateFn = fn(&str) -> bool;

/// What a model accepts: documents are multi-line; single-line fields drop line breaks and may
/// refuse text their validator rejects.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EditPolicy {
    pub(crate) multi_line: bool,
    pub(crate) validate: Option<ValidateFn>,
}

/// The rows an edit replaced, so views can update only those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineChange {
    Splice {
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    },
    Full,
}

/// Where the selection goes after an edit: a caret after the last change, the old selection
/// carried through the changes, or a given selection.
#[derive(Clone, Copy, Debug)]
enum SelectionAfter {
    AfterLastEdit,
    Remap,
    Explicit(Selection),
}

/// Uncommitted IME text at `start..end`, with its caret `cursor` bytes in, and what it replaced.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Composition {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) cursor: usize,
    replaced_text: String,
    sel_before: Selection,
}

/// How an edit merges into the history: runs of typing, backspacing or forward deleting
/// become one undo step; everything else is its own step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Insert,
    Backspace,
    DeleteForward,
    Atomic,
}

/// An edit: replace a byte range with new text.
type Edit = (Range<usize>, String);

/// A text with its selection, IME composition and undo history. Every edit goes through
/// `commit`, which keeps the selection on grapheme boundaries and bumps `revision`.
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
            history: History::default(),
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

    pub(crate) fn cursor(&self) -> usize {
        match &self.composition {
            Some(c) => c.start.saturating_add(c.cursor).clamp(c.start, c.end),
            None => self.selection.head,
        }
    }

    pub(crate) fn caret_line_col(&self) -> (usize, usize) {
        let offset = self.cursor();
        let row = self.text.offset_to_point(offset).row;
        let line_start = self.text.line_start_offset(row);
        let col = self.text.byte_to_char_idx(offset) - self.text.byte_to_char_idx(line_start);
        (row + 1, col + 1)
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.history.saved == self.history.version && self.composition.is_none()
    }

    /// Ends any composition and typing run; returns the version to pass to `mark_saved_version`
    /// once the save succeeds.
    pub(crate) fn begin_save(&mut self) -> u64 {
        self.commit_composition();
        self.history.coalesce_broken = true;
        self.history.version
    }

    pub(crate) fn mark_saved_version(&mut self, version: u64) {
        self.history.saved = version;
    }

    pub(crate) fn break_coalescing(&mut self) {
        self.history.coalesce_broken = true;
    }

    pub(crate) fn snap_boundary(&self, offset: usize, bias: Bias) -> usize {
        snap_grapheme_boundary(&self.text, offset, bias)
    }

    /// Single-line fields drop line breaks; documents store every line break as `\n`.
    fn normalize<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self.policy.multi_line && text.contains(['\n', '\r']) {
            Cow::Owned(text.replace(['\n', '\r'], ""))
        } else if self.policy.multi_line && text.contains('\r') {
            Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            Cow::Borrowed(text)
        }
    }

    fn accepts(&self, candidate: &str) -> bool {
        candidate.is_empty() || self.policy.validate.is_none_or(|f| f(candidate))
    }

    /// Applies `edits` (clipped to char boundaries, line breaks normalized), places the
    /// selection, and records the edit in the history as `kind` (`None` leaves it untracked).
    /// With `validate`, a single-line field rejects an edit its validator refuses; restores
    /// (undo, redo, cancelling a composition, `set_value`) skip that, as they must go through.
    /// Returns the changed lines.
    fn commit(
        &mut self,
        edits: Vec<Edit>,
        after: SelectionAfter,
        kind: Option<EditKind>,
        validate: bool,
    ) -> Option<LineChange> {
        if edits.is_empty() {
            return None;
        }
        let (before, sel_before) = (self.text.clone(), self.selection);
        let mut edits: Vec<Edit> = edits
            .into_iter()
            .map(|(range, text)| {
                let start = self.text.clip_offset(range.start, Bias::Left);
                let end = self.text.clip_offset(range.end, Bias::Right);
                (
                    start.min(end)..start.max(end),
                    self.normalize(&text).into_owned(),
                )
            })
            .collect();
        edits.sort_by_key(|(range, _)| range.start);
        debug_assert!(
            edits.windows(2).all(|w| w[0].0.end <= w[1].0.start),
            "edits overlap"
        );

        let first = edits[0].0.start;
        let row = |text: &Rope, offset| text.offset_to_point(offset).row;
        let old_lines = row(&self.text, first)..row(&self.text, edits[edits.len() - 1].0.end) + 1;
        if edits.len() > BULK_REBUILD_THRESHOLD {
            self.text = rebuild_rope(&self.text, &edits);
        } else {
            for (range, text) in edits.iter().rev() {
                self.text.replace(range.clone(), text);
            }
        }
        if validate
            && !self.policy.multi_line
            && self.policy.validate.is_some()
            && !self.accepts(&self.text.to_string())
            && self.accepts(&before.to_string())
        {
            self.text = before;
            return None;
        }

        let (mut last_end, mut shift) = (first, 0isize);
        for (range, text) in &edits {
            last_end = range.start.wrapping_add_signed(shift) + text.len();
            shift += text.len() as isize - range.len() as isize;
        }
        let new_lines = row(&self.text, first)..row(&self.text, last_end) + 1;
        self.settle(match after {
            SelectionAfter::AfterLastEdit => Selection::caret(last_end),
            SelectionAfter::Remap => Selection {
                anchor: remap_offset(&edits, sel_before.anchor),
                head: remap_offset(&edits, sel_before.head),
                end_affinity: false,
            },
            SelectionAfter::Explicit(sel) => sel,
        });
        if let Some(kind) = kind {
            let payload = if edits.len() > SNAPSHOT_HISTORY_THRESHOLD {
                let bytes = edits
                    .iter()
                    .map(|(range, text)| range.len() + text.len())
                    .sum::<usize>();
                Payload::Snapshot {
                    text: [before, self.text.clone()],
                    bytes,
                }
            } else {
                let changes = edits.into_iter().map(|(range, new)| Change {
                    at: range.start,
                    old: before.slice(range).to_string(),
                    new,
                });
                Payload::Changes(changes.collect())
            };
            self.history
                .push(kind, payload, [sel_before, self.selection]);
        }
        Some(LineChange::Splice {
            old_lines,
            new_lines,
        })
    }

    /// Bookkeeping after any text change: new revision, selection snapped, composition ended.
    fn settle(&mut self, mut sel: Selection) {
        self.revision = self.revision.wrapping_add(1);
        sel.anchor = self.snap_boundary(sel.anchor, Bias::Left);
        sel.head = self.snap_boundary(sel.head, Bias::Left);
        self.selection = sel;
        self.composition = None;
    }

    fn edit(&mut self, range: Range<usize>, text: String, kind: EditKind) -> Option<LineChange> {
        let edit = vec![(range, text)];
        self.commit(edit, SelectionAfter::AfterLastEdit, Some(kind), true)
    }

    pub(crate) fn replace_all(
        &mut self,
        ranges: &[Range<usize>],
        replacement: &str,
    ) -> Option<LineChange> {
        self.commit_composition();
        let text = self.normalize(replacement).into_owned();
        let mut edits: Vec<Edit> = ranges.iter().map(|r| (r.clone(), text.clone())).collect();
        edits.sort_by_key(|(range, _)| range.start);
        let caret = Selection::caret(remap_offset(&edits, self.cursor()));
        self.commit(
            edits,
            SelectionAfter::Explicit(caret),
            Some(EditKind::Atomic),
            true,
        )
    }

    /// Replaces the whole text and starts a fresh history.
    pub(crate) fn set_value(&mut self, value: &str) -> Option<LineChange> {
        self.composition = None;
        let value = self.normalize(value).into_owned();
        let caret = Selection::caret(if self.policy.multi_line {
            0
        } else {
            value.len()
        });
        let edit = vec![(0..self.text.len(), value)];
        let lines = self.commit(edit, SelectionAfter::Explicit(caret), None, false);
        self.history = History::default();
        lines.map(|_| LineChange::Full)
    }

    /// Types `text` over the selection, or commits it in place of an active composition.
    pub(crate) fn insert_text(&mut self, text: &str) -> Option<LineChange> {
        let text = self.normalize(text).into_owned();
        if let Some(comp) = self.composition.take() {
            let edit = vec![(comp.start..comp.end, text.clone())];
            let Some(lines) = self.commit(edit, SelectionAfter::AfterLastEdit, None, true) else {
                self.composition = Some(comp);
                return None;
            };
            let change = Change {
                at: comp.start,
                old: comp.replaced_text,
                new: text,
            };
            let selection = [comp.sel_before, self.selection];
            self.history
                .push(EditKind::Atomic, Payload::Changes(vec![change]), selection);
            return Some(lines);
        }
        let range = self.selection.range();
        let typing = range.is_empty() && !text.is_empty() && !text.contains('\n');
        self.edit(
            range,
            text,
            if typing {
                EditKind::Insert
            } else {
                EditKind::Atomic
            },
        )
    }

    pub(crate) fn replace_selection(&mut self, text: &str) -> Option<LineChange> {
        self.commit_composition();
        self.edit(self.selection.range(), text.to_owned(), EditKind::Atomic)
    }

    /// Shows uncommitted IME text in place of the selection (or the previous composition).
    /// It stays out of the history until committed.
    pub(crate) fn set_composition(
        &mut self,
        text: &str,
        cursor_utf16: usize,
    ) -> Option<LineChange> {
        let text = self.normalize(text).into_owned();
        if text.is_empty() {
            return self.cancel_composition();
        }
        let cursor = utf16_offset_to_byte(&text, cursor_utf16);
        let (range, replaced_text, sel_before) = match &self.composition {
            Some(c) => (c.start..c.end, c.replaced_text.clone(), c.sel_before),
            None => {
                let range = self.selection.range();
                (
                    range.clone(),
                    self.text.slice(range).to_string(),
                    self.selection,
                )
            }
        };
        let (start, end) = (range.start, range.start + text.len());
        let lines = self.commit(
            vec![(range, text)],
            SelectionAfter::AfterLastEdit,
            None,
            true,
        )?;
        let caret = self.snap_boundary(start + cursor, Bias::Left);
        let cursor = caret.saturating_sub(start);
        self.composition = Some(Composition {
            start,
            end,
            cursor,
            replaced_text,
            sel_before,
        });
        self.selection = Selection::caret(caret);
        Some(lines)
    }

    /// Keeps the composed text and records it as one undoable edit. Returns whether a
    /// composition was active.
    pub(crate) fn commit_composition(&mut self) -> bool {
        let Some(comp) = self.composition.take() else {
            return false;
        };
        let committed = self.text.slice(comp.start..comp.end).to_string();
        self.selection = Selection::caret(self.snap_boundary(comp.end, Bias::Left));
        if committed != comp.replaced_text {
            let change = Change {
                at: comp.start,
                old: comp.replaced_text,
                new: committed,
            };
            let selection = [comp.sel_before, self.selection];
            self.history
                .push(EditKind::Atomic, Payload::Changes(vec![change]), selection);
        }
        true
    }

    /// Restores the text and selection from before the composition.
    pub(crate) fn cancel_composition(&mut self) -> Option<LineChange> {
        let comp = self.composition.take()?;
        let edit = vec![(comp.start..comp.end, comp.replaced_text)];
        self.commit(edit, SelectionAfter::Explicit(comp.sel_before), None, false)
    }

    /// Deletes the selection, or with none the span `span` gives from the caret.
    fn delete_with(
        &mut self,
        kind: EditKind,
        span: impl FnOnce(&Rope, usize) -> Range<usize>,
    ) -> Option<LineChange> {
        self.commit_composition();
        if !self.selection.is_empty() {
            return self.edit(self.selection.range(), String::new(), EditKind::Atomic);
        }
        let span = span(&self.text, self.selection.head);
        if span.start == span.end {
            return None;
        }
        self.edit(
            span.start.min(span.end)..span.start.max(span.end),
            String::new(),
            kind,
        )
    }

    pub(crate) fn delete_backward(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Backspace, |t, at| {
            prev_grapheme_boundary(t, at)..at
        })
    }

    pub(crate) fn delete_forward(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::DeleteForward, |t, at| {
            at..next_grapheme_boundary(t, at)
        })
    }

    pub(crate) fn delete_prev_word(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Atomic, |t, at| prev_word_start(t, at)..at)
    }

    pub(crate) fn delete_next_word(&mut self) -> Option<LineChange> {
        self.delete_with(EditKind::Atomic, |t, at| at..next_word_end(t, at))
    }

    pub(crate) fn enter(&mut self) -> Option<LineChange> {
        self.commit_composition();
        if !self.policy.multi_line {
            return None;
        }
        self.edit(self.selection.range(), "\n".into(), EditKind::Atomic)
    }

    /// Tab: inserts a tab, or with `block` or a multi-line selection indents every selected
    /// line. Single-line fields don't take tabs.
    pub(crate) fn indent(&mut self, block: bool) -> Option<LineChange> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let sel = self.selection;
        let row = |offset| self.text.offset_to_point(offset).row;
        if !block && (sel.is_empty() || row(sel.start()) == row(sel.end())) {
            return self.edit(sel.range(), "\t".into(), EditKind::Atomic);
        }
        let edits = self.selected_rows().map(|r| {
            let start = self.text.line_start_offset(r);
            (start..start, "\t".to_owned())
        });
        self.commit(
            edits.collect(),
            SelectionAfter::Remap,
            Some(EditKind::Atomic),
            true,
        )
    }

    /// Removes one tab, or up to eight spaces, from the start of every selected line.
    pub(crate) fn outdent(&mut self) -> Option<LineChange> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let edits = self.selected_rows().filter_map(|r| {
            let start = self.text.line_start_offset(r);
            let line = self.text.slice(start..);
            let width = if line.chars().next() == Some('\t') {
                1
            } else {
                line.chars().take(8).take_while(|&ch| ch == ' ').count()
            };
            (width > 0).then(|| (start..start + width, String::new()))
        });
        self.commit(
            edits.collect(),
            SelectionAfter::Remap,
            Some(EditKind::Atomic),
            true,
        )
    }

    /// Rows the selection covers; a selection ending at a line start excludes that line.
    fn selected_rows(&self) -> std::ops::RangeInclusive<usize> {
        let sel = self.selection;
        let start = self.text.offset_to_point(sel.start()).row;
        let end = self.text.offset_to_point(sel.end());
        let skip_last = !sel.is_empty() && end.column == 0;
        start..=end.row - usize::from(skip_last)
    }

    pub(crate) fn undo(&mut self) -> Option<LineChange> {
        self.replay(true)
    }

    pub(crate) fn redo(&mut self) -> Option<LineChange> {
        self.replay(false)
    }

    fn replay(&mut self, undo: bool) -> Option<LineChange> {
        self.commit_composition();
        let side = usize::from(!undo);
        let entry = self.history.step(undo)?;
        let selection = entry.selection[side];
        match &entry.payload {
            Payload::Snapshot { text, .. } => {
                self.text = text[side].clone();
                self.settle(selection);
                Some(LineChange::Full)
            }
            Payload::Changes(changes) => {
                let mut shift = 0isize;
                let edits = changes.iter().map(|c| {
                    if !undo {
                        return (c.at..c.at + c.old.len(), c.new.clone());
                    }
                    let at = c.at.wrapping_add_signed(shift);
                    shift += c.new.len() as isize - c.old.len() as isize;
                    (at..at + c.new.len(), c.old.clone())
                });
                let edits = edits.collect();
                self.commit(edits, SelectionAfter::Explicit(selection), None, false)
            }
        }
    }

    pub(crate) fn select_all(&mut self) {
        self.commit_composition();
        self.selection = Selection::from_range(0..self.text.len());
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.commit_composition();
        let end_bias = if range.is_empty() {
            Bias::Left
        } else {
            Bias::Right
        };
        let start = self.snap_boundary(range.start, Bias::Left);
        self.selection = Selection::from_range(start..self.snap_boundary(range.end, end_bias));
    }

    pub(crate) fn go_to_line(&mut self, line: usize) {
        self.commit_composition();
        let offset = self.snap_boundary(self.text.line_start_offset(line), Bias::Left);
        self.selection = Selection::caret(offset);
    }

    pub(crate) fn place_caret(&mut self, offset: usize, end_affinity: bool) {
        self.commit_composition();
        let offset = self.snap_boundary(offset, Bias::Left);
        self.selection.place(offset, end_affinity);
    }

    pub(crate) fn extend_selection(
        &mut self,
        offset: usize,
        end_affinity: bool,
        anchor_word: Option<Range<usize>>,
    ) {
        self.commit_composition();
        let offset = self.snap_boundary(offset, Bias::Left);
        self.selection
            .extend_to(offset, end_affinity, anchor_word, self.text.len());
    }

    pub(crate) fn select_word_at(&mut self, offset: usize) -> Option<Range<usize>> {
        self.commit_composition();
        let range = word_range_at(&self.text, offset)?;
        self.selection = Selection::from_range(range.clone());
        Some(range)
    }

    pub(crate) fn select_line_at(&mut self, offset: usize) {
        self.commit_composition();
        self.selection = Selection::from_range(line_range_at(&self.text, offset));
    }
}

/// Where `offset` lands after `edits` (sorted, in pre-edit positions). An offset inside a
/// replaced range keeps its distance into the new text, up to that text's end.
fn remap_offset(edits: &[Edit], offset: usize) -> usize {
    let mut out = offset;
    for (range, text) in edits {
        if range.end <= offset {
            out = out - range.len() + text.len();
        } else {
            if range.start < offset {
                let inside = offset - range.start;
                out = out - inside + inside.min(text.len());
            }
            break;
        }
    }
    out
}

/// The byte offset of the `target_utf16`th UTF-16 unit (IME carets come in UTF-16).
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

/// Builds the edited text in one pass, for edits too many to apply one by one.
fn rebuild_rope(text: &Rope, edits: &[Edit]) -> Rope {
    let mut builder = RopeBuilder::new();
    let mut cursor = 0;
    for (range, new) in edits {
        for chunk in text.slice(cursor..range.start).chunks() {
            builder.append(chunk);
        }
        builder.append(new);
        cursor = range.end;
    }
    for chunk in text.slice(cursor..).chunks() {
        builder.append(chunk);
    }
    builder.finish()
}

/// History limits: undo steps kept, keystrokes merged into one step, and bytes of edits kept.
const MAX_HISTORY_ENTRIES: usize = 1000;
const MAX_KEYSTROKES_PER_ENTRY: usize = 1000;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;

/// One replacement, kept with the text it replaced so it can be undone and redone.
#[derive(Debug, PartialEq)]
struct Change {
    at: usize,
    old: String,
    new: String,
}

impl Change {
    /// Merges `next`, the following keystroke of the same `kind`, if it continues this one:
    /// typing on at the end, backspacing on from the start, or deleting on from the same spot.
    fn absorb(&mut self, kind: EditKind, next: &Change) -> bool {
        let breaks = ['\n', '\r'];
        match kind {
            EditKind::Insert => {
                let cut = next.at.wrapping_sub(self.at);
                if !self.old.is_empty()
                    || self.new.contains(breaks)
                    || next.new.contains(breaks)
                    || next.at < self.at
                    || next.at + next.old.len() != self.at + self.new.len()
                    || !self.new.is_char_boundary(cut)
                {
                    return false;
                }
                self.new.truncate(cut);
                self.new.push_str(&next.new);
            }
            EditKind::Backspace => {
                if !self.new.is_empty()
                    || !next.new.is_empty()
                    || next.at + next.old.len() != self.at
                {
                    return false;
                }
                self.at = next.at;
                self.old.insert_str(0, &next.old);
            }
            EditKind::DeleteForward => {
                if !self.new.is_empty() || !next.new.is_empty() || next.at != self.at {
                    return false;
                }
                self.old.push_str(&next.old);
            }
            EditKind::Atomic => return false,
        }
        true
    }
}

/// What an undo step restores.
enum Payload {
    /// Each change, with the text it replaced.
    Changes(Vec<Change>),
    /// The whole text before and after an edit with too many changes to keep one by one.
    Snapshot { text: [Rope; 2], bytes: usize },
}

impl Payload {
    /// What the entry is charged against the history budget.
    fn bytes(&self) -> usize {
        match self {
            Self::Changes(changes) => changes
                .iter()
                .map(|c| c.old.len() + c.new.len() + size_of::<Change>())
                .sum(),
            Self::Snapshot { bytes, .. } => *bytes,
        }
    }
}

/// One undo step, `keystrokes` long. `selection` and `version` hold the state before and
/// after it.
struct Entry {
    kind: EditKind,
    payload: Payload,
    keystrokes: usize,
    bytes: usize,
    selection: [Selection; 2],
    version: [u64; 2],
}

/// Undo and redo stacks within an entry and byte budget, plus the version the text was
/// last saved at.
#[derive(Default)]
struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    bytes: usize,
    coalesce_broken: bool,
    last_version: u64,
    version: u64,
    saved: u64,
}

impl History {
    /// Records an edit, merging a keystroke into the previous entry when it continues it.
    fn push(&mut self, kind: EditKind, payload: Payload, selection: [Selection; 2]) {
        if let Payload::Changes(changes) = &payload
            && changes.iter().all(|c| c.old == c.new)
        {
            self.coalesce_broken = true;
            return;
        }
        for entry in self.redo.drain(..) {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
        self.last_version += 1;
        let version = self.last_version;
        if !self.coalesce_broken
            && kind != EditKind::Atomic
            && let Some(prev) = self.undo.last_mut()
            && prev.kind == kind
            && prev.keystrokes < MAX_KEYSTROKES_PER_ENTRY
            && prev.selection[1] == selection[0]
            && let (Payload::Changes(prev_changes), Payload::Changes(changes)) =
                (&mut prev.payload, &payload)
            && let ([prev_change], [change]) = (prev_changes.as_mut_slice(), changes.as_slice())
            && prev_change.absorb(kind, change)
        {
            let bytes = prev.payload.bytes();
            self.bytes = self.bytes.saturating_sub(prev.bytes) + bytes;
            prev.bytes = bytes;
            prev.keystrokes += 1;
            prev.selection[1] = selection[1];
            prev.version[1] = version;
        } else {
            let bytes = payload.bytes();
            self.bytes += bytes;
            let version = [self.version, version];
            self.undo.push(Entry {
                kind,
                payload,
                keystrokes: 1,
                bytes,
                selection,
                version,
            });
            self.coalesce_broken = kind == EditKind::Atomic;
        }
        self.version = version;
        while self.undo.len() > MAX_HISTORY_ENTRIES
            || (self.undo.len() > 1 && self.bytes > MAX_HISTORY_BYTES)
        {
            self.bytes = self.bytes.saturating_sub(self.undo.remove(0).bytes);
        }
    }

    /// Moves the newest entry to the other stack (undo: onto redo) and returns it.
    fn step(&mut self, undo: bool) -> Option<&Entry> {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let entry = from.pop()?;
        self.version = entry.version[usize::from(!undo)];
        self.coalesce_broken = true;
        to.push(entry);
        to.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> TextModel {
        let policy = EditPolicy {
            multi_line: true,
            validate: None,
        };
        TextModel::new(policy).with_rope(Rope::from(text))
    }

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
    fn rope_lines_and_offsets_handle_multibyte_and_crlf() {
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
            TextPoint {
                row: 0,
                column: "hello 中文🎉\nR".len()
            }
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
    fn word_navigation_steps_over_line_breaks_and_long_runs() {
        let long_word = "a".repeat(320);
        let long_spaces = " ".repeat(320);
        let sample =
            format!("  hello, don't 1,234.56 foo_bar\n日本語🎉 test{long_spaces}{long_word}  tail");
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
    }

    #[test]
    fn grapheme_steps_agree_forward_and_backward() {
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
        let mut m = doc("hello world");
        m.set_selected_range(6..11);

        m.set_composition("に", 1);
        assert_eq!(m.text().to_string(), "hello に");
        assert!(!m.can_undo());
        assert!(!m.is_clean());

        m.set_composition("にほん", 3);
        assert_eq!(m.text().to_string(), "hello にほん");
        assert!(!m.can_undo());

        // Cancelling restores original replaced text without adding an undo entry.
        m.cancel_composition();
        assert_eq!(m.text().to_string(), "hello world");
        assert_eq!(m.selection().range(), 6..11);
        assert!(!m.can_undo());
        assert!(m.is_clean());

        // Committing pushes one atomic undo entry restoring the replaced text.
        m.set_composition("に", 1);
        m.insert_text("日本");
        assert_eq!(m.text().to_string(), "hello 日本");
        assert!(m.can_undo());
        m.undo();
        assert_eq!(m.text().to_string(), "hello world");
        assert_eq!(m.selection().range(), 6..11);
        assert!(m.is_clean());
    }

    #[test]
    fn single_line_fields_validate_and_refuse_line_breaks() {
        let mut field = TextModel::new(EditPolicy {
            multi_line: false,
            validate: Some(|s| s.bytes().all(|b| b.is_ascii_digit())),
        });
        assert!(field.insert_text("123").is_some());
        assert!(field.insert_text("abc").is_none());
        assert_eq!(field.text().to_string(), "123");
        assert!(field.enter().is_none());
        assert!(field.indent(false).is_none());
    }

    #[test]
    fn indent_and_outdent_keep_the_selection_on_its_text() {
        let mut doc = doc("alpha\nbeta\ngamma");
        doc.set_selected_range(2..9);
        doc.indent(true);
        assert_eq!(doc.text().to_string(), "\talpha\n\tbeta\ngamma");
        assert_eq!(doc.selection().range(), 3..11);
        doc.outdent();
        assert_eq!(doc.text().to_string(), "alpha\nbeta\ngamma");
        assert_eq!(doc.selection().range(), 2..9);
    }

    #[test]
    fn replace_all_remaps_cursor_and_round_trips_undo_redo() {
        let remap = |len: usize, offset| {
            let edits = [(0..3, "x".repeat(len)), (8..11, "x".repeat(len))];
            remap_offset(&edits, offset)
        };
        assert_eq!(remap(1, 0), 0);
        assert_eq!(remap(1, 2), 1);
        assert_eq!(remap(0, 2), 0);
        assert_eq!(remap(5, 2), 2);
        assert_eq!(remap(1, 3), 1);
        assert_eq!(remap(1, 5), 3);
        assert_eq!(remap(1, 10), 7);
        assert_eq!(remap(0, 10), 5);
        assert_eq!(remap(1, 11), 7);

        let mut doc = doc("aa bb cc aa dd aa");
        doc.place_caret(10, false);
        doc.replace_all(&[0..2, 9..11, 15..17], "XYZ");
        assert_eq!(doc.text().to_string(), "XYZ bb cc XYZ dd XYZ");
        assert_eq!(doc.cursor(), 11);

        doc.undo();
        assert_eq!(doc.text().to_string(), "aa bb cc aa dd aa");
        assert_eq!(doc.cursor(), 10);
        doc.redo();
        assert_eq!(doc.text().to_string(), "XYZ bb cc XYZ dd XYZ");
        assert_eq!(doc.cursor(), 11);
    }

    fn typed(h: &mut History, at: usize, text: &str, kind: EditKind) {
        let change = Change {
            at,
            old: String::new(),
            new: text.into(),
        };
        let selection = [Selection::caret(at), Selection::caret(at + text.len())];
        h.push(kind, Payload::Changes(vec![change]), selection);
    }

    /// Undoes the newest entry and returns its changes as `(at, old, new)`.
    fn undone(h: &mut History) -> Vec<(usize, String, String)> {
        let Payload::Changes(changes) = &h.step(true).unwrap().payload else {
            panic!("expected changes");
        };
        changes
            .iter()
            .map(|c| (c.at, c.old.clone(), c.new.clone()))
            .collect()
    }

    #[test]
    fn adjacent_typing_coalesces_and_splits_at_limit() {
        let mut h = History::default();
        typed(&mut h, 0, "a", EditKind::Insert);
        typed(&mut h, 1, "b", EditKind::Insert);
        assert_eq!(undone(&mut h), [(0, String::new(), "ab".into())]);
        assert!(h.step(true).is_none());

        let mut h = History::default();
        for i in 0..1_100 {
            typed(&mut h, i, "a", EditKind::Insert);
        }
        let run = |len| "a".repeat(len);
        assert_eq!(
            undone(&mut h),
            [(MAX_KEYSTROKES_PER_ENTRY, String::new(), run(100))]
        );
        assert_eq!(
            undone(&mut h),
            [(0, String::new(), run(MAX_KEYSTROKES_PER_ENTRY))]
        );
        assert!(h.step(true).is_none());
    }

    #[test]
    fn multi_edit_atomic_isolation_and_stack_cap() {
        let mut h = History::default();
        let changes = ["a", "b", "c"].iter().enumerate();
        let changes = changes.map(|(at, t)| Change {
            at,
            old: String::new(),
            new: t.to_string(),
        });
        let selection = [Selection::caret(0), Selection::caret(5)];
        h.push(
            EditKind::Atomic,
            Payload::Changes(changes.collect()),
            selection,
        );
        assert_eq!(undone(&mut h).len(), 3);

        typed(&mut h, 0, "x", EditKind::Atomic);
        typed(&mut h, 1, "y", EditKind::Insert);
        assert_eq!(undone(&mut h).len(), 1);
        assert_eq!(undone(&mut h).len(), 1);

        let mut capped = History::default();
        for i in 0..1_100 {
            typed(&mut capped, i, "a", EditKind::Atomic);
        }
        for _ in 0..MAX_HISTORY_ENTRIES {
            assert!(capped.step(true).is_some());
        }
        assert!(capped.step(true).is_none());
    }

    #[test]
    fn clean_state_tracks_undo_redo_and_save_boundaries() {
        let clean = |h: &History| h.saved == h.version;
        let mut h = History::default();
        assert!(clean(&h));

        typed(&mut h, 0, "a", EditKind::Insert);
        assert!(!clean(&h));
        assert!(h.step(true).is_some());
        assert!(clean(&h));
        assert!(!h.redo.is_empty());
        assert!(h.step(false).is_some());
        assert!(!clean(&h));

        h.coalesce_broken = true;
        h.saved = h.version;
        assert!(clean(&h));
        typed(&mut h, 1, "b", EditKind::Insert);
        assert!(!clean(&h));
        assert_eq!(undone(&mut h), [(1, String::new(), "b".into())]);
        assert!(clean(&h));

        h.coalesce_broken = true;
        let saving = h.version;
        typed(&mut h, 1, "c", EditKind::Insert);
        h.saved = saving;
        assert!(!clean(&h));
        assert!(h.step(true).is_some());
        assert!(clean(&h));

        assert!(h.step(true).is_some());
        typed(&mut h, 0, "z", EditKind::Insert);
        assert!(!clean(&h));
        assert!(h.step(true).is_some());
        assert!(!clean(&h));
    }
}
