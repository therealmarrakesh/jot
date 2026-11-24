use std::ops::Index;
use std::time::Duration;

use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::Editor;
use crate::ids::{EditorSlot, TabId};
use crate::platform::TimerHandle;
use crate::types::SharedStr;

use super::model::{SlotKey, Tab, TabData};

pub(super) const TAB_WIDTH: f32 = 240.;
pub(super) const TAB_MIN_WIDTH: f32 = 80.;
pub(crate) const TAB_HEIGHT: f32 = 32.;
pub(crate) const LIST_PAD: f32 = 4.;
pub(super) const SCROLL_OUTER_PAD: f32 = 8.;
pub(super) const SCROLL_INNER_PAD: f32 = 3.;
pub(super) const BUTTON_WIDTH: f32 = 32.;
pub(super) const BUTTON_HEIGHT: f32 = 24.;
pub(super) const BUTTON_TOP: f32 = 6.;
pub(super) const SCROLL_AREA: f32 = SCROLL_OUTER_PAD + BUTTON_WIDTH + SCROLL_INNER_PAD;
pub(super) const NEW_TAB_GAP: f32 = 5.;
pub(super) const NEW_TAB_GAP_SCROLLING: f32 = 3.;
pub(crate) const STRIP_END_RESERVE: f32 = NEW_TAB_GAP_SCROLLING + BUTTON_WIDTH;
pub(crate) const MIN_STRIP_WIDTH: f32 = 2. * LIST_PAD + STRIP_END_RESERVE;
pub(crate) const SCROLL_STEP: f32 = 50.;
pub(crate) const SCROLL_REPEAT_DELAY: Duration = Duration::from_millis(250);
pub(crate) const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const WHEEL_SHARE: f32 = 0.15;
pub(super) const CLOSE_GLYPH: &str = "\u{E711}";
const REORDER_THRESHOLD: f32 = 16.;

pub(crate) enum Removal {
    LastTab,
    Removed {
        id: TabId,
        closed_active: bool,
        next_active: usize,
    },
}

pub(super) struct Ghost {
    pub(super) name: SharedStr,
    pub(super) key: SlotKey,
    pub(super) x: f32,
    pub(super) was_active: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    pub(super) id: TabId,
    grab: f32,
    last_move: f32,
    left: f32,
}

pub(crate) struct TabSet {
    pub(super) tabs: Vec<Tab>,
    pub(super) active: usize,
    pub(super) extent: f32,
    pub(super) scroll: f32,
    pub(super) snap_layout: bool,
    pub(super) snap_scroll: bool,
    next_key: usize,
    pub(super) column: f32,
    pub(super) width: f32,
    pub(super) overflow: bool,
    pub(super) buttons: bool,
    frozen: Option<f32>,
    pub(super) ghosts: Vec<Ghost>,
    pub(super) drag: Option<Drag>,
    pub(super) lifted: Option<TabId>,
    repeat: TimerHandle,
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
        let mut set = Self {
            tabs: Vec::with_capacity(1),
            active: 0,
            extent: TAB_WIDTH,
            scroll: 0.,
            snap_layout: false,
            snap_scroll: false,
            next_key: 0,
            column: f32::INFINITY,
            width: TAB_WIDTH,
            overflow: false,
            buttons: false,
            frozen: None,
            ghosts: Vec::new(),
            drag: None,
            lifted: None,
            repeat: TimerHandle::default(),
        };
        let slot = set.next_slot();
        set.tabs.push(Tab::new(initial, slot));
        set
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
        tab.data.doc_path.is_none() && tab.data.editor.model().text().len() == 0
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

    fn next_slot(&mut self) -> SlotKey {
        self.next_key += 1;
        SlotKey(self.next_key)
    }

    pub(super) fn snap(&mut self) {
        self.snap_layout = true;
        self.snap_scroll = true;
    }

    pub(super) fn measure(&mut self) {
        self.measure_for(self.tabs.len());
    }

    fn measure_for(&mut self, count: usize) {
        let alone = count <= 1;
        let count = count.max(1) as f32;
        let room = (self.column - 2. * LIST_PAD).max(0.);
        let min_width = if alone { 0. } else { TAB_MIN_WIDTH };
        let fit = (room / count).clamp(min_width, TAB_WIDTH);
        self.width = self.frozen.unwrap_or(fit);
        self.overflow = !alone && self.width * count > room;
        let between_buttons = room - 2. * SCROLL_AREA;
        self.buttons = self.overflow && between_buttons >= TAB_MIN_WIDTH;
        if self.overflow && self.frozen.is_none() {
            self.width = if self.buttons {
                (between_buttons / count).clamp(TAB_MIN_WIDTH, TAB_WIDTH)
            } else {
                room.min(TAB_WIDTH)
            };
        }
    }

    pub(super) fn layout(&mut self) {
        self.measure();
        self.extent = self.tabs.len() as f32 * self.width;
        self.scroll_to(self.scroll);
    }

    fn scroll_area(&self) -> f32 {
        if self.buttons { SCROLL_AREA } else { 0. }
    }

