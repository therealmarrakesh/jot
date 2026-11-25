use std::ops::Index;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::command::{AppCommand, shortcut_tooltip};
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{DocOptions, Editor};
use crate::text::Rope;
use crate::geom::{Edges, size, transparent_black};
use crate::platform::CursorStyle;
use crate::theme::{OVERLAY_RADIUS, Palette, SMALL_FONT};
use crate::types::SharedStr;
use crate::ui::{
    AnimKey, AnimStatus, AppNode, AppUi, Brushes, Easing, EditorSlot, HitId, TabHit, TabId, box_,
    row, text,
};
use crate::widgets::icon_button;

const TAB_WIDTH: f32 = 240.;
const TAB_MIN_WIDTH: f32 = 80.;
pub(crate) const TAB_HEIGHT: f32 = 32.;
pub(crate) const LIST_PAD: f32 = 4.;
const SCROLL_OUTER_PAD: f32 = 8.;
const SCROLL_INNER_PAD: f32 = 3.;
const BUTTON_WIDTH: f32 = 32.;
const BUTTON_HEIGHT: f32 = 24.;
const BUTTON_TOP: f32 = 6.;
const SCROLL_AREA: f32 = SCROLL_OUTER_PAD + BUTTON_WIDTH + SCROLL_INNER_PAD;
const NEW_TAB_GAP: f32 = 5.;
const NEW_TAB_GAP_SCROLLING: f32 = 3.;
pub(crate) const STRIP_END_RESERVE: f32 = NEW_TAB_GAP_SCROLLING + BUTTON_WIDTH;
pub(crate) const MIN_STRIP_WIDTH: f32 = 2. * LIST_PAD + STRIP_END_RESERVE;
pub(crate) const SCROLL_STEP: f32 = 50.;
pub(crate) const SCROLL_REPEAT_DELAY: Duration = Duration::from_millis(250);
pub(crate) const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const WHEEL_SHARE: f32 = 0.15;
const CLOSE_GLYPH: &str = "\u{E711}";
const REORDER_THRESHOLD: f32 = 16.;

pub(crate) enum Removal {
    LastTab,
    Removed {
        id: TabId,
        closed_active: bool,
        next_active: usize,
    },
}

struct Ghost {
    name: SharedStr,
    key: SlotKey,
    x: f32,
    was_active: bool,
}

#[derive(Clone, Copy)]
struct Drag {
    id: TabId,
    grab: f32,
    last_move: f32,
    left: f32,
}

pub(crate) struct TabSet {
    tabs: Vec<Tab>,
    active: usize,
    extent: f32,
    scroll: f32,
    snap_layout: bool,
    snap_scroll: bool,
    next_key: usize,
    column: f32,
    width: f32,
    overflow: bool,
    buttons: bool,
    frozen: Option<f32>,
    ghosts: Vec<Ghost>,
    drag: Option<Drag>,
    lifted: Option<TabId>,
    /// While a scroll arrow is held: when it next scrolls, and by how much.
    pub(crate) repeat: Option<(Instant, f32)>,
}

