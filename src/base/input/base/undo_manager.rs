use crate::base::input::change::Change;

use super::cursor::CursorSelection;

const MAX_UNDO_TRANSACTIONS: usize = 1000;
const MAX_CHANGES_PER_TRANSACTION: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditIntent {
    Typing,
    Backspace,
    DeleteForward,
    Atomic,
}

#[derive(Debug)]
struct UndoTransaction {
    intent: EditIntent,
    changes: Vec<Change>,
    recorded_changes: usize,
    selection_before: Option<CursorSelection>,
    selection_after: Option<CursorSelection>,
    version_before: u64,
    version_after: u64,
}

#[derive(Debug)]
struct PendingTransaction {
    intent: EditIntent,
    changes: Vec<Change>,
    selection_before: Option<CursorSelection>,
    selection_after: Option<CursorSelection>,
}

pub(crate) struct Replay {
    pub(super) changes: Vec<Change>,
    pub(super) selection: Option<CursorSelection>,
}

/// Coordinates undo and redo as explicit editing transactions.
#[derive(Debug)]
pub(crate) struct UndoManager {
    undo_transactions: Vec<UndoTransaction>,
    redo_transactions: Vec<UndoTransaction>,
    ignoring: bool,
    transaction_depth: usize,
    pending: Option<PendingTransaction>,
    pending_intent: Option<EditIntent>,
    coalescing_boundary: bool,
    next_version: u64,
    current_version: u64,
    saved_version: Option<u64>,
}

impl UndoManager {
    pub(super) fn new() -> Self {
        Self {
            undo_transactions: Vec::new(),
            redo_transactions: Vec::new(),
            ignoring: false,
            transaction_depth: 0,
            pending: None,
            pending_intent: None,
            coalescing_boundary: false,
            next_version: 1,
            current_version: 0,
            saved_version: Some(0),
        }
    }

    pub(super) fn take_pending_intent(&mut self) -> Option<EditIntent> {
        self.pending_intent.take()
    }

    pub(super) fn set_pending_intent(&mut self, intent: EditIntent) {
        self.pending_intent = Some(intent);
    }

    pub(super) fn record_transaction(&mut self, change: Change, intent: EditIntent) -> bool {
        if self.ignoring {
            return false;
        }
        if change.old_range == change.new_range && change.old_text == change.new_text {
            self.break_transaction_coalescing();
            return false;
        }

        match self.pending.as_mut() {
            Some(pending) => pending.changes.push(change),
            None => self.push_batch(vec![change], intent),
        }
        true
    }

    pub(super) fn begin_transaction(&mut self) {
        self.begin_transaction_with(EditIntent::Atomic);
    }

    pub(super) fn begin_transaction_with(&mut self, intent: EditIntent) {
        self.transaction_depth += 1;
        if self.transaction_depth == 1 {
            self.pending = Some(PendingTransaction {
                intent,
                changes: Vec::new(),
                selection_before: None,
                selection_after: None,
            });
        }
    }

    pub(super) fn commit_transaction(&mut self) {
        if self.transaction_depth == 0 {
            return;
        }
        self.transaction_depth -= 1;
        if self.transaction_depth > 0 {
            return;
        }

        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.changes.is_empty() || is_noop_batch(&pending.changes) {
            return;
        }
        self.push_batch(pending.changes, pending.intent);
        if let Some(before) = pending.selection_before {
            self.record_selection_before(before);
        }
        if let Some(after) = pending.selection_after {
            self.record_selection_after(after);
        }
    }

    fn push_batch(&mut self, changes: Vec<Change>, intent: EditIntent) {
        if changes.is_empty() {
            return;
        }

        self.redo_transactions.clear();
        let can_coalesce = !self.coalescing_boundary
            && intent != EditIntent::Atomic
            && changes.len() == 1
            && self.undo_transactions.last().is_some_and(|previous| {
                previous.intent == intent
                    && previous.recorded_changes < MAX_CHANGES_PER_TRANSACTION
                    && previous
                        .changes
                        .last()
                        .is_some_and(|last| is_adjacent(intent, last, &changes[0]))
            });

        if can_coalesce {
            let previous = self
                .undo_transactions
                .last_mut()
                .expect("coalescing requires a previous transaction");
            let version = self.next_version;
            self.next_version += 1;
            previous.version_after = version;
            self.current_version = version;
            previous.recorded_changes += 1;
            if intent == EditIntent::Typing
                && let [change] = changes.as_slice()
                && let Some(last) = previous.changes.last_mut()
            {
                last.new_text
                    .truncate(change.old_range.start - last.new_range.start);
                last.new_text.push_str(&change.new_text);
                last.new_range.end = change.new_range.end;
                return;
            }
            previous.changes.extend(changes);
            return;
        }

        if self.undo_transactions.len() >= MAX_UNDO_TRANSACTIONS {
            self.undo_transactions.remove(0);
        }
        let version_before = self.current_version;
        let version_after = self.next_version;
        self.next_version += 1;
        self.current_version = version_after;
        self.undo_transactions.push(UndoTransaction {
            intent,
            recorded_changes: changes.len(),
            changes,
            selection_before: None,
            selection_after: None,
            version_before,
            version_after,
        });
        self.coalescing_boundary = intent == EditIntent::Atomic;
    }

