//! Coalescing undo/redo history with nested transactions and save-point tracking.

use super::buffer::{CaretSelection, Selection};

const MAX_HISTORY_ENTRIES: usize = 1000;
const MAX_CHANGES_PER_ENTRY: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insert,
    Backspace,
    DeleteForward,
    Atomic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditRecord {
    pub(crate) old_range: Selection,
    pub(crate) old_text: String,
    pub(crate) new_range: Selection,
    pub(crate) new_text: String,
}

impl EditRecord {
    pub(crate) fn new(
        old_range: impl Into<Selection>,
        old_text: &str,
        new_range: impl Into<Selection>,
        new_text: &str,
    ) -> Self {
        Self {
            old_range: old_range.into(),
            old_text: old_text.to_string(),
            new_range: new_range.into(),
            new_text: new_text.to_string(),
        }
    }
}

#[derive(Debug)]
struct HistoryEntry {
    kind: EditKind,
    edits: Vec<EditRecord>,
    merged_count: usize,
    selection_before: Option<CaretSelection>,
    selection_after: Option<CaretSelection>,
    version_before: u64,
    version_after: u64,
}

#[derive(Debug)]
struct OpenGroup {
    kind: EditKind,
    edits: Vec<EditRecord>,
    selection_before: Option<CaretSelection>,
    selection_after: Option<CaretSelection>,
}

pub(crate) struct HistoryReplay {
    pub(crate) edits: Vec<EditRecord>,
    pub(crate) selection: Option<CaretSelection>,
}

#[derive(Debug)]
pub(crate) struct History {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    ignoring: bool,
    group_depth: usize,
    open_group: Option<OpenGroup>,
    next_kind: Option<EditKind>,
    coalesce_broken: bool,
    next_version: u64,
    current_version: u64,
    saved_version: Option<u64>,
}