impl Index<usize> for TabSet {
    type Output = Tab;

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
            repeat: None,
        };
        let slot = set.next_slot();
        set.tabs.push(Tab { data: initial, slot });
        set
    }

    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    pub(crate) fn active(&self) -> &TabData {
        &self.tabs[self.active].data
    }

    pub(crate) fn active_mut(&mut self) -> &mut TabData {
        &mut self.tabs[self.active].data
    }

    pub(crate) fn active_id(&self) -> TabId {
        self.tabs[self.active].data.id
    }

    pub(crate) fn active_slot(&self) -> EditorSlot {
        EditorSlot::Tab(self.active_id())
    }

    #[cfg(test)]
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

    fn snap(&mut self) {
        self.snap_layout = true;
        self.snap_scroll = true;
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

    fn layout(&mut self) {
        self.measure_for(self.tabs.len());
        self.extent = self.tabs.len() as f32 * self.width;
        self.scroll_to(self.scroll);
    }

    fn scroll_area(&self) -> f32 {
        if self.buttons { SCROLL_AREA } else { 0. }
    }

    pub(crate) fn scroll_viewport(&self) -> f32 {
        self.column - 2. * self.scroll_area()
    }

    fn max_scroll(&self) -> f32 {
        if self.overflow {
            (2. * LIST_PAD + self.extent - self.scroll_viewport()).max(0.)
        } else {
            0.
        }
    }

    fn origin_in_strip(&self) -> f32 {
        self.scroll_area() + LIST_PAD - self.scroll
    }

    fn new_tab_gap(&self) -> f32 {
        if self.buttons {
            NEW_TAB_GAP_SCROLLING
        } else {
            NEW_TAB_GAP
        }
    }

    fn tab_target(&self, ix: usize) -> f32 {
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

    fn reveal(&mut self, left: f32, right: f32) {
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

    fn reveal_tab(&mut self, ix: usize) {
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
        self.tabs.push(Tab { data, slot });
        self.layout();
        self.tabs.len() - 1
    }

    pub(crate) fn replace(&mut self, ix: usize, data: TabData) {
        let slot = self.tabs[ix].slot;
        let replaced = std::mem::replace(&mut self.tabs[ix], Tab { data, slot });
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
        self.tabs.insert(ix, Tab { data, slot });
        self.layout();
        ix
    }

    pub(crate) fn restore(&mut self, data: TabData, ix: usize) -> usize {
        let slot = self.next_slot();
        let ix = ix.min(self.tabs.len());
        self.tabs.insert(ix, Tab { data, slot });
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

    pub(crate) fn overflow(&self) -> bool {
        self.overflow
    }

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

fn slot_index(left: f32, width: f32, last: usize) -> usize {
    ((left / width - 0.5).ceil().max(0.) as usize).min(last)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotKey(usize);

pub(crate) struct TabData {
    pub(crate) id: TabId,
    pub(crate) editor: Editor,
    pub(crate) zoom: f32,
    pub(crate) doc_path: Option<DocPath>,
    pub(crate) format: Format,
}

impl TabData {
    pub(crate) fn path(&self) -> Option<&Path> {
        self.doc_path.as_ref().map(DocPath::as_path)
    }

    pub(crate) fn path_key(&self) -> Option<&PathKey> {
        self.doc_path.as_ref().map(DocPath::key)
    }

    pub(crate) fn is_dirty(&self) -> bool {
        let m = self.editor.model();
        !m.is_clean() && (self.doc_path.is_some() || m.text().len() > 0)
    }

    pub(crate) fn create(
        content: Option<document::Loaded>,
        word_wrap: bool,
        paddings: Edges<f32>,
    ) -> Self {
        let (doc_path, text, format) = match content {
            Some(loaded) => (Some(loaded.doc_path), loaded.text, loaded.format),
            None => (None, Rope::new(), Format::default()),
        };
        let editor = Editor::document(
            text,
            DocOptions {
                soft_wrap: word_wrap,
                paddings,
            },
        );
        Self {
            id: TabId::next(),
            editor,
            zoom: 1.,
            doc_path,
            format,
        }
    }
}

pub(crate) struct Tab {
    pub(crate) data: TabData,
    pub(crate) slot: SlotKey,
}

const TAB_LABEL_INSET: f32 = 8.;
const TAB_LABEL_TRAILING: f32 = 44.;
const TAB_LABEL_TRAILING_BARE: f32 = 8.;
const UNSAVED_DOT: f32 = 6.;
const UNSAVED_DOT_TOP: f32 = 13.;
const UNSAVED_DOT_RIGHT: f32 = 19.;
const CLOSE_BUTTON_TOP: f32 = 5.;
const CLOSE_BUTTON_RIGHT: f32 = 6.;
const SCROLL_GLYPH_SIZE: f32 = 8.;
const SLIDE_DURATION: Duration = Duration::from_millis(200);
const SLIDE_EASING: Easing = Easing::CubicBezier {
    x1: 1. / 3.,
    y1: 2. / 3.,
    x2: 2. / 3.,
    y2: 1.,
};
const CARET_LEFT: &str = "\u{EDD9}";
const CARET_RIGHT: &str = "\u{EDDA}";
const ADD_GLYPH: &str = "\u{E710}";
const BUTTON_GLYPH_SIZE: f32 = 12.;

pub(crate) struct StripFrame {
    shift: f32,
    viewport: f32,
    tabs: Vec<(f32, f32)>,
    ghosts: Vec<(SharedStr, f32, f32, bool)>,
}

impl TabSet {
    pub(crate) fn sample(&mut self, column: f32, ui: &mut AppUi) -> StripFrame {
        if column != self.column {
            self.column = column;
            self.snap();
            self.layout();
            self.reveal_tab(self.active);
        }
        let snap = std::mem::take(&mut self.snap_layout);
        let snap_scroll = std::mem::take(&mut self.snap_scroll);
        let (scroll, _) = slide(AnimKey::TabScroll, 0., self.scroll, snap_scroll, ui);
        let (extent, _) = slide(AnimKey::TabExtent, TAB_WIDTH, self.extent, snap, ui);
        let mut right = 0.;
        let mut lifted_moving = false;
        let mut sampled_tabs = Vec::with_capacity(self.tabs.len());
        for ix in 0..self.tabs.len() {
            let tab = &self.tabs[ix];
            let id = tab.data.id;
            let dragged = self.drag.is_some_and(|drag| drag.id == id);
            let (x, moving) = slide(
                AnimKey::TabX(tab.slot.0),
                right,
                self.tab_target(ix),
                snap || dragged,
                ui,
            );
            let (width, _) = slide(AnimKey::TabWidth(tab.slot.0), 0., self.width, snap, ui);
            if self.lifted == Some(id) {
                lifted_moving = moving;
            }
            right = x + width;
            sampled_tabs.push((x, width));
        }
        let mut ghosts = Vec::new();
        self.ghosts.retain(|ghost| {
            let (x, sliding) = slide(AnimKey::TabX(ghost.key.0), ghost.x, ghost.x, false, ui);
            let (width, shrinking) = slide(AnimKey::TabWidth(ghost.key.0), 0., 0., false, ui);
            let moving = sliding || shrinking;
            if moving {
                ghosts.push((ghost.name.clone(), x, width, ghost.was_active));
            }
            moving
        });
        if self.drag.is_none() && !lifted_moving {
            self.lifted = None;
        }
        let viewport = if self.overflow {
            self.scroll_viewport()
        } else {
            LIST_PAD + extent
        };
        StripFrame {
            shift: LIST_PAD - scroll,
            viewport,
            tabs: sampled_tabs,
            ghosts,
        }
    }

    pub(crate) fn render(&self, sampled: &StripFrame, p: Palette, ui: &mut AppUi) -> AppNode {
        let shift = sampled.shift;
        let viewport = sampled.viewport;
        let lifted = self.lifted.and_then(|id| self.index_of(id));
        let cover = lifted.map(|ix| {
            let (x, width) = sampled.tabs[ix];
            (shift + x, shift + x + width)
        });
        let hovered_ix = self
            .tabs
            .iter()
            .position(|t| ui.is_hovered(HitId::Tab(TabHit::Item(t.data.id))));

        let ghosts: Vec<AppNode> = sampled
            .ghosts
            .iter()
            .map(|(name, x, width, was_active)| {
                let bg = if *was_active {
                    p.layer
                } else {
                    transparent_black()
                };
                tab_frame(
                    shift + x,
                    *width,
                    cover,
                    tab_face(
                        p,
                        *was_active,
                        bg,
                        TAB_LABEL_TRAILING_BARE,
                        name.clone(),
                    ),
                )
            })
            .collect();

        let tab_nodes: Vec<AppNode> = (0..self.tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted)
            .map(|ix| {
                let (x, width) = sampled.tabs[ix];
                tab_frame(
                    shift + x,
                    width,
                    cover.filter(|_| Some(ix) != lifted),
                    render_tab(&self.tabs, ix, self.active, hovered_ix, p, ui),
                )
            })
            .collect();

        let scroll_back = self.buttons.then(|| self.scroll_button(false, p, ui));
        let scroll_fwd = self.buttons.then(|| self.scroll_button(true, p, ui));
        let new_tab = icon_button(
            p,
            HitId::Tab(TabHit::New),
            shortcut_tooltip("New tab", AppCommand::NewTab.into()),
            (ADD_GLYPH, BUTTON_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            true,
            ui,
        )
        .ml(self.new_tab_gap())
        .mt(BUTTON_TOP);

        row()
            .hit(HitId::Tab(TabHit::Strip))
            .h(TAB_HEIGHT)
            .when_some(scroll_back, |this, btn| this.child(btn))
            .child(
                box_()
                    .hit(HitId::Tab(TabHit::List))
                    .flex_none()
                    .h_full()
                    .w(viewport)
                    .overflow_hidden()
                    .children(ghosts)
                    .children(tab_nodes),
            )
            .when_some(scroll_fwd, |this, btn| this.child(btn))
            .child(new_tab)
    }

    fn scroll_button(&self, forward: bool, p: Palette, ui: &mut AppUi) -> AppNode {
        let target = self.scroll;
        let enabled = if forward {
            target < self.max_scroll() - 0.1
        } else {
            target > 0.1
        };
        let (hit, tip, glyph, left, right) = if forward {
            let tip = "Scroll tab list forward";
            (TabHit::ScrollRight, tip, CARET_RIGHT, SCROLL_INNER_PAD, SCROLL_OUTER_PAD)
        } else {
            let tip = "Scroll tab list backward";
            (TabHit::ScrollLeft, tip, CARET_LEFT, SCROLL_OUTER_PAD, SCROLL_INNER_PAD)
        };
        let hit = HitId::Tab(hit);
        let button = icon_button(
            p,
            hit,
            tip,
            (glyph, SCROLL_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            enabled,
            ui,
        )
        .mt(BUTTON_TOP)
        .when(enabled, |this| this.text_color(p.text_secondary));
        box_()
            .flex_none()
            .h_full()
            .pl(left)
            .pr(right)
            .child(button)
    }
}

fn slide(key: AnimKey, seed: f32, target: f32, snap: bool, ui: &mut AppUi) -> (f32, bool) {
    let duration = if snap { Duration::ZERO } else { SLIDE_DURATION };
    let sample = ui.animate_f32_from(key, seed, target, duration, SLIDE_EASING);
    (sample.value.round(), sample.status == AnimStatus::Running)
}

fn visible(x: f32, right: f32, cover: Option<(f32, f32)>) -> (f32, f32) {
    match cover {
        Some((left, end)) if left < right && x < end => {
            if left <= x {
                (end.min(right), right)
            } else {
                (x, left.min(right))
            }
        }
        _ => (x, right),
    }
}

fn tab_frame(x: f32, width: f32, cover: Option<(f32, f32)>, content: AppNode) -> AppNode {
    let (left, right) = visible(x, x + width, cover);
    box_()
        .absolute()
        .top(0.)
        .left(left)
        .w((right - left).max(0.))
        .h(TAB_HEIGHT)
        .overflow_hidden()
        .child(
            box_()
                .absolute()
                .top(0.)
                .left(x - left)
                .w(width)
                .h_full()
                .child(content),
        )
}

fn render_tab(
    tabs: &[Tab],
    ix: usize,
    active_ix: usize,
    hovered_ix: Option<usize>,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let tab = &tabs[ix];
    let id = tab.data.id;
    let active = ix == active_ix;
    let is_hovered = hovered_ix == Some(ix);
    let separator =
        !active && ix + 1 != active_ix && hovered_ix != Some(ix) && hovered_ix != Some(ix + 1);
    let dirty = tab.data.is_dirty();
    let show_close = is_hovered || (active && !dirty);
    let show_dot = dirty && !show_close;
    let trailing = if show_close || show_dot {
        TAB_LABEL_TRAILING
    } else {
        TAB_LABEL_TRAILING_BARE
    };
    let inset = if active { 1. } else { 0. };
    let name = document::display_name(tab.data.path());
    let tip: SharedStr = tab
        .data
        .path()
        .map_or_else(|| name.clone(), |path| path.display().to_string())
        .into();

    let hit = HitId::Tab(TabHit::Item(id));
    let face_bg = if active {
        p.layer
    } else {
        ui.brush(
            hit,
            Brushes {
                rest: transparent_black(),
                hover: p.layer_hover,
                pressed: p.layer_hover,
            },
            true,
        )
    };

    let close_node = show_close.then(|| {
        icon_button(
            p,
            HitId::Tab(TabHit::Close(id)),
            shortcut_tooltip("Close tab", AppCommand::CloseTab.into()),
            (CLOSE_GLYPH, BUTTON_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            true,
            ui,
        )
        .absolute()
        .top(CLOSE_BUTTON_TOP - inset)
        .right(CLOSE_BUTTON_RIGHT - inset)
    });

    box_()
        .hit(hit)
        .tooltip(tip)
        .cursor(CursorStyle::Arrow)
        .size_full()
        .overflow_hidden()
        .when(active, |this| {
            this.border_t(1.)
                .border_l(1.)
                .border_r(1.)
                .border_color(p.stroke)
                .rounded_t(OVERLAY_RADIUS)
        })
        .child(tab_face(p, active, face_bg, trailing, name))
        .when(show_dot, |this| this.child(unsaved_dot(p, inset)))
        .when(separator, |this| this.child(tab_separator(p)))
        .when_some(close_node, |this, btn| this.child(btn))
}

fn tab_face(
    p: Palette,
    active: bool,
    bg: crate::geom::Rgba,
    trailing: f32,
    name: impl Into<SharedStr>,
) -> AppNode {
    row()
        .items_center()
        .size_full()
        .pl(TAB_LABEL_INSET)
        .pr(trailing)
        .font_family(SMALL_FONT)
        .text_size(12.)
        .bg(bg)
        .when(active, |this| {
            this.rounded_t(OVERLAY_RADIUS - 1.)
                .text_color(p.text_primary)
                .font_semibold()
        })
        .when(!active, |this| {
            this.rounded_t(OVERLAY_RADIUS)
                .text_color(p.text_secondary)
        })
        .child(
            box_()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .nowrap()
                .child(text(name)),
        )
}

fn unsaved_dot(p: Palette, inset: f32) -> AppNode {
    box_()
        .absolute()
        .top(UNSAVED_DOT_TOP - inset)
        .right(UNSAVED_DOT_RIGHT - inset)
        .size(UNSAVED_DOT)
        .rounded_full()
        .bg(p.text_tertiary)
}

fn tab_separator(p: Palette) -> AppNode {
    box_()
        .absolute()
        .right(0.)
        .top(8.)
        .w(1.)
        .h(16.)
        .bg(p.divider)
}

#[cfg(test)]
mod tests {
    use crate::window::EDITOR_PADDINGS;

    use super::*;

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

    #[test]
    fn visible_clips_covered_tab_edges() {
        assert_eq!(visible(0., 100., None), (0., 100.));
        assert_eq!(visible(0., 100., Some((120., 200.))), (0., 100.));
        assert_eq!(visible(0., 100., Some((0., 40.))), (40., 100.));
        assert_eq!(visible(0., 100., Some((60., 140.))), (0., 60.));
        assert_eq!(visible(0., 100., Some((-10., 120.))), (100., 100.));
    }
}