    pub(super) fn record_selection(&mut self, before: CursorSelection, after: CursorSelection) {
        if self.ignoring {
            return;
        }
        self.record_selection_before(before);
        self.record_selection_after(after);
    }

    fn record_selection_before(&mut self, before: CursorSelection) {
        if let Some(pending) = self.pending.as_mut() {
            pending.selection_before.get_or_insert(before);
        } else if let Some(transaction) = self.undo_transactions.last_mut() {
            transaction.selection_before.get_or_insert(before);
        }
    }

    fn record_selection_after(&mut self, after: CursorSelection) {
        if let Some(pending) = self.pending.as_mut() {
            pending.selection_after = Some(after);
        } else if let Some(transaction) = self.undo_transactions.last_mut() {
            transaction.selection_after = Some(after);
        }
    }

    pub(super) fn has_open_transaction(&self) -> bool {
        self.transaction_depth > 0
    }

    pub(super) fn break_transaction_coalescing(&mut self) {
        if self.transaction_depth == 0 {
            self.commit_all_transactions();
        }
        self.coalescing_boundary = true;
    }

    fn commit_all_transactions(&mut self) {
        if self.transaction_depth == 0 {
            return;
        }
        self.transaction_depth = 1;
        self.commit_transaction();
    }

    pub(super) fn is_ignoring(&self) -> bool {
        self.ignoring
    }

    pub(super) fn set_ignoring(&mut self, ignoring: bool) {
        self.ignoring = ignoring;
        if ignoring {
            self.commit_all_transactions();
        }
    }

    pub(super) fn clear(&mut self) {
        self.undo_transactions.clear();
        self.redo_transactions.clear();
        self.transaction_depth = 0;
        self.pending = None;
        self.pending_intent = None;
        self.coalescing_boundary = false;
        self.next_version = 1;
        self.current_version = 0;
        self.saved_version = Some(0);
    }

    pub(super) fn begin_save(&mut self) -> u64 {
        self.commit_all_transactions();
        self.coalescing_boundary = true;
        self.current_version
    }

    pub(super) fn mark_saved_version(&mut self, version: u64) {
        self.saved_version = Some(version);
    }

    pub(super) fn is_clean(&self) -> bool {
        self.saved_version == Some(self.current_version)
            && self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.changes.is_empty())
    }

    pub(super) fn undo(&mut self) -> Option<Replay> {
        self.commit_all_transactions();
        let transaction = self.undo_transactions.pop()?;
        self.current_version = transaction.version_before;
        let replay = Replay {
            changes: transaction.changes.iter().rev().cloned().collect(),
            selection: transaction.selection_before,
        };
        self.redo_transactions.push(transaction);
        self.coalescing_boundary = true;
        Some(replay)
    }

    pub(super) fn redo(&mut self) -> Option<Replay> {
        self.commit_all_transactions();
        let transaction = self.redo_transactions.pop()?;
        self.current_version = transaction.version_after;
        let replay = Replay {
            changes: transaction.changes.clone(),
            selection: transaction.selection_after,
        };
        self.undo_transactions.push(transaction);
        self.coalescing_boundary = true;
        Some(replay)
    }

    pub(super) fn has_undos(&self) -> bool {
        !self.undo_transactions.is_empty()
    }
}

fn is_noop_batch(changes: &[Change]) -> bool {
    let Some(first) = changes.first() else {
        return true;
    };

    let mut last = first;
    for change in &changes[1..] {
        if last.new_range.start != change.old_range.start
            || last.new_range.end != change.old_range.end
        {
            return false;
        }
        last = change;
    }

    first.old_range.start == last.new_range.start
        && first.old_range.end == last.new_range.end
        && first.old_text == last.new_text
}

