use std::borrow::Cow;
use std::ops::Range;

use ropey::{Rope, RopeBuilder, RopeSlice};

use super::buffer::{
    Bias, RopeExt as _, Selection, line_range_at, next_grapheme_boundary, next_word_end,
    prev_grapheme_boundary, prev_word_start, snap_grapheme_boundary, word_range_at,
};
use super::history::{
    Change, ChangeSet, EditKind, History, ReplayPayload, Transaction, TxPayload,
};

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

    pub(crate) fn with_validate(mut self, f: ValidateFn) -> Self {
        self.policy.validate = Some(f);
        self
    }

    #[inline]
    pub(crate) fn is_multi_line(&self) -> bool {
        self.policy.multi_line
    }

    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    #[inline]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    #[inline]
    pub(crate) fn selection(&self) -> Selection {
        self.selection
    }

    #[inline]
    pub(crate) fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }

    #[inline]
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
                        goal: None,
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
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, normalized)),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Undoable(kind),
            validate: true,
        })
    }

    pub(crate) fn replace_selection(&mut self, text: &str) -> Option<LineChange> {
        self.commit_composition();
        let range = self.selected_range();
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, text)),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Undoable(EditKind::Atomic),
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
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, "")),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Undoable(kind),
            validate: true,
        })
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
        let range = self.selected_range();
        self.commit(Edit {
            source: EditSource::Changes(ChangeSet::single(range, "\n")),
            selection: SelectionAfter::AfterLastEdit,
            record: Record::Undoable(EditKind::Atomic),
            validate: true,
        })
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
            let range = before.range();
            return Some(self.commit(Edit {
                source: EditSource::Changes(ChangeSet::single(range, "\t")),
                selection: SelectionAfter::AfterLastEdit,
                record: Record::Undoable(EditKind::Atomic),
                validate: true,
            }));
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
        let replay = if is_undo {
            self.history.undo()
        } else {
            self.history.redo()
        }?;
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

    pub(crate) fn place_caret(
        &mut self,
        offset: usize,
        end_affinity: bool,
        goal: Option<(f32, usize)>,
    ) {
        self.commit_composition();
        let offset = self.snap_boundary(offset, Bias::Left);
        self.selection.place(offset, end_affinity, goal);
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

#[cfg(test)]
mod tests {
    use super::super::history::remap_offset;
    use super::*;

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
        doc.place_caret(10, false, None);
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
}
