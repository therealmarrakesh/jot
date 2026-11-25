//! A window's tabs and its strip: tabs share the width between a minimum and a maximum, then
//! the strip scrolls; tabs slide into place, a closed one shrinks away as a ghost, and a
//! dragged one follows the pointer over the others.

use std::ops::Index;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::command::{AppCommand, shortcut_tooltip};
use crate::document::{self, DocPath, Format, PathKey};
use crate::editor::{DocOptions, Editor};
use crate::geom::{Edges, Point, Rgba, size, transparent_black};
use crate::platform::CursorStyle;
use crate::text::Rope;
use crate::theme::{OVERLAY_RADIUS, Palette, SMALL_FONT};
use crate::types::SharedStr;
use crate::ui::{
    AnimKey, AnimStatus, AppNode, AppUi, Brushes, Curve, Easing, EditorSlot, HitId, TabHit, TabId,
    box_, row, text,
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
/// How far a dragged tab moves before it can swap places again.
const REORDER_THRESHOLD: f32 = 16.;

const SCROLL_STEP: f32 = 50.;
const SCROLL_REPEAT_DELAY: Duration = Duration::from_millis(250);
const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
const WHEEL_SHARE: f32 = 0.15;

const TAB_LABEL_INSET: f32 = 8.;
const TAB_LABEL_TRAILING: f32 = 44.;
const TAB_LABEL_TRAILING_BARE: f32 = 8.;
const UNSAVED_DOT: f32 = 6.;
const UNSAVED_DOT_TOP: f32 = 13.;
const UNSAVED_DOT_RIGHT: f32 = 19.;
const CLOSE_BUTTON_TOP: f32 = 5.;
const CLOSE_BUTTON_RIGHT: f32 = 6.;
const SCROLL_GLYPH_SIZE: f32 = 8.;
const BUTTON_GLYPH_SIZE: f32 = 12.;
const CLOSE_GLYPH: &str = "\u{E711}";
const CARET_LEFT: &str = "\u{EDD9}";
const CARET_RIGHT: &str = "\u{EDDA}";
const ADD_GLYPH: &str = "\u{E710}";
const SLIDE: Curve = Curve::new(
    Duration::from_millis(200),
    Easing::CubicBezier {
        x1: 1. / 3.,
        y1: 2. / 3.,
        x2: 2. / 3.,
        y2: 1.,
    },
);

// ---- 1. Tabs --------------------------------------------------------------------------------

pub(crate) struct TabData {
    pub(crate) id: TabId,
    pub(crate) editor: Editor,
    pub(crate) zoom: f32,
    pub(crate) doc_path: Option<DocPath>,
    pub(crate) format: Format,
}

impl TabData {
    pub(crate) fn create(
        content: Option<document::Loaded>,
        word_wrap: bool,
        paddings: Edges<f32>,
    ) -> Self {
        let (doc_path, text, format) = match content {
            Some(loaded) => (Some(loaded.doc_path), loaded.text, loaded.format),
            None => (None, Rope::new(), Format::default()),
        };
        let id = TabId::next();
        let options = DocOptions {
            tab: id,
            soft_wrap: word_wrap,
            paddings,
        };
        Self {
            id,
            editor: Editor::document(text, options),
            zoom: 1.,
            doc_path,
            format,
        }
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.doc_path.as_ref().map(DocPath::as_path)
    }

    fn path_key(&self) -> Option<&PathKey> {
        self.doc_path.as_ref().map(DocPath::key)
    }

    /// Whether it has changes to save: an edited file, or an untitled document with text.
    pub(crate) fn is_dirty(&self) -> bool {
        let m = self.editor.model();
        !m.is_clean() && (self.doc_path.is_some() || m.text().len() > 0)
    }
}

/// A tab: its data, and the key its slides run under (kept when the data is replaced).
pub(crate) struct Tab {
    pub(crate) data: TabData,
    pub(crate) slot: SlotKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotKey(usize);

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

/// A tab being dragged: held `grab` from its left, at `left`, and where it last swapped.
#[derive(Clone, Copy)]
struct Drag {
    id: TabId,
    grab: f32,
    last_move: f32,
    left: f32,
}

// ---- 2. The set -----------------------------------------------------------------------------

pub(crate) struct TabSet {
    tabs: Vec<Tab>,
    active: usize,
    extent: f32,
    scroll: f32,
    /// The next frame jumps to the layout, or to the scroll, instead of sliding.
    snap_layout: bool,
    snap_scroll: bool,
    next_key: usize,
    column: f32,
    width: f32,
    /// The tabs don't fit; whether there's room for the scroll arrows too.
    overflow: bool,
    buttons: bool,
    /// The width held while the pointer stays on the strip after a close.
    frozen: Option<f32>,
    ghosts: Vec<Ghost>,
    drag: Option<Drag>,
    /// The tab drawn over the others: the dragged one, until it settles.
    lifted: Option<TabId>,
    /// While a scroll arrow is held: when it next scrolls, and by how much.
    repeat: Option<(Instant, f32)>,
}

impl Index<usize> for TabSet {
    type Output = Tab;

    fn index(&self, index: usize) -> &Tab {
        &self.tabs[index]
    }
}

impl TabSet {
    pub(crate) fn new(initial: TabData) -> Self {
        Self {
            tabs: vec![Tab {
                data: initial,
                slot: SlotKey(1),
            }],
            active: 0,
            extent: TAB_WIDTH,
            scroll: 0.,
            snap_layout: false,
            snap_scroll: false,
            next_key: 1,
            column: f32::INFINITY,
            width: TAB_WIDTH,
            overflow: false,
            buttons: false,
            frozen: None,
            ghosts: Vec::new(),
            drag: None,
            lifted: None,
            repeat: None,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Tab> {
        self.tabs.iter()
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
        self.active().id
    }

    pub(crate) fn active_slot(&self) -> EditorSlot {
        EditorSlot::Tab(self.active_id())
    }

    pub(crate) fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.data.id == id)
    }

    pub(crate) fn editor(&self, id: TabId) -> Option<&Editor> {
        Some(&self.tabs[self.index_of(id)?].data.editor)
    }

    pub(crate) fn editor_mut(&mut self, id: TabId) -> Option<&mut Editor> {
        let ix = self.index_of(id)?;
        Some(&mut self.tabs[ix].data.editor)
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
            .position(|t| t.data.path_key() == Some(key))
    }

    pub(crate) fn is_blank(&self, ix: usize) -> bool {
        let data = &self.tabs[ix].data;
        data.doc_path.is_none() && data.editor.model().text().len() == 0
    }

    pub(crate) fn has_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.data.is_dirty())
    }

    pub(crate) fn dirty_tabs(&self) -> Vec<TabId> {
        let dirty = self.tabs.iter().filter(|t| t.data.is_dirty());
        dirty.map(|t| t.data.id).collect()
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
        (data.doc_path, data.format) = (Some(doc_path), format);
        true
    }
}

