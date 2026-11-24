use std::borrow::Cow;
use std::ops::Range;

use ropey::{Rope, RopeBuilder, RopeSlice};

use super::buffer::{
    Bias, RopeExt as _, Selection, line_range_at, next_grapheme_boundary, next_word_end,
    prev_grapheme_boundary, prev_word_start, snap_grapheme_boundary, word_range_at,
};
use super::history::{EditKind, EditRecord, History, ReplayPayload, Transaction, TxPayload};

pub(crate) type ValidateFn = fn(&str) -> bool;

#[derive(Clone, Copy, Debug)]
pub(crate) struct EditPolicy {
    pub(crate) multi_line: bool,
    pub(crate) validate: Option<ValidateFn>,
}

#[derive(Clone, Debug)]
pub(crate) struct Change {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Default)]
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

    pub(crate) fn from_sorted(changes: Vec<Change>) -> Self {
        Self { changes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty()
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineChange {
    #[default]
    None,
    Splice {
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    },
    Full,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SelectionAfter {
    AfterLastEdit,
    Remap,
    Explicit(Selection),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct EditEffects {
    pub(crate) lines: LineChange,
    pub(crate) changed: bool,
    pub(crate) caret_moved: bool,
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
            return comp.start.saturating_add(comp.cursor).clamp(comp.start, comp.end);
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

    fn infer_kind(range: &Range<usize>, inserted: &str) -> EditKind {
        if range.is_empty() && !inserted.is_empty() && !inserted.contains(['\n', '\r']) {
            EditKind::Insert
        } else {
            EditKind::Atomic
        }
    }

    pub(crate) fn apply(
        &mut self,
        changes: ChangeSet,
        kind: EditKind,
        sel_after: SelectionAfter,
        record_history: bool,
    ) -> EditEffects {
        if changes.is_empty() {
            return EditEffects::default();
        }

        let sel_before = self.selection;
        let normalized: Vec<Change> = changes
            .changes
            .into_iter()
            .map(|c| {
                let start = self.text.clip_offset(c.range.start, Bias::Left);
                let end = self.text.clip_offset(c.range.end, Bias::Right);
                let text = self.normalize(&c.text).into_owned();
                Change {
                    range: start.min(end)..start.max(end),
                    text,
                }
            })
            .collect();

        let changes = ChangeSet::from_sorted(normalized);
        let first_start = changes.changes.first().unwrap().range.start;
        let last_end = changes.changes.last().unwrap().range.end;
        let old_start_line = self.text.offset_to_point(first_start).row;
        let old_end_line = self.text.offset_to_point(last_end).row;

        let before_rope = self.text.clone();
        let mut records = Vec::with_capacity(changes.changes.len());
        for change in changes.changes.iter().rev() {
            let old_text = self.text.slice(change.range.clone()).to_string();
            let new_range = change.range.start..change.range.start + change.text.len();
            self.text.replace(change.range.clone(), &change.text);
            if record_history {
                records.push(EditRecord::new(
                    change.range.clone(),
                    old_text,
                    new_range,
                    change.text.clone(),
                ));
            }
        }

        if record_history
            && !self.policy.multi_line
            && self.policy.validate.is_some()
            && !self.accepts(&self.text.to_string())
            && self.accepts(&before_rope.to_string())
        {
            self.text = before_rope;
            return EditEffects::default();
        }

        let mut shift: isize = 0;
        let mut last_new_end = first_start;
        for change in &changes.changes {
            last_new_end = (change.range.start as isize + shift) as usize + change.text.len();
            shift += change.text.len() as isize - (change.range.end - change.range.start) as isize;
        }
        let new_start_line = self.text.offset_to_point(first_start).row;
        let new_end_line = self.text.offset_to_point(last_new_end).row;

        self.revision = self.revision.wrapping_add(1);
        self.composition = None;
        self.selection = match sel_after {
            SelectionAfter::AfterLastEdit => {
                Selection::caret(self.snap_boundary(last_new_end, Bias::Left))
            }
            SelectionAfter::Remap => {
                let anchor = self.snap_boundary(changes.remap_offset(sel_before.anchor), Bias::Left);
                let head = self.snap_boundary(changes.remap_offset(sel_before.head), Bias::Left);
                Selection {
                    anchor,
                    head,
                    end_affinity: false,
                    goal: None,
                }
            }
            SelectionAfter::Explicit(mut sel) => {
                sel.anchor = self.snap_boundary(sel.anchor, Bias::Left);
                sel.head = self.snap_boundary(sel.head, Bias::Left);
                sel
            }
        };

        if record_history {
            self.history.push(Transaction {
                kind,
                payload: TxPayload::Edits(records),
                selection_before: sel_before,
                selection_after: self.selection,
            });
        }
        self.debug_assert_invariants();

        EditEffects {
            lines: LineChange::Splice {
                old_lines: old_start_line..old_end_line + 1,
                new_lines: new_start_line..new_end_line + 1,
            },
            changed: true,
            caret_moved: true,
        }
    }

    pub(crate) fn replace_all(
        &mut self,
        ranges: &[Range<usize>],
        replacement: &str,
    ) -> EditEffects {
        self.commit_composition();
        if ranges.is_empty() {
            return EditEffects::default();
        }
        let replacement = self.normalize(replacement);
        let before = self.text.clone();
        let sel_before = self.selection;
        let cursor = self.cursor();
        let next_cursor = remap_offset(
            ranges.iter().map(|r| (r.clone(), replacement.len())),
            cursor,
        );

        let rebuilt = rebuild_replaced_rope(&self.text, ranges, &replacement);
        self.text = rebuilt;
        self.revision = self.revision.wrapping_add(1);
        let target = self.snap_boundary(next_cursor, Bias::Left);
        self.selection = Selection::caret(target);
        self.history.push(Transaction {
            kind: EditKind::Atomic,
            payload: TxPayload::Snapshot {
                before,
                after: self.text.clone(),
            },
            selection_before: sel_before,
            selection_after: self.selection,
        });
        self.debug_assert_invariants();
        EditEffects {
            lines: LineChange::Full,
            changed: true,
            caret_moved: true,
        }
    }

    pub(crate) fn set_value(&mut self, value: &str) -> EditEffects {
        self.composition = None;
        let full = 0..self.text.len();
        let normalized = self.normalize(value).into_owned();
        let target_offset = if self.policy.multi_line {
            0
        } else {
            normalized.len()
        };
        let mut fx = self.apply(
            ChangeSet::single(full, normalized),
            EditKind::Atomic,
            SelectionAfter::Explicit(Selection::caret(target_offset)),
            false,
        );
        fx.lines = LineChange::Full;
        self.history.clear();
        fx
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> EditEffects {
        if let Some(comp) = self.composition.take() {
            let normalized = self.normalize(text).into_owned();
            let comp_range = comp.start..comp.end;
            let old_start_line = self.text.offset_to_point(comp.start).row;
            let old_end_line = self.text.offset_to_point(comp.end).row;
            let before_rope = self.text.clone();
            self.text.replace(comp_range, &normalized);

            if !self.policy.multi_line
                && self.policy.validate.is_some()
                && !self.accepts(&self.text.to_string())
                && self.accepts(&before_rope.to_string())
            {
                self.text = before_rope;
                self.composition = Some(comp);
                return EditEffects::default();
            }

            let new_end = comp.start + normalized.len();
            let new_start_line = self.text.offset_to_point(comp.start).row;
            let new_end_line = self.text.offset_to_point(new_end).row;
            self.revision = self.revision.wrapping_add(1);
            self.selection = Selection::caret(self.snap_boundary(new_end, Bias::Left));

            let orig_end = comp.start + comp.replaced_text.len();
            self.history.push(Transaction {
                kind: EditKind::Atomic,
                payload: TxPayload::Edits(vec![EditRecord::new(
                    comp.start..orig_end,
                    comp.replaced_text,
                    comp.start..new_end,
                    normalized,
                )]),
                selection_before: comp.sel_before,
                selection_after: self.selection,
            });
            self.debug_assert_invariants();
            return EditEffects {
                lines: LineChange::Splice {
                    old_lines: old_start_line..old_end_line + 1,
                    new_lines: new_start_line..new_end_line + 1,
                },
                changed: true,
                caret_moved: true,
            };
        }

        let range = self.selected_range();
        let normalized = self.normalize(text).into_owned();
        let kind = Self::infer_kind(&range, &normalized);
        self.apply(
            ChangeSet::single(range, normalized),
            kind,
            SelectionAfter::AfterLastEdit,
            true,
        )
    }

    pub(crate) fn replace_selection(&mut self, text: &str) -> EditEffects {
        self.commit_composition();
        let range = self.selected_range();
        self.apply(
            ChangeSet::single(range, text),
            EditKind::Atomic,
            SelectionAfter::AfterLastEdit,
            true,
        )
    }

    pub(crate) fn set_composition(&mut self, text: &str, cursor_utf16: usize) -> EditEffects {
        if text.is_empty() {
            return self.cancel_composition().unwrap_or_default();
        }

        let normalized = self.normalize(text).into_owned();
        if normalized.is_empty() {
            return self.cancel_composition().unwrap_or_default();
        }
        let cursor = utf16_offset_to_byte(&normalized, cursor_utf16);

        let (range, replaced_text, sel_before) = match &self.composition {
            Some(comp) => (comp.start..comp.end, comp.replaced_text.clone(), comp.sel_before),
            None => {
                let r = self.selected_range();
                let old = self.text.slice(r.clone()).to_string();
                (r, old, self.selection)
            }
        };

        let old_start_line = self.text.offset_to_point(range.start).row;
        let old_end_line = self.text.offset_to_point(range.end).row;
        let before_rope = self.text.clone();
        self.text.replace(range.clone(), &normalized);

        if !self.policy.multi_line
            && self.policy.validate.is_some()
            && !self.accepts(&self.text.to_string())
            && self.accepts(&before_rope.to_string())
        {
            self.text = before_rope;
            return EditEffects::default();
        }

        let new_end = range.start + normalized.len();
        let new_start_line = self.text.offset_to_point(range.start).row;
        let new_end_line = self.text.offset_to_point(new_end).row;
        self.revision = self.revision.wrapping_add(1);
        let caret = self.snap_boundary(range.start + cursor, Bias::Left);
        self.composition = Some(Composition {
            start: range.start,
            end: new_end,
            cursor: caret.saturating_sub(range.start),
            replaced_text,
            sel_before,
        });
        self.selection = Selection::caret(caret);
        self.debug_assert_invariants();

        EditEffects {
            lines: LineChange::Splice {
                old_lines: old_start_line..old_end_line + 1,
                new_lines: new_start_line..new_end_line + 1,
            },
            changed: true,
            caret_moved: true,
        }
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
                payload: TxPayload::Edits(vec![EditRecord::new(
                    comp.start..orig_end,
                    comp.replaced_text,
                    comp.start..comp.end,
                    committed,
                )]),
                selection_before: comp.sel_before,
                selection_after: self.selection,
            });
        }
        self.debug_assert_invariants();
        true
    }

    pub(crate) fn cancel_composition(&mut self) -> Option<EditEffects> {
        let comp = self.composition.take()?;
        let old_start_line = self.text.offset_to_point(comp.start).row;
        let old_end_line = self.text.offset_to_point(comp.end).row;
        self.text.replace(comp.start..comp.end, &comp.replaced_text);
        let restored_end = comp.start + comp.replaced_text.len();
        let new_start_line = self.text.offset_to_point(comp.start).row;
        let new_end_line = self.text.offset_to_point(restored_end).row;
        self.revision = self.revision.wrapping_add(1);
        let mut sel = comp.sel_before;
        sel.anchor = self.snap_boundary(sel.anchor, Bias::Left);
        sel.head = self.snap_boundary(sel.head, Bias::Left);
        self.selection = sel;
        self.debug_assert_invariants();
        Some(EditEffects {
            lines: LineChange::Splice {
                old_lines: old_start_line..old_end_line + 1,
                new_lines: new_start_line..new_end_line + 1,
            },
            changed: true,
            caret_moved: true,
        })
    }

    fn delete_with(
        &mut self,
        collapsed_kind: EditKind,
        collapsed_span: impl FnOnce(&Rope, usize) -> Range<usize>,
    ) -> EditEffects {
        self.commit_composition();
        let (range, kind) = if self.selection.is_empty() {
            let span = collapsed_span(&self.text, self.selection.head);
            if span.start == span.end {
                return EditEffects::default();
            }
            (span.start.min(span.end)..span.start.max(span.end), collapsed_kind)
        } else {
            (self.selection.range(), EditKind::Atomic)
        };
        self.apply(
            ChangeSet::single(range, ""),
            kind,
            SelectionAfter::AfterLastEdit,
            true,
        )
    }

    pub(crate) fn delete_backward(&mut self) -> EditEffects {
        self.delete_with(EditKind::Backspace, |t, off| {
            prev_grapheme_boundary(t, off)..off
        })
    }

    pub(crate) fn delete_forward(&mut self) -> EditEffects {
        self.delete_with(EditKind::DeleteForward, |t, off| {
            off..next_grapheme_boundary(t, off)
        })
    }

    pub(crate) fn delete_prev_word(&mut self) -> EditEffects {
        self.delete_with(EditKind::Atomic, |t, off| prev_word_start(t, off)..off)
    }

    pub(crate) fn delete_next_word(&mut self) -> EditEffects {
        self.delete_with(EditKind::Atomic, |t, off| off..next_word_end(t, off))
    }

    pub(crate) fn enter(&mut self) -> Option<EditEffects> {
        self.commit_composition();
        if !self.policy.multi_line {
            self.history.break_coalescing();
            return None;
        }
        let range = self.selected_range();
        Some(self.apply(
            ChangeSet::single(range, "\n"),
            EditKind::Atomic,
            SelectionAfter::AfterLastEdit,
            true,
        ))
    }

    pub(crate) fn indent(&mut self, block: bool) -> Option<EditEffects> {
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
            return Some(self.apply(
                ChangeSet::single(range, "\t"),
                EditKind::Atomic,
                SelectionAfter::AfterLastEdit,
                true,
            ));
        }
        let (start_row, end_row) = self.selected_line_bounds(before);
        let changes: Vec<Change> = (start_row..=end_row)
            .map(|row| {
                let start = self.text.line_start_offset(row);
                Change {
                    range: start..start,
                    text: "\t".into(),
                }
            })
            .collect();
        Some(self.apply(
            ChangeSet::from_sorted(changes),
            EditKind::Atomic,
            SelectionAfter::Remap,
            true,
        ))
    }

    pub(crate) fn outdent(&mut self) -> Option<EditEffects> {
        if !self.policy.multi_line {
            return None;
        }
        self.commit_composition();
        let (start_row, end_row) = self.selected_line_bounds(self.selection);
        let mut changes = Vec::new();
        for row in start_row..=end_row {
            let start = self.text.line_start_offset(row);
            let remove = self.leading_outdent_bytes(start);
            if remove > 0 {
                changes.push(Change {
                    range: start..start + remove,
                    text: String::new(),
                });
            }
        }
        if changes.is_empty() {
            return Some(EditEffects::default());
        }
        Some(self.apply(
            ChangeSet::from_sorted(changes),
            EditKind::Atomic,
            SelectionAfter::Remap,
            true,
        ))
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
        let mut spaces = 0usize;
        for ch in self.text.slice(line_start..).chars().take(8) {
            if ch == ' ' {
                spaces += 1;
            } else {
                break;
            }
        }
        spaces
    }

    pub(crate) fn undo(&mut self) -> Option<EditEffects> {
        self.replay(true)
    }

    pub(crate) fn redo(&mut self) -> Option<EditEffects> {
        self.replay(false)
    }

    fn replay(&mut self, is_undo: bool) -> Option<EditEffects> {
        self.commit_composition();
        let replay = if is_undo {
            self.history.undo()
        } else {
            self.history.redo()
        }?;

        let lines = match replay.payload {
            ReplayPayload::Edits(edits) => {
                if edits.len() == 1 {
                    let edit = &edits[0];
                    let (range, text) = if is_undo {
                        (edit.new_range.clone(), edit.old_text.as_str())
                    } else {
                        (edit.old_range.clone(), edit.new_text.as_str())
                    };
                    let old_start = self.text.offset_to_point(range.start).row;
                    let old_end = self.text.offset_to_point(range.end).row;
                    self.text.replace(range.clone(), text);
                    let new_start = self.text.offset_to_point(range.start).row;
                    let new_end = self.text.offset_to_point(range.start + text.len()).row;
                    LineChange::Splice {
                        old_lines: old_start..old_end + 1,
                        new_lines: new_start..new_end + 1,
                    }
                } else {
                    for edit in &edits {
                        let (range, text) = if is_undo {
                            (edit.new_range.clone(), edit.old_text.as_str())
                        } else {
                            (edit.old_range.clone(), edit.new_text.as_str())
                        };
                        self.text.replace(range, text);
                    }
                    LineChange::Full
                }
            }
            ReplayPayload::Snapshot(rope) => {
                self.text = rope;
                LineChange::Full
            }
        };

        self.revision = self.revision.wrapping_add(1);
        let mut sel = replay.selection;
        sel.anchor = self.snap_boundary(sel.anchor, Bias::Left);
        sel.head = self.snap_boundary(sel.head, Bias::Left);
        self.selection = sel;
        self.debug_assert_invariants();

        Some(EditEffects {
            lines,
            changed: true,
            caret_moved: true,
        })
    }

    pub(crate) fn select_all(&mut self) {
        self.commit_composition();
        self.history.break_coalescing();
        self.selection = Selection::from_range(0..self.text.len());
        self.debug_assert_invariants();
    }

    pub(crate) fn set_selected_range(&mut self, range: Range<usize>) {
        self.commit_composition();
        self.history.break_coalescing();
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
        self.history.break_coalescing();
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
        self.history.break_coalescing();
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
        self.history.break_coalescing();
        self.selection = Selection::from_range(range.clone());
        self.debug_assert_invariants();
        Some(range)
    }

    pub(crate) fn select_line_at(&mut self, offset: usize) {
        let range = line_range_at(&self.text, offset);
        self.history.break_coalescing();
        self.selection = Selection::from_range(range);
        self.debug_assert_invariants();
    }
}

fn rebuild_replaced_rope(text: &Rope, ranges: &[Range<usize>], replacement: &str) -> Rope {
    let mut builder = RopeBuilder::new();
    let mut cursor = 0usize;
    for range in ranges {
        if range.start > cursor {
            for chunk in text.slice(cursor..range.start).chunks() {
                builder.append(chunk);
            }
        }
        if !replacement.is_empty() {
            builder.append(replacement);
        }
        cursor = range.end;
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
        assert!(field.insert_text("123").changed);
        assert!(!field.insert_text("abc").changed);
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