    pub(crate) fn scroll_viewport(&self) -> f32 {
        self.column - 2. * self.scroll_area()
    }

    pub(super) fn max_scroll(&self) -> f32 {
        if self.overflow {
            (2. * LIST_PAD + self.extent - self.scroll_viewport()).max(0.)
        } else {
            0.
        }
    }

    fn origin_in_strip(&self) -> f32 {
        self.scroll_area() + LIST_PAD - self.scroll
    }

    pub(super) fn new_tab_gap(&self) -> f32 {
        if self.buttons {
            NEW_TAB_GAP_SCROLLING
        } else {
            NEW_TAB_GAP
        }
    }

    pub(super) fn tab_target(&self, ix: usize) -> f32 {
        let id = self.tabs[ix].data.id;
        self.drag
            .filter(|drag| drag.id == id)
            .map_or(ix as f32 * self.width, |drag| drag.left)
    }

    fn place(&self, strip_x: f32, grab: f32, last: usize) -> (f32, f32, usize) {
        let x = strip_x - self.origin_in_strip();
        let left = (x - grab).clamp(0., last as f32 * self.width);
        (x, left, slot_index(left, self.width, last))
    }

    fn depart(&mut self, id: TabId) {
        if self.drag.is_some_and(|drag| drag.id == id) {
            self.drag = None;
        }
        if self.lifted == Some(id) {
            self.lifted = None;
        }
    }

    fn scroll_to(&mut self, target: f32) {
        self.scroll = target.clamp(0., self.max_scroll()).round();
    }

    pub(crate) fn scroll_by(&mut self, delta: f32) -> bool {
        let before = self.scroll;
        self.scroll_to(before + delta);
        self.scroll != before
    }

    pub(super) fn reveal(&mut self, left: f32, right: f32) {
        if !self.overflow {
            self.scroll = 0.;
            self.snap_scroll = true;
            return;
        }
        let viewport = self.scroll_viewport();
        let current = self.scroll;
        let target = if left < current {
            left
        } else if right + 2. * LIST_PAD > current + viewport {
            (right + 2. * LIST_PAD - viewport).min(left)
        } else {
            current
        };
        self.scroll_to(target);
    }

    pub(super) fn reveal_tab(&mut self, ix: usize) {
        if ix < self.tabs.len() {
            let left = self.tab_target(ix);
            self.reveal(left, left + self.width);
        }
    }

    pub(crate) fn select(&mut self, ix: usize) -> bool {
        let changed = self.active != ix;
        self.active = ix;
        self.reveal_tab(ix);
        changed
    }

    pub(crate) fn push(&mut self, data: TabData) -> usize {
        let slot = self.next_slot();
        self.tabs.push(Tab::new(data, slot));
        self.layout();
        self.tabs.len() - 1
    }

    pub(crate) fn replace(&mut self, ix: usize, data: TabData) {
        let slot = self.tabs[ix].slot;
        let replaced = std::mem::replace(&mut self.tabs[ix], Tab::new(data, slot));
        self.depart(replaced.data.id);
    }

    pub(crate) fn remove(&mut self, ix: usize, freeze_if_hovering: bool) -> Removal {
        if self.tabs.len() == 1 {
            return Removal::LastTab;
        }
        let was_active = ix == self.active;
        let width = self.width;
        let name: SharedStr = self.tab_name(ix).into();
        let tab = self.tabs.remove(ix);
        let id = tab.data.id;
        self.depart(id);
        if ix < self.tabs.len() && freeze_if_hovering {
            self.frozen = Some(width);
        }
        self.ghosts.push(Ghost {
            name,
            key: tab.slot,
            x: ix as f32 * width,
            was_active,
        });
        let (closed_active, next_active) = self.settle_after_removal(ix);
        Removal::Removed {
            id,
            closed_active,
            next_active,
        }
    }