fn is_adjacent(intent: EditIntent, previous: &Change, current: &Change) -> bool {
    match intent {
        EditIntent::Typing => {
            previous.old_range.is_empty()
                && !previous.new_text.contains(['\n', '\r'])
                && !current.new_text.contains(['\n', '\r'])
                && previous.new_range.start <= current.old_range.start
                && previous.new_range.end == current.old_range.end
        }
        EditIntent::Backspace => {
            previous.new_text.is_empty()
                && current.new_text.is_empty()
                && current.old_range.end == previous.old_range.start
        }
        EditIntent::DeleteForward => {
            previous.new_text.is_empty()
                && current.new_text.is_empty()
                && current.old_range.start == previous.old_range.start
        }
        EditIntent::Atomic => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typing_change(offset: usize, text: &str) -> Change {
        let end = offset + text.len();
        Change::new(offset..offset, "", offset..end, text)
    }

    #[test]
    fn adjacent_typing_transactions_coalesce() {
        let mut manager = UndoManager::new();
        manager.record_transaction(typing_change(0, "a"), EditIntent::Typing);
        manager.record_transaction(typing_change(1, "b"), EditIntent::Typing);

        assert_eq!(manager.undo().unwrap().changes, [typing_change(0, "ab")]);
        assert!(manager.undo().is_none());
    }

    #[test]
    fn explicit_transaction_collects_multiple_changes() {
        let mut manager = UndoManager::new();
        manager.begin_transaction();
        manager.record_transaction(typing_change(0, "a"), EditIntent::Typing);
        manager.record_transaction(typing_change(1, "b"), EditIntent::Typing);
        manager.commit_transaction();

        let transaction = manager.undo().unwrap().changes;
        assert_eq!(transaction.len(), 2);
        assert_eq!(transaction[0].new_text, "b");
        assert_eq!(transaction[1].new_text, "a");
        assert!(manager.undo().is_none());
    }

    #[test]
    fn nested_transactions_commit_as_one_entry() {
        let mut manager = UndoManager::new();
        manager.begin_transaction();
        manager.record_transaction(typing_change(0, "a"), EditIntent::Typing);
        manager.begin_transaction();
        manager.record_transaction(typing_change(1, "b"), EditIntent::Typing);
        manager.commit_transaction();
        assert!(!manager.has_undos());
        manager.record_transaction(typing_change(2, "c"), EditIntent::Typing);
        manager.commit_transaction();

        assert_eq!(manager.undo().unwrap().changes.len(), 3);
        assert!(manager.undo().is_none());
    }

    #[test]
    fn an_atomic_batch_never_coalesces() {
        let mut manager = UndoManager::new();

        manager.begin_transaction();
        manager.record_transaction(typing_change(0, "a"), EditIntent::Typing);
        manager.commit_transaction();
        manager.record_transaction(typing_change(1, "b"), EditIntent::Typing);

        assert_eq!(manager.undo().unwrap().changes.len(), 1);
        assert_eq!(manager.undo().unwrap().changes.len(), 1);
    }

    #[test]
    fn limits_the_number_of_retained_transactions() {
        let mut manager = UndoManager::new();

        for offset in 0..1_100 {
            manager.record_transaction(typing_change(offset, "a"), EditIntent::Atomic);
        }

        for _ in 0..MAX_UNDO_TRANSACTIONS {
            assert!(manager.undo().is_some());
        }
        assert!(manager.undo().is_none());
    }

    #[test]
    fn splits_a_coalesced_transaction_before_its_change_list_grows_too_large() {
        let mut manager = UndoManager::new();

        for offset in 0..1_100 {
            manager.record_transaction(typing_change(offset, "a"), EditIntent::Typing);
        }

        assert_eq!(
            manager.undo().unwrap().changes,
            [typing_change(MAX_CHANGES_PER_TRANSACTION, &"a".repeat(100))]
        );
        assert_eq!(
            manager.undo().unwrap().changes,
            [typing_change(0, &"a".repeat(MAX_CHANGES_PER_TRANSACTION))]
        );
        assert!(manager.undo().is_none());
    }

    #[test]
    fn clean_state_tracks_undo_redo_and_save_boundaries() {
        let mut manager = UndoManager::new();
        assert!(manager.is_clean());

        manager.record_transaction(typing_change(0, "a"), EditIntent::Typing);
        assert!(!manager.is_clean());
        assert!(manager.undo().is_some());
        assert!(manager.is_clean());
        assert!(manager.redo().is_some());
        assert!(!manager.is_clean());

        let saved = manager.begin_save();
        manager.mark_saved_version(saved);
        assert!(manager.is_clean());
        manager.record_transaction(typing_change(1, "b"), EditIntent::Typing);
        assert!(!manager.is_clean());
        let replay = manager.undo().unwrap();
        assert_eq!(replay.changes, [typing_change(1, "b")]);
        assert!(manager.is_clean());

        let snapshot = manager.begin_save();
        manager.record_transaction(typing_change(1, "c"), EditIntent::Typing);
        manager.mark_saved_version(snapshot);
        assert!(!manager.is_clean());
        assert!(manager.undo().is_some());
        assert!(manager.is_clean());

        assert!(manager.undo().is_some());
        assert!(!manager.is_clean());
        assert!(manager.redo().is_some());
        assert!(manager.is_clean());

        assert!(manager.undo().is_some());
        manager.record_transaction(typing_change(0, "x"), EditIntent::Typing);
        assert!(!manager.is_clean());
        assert!(manager.undo().is_some());
        assert!(!manager.is_clean());
    }
}
