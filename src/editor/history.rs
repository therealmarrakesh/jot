use std::ops::Range;

use ropey::Rope;

use super::buffer::Selection;

const MAX_HISTORY_ENTRIES: usize = 1000;
const MAX_CHANGES_PER_ENTRY: usize = 1000;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insert,
    Backspace,
    DeleteForward,
    Atomic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditRecord {
    pub(crate) old_range: Range<usize>,
    pub(crate) old_text: String,
    pub(crate) new_range: Range<usize>,
    pub(crate) new_text: String,
}

impl EditRecord {
    pub(crate) fn new(
        old_range: Range<usize>,
        old_text: String,
        new_range: Range<usize>,
        new_text: String,
    ) -> Self {
        Self {
            old_range,
            old_text,
            new_range,
            new_text,
        }
    }

    fn approx_bytes(&self) -> usize {
        self.old_text.len() + self.new_text.len() + std::mem::size_of::<Self>()
    }
}

#[derive(Debug)]
pub(crate) enum TxPayload {
    Edits(Vec<EditRecord>),
    Snapshot { before: Rope, after: Rope },
}

impl TxPayload {
    fn approx_bytes(&self) -> usize {
        match self {
            Self::Edits(edits) => edits.iter().map(EditRecord::approx_bytes).sum(),
            Self::Snapshot { .. } => 4096,
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
    Edits(Vec<EditRecord>),
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

        if let TxPayload::Edits(edits) = &payload {
            if edits.is_empty() {
                return;
            }
            if edits.len() == 1
                && edits[0].old_range == edits[0].new_range
                && edits[0].old_text == edits[0].new_text
            {
                self.coalesce_broken = true;
                return;
            }
        }

        self.clear_redo();

        let can_coalesce = !self.coalesce_broken
            && kind != EditKind::Atomic
            && matches!(&payload, TxPayload::Edits(e) if e.len() == 1)
            && self.undo_stack.last().is_some_and(|prev| {
                prev.kind == kind
                    && prev.merged_count < MAX_CHANGES_PER_ENTRY
                    && match (&prev.payload, &payload) {
                        (TxPayload::Edits(prev_edits), TxPayload::Edits(curr)) => prev_edits
                            .last()
                            .is_some_and(|last| edits_are_adjacent(kind, last, &curr[0])),
                        _ => false,
                    }
            });

        if can_coalesce {
            let TxPayload::Edits(mut edits) = payload else {
                unreachable!()
            };
            let edit = edits.pop().unwrap();
            let prev = self.undo_stack.last_mut().expect("previous entry exists");
            let old_bytes = prev.approx_bytes;
            let version = self.next_version;
            self.next_version += 1;
            prev.version_after = version;
            self.current_version = version;
            prev.merged_count += 1;
            prev.selection_after = selection_after;

            if let TxPayload::Edits(prev_edits) = &mut prev.payload {
                if kind == EditKind::Insert
                    && let Some(last) = prev_edits.last_mut()
                {
                    let cut = edit.old_range.start.saturating_sub(last.new_range.start);
                    if cut <= last.new_text.len() && last.new_text.is_char_boundary(cut) {
                        last.new_text.truncate(cut);
                        last.new_text.push_str(&edit.new_text);
                        last.new_range.end = edit.new_range.end;
                    } else {
                        prev_edits.push(edit);
                    }
                } else {
                    prev_edits.push(edit);
                }
            }
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
            TxPayload::Edits(edits) => edits.len(),
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
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.total_bytes = 0;
        self.coalesce_broken = false;
        self.next_version = 1;
        self.current_version = 0;
        self.saved_version = 0;
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

    pub(crate) fn undo(&mut self) -> Option<HistoryReplay> {
        let entry = self.undo_stack.pop()?;
        self.current_version = entry.version_before;
        let payload = match &entry.payload {
            TxPayload::Edits(edits) => ReplayPayload::Edits(edits.iter().rev().cloned().collect()),
            TxPayload::Snapshot { before, .. } => ReplayPayload::Snapshot(before.clone()),
        };
        let replay = HistoryReplay {
            payload,
            selection: entry.selection_before,
        };
        self.redo_stack.push(entry);
        self.coalesce_broken = true;
        Some(replay)
    }

    pub(crate) fn redo(&mut self) -> Option<HistoryReplay> {
        let entry = self.redo_stack.pop()?;
        self.current_version = entry.version_after;
        let payload = match &entry.payload {
            TxPayload::Edits(edits) => ReplayPayload::Edits(edits.clone()),
            TxPayload::Snapshot { after, .. } => ReplayPayload::Snapshot(after.clone()),
        };
        let replay = HistoryReplay {
            payload,
            selection: entry.selection_after,
        };
        self.undo_stack.push(entry);
        self.coalesce_broken = true;
        Some(replay)
    }
}

fn edits_are_adjacent(kind: EditKind, prev: &EditRecord, curr: &EditRecord) -> bool {
    match kind {
        EditKind::Insert => {
            prev.old_range.is_empty()
                && !prev.new_text.contains(['\n', '\r'])
                && !curr.new_text.contains(['\n', '\r'])
                && prev.new_range.start <= curr.old_range.start
                && prev.new_range.end == curr.old_range.end
        }
        EditKind::Backspace => {
            prev.new_text.is_empty()
                && curr.new_text.is_empty()
                && curr.old_range.end == prev.old_range.start
        }
        EditKind::DeleteForward => {
            prev.new_text.is_empty()
                && curr.new_text.is_empty()
                && curr.old_range.start == prev.old_range.start
        }
        EditKind::Atomic => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_tx(offset: usize, text: &str, kind: EditKind) -> Transaction {
        Transaction {
            kind,
            payload: TxPayload::Edits(vec![EditRecord::new(
                offset..offset,
                String::new(),
                offset..offset + text.len(),
                text.to_string(),
            )]),
            selection_before: Selection::caret(offset),
            selection_after: Selection::caret(offset + text.len()),
        }
    }

    fn undo_edits(h: &mut History) -> Vec<EditRecord> {
        match h.undo().unwrap().payload {
            ReplayPayload::Edits(e) => e,
            ReplayPayload::Snapshot(_) => panic!("expected edits"),
        }
    }

    #[test]
    fn adjacent_typing_coalesces_and_splits_at_limit() {
        let mut h = History::new();
        h.push(insert_tx(0, "a", EditKind::Insert));
        h.push(insert_tx(1, "b", EditKind::Insert));
        assert_eq!(
            undo_edits(&mut h),
            [EditRecord::new(0..0, String::new(), 0..2, "ab".into())]
        );
        assert!(h.undo().is_none());

        let mut h = History::new();
        for i in 0..1_100 {
            h.push(insert_tx(i, "a", EditKind::Insert));
        }
        assert_eq!(
            undo_edits(&mut h),
            [EditRecord::new(
                MAX_CHANGES_PER_ENTRY..MAX_CHANGES_PER_ENTRY,
                String::new(),
                MAX_CHANGES_PER_ENTRY..MAX_CHANGES_PER_ENTRY + 100,
                "a".repeat(100),
            )]
        );
        assert_eq!(
            undo_edits(&mut h),
            [EditRecord::new(
                0..0,
                String::new(),
                0..MAX_CHANGES_PER_ENTRY,
                "a".repeat(MAX_CHANGES_PER_ENTRY),
            )]
        );
        assert!(h.undo().is_none());
    }

    #[test]
    fn multi_edit_atomic_isolation_and_stack_cap() {
        let mut h = History::new();
        h.push(Transaction {
            kind: EditKind::Atomic,
            payload: TxPayload::Edits(vec![
                EditRecord::new(0..0, String::new(), 0..1, "a".into()),
                EditRecord::new(1..1, String::new(), 1..2, "b".into()),
                EditRecord::new(2..2, String::new(), 2..3, "c".into()),
            ]),
            selection_before: Selection::caret(0),
            selection_after: Selection::caret(3),
        });
        let edits = undo_edits(&mut h);
        assert_eq!(edits.len(), 3);
        assert_eq!(edits[0].new_text, "c");
        assert_eq!(edits[2].new_text, "a");

        h.push(insert_tx(0, "x", EditKind::Atomic));
        h.push(insert_tx(1, "y", EditKind::Insert));
        assert_eq!(undo_edits(&mut h).len(), 1);
        assert_eq!(undo_edits(&mut h).len(), 1);

        let mut capped = History::new();
        for i in 0..1_100 {
            capped.push(insert_tx(i, "a", EditKind::Atomic));
        }
        for _ in 0..MAX_HISTORY_ENTRIES {
            assert!(capped.undo().is_some());
        }
        assert!(capped.undo().is_none());
    }

    #[test]
    fn clean_state_tracks_undo_redo_and_save_boundaries() {
        let mut h = History::new();
        assert!(h.is_clean());

        h.push(insert_tx(0, "a", EditKind::Insert));
        assert!(!h.is_clean());
        assert!(h.undo().is_some());
        assert!(h.is_clean());
        assert!(h.can_redo());
        assert!(h.redo().is_some());
        assert!(!h.is_clean());

        let saved = h.begin_save();
        h.mark_saved(saved);
        assert!(h.is_clean());
        h.push(insert_tx(1, "b", EditKind::Insert));
        assert!(!h.is_clean());
        assert_eq!(
            undo_edits(&mut h),
            [EditRecord::new(1..1, String::new(), 1..2, "b".into())]
        );
        assert!(h.is_clean());

        let snapshot = h.begin_save();
        h.push(insert_tx(1, "c", EditKind::Insert));
        h.mark_saved(snapshot);
        assert!(!h.is_clean());
        assert!(h.undo().is_some());
        assert!(h.is_clean());

        assert!(h.undo().is_some());
        h.push(insert_tx(0, "z", EditKind::Insert));
        assert!(!h.is_clean());
        assert!(h.undo().is_some());
        assert!(!h.is_clean());
    }
}