impl History {
    pub(crate) fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            ignoring: false,
            group_depth: 0,
            open_group: None,
            next_kind: None,
            coalesce_broken: false,
            next_version: 1,
            current_version: 0,
            saved_version: Some(0),
        }
    }

    pub(crate) fn set_next_kind(&mut self, kind: EditKind) {
        self.next_kind = Some(kind);
    }

    pub(crate) fn take_next_kind(&mut self) -> Option<EditKind> {
        self.next_kind.take()
    }

    pub(crate) fn is_ignoring(&self) -> bool {
        self.ignoring
    }

    pub(crate) fn set_ignoring(&mut self, ignoring: bool) {
        self.ignoring = ignoring;
        if ignoring {
            self.flush_open_groups();
        }
    }

    pub(crate) fn has_open_group(&self) -> bool {
        self.group_depth > 0
    }

    pub(crate) fn begin_group(&mut self) {
        self.begin_group_with(EditKind::Atomic);
    }

    pub(crate) fn begin_group_with(&mut self, kind: EditKind) {
        self.group_depth += 1;
        if self.group_depth == 1 {
            self.open_group = Some(OpenGroup {
                kind,
                edits: Vec::new(),
                selection_before: None,
                selection_after: None,
            });
        }
    }

    pub(crate) fn end_group(&mut self) {
        if self.group_depth == 0 {
            return;
        }
        self.group_depth -= 1;
        if self.group_depth > 0 {
            return;
        }
        let Some(group) = self.open_group.take() else {
            return;
        };
        if group.edits.is_empty() || is_net_noop(&group.edits) {
            return;
        }
        self.commit_edits(group.edits, group.kind);
        if let Some(before) = group.selection_before {
            self.set_selection_before(before);
        }
        if let Some(after) = group.selection_after {
            self.set_selection_after(after);
        }
    }

    fn flush_open_groups(&mut self) {
        if self.group_depth == 0 {
            return;
        }
        self.group_depth = 1;
        self.end_group();
    }

    pub(crate) fn break_coalescing(&mut self) {
        if self.group_depth == 0 {
            self.flush_open_groups();
        }
        self.coalesce_broken = true;
    }

    pub(crate) fn record_edit(&mut self, edit: EditRecord, kind: EditKind) -> bool {
        if self.ignoring {
            return false;
        }
        if edit.old_range == edit.new_range && edit.old_text == edit.new_text {
            self.break_coalescing();
            return false;
        }
        if let Some(group) = self.open_group.as_mut() {
            group.edits.push(edit);
        } else {
            self.commit_edits(vec![edit], kind);
        }
        true
    }

    fn commit_edits(&mut self, edits: Vec<EditRecord>, kind: EditKind) {
        if edits.is_empty() {
            return;
        }
        self.redo_stack.clear();

        let can_coalesce = !self.coalesce_broken
            && kind != EditKind::Atomic
            && edits.len() == 1
            && self.undo_stack.last().is_some_and(|prev| {
                prev.kind == kind
                    && prev.merged_count < MAX_CHANGES_PER_ENTRY
                    && prev
                        .edits
                        .last()
                        .is_some_and(|last| edits_are_adjacent(kind, last, &edits[0]))
            });

        if can_coalesce {
            let prev = self.undo_stack.last_mut().expect("previous entry exists");
            let version = self.next_version;
            self.next_version += 1;
            prev.version_after = version;
            self.current_version = version;
            prev.merged_count += 1;

            if kind == EditKind::Insert
                && let [edit] = edits.as_slice()
                && let Some(last) = prev.edits.last_mut()
            {
                last.new_text
                    .truncate(edit.old_range.start - last.new_range.start);
                last.new_text.push_str(&edit.new_text);
                last.new_range.end = edit.new_range.end;
                return;
            }
            prev.edits.extend(edits);
            return;
        }

        if self.undo_stack.len() >= MAX_HISTORY_ENTRIES {
            self.undo_stack.remove(0);
        }
        let version_before = self.current_version;
        let version_after = self.next_version;
        self.next_version += 1;
        self.current_version = version_after;
        self.undo_stack.push(HistoryEntry {
            kind,
            merged_count: edits.len(),
            edits,
            selection_before: None,
            selection_after: None,
            version_before,
            version_after,
        });
        self.coalesce_broken = kind == EditKind::Atomic;
    }

    pub(crate) fn record_selection(&mut self, before: CaretSelection, after: CaretSelection) {
        if self.ignoring {
            return;
        }
        self.set_selection_before(before);
        self.set_selection_after(after);
    }

    fn set_selection_before(&mut self, before: CaretSelection) {
        if let Some(group) = self.open_group.as_mut() {
            group.selection_before.get_or_insert(before);
        } else if let Some(entry) = self.undo_stack.last_mut() {
            entry.selection_before.get_or_insert(before);
        }
    }

    fn set_selection_after(&mut self, after: CaretSelection) {
        if let Some(group) = self.open_group.as_mut() {
            group.selection_after = Some(after);
        } else if let Some(entry) = self.undo_stack.last_mut() {
            entry.selection_after = Some(after);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.group_depth = 0;
        self.open_group = None;
        self.next_kind = None;
        self.coalesce_broken = false;
        self.next_version = 1;
        self.current_version = 0;
        self.saved_version = Some(0);
    }

    pub(crate) fn begin_save(&mut self) -> u64 {
        self.flush_open_groups();
        self.coalesce_broken = true;
        self.current_version
    }

    pub(crate) fn mark_saved(&mut self, version: u64) {
        self.saved_version = Some(version);
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.saved_version == Some(self.current_version)
            && self
                .open_group
                .as_ref()
                .is_none_or(|g| g.edits.is_empty())
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub(crate) fn undo(&mut self) -> Option<HistoryReplay> {
        self.flush_open_groups();
        let entry = self.undo_stack.pop()?;
        self.current_version = entry.version_before;
        let replay = HistoryReplay {
            edits: entry.edits.iter().rev().cloned().collect(),
            selection: entry.selection_before,
        };
        self.redo_stack.push(entry);
        self.coalesce_broken = true;
        Some(replay)
    }

    pub(crate) fn redo(&mut self) -> Option<HistoryReplay> {
        self.flush_open_groups();
        let entry = self.redo_stack.pop()?;
        self.current_version = entry.version_after;
        let replay = HistoryReplay {
            edits: entry.edits.clone(),
            selection: entry.selection_after,
        };
        self.undo_stack.push(entry);
        self.coalesce_broken = true;
        Some(replay)
    }
}

fn is_net_noop(edits: &[EditRecord]) -> bool {
    let Some(first) = edits.first() else {
        return true;
    };
    let mut prev = first;
    for next in &edits[1..] {
        if prev.new_range != next.old_range {
            return false;
        }
        prev = next;
    }
    first.old_range == prev.new_range && first.old_text == prev.new_text
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

    fn insert_edit(offset: usize, text: &str) -> EditRecord {
        EditRecord::new(offset..offset, "", offset..offset + text.len(), text)
    }

    #[test]
    fn adjacent_typing_coalesces_and_splits_at_limit() {
        let mut h = History::new();
        h.record_edit(insert_edit(0, "a"), EditKind::Insert);
        h.record_edit(insert_edit(1, "b"), EditKind::Insert);
        assert_eq!(h.undo().unwrap().edits, [insert_edit(0, "ab")]);
        assert!(h.undo().is_none());

        let mut h = History::new();
        for i in 0..1_100 {
            h.record_edit(insert_edit(i, "a"), EditKind::Insert);
        }
        assert_eq!(
            h.undo().unwrap().edits,
            [insert_edit(MAX_CHANGES_PER_ENTRY, &"a".repeat(100))]
        );
        assert_eq!(
            h.undo().unwrap().edits,
            [insert_edit(0, &"a".repeat(MAX_CHANGES_PER_ENTRY))]
        );
        assert!(h.undo().is_none());
    }

    #[test]
    fn groups_nesting_atomic_isolation_and_stack_cap() {
        let mut h = History::new();
        h.begin_group();
        h.record_edit(insert_edit(0, "a"), EditKind::Insert);
        h.begin_group();
        h.record_edit(insert_edit(1, "b"), EditKind::Insert);
        h.end_group();
        assert!(!h.can_undo());
        h.record_edit(insert_edit(2, "c"), EditKind::Insert);
        h.end_group();

        let edits = h.undo().unwrap().edits;
        assert_eq!(edits.len(), 3);
        assert_eq!(edits[0].new_text, "c");
        assert_eq!(edits[2].new_text, "a");

        h.begin_group();
        h.record_edit(insert_edit(0, "x"), EditKind::Insert);
        h.end_group();
        h.record_edit(insert_edit(1, "y"), EditKind::Insert);
        assert_eq!(h.undo().unwrap().edits.len(), 1);
        assert_eq!(h.undo().unwrap().edits.len(), 1);

        let mut capped = History::new();
        for i in 0..1_100 {
            capped.record_edit(insert_edit(i, "a"), EditKind::Atomic);
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

        h.record_edit(insert_edit(0, "a"), EditKind::Insert);
        assert!(!h.is_clean());
        assert!(h.undo().is_some());
        assert!(h.is_clean());
        assert!(h.redo().is_some());
        assert!(!h.is_clean());

        let saved = h.begin_save();
        h.mark_saved(saved);
        assert!(h.is_clean());
        h.record_edit(insert_edit(1, "b"), EditKind::Insert);
        assert!(!h.is_clean());
        assert_eq!(h.undo().unwrap().edits, [insert_edit(1, "b")]);
        assert!(h.is_clean());

        let snapshot = h.begin_save();
        h.record_edit(insert_edit(1, "c"), EditKind::Insert);
        h.mark_saved(snapshot);
        assert!(!h.is_clean());
        assert!(h.undo().is_some());
        assert!(h.is_clean());

        assert!(h.undo().is_some());
        h.record_edit(insert_edit(0, "z"), EditKind::Insert);
        assert!(!h.is_clean());
        assert!(h.undo().is_some());
        assert!(!h.is_clean());
    }
}