    pub(crate) fn take(&mut self, id: TabId) -> Option<(TabData, Removal)> {
        let ix = self.index_of(id)?;
        let tab = self.tabs.remove(ix);
        self.depart(id);
        let outcome = if self.tabs.is_empty() {
            Removal::LastTab
        } else {
            let (closed_active, next_active) = self.settle_after_removal(ix);
            Removal::Removed {
                id,
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
        self.layout();
        (closed_active, self.active)
    }

    pub(crate) fn insert_dragged(&mut self, data: TabData, strip_x: f32, grab: f32) -> usize {
        let slot = self.next_slot();
        let id = data.id;
        let count_before = self.tabs.len();
        self.measure_for(count_before + 1);
        let (x, left, ix) = self.place(strip_x, grab, count_before);
        self.drag = Some(Drag {
            id,
            grab,
            last_move: x,
            left,
        });
        self.lifted = Some(id);
        self.snap();
        self.tabs.insert(ix, Tab::new(data, slot));
        self.layout();
        ix
    }

    pub(crate) fn restore(&mut self, data: TabData, ix: usize) -> usize {
        let slot = self.next_slot();
        let ix = ix.min(self.tabs.len());
        self.tabs.insert(ix, Tab::new(data, slot));
        self.layout();
        ix
    }

    pub(crate) fn return_tab(&mut self, id: TabId, ix: usize) -> Option<usize> {
        self.drag = None;
        if let Some(from) = self.index_of(id) {
            let tab = self.tabs.remove(from);
            let ix = ix.min(self.tabs.len());
            self.tabs.insert(ix, tab);
        }
        self.layout();
        self.index_of(id)
    }

    pub(crate) fn drag(&mut self, id: TabId, strip_x: f32, grab: f32, strip_start: f32) {
        let Some(from) = self.index_of(id) else {
            return;
        };
        let active_id = self.active_id();
        let origin = self.origin_in_strip();
        let mut drag = self.drag.filter(|d| d.id == id).unwrap_or(Drag {
            id,
            grab,
            last_move: strip_start - origin,
            left: 0.,
        });
        self.lifted = Some(id);
        let width = self.width;
        let last = self.tabs.len() - 1;
        let (x, left, to) = self.place(strip_x, drag.grab, last);
        drag.left = left;
        self.snap_scroll = true;
        self.reveal(left, left + width);
        if to != from && (x - drag.last_move).abs() > REORDER_THRESHOLD {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            self.active = self.index_of(active_id).unwrap_or(to);
            drag.last_move = x;
        }
        self.drag = Some(drag);
        self.layout();
    }

    pub(crate) fn release_drag(&mut self) -> bool {
        if self.drag.take().is_some() {
            self.layout();
            true
        } else {
            false
        }
    }

    pub(crate) fn mark_saved(
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

    pub(crate) fn set_scroll_repeat(&mut self, handle: TimerHandle) {
        self.repeat = handle;
    }

    pub(crate) fn stop_scroll_repeat(&mut self) {
        self.repeat.cancel();
    }

    #[inline]
    pub(crate) fn overflow(&self) -> bool {
        self.overflow
    }

    #[inline]
    pub(crate) fn is_frozen(&self) -> bool {
        self.frozen.is_some()
    }

    pub(crate) fn thaw_if_frozen(&mut self) -> bool {
        if self.frozen.take().is_some() {
            self.layout();
            true
        } else {
            false
        }
    }
}

pub(super) fn slot_index(left: f32, width: f32, last: usize) -> usize {
    ((left / width - 0.5).ceil().max(0.) as usize).min(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::EDITOR_PADDINGS;

    fn make_tab() -> TabData {
        TabData::create(None, true, EDITOR_PADDINGS)
    }

    #[test]
    fn slot_index_rounds_at_tab_midpoint_and_clamps() {
        assert_eq!(slot_index(0., 100., 3), 0);
        assert_eq!(slot_index(49., 100., 3), 0);
        assert_eq!(slot_index(51., 100., 3), 1);
        assert_eq!(slot_index(150., 100., 3), 1);
        assert_eq!(slot_index(151., 100., 3), 2);
        assert_eq!(slot_index(999., 100., 3), 3);
        assert_eq!(slot_index(-50., 100., 3), 0);
    }

    #[test]
    fn strip_measure_and_reveal_handle_fit_and_overflow() {
        let mut set = TabSet::new(make_tab());
        set.column = 600.;
        set.push(make_tab());
        assert_eq!(set.width, TAB_WIDTH);
        assert!(!set.overflow);
        assert!(!set.buttons);

        for _ in 0..8 {
            set.push(make_tab());
        }
        assert!(set.overflow);
        assert!(set.buttons);
        assert_eq!(set.width, TAB_MIN_WIDTH);

        set.reveal(9. * set.width, 10. * set.width);
        assert!(set.scroll > 0.);
        assert!(set.scroll <= set.max_scroll());

        set.reveal(0., set.width);
        assert_eq!(set.scroll, 0.);
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

        match set.remove(0, false) {
            Removal::Removed {
                id,
                closed_active,
                next_active,
            } => {
                assert_eq!(id, id0);
                assert!(!closed_active);
                assert_eq!(next_active, 1);
            }
            Removal::LastTab => panic!("expected Removed"),
        }
        assert_eq!(set.active_id(), id2);

        let kept_slot = set[0].slot;
        let replacement = make_tab();
        let repl_id = replacement.id;
        set.replace(0, replacement);
        assert_eq!(set[0].slot, kept_slot);
        assert_eq!(set[0].data.id, repl_id);
        assert!(set.index_of(id1).is_none());

        let restored_ix = set.restore(make_tab(), 99);
        assert_eq!(restored_ix, 2);
        assert_ne!(set[2].slot, kept_slot);

        let (_, out) = set.take(id2).unwrap();
        assert!(matches!(out, Removal::Removed { .. }));
        let rem_id = set[1].data.id;
        let (_, out) = set.take(rem_id).unwrap();
        assert!(matches!(out, Removal::Removed { .. }));
        assert!(matches!(set.remove(0, false), Removal::LastTab));
        let (_, out) = set.take(repl_id).unwrap();
        assert!(matches!(out, Removal::LastTab));
    }
}