// ---- 3. Layout and scrolling ----------------------------------------------------------------

/// The slot a tab whose left edge is at `left` takes: the one its middle is over.
fn slot_index(left: f32, width: f32, last: usize) -> usize {
    ((left / width - 0.5).ceil().max(0.) as usize).min(last)
}

impl TabSet {
    /// Works out the tabs' width for `count` of them, and whether they overflow the strip.
    fn measure_for(&mut self, count: usize) {
        let alone = count <= 1;
        let count = count.max(1) as f32;
        let room = (self.column - 2. * LIST_PAD).max(0.);
        let min = if alone { 0. } else { TAB_MIN_WIDTH };
        self.width = self.frozen.unwrap_or((room / count).clamp(min, TAB_WIDTH));
        self.overflow = !alone && self.width * count > room;
        let between_arrows = room - 2. * SCROLL_AREA;
        self.buttons = self.overflow && between_arrows >= TAB_MIN_WIDTH;
        // Overflowing, the tabs shrink to fit between the arrows (or all the room).
        if self.overflow && self.frozen.is_none() {
            self.width = if self.buttons {
                (between_arrows / count).clamp(TAB_MIN_WIDTH, TAB_WIDTH)
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

    fn snap(&mut self) {
        (self.snap_layout, self.snap_scroll) = (true, true);
    }

    fn scroll_area(&self) -> f32 {
        if self.buttons { SCROLL_AREA } else { 0. }
    }

    fn scroll_viewport(&self) -> f32 {
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

    /// Where tab `ix` belongs: its slot, or where it's dragged.
    fn tab_target(&self, ix: usize) -> f32 {
        let id = self.tabs[ix].data.id;
        match self.drag {
            Some(drag) if drag.id == id => drag.left,
            _ => ix as f32 * self.width,
        }
    }

    /// The pointer at `strip_x`, holding a tab `grab` in: where it is along the list, where the
    /// tab's left edge goes (kept within the list), and the slot it's over.
    fn place(&self, strip_x: f32, grab: f32, last: usize) -> (f32, f32, usize) {
        let x = strip_x - self.origin_in_strip();
        let left = (x - grab).clamp(0., last as f32 * self.width);
        (x, left, slot_index(left, self.width, last))
    }

    fn scroll_to(&mut self, target: f32) {
        self.scroll = target.clamp(0., self.max_scroll()).round();
    }

    fn scroll_by(&mut self, delta: f32) -> bool {
        let before = self.scroll;
        self.scroll_to(before + delta);
        self.scroll != before
    }

    /// Scrolls the least that shows `left..right` of the list.
    fn reveal(&mut self, left: f32, right: f32) {
        if !self.overflow {
            self.scroll = 0.;
            self.snap_scroll = true;
            return;
        }
        let (viewport, current) = (self.scroll_viewport(), self.scroll);
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
}

// ---- 4. Changing the set --------------------------------------------------------------------

impl TabSet {
    fn next_slot(&mut self) -> SlotKey {
        self.next_key += 1;
        SlotKey(self.next_key)
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

    /// Puts `data` in tab `ix`'s place (it keeps the slot, so it doesn't slide).
    pub(crate) fn replace(&mut self, ix: usize, data: TabData) {
        let replaced = std::mem::replace(&mut self.tabs[ix].data, data);
        self.depart(replaced.id);
    }

    pub(crate) fn restore(&mut self, data: TabData, ix: usize) -> usize {
        let slot = self.next_slot();
        let ix = ix.min(self.tabs.len());
        self.tabs.insert(ix, Tab { data, slot });
        self.layout();
        ix
    }

    /// Closes tab `ix` into a ghost; `freeze_if_hovering` holds the width till the pointer goes.
    pub(crate) fn remove(&mut self, ix: usize, freeze_if_hovering: bool) -> Removal {
        if self.tabs.len() == 1 {
            return Removal::LastTab;
        }
        let width = self.width;
        let name = self.tab_name(ix).into();
        let tab = self.tabs.remove(ix);
        self.depart(tab.data.id);
        if ix < self.tabs.len() && freeze_if_hovering {
            self.frozen = Some(width);
        }
        self.ghosts.push(Ghost {
            name,
            key: tab.slot,
            x: ix as f32 * width,
            was_active: ix == self.active,
        });
        self.settle(tab.data.id, ix)
    }

    /// Takes tab `id` out (to another window), without a ghost; even the last one.
    pub(crate) fn take(&mut self, id: TabId) -> Option<(TabData, Removal)> {
        let ix = self.index_of(id)?;
        let tab = self.tabs.remove(ix);
        self.depart(id);
        let removal = if self.tabs.is_empty() {
            Removal::LastTab
        } else {
            self.settle(id, ix)
        };
        Some((tab.data, removal))
    }

    /// After tab `id` left from `ix`: the same tab stays active, or the next if it was this one.
    fn settle(&mut self, id: TabId, ix: usize) -> Removal {
        let closed_active = ix == self.active;
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.layout();
        Removal::Removed {
            id,
            closed_active,
            next_active: self.active,
        }
    }

    /// A tab that left stops being dragged or drawn on top.
    fn depart(&mut self, id: TabId) {
        if self.drag.is_some_and(|drag| drag.id == id) {
            self.drag = None;
        }
        if self.lifted == Some(id) {
            self.lifted = None;
        }
    }

    /// A tab dragged in from another window: it takes the slot under the pointer, still dragged.
    pub(crate) fn insert_dragged(&mut self, data: TabData, strip_x: f32, grab: f32) -> usize {
        let (slot, id, before) = (self.next_slot(), data.id, self.tabs.len());
        self.measure_for(before + 1);
        let (x, left, ix) = self.place(strip_x, grab, before);
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

    /// A dragged tab goes back to `ix`; where it ended up.
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

    /// Tab `id` follows the pointer, swapping into the slot it's over once it's moved far enough.
    pub(crate) fn drag(&mut self, id: TabId, strip_x: f32, grab: f32, strip_start: f32) {
        let Some(from) = self.index_of(id) else {
            return;
        };
        let active = self.active_id();
        let fresh = Drag {
            id,
            grab,
            last_move: strip_start - self.origin_in_strip(),
            left: 0.,
        };
        let mut drag = self.drag.filter(|d| d.id == id).unwrap_or(fresh);
        self.lifted = Some(id);
        let (x, left, to) = self.place(strip_x, drag.grab, self.tabs.len() - 1);
        drag.left = left;
        self.snap_scroll = true;
        self.reveal(left, left + self.width);
        if to != from && (x - drag.last_move).abs() > REORDER_THRESHOLD {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            self.active = self.index_of(active).unwrap_or(to);
            drag.last_move = x;
        }
        self.drag = Some(drag);
        self.layout();
    }

    pub(crate) fn release_drag(&mut self) -> bool {
        let dragged = self.drag.take().is_some();
        if dragged {
            self.layout();
        }
        dragged
    }

    /// The pointer left the strip: a width held after a close is let go; whether there was one.
    pub(crate) fn thaw_if_frozen(&mut self) -> bool {
        let frozen = self.frozen.take().is_some();
        if frozen {
            self.layout();
        }
        frozen
    }
}

// ---- 5. Scroll arrows and the wheel ---------------------------------------------------------

impl TabSet {
    /// A press on a scroll arrow scrolls a step, and keeps scrolling while it's held.
    pub(crate) fn press_arrow(&mut self, right: bool, now: Instant) {
        let step = if right { SCROLL_STEP } else { -SCROLL_STEP };
        if self.scroll_by(step) {
            self.repeat = Some((now + SCROLL_REPEAT_DELAY, step));
        }
    }

    pub(crate) fn release_arrow(&mut self) {
        self.repeat = None;
    }

    /// A wheel turn over the strip scrolls it, if it overflows; whether it did.
    pub(crate) fn wheel(&mut self, lines: Point) -> bool {
        if !self.overflow {
            return false;
        }
        let notches = if lines.y != 0. { lines.y } else { lines.x };
        self.scroll_by(-notches * WHEEL_SHARE * self.scroll_viewport());
        true
    }

    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let Some((_, step)) = self.repeat.filter(|&(at, _)| at <= now) else {
            return false;
        };
        let moved = self.scroll_by(step);
        self.repeat = moved.then_some((now + SCROLL_REPEAT_INTERVAL, step));
        true
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.repeat.map(|(at, _)| at)
    }
}

// ---- 6. Drawing -----------------------------------------------------------------------------

/// Where this frame draws the strip: the list's shift and width, the tabs, and shrinking ghosts.
pub(crate) struct StripFrame {
    shift: f32,
    viewport: f32,
    tabs: Vec<(f32, f32)>,
    ghosts: Vec<(SharedStr, f32, f32, bool)>,
}

/// Slides `key` to `target` (from `seed`), or jumps with `snap`: whole pixels, and if moving.
fn slide(key: AnimKey, seed: f32, target: f32, snap: bool, ui: &mut AppUi) -> (f32, bool) {
    let curve = if snap {
        Curve {
            duration: Duration::ZERO,
            ..SLIDE
        }
    } else {
        SLIDE
    };
    let sample = ui.anims.animate_f32_from(key, seed, target, curve);
    (sample.value.round(), sample.status == AnimStatus::Running)
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
        // Each tab slides in from where the one before it ends.
        let (mut right, mut lifted_moving) = (0., false);
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (ix, tab) in self.tabs.iter().enumerate() {
            let id = tab.data.id;
            let dragged = self.drag.is_some_and(|d| d.id == id);
            let key = tab.slot.0;
            let (x, moving) = slide(
                AnimKey::TabX(key),
                right,
                self.tab_target(ix),
                snap || dragged,
                ui,
            );
            let (width, _) = slide(AnimKey::TabWidth(key), 0., self.width, snap, ui);
            if self.lifted == Some(id) {
                lifted_moving = moving;
            }
            right = x + width;
            tabs.push((x, width));
        }
        let mut ghosts = Vec::new();
        self.ghosts.retain(|ghost| {
            let key = ghost.key.0;
            let (x, sliding) = slide(AnimKey::TabX(key), ghost.x, ghost.x, false, ui);
            let (width, shrinking) = slide(AnimKey::TabWidth(key), 0., 0., false, ui);
            let moving = sliding || shrinking;
            if moving {
                ghosts.push((ghost.name.clone(), x, width, ghost.was_active));
            }
            moving
        });
        // A dropped tab stays on top until it has slid into place.
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
            tabs,
            ghosts,
        }
    }

    /// The strip: arrows, the list (ghosts, then tabs, the lifted one on top), the new-tab button.
    pub(crate) fn render(&self, frame: &StripFrame, p: Palette, ui: &mut AppUi) -> AppNode {
        let shift = frame.shift;
        let lifted = self.lifted.and_then(|id| self.index_of(id));
        let cover = lifted.map(|ix| {
            let (x, width) = frame.tabs[ix];
            (shift + x, shift + x + width)
        });
        let hovered = self
            .tabs
            .iter()
            .position(|t| ui.is_hovered(HitId::Tab(TabHit::Item(t.data.id))));
        let ghosts: Vec<AppNode> = frame
            .ghosts
            .iter()
            .map(|(name, x, width, was_active)| {
                let bg = if *was_active {
                    p.layer
                } else {
                    transparent_black()
                };
                let face = tab_face(p, *was_active, bg, TAB_LABEL_TRAILING_BARE, name.clone());
                tab_frame(shift + x, *width, cover, face)
            })
            .collect();
        let order = (0..self.tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted);
        let tabs: Vec<AppNode> = order
            .map(|ix| {
                let (x, width) = frame.tabs[ix];
                let under = cover.filter(|_| Some(ix) != lifted);
                let tab = render_tab(&self.tabs, ix, self.active, hovered, p, ui);
                tab_frame(shift + x, width, under, tab)
            })
            .collect();
        let back = self.buttons.then(|| self.scroll_button(false, p, ui));
        let forward = self.buttons.then(|| self.scroll_button(true, p, ui));
        let new_tab = icon_button(
            p,
            HitId::Tab(TabHit::New),
            shortcut_tooltip("New tab", AppCommand::NewTab.into()),
            (ADD_GLYPH, BUTTON_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            true,
            ui,
        )
        .ml(if self.buttons {
            NEW_TAB_GAP_SCROLLING
        } else {
            NEW_TAB_GAP
        })
        .mt(BUTTON_TOP);
        let list = box_()
            .hit(HitId::Tab(TabHit::List))
            .flex_none()
            .h_full()
            .w(frame.viewport)
            .overflow_hidden()
            .children(ghosts)
            .children(tabs);
        row()
            .hit(HitId::Tab(TabHit::Strip))
            .h(TAB_HEIGHT)
            .when_some(back, AppNode::child)
            .child(list)
            .when_some(forward, AppNode::child)
            .child(new_tab)
    }

    fn scroll_button(&self, forward: bool, p: Palette, ui: &mut AppUi) -> AppNode {
        let (enabled, hit, tip, glyph, left, right) = if forward {
            let tip = "Scroll tab list forward";
            let enabled = self.scroll < self.max_scroll() - 0.1;
            (
                enabled,
                TabHit::ScrollRight,
                tip,
                CARET_RIGHT,
                SCROLL_INNER_PAD,
                SCROLL_OUTER_PAD,
            )
        } else {
            let tip = "Scroll tab list backward";
            let enabled = self.scroll > 0.1;
            (
                enabled,
                TabHit::ScrollLeft,
                tip,
                CARET_LEFT,
                SCROLL_OUTER_PAD,
                SCROLL_INNER_PAD,
            )
        };
        let button = icon_button(
            p,
            HitId::Tab(hit),
            tip,
            (glyph, SCROLL_GLYPH_SIZE),
            size(BUTTON_WIDTH, BUTTON_HEIGHT),
            enabled,
            ui,
        )
        .mt(BUTTON_TOP)
        .when(enabled, |this| this.text_color(p.text_secondary));
        box_().flex_none().h_full().pl(left).pr(right).child(button)
    }
}

/// The part of `x..right` not under `cover` (the lifted tab), from whichever side shows.
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

/// A tab's place in the list, `width` wide at `x`, clipped to what `cover` leaves showing.
fn tab_frame(x: f32, width: f32, cover: Option<(f32, f32)>, content: AppNode) -> AppNode {
    let (left, right) = visible(x, x + width, cover);
    let content = box_()
        .absolute()
        .top(0.)
        .left(x - left)
        .w(width)
        .h_full()
        .child(content);
    box_()
        .absolute()
        .top(0.)
        .left(left)
        .w((right - left).max(0.))
        .h(TAB_HEIGHT)
        .overflow_hidden()
        .child(content)
}

/// Tab `ix`: raised if active, else hover fill and separator; a close button, or an unsaved dot.
fn render_tab(
    tabs: &[Tab],
    ix: usize,
    active_ix: usize,
    hovered: Option<usize>,
    p: Palette,
    ui: &mut AppUi,
) -> AppNode {
    let data = &tabs[ix].data;
    let id = data.id;
    let active = ix == active_ix;
    let separator =
        !active && ix + 1 != active_ix && hovered != Some(ix) && hovered != Some(ix + 1);
    let dirty = data.is_dirty();
    let show_close = hovered == Some(ix) || (active && !dirty);
    let show_dot = dirty && !show_close;
    let trailing = if show_close || show_dot {
        TAB_LABEL_TRAILING
    } else {
        TAB_LABEL_TRAILING_BARE
    };
    let inset = if active { 1. } else { 0. };
    let name = document::display_name(data.path());
    let tip: SharedStr = data
        .path()
        .map_or_else(|| name.clone(), |path| path.display().to_string())
        .into();
    let hit = HitId::Tab(TabHit::Item(id));
    let face_bg = if active {
        p.layer
    } else {
        let brushes = Brushes {
            rest: transparent_black(),
            hover: p.layer_hover,
            pressed: p.layer_hover,
        };
        ui.brush(hit, brushes, true)
    };
    let close = show_close.then(|| {
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
        .when(show_dot, |this| {
            let dot = box_()
                .absolute()
                .top(UNSAVED_DOT_TOP - inset)
                .right(UNSAVED_DOT_RIGHT - inset)
                .size(UNSAVED_DOT)
                .rounded_full()
                .bg(p.text_tertiary);
            this.child(dot)
        })
        .when(separator, |this| {
            let divider = box_()
                .absolute()
                .right(0.)
                .top(8.)
                .w(1.)
                .h(16.)
                .bg(p.divider);
            this.child(divider)
        })
        .when_some(close, AppNode::child)
}

fn tab_face(
    p: Palette,
    active: bool,
    bg: Rgba,
    trailing: f32,
    name: impl Into<SharedStr>,
) -> AppNode {
    let (radius, color) = if active {
        (OVERLAY_RADIUS - 1., p.text_primary)
    } else {
        (OVERLAY_RADIUS, p.text_secondary)
    };
    let label = box_()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .nowrap()
        .child(text(name));
    row()
        .items_center()
        .size_full()
        .pl(TAB_LABEL_INSET)
        .pr(trailing)
        .font_family(SMALL_FONT)
        .text_size(12.)
        .bg(bg)
        .rounded_t(radius)
        .text_color(color)
        .when(active, AppNode::font_semibold)
        .child(label)
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
