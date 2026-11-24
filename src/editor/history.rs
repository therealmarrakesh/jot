use std::ops::Range;

use ropey::Rope;

use super::buffer::Selection;

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
    pub(super) changes: Vec<Change>,
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

pub(super) fn remap_offset(
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
            TxPayload::Changes { undo, .. } => ReplayPayload::Changes(undo.clone()),
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
            TxPayload::Changes { redo, .. } => ReplayPayload::Changes(redo.clone()),
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
        match h.undo().unwrap().payload {
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
        assert!(h.undo().is_none());

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
        assert!(h.undo().is_none());
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
        assert_eq!(undo_changes(&mut h), ChangeSet::single(1..2, ""));
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
