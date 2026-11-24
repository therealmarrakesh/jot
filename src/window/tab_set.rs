use std::ops::Index;

use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::Editor;
use crate::ids::{EditorSlot, Node, TabId, UiState};
use crate::platform::TimerHandle;
use crate::tabs::{StripFrame, Tab, TabData, TabStrip};
use crate::theme::Palette;

pub(super) enum RemoveOutcome {
    LastTab,
    Removed {
        id: TabId,
        closed_active: bool,
        next_active: usize,
    },
}

pub(super) enum TakeOutcome {
    NowEmpty,
    Settled {
        closed_active: bool,
        next_active: usize,
    },
}

pub(crate) struct TabSet {
    tabs: Vec<Tab>,
    active: usize,
    strip: TabStrip,
}

impl Index<usize> for TabSet {
    type Output = Tab;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.tabs[index]
    }
}

impl TabSet {
    pub(crate) fn new(initial: TabData) -> Self {
        let mut strip = TabStrip::new();
        let slot = strip.next_slot();
        Self {
            tabs: vec![Tab::new(initial, slot)],
            active: 0,
            strip,
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    #[inline]
    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    #[inline]
    pub(crate) fn active(&self) -> &TabData {
        &self.tabs[self.active].data
    }

    #[inline]
    pub(crate) fn active_mut(&mut self) -> &mut TabData {
        &mut self.tabs[self.active].data
    }

    #[inline]
    pub(crate) fn active_id(&self) -> TabId {
        self.tabs[self.active].data.id
    }

    #[inline]
    pub(crate) fn active_slot(&self) -> EditorSlot {
        EditorSlot::Tab(self.active_id())
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Tab> {
        self.tabs.iter()
    }

    pub(crate) fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.data.id == id)
    }

    pub(crate) fn editor(&self, id: TabId) -> Option<&Editor> {
        self.index_of(id).map(|ix| &self.tabs[ix].data.editor)
    }

    pub(crate) fn editor_mut(&mut self, id: TabId) -> Option<&mut Editor> {
        self.index_of(id).map(|ix| &mut self.tabs[ix].data.editor)
    }

    pub(crate) fn for_each_editor_mut(&mut self, mut f: impl FnMut(&mut Editor)) {
        for tab in &mut self.tabs {
            f(&mut tab.data.editor);
        }
    }

    pub(crate) fn tab_name(&self, ix: usize) -> String {
        document::display_name(self.tabs[ix].data.path())
    }

    pub(crate) fn find_by_key(&self, key: &PathKey) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.data.path_key() == Some(key))
    }

    pub(crate) fn is_blank(&self, ix: usize) -> bool {
        let tab = &self.tabs[ix];
        tab.data.doc_path.is_none() && tab.data.editor.text().len() == 0
    }

    pub(crate) fn has_dirty(&self) -> bool {
        self.tabs.iter().any(|tab| tab.data.is_dirty())
    }

    pub(crate) fn dirty_tabs(&self) -> Vec<TabId> {
        self.tabs
            .iter()
            .filter(|tab| tab.data.is_dirty())
            .map(|tab| tab.data.id)
            .collect()
    }

    pub(super) fn select(&mut self, ix: usize) -> bool {
        let changed = self.active != ix;
        self.active = ix;
        self.strip.reveal_tab(&self.tabs, ix);
        changed
    }

    pub(super) fn push(&mut self, data: TabData) -> usize {
        let slot = self.strip.next_slot();
        self.tabs.push(Tab::new(data, slot));
        self.strip.layout(self.tabs.len());
        self.tabs.len() - 1
    }

    pub(super) fn replace(&mut self, ix: usize, data: TabData) {
        let slot = self.tabs[ix].slot;
        let replaced = std::mem::replace(&mut self.tabs[ix], Tab::new(data, slot));
        self.strip.depart(replaced.data.id);
    }

    pub(super) fn remove(&mut self, ix: usize, freeze_if_hovering: bool) -> RemoveOutcome {
        if self.tabs.len() == 1 {
            return RemoveOutcome::LastTab;
        }
        let was_active = ix == self.active;
        let width = self.strip.width;
        let name: crate::types::SharedStr = self.tab_name(ix).into();
        let tab = self.tabs.remove(ix);
        let id = tab.data.id;
        self.strip.depart(id);
        if ix < self.tabs.len() && freeze_if_hovering {
            self.strip.freeze(width);
        }
        self.strip.push_ghost(name, tab.slot, ix, width, was_active);
        let (closed_active, next_active) = self.settle_after_removal(ix);
        RemoveOutcome::Removed {
            id,
            closed_active,
            next_active,
        }
    }

    pub(super) fn take(&mut self, id: TabId) -> Option<(TabData, TakeOutcome)> {
        let ix = self.index_of(id)?;
        let tab = self.tabs.remove(ix);
        self.strip.depart(id);
        let outcome = if self.tabs.is_empty() {
            TakeOutcome::NowEmpty
        } else {
            let (closed_active, next_active) = self.settle_after_removal(ix);
            TakeOutcome::Settled {
                closed_active,
                next_active,
            }
        };
        Some((tab.data, outcome))
    }

    fn settle_after_removal(&mut self, ix: usize) -> (bool, usize) {
        let closed_active = ix == self.active;
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.strip.layout(self.tabs.len());
        (closed_active, self.active)
    }

    pub(super) fn insert_dragged(&mut self, data: TabData, x: f32, grab: f32) -> usize {
        let slot = self.strip.next_slot();
        let id = data.id;
        let ix = self
            .strip
            .begin_inserted_drag(id, x, grab, self.tabs.len());
        self.tabs.insert(ix, Tab::new(data, slot));
        self.strip.layout(self.tabs.len());
        ix
    }

    pub(super) fn restore(&mut self, data: TabData, ix: usize) -> usize {
        let slot = self.strip.next_slot();
        let ix = ix.min(self.tabs.len());
        self.tabs.insert(ix, Tab::new(data, slot));
        self.strip.layout(self.tabs.len());
        ix
    }

    pub(super) fn return_tab(&mut self, id: TabId, ix: usize) -> Option<usize> {
        self.strip.clear_drag();
        if let Some(from) = self.index_of(id) {
            let tab = self.tabs.remove(from);
            let ix = ix.min(self.tabs.len());
            self.tabs.insert(ix, tab);
        }
        self.strip.layout(self.tabs.len());
        self.index_of(id)
    }

    pub(super) fn drag(&mut self, id: TabId, x: f32, grab: f32, start: f32) {
        let active_id = self.active_id();
        if let Some(next_active) =
            self.strip
                .drag_tab(&mut self.tabs, active_id, id, x, grab, start)
        {
            self.active = next_active;
        }
    }

    pub(super) fn release_drag(&mut self) -> bool {
        self.strip.release_drag(self.tabs.len())
    }

    pub(super) fn mark_saved(
        &mut self,
        tab: TabId,
        snapshot: u64,
        doc_path: DocPath,
        format: Format,
    ) -> bool {
        let Some(ix) = self.index_of(tab) else {
            return false;
        };
        let data = &mut self.tabs[ix].data;
        data.editor.mark_saved_version(snapshot);
        data.doc_path = Some(doc_path);
        data.format = format;
        true
    }

    pub(super) fn scroll_by(&mut self, delta: f32) -> bool {
        self.strip.scroll_by(delta)
    }

    pub(super) fn set_scroll_repeat(&mut self, handle: TimerHandle) {
        self.strip.repeat = handle;
    }

    pub(super) fn stop_scroll_repeat(&mut self) {
        self.strip.stop_repeat();
    }

    #[inline]
    pub(super) fn strip_overflow(&self) -> bool {
        self.strip.overflow
    }

    #[inline]
    pub(super) fn strip_scroll_viewport(&self) -> f32 {
        self.strip.scroll_viewport()
    }

    #[inline]
    pub(super) fn strip_is_frozen(&self) -> bool {
        self.strip.is_frozen()
    }

    pub(super) fn thaw_if_frozen(&mut self) -> bool {
        self.strip.thaw(self.tabs.len())
    }

    pub(super) fn sample(&mut self, width: f32, ui: &mut UiState) -> StripFrame {
        self.strip.sample(&self.tabs, self.active, width, ui)
    }

    pub(super) fn render(&mut self, strip: &StripFrame, p: Palette, ui: &mut UiState) -> Node {
        self.strip.render(&self.tabs, self.active, strip, p, ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::EDITOR_PADDINGS;

    fn make_tab() -> TabData {
        TabData::create(None, true, EDITOR_PADDINGS)
    }

    #[test]
    fn tab_set_maintains_active_index_and_unique_slots_across_mutations() {
        let mut set = TabSet::new(make_tab());
        let id0 = set.active_id();
        let slot0 = set[0].slot;

        let ix1 = set.push(make_tab());
        let id1 = set[ix1].data.id;
        let ix2 = set.push(make_tab());
        let id2 = set[ix2].data.id;
        assert_eq!(set.len(), 3);
        assert_ne!(slot0, set[1].slot);
        assert_ne!(set[1].slot, set[2].slot);

        set.select(2);
        assert_eq!(set.active_id(), id2);

        // Removing a tab before active shifts active down while keeping active tab id.
        match set.remove(0, false) {
            RemoveOutcome::Removed {
                id,
                closed_active,
                next_active,
            } => {
                assert_eq!(id, id0);
                assert!(!closed_active);
                assert_eq!(next_active, 1);
            }
            RemoveOutcome::LastTab => panic!("expected Removed"),
        }
        assert_eq!(set.active_id(), id2);

        // Replacing keeps slot key intact.
        let kept_slot = set[0].slot;
        let replacement = make_tab();
        let repl_id = replacement.id;
        set.replace(0, replacement);
        assert_eq!(set[0].slot, kept_slot);
        assert_eq!(set[0].data.id, repl_id);
        assert!(set.index_of(id1).is_none());

        // Restoring allocates a fresh slot and clamps index.
        let restored_ix = set.restore(make_tab(), 99);
        assert_eq!(restored_ix, 2);
        assert_ne!(set[2].slot, kept_slot);

        // Taking all tabs down to empty signals NowEmpty.
        let (_, out) = set.take(id2).unwrap();
        assert!(matches!(out, TakeOutcome::Settled { .. }));
        let rem_id = set[1].data.id;
        let (_, out) = set.take(rem_id).unwrap();
        assert!(matches!(out, TakeOutcome::Settled { .. }));
        assert!(matches!(set.remove(0, false), RemoveOutcome::LastTab));
        let (_, out) = set.take(repl_id).unwrap();
        assert!(matches!(out, TakeOutcome::NowEmpty));
    }
}
