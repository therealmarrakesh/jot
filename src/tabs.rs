use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::color::transparent_black;
use crate::document::{self, Format};
use crate::editor::{Editor, Rope};
use crate::geom::Edges;
use crate::ids::{AnimKey, HitId, Node, UiState};
use crate::platform::{CursorStyle, TimerHandle};
use crate::theme::Palette;
use crate::types::SharedStr;
use crate::ui::controls::{CONTROL_RADIUS, IconButtonStyle, OVERLAY_RADIUS, SMALL_FONT, icon_button};
use crate::ui::{AnimStatus, Brushes, Easing, box_, row, text};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TabId(u64);

impl TabId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

pub(crate) const TAB_WIDTH: f32 = 240.;
pub(crate) const TAB_MIN_WIDTH: f32 = 80.;
pub(crate) const TAB_HEIGHT: f32 = 32.;
const TAB_LABEL_INSET: f32 = 8.;
const TAB_LABEL_TRAILING: f32 = 44.;
const TAB_LABEL_TRAILING_BARE: f32 = 8.;
const UNSAVED_DOT: f32 = 6.;
const UNSAVED_DOT_TOP: f32 = 13.;
const UNSAVED_DOT_RIGHT: f32 = 19.;
const CLOSE_BUTTON_TOP: f32 = 5.;
const CLOSE_BUTTON_RIGHT: f32 = 6.;
pub(crate) const HEADER_WIDTH: f32 = 5.;
const LIST_PAD: f32 = 4.;
pub(crate) const STRIP_LEFT: f32 = HEADER_WIDTH + LIST_PAD;
pub(crate) const STRIP_TOP: f32 = 10.;
pub(crate) const CAPTION_BUTTON_WIDTH: f32 = 46.;
pub(crate) const CAPTION_WIDTH: f32 = 3. * CAPTION_BUTTON_WIDTH;
const DRAG_RESERVE: f32 = 43.;
const STRIP_RESERVED: f32 = CAPTION_WIDTH + DRAG_RESERVE + NEW_TAB_GAP_SCROLLING + BUTTON_WIDTH;
pub(crate) const TITLE_BAR_MIN_WIDTH: f32 = HEADER_WIDTH + 2. * LIST_PAD + STRIP_RESERVED;
const SCROLL_AREA: f32 = SCROLL_OUTER_PAD + BUTTON_WIDTH + SCROLL_INNER_PAD;
const SCROLL_OUTER_PAD: f32 = 8.;
const SCROLL_INNER_PAD: f32 = 3.;
pub(crate) const SCROLL_STEP: f32 = 50.;
pub(crate) const SCROLL_REPEAT_DELAY: Duration = Duration::from_millis(250);
pub(crate) const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const WHEEL_SHARE: f32 = 0.15;
const BUTTON_WIDTH: f32 = 32.;
const BUTTON_HEIGHT: f32 = 24.;
const BUTTON_TOP: f32 = 6.;
const SCROLL_GLYPH_SIZE: f32 = 8.;
const NEW_TAB_GAP: f32 = 5.;
const NEW_TAB_GAP_SCROLLING: f32 = 3.;
const SLIDE_DURATION: Duration = Duration::from_millis(200);
const SLIDE_EASING: Easing = Easing::CubicBezier {
    x1: 1. / 3.,
    y1: 2. / 3.,
    x2: 2. / 3.,
    y2: 1.,
};
const REORDER_THRESHOLD: f32 = 16.;
const CARET_LEFT: &str = "\u{EDD9}";
const CARET_RIGHT: &str = "\u{EDDA}";
const ADD_GLYPH: &str = "\u{E710}";
pub(crate) const CLOSE_GLYPH: &str = "\u{E711}";
const BUTTON_GLYPH_SIZE: f32 = 12.;

#[derive(Clone, Copy, Default)]
pub(crate) struct SlotKey(usize);

#[derive(Clone, Debug)]
pub(crate) struct PendingSave {
    pub(crate) path: PathBuf,
    pub(crate) format: Format,
    pub(crate) wf_gen: crate::files::WorkflowGen,
}

#[derive(Default)]
pub(crate) enum TabSaveState {
    #[default]
    Idle,
    InFlight {
        pending: Option<PendingSave>,
    },
}

pub(crate) struct TabData {
    pub(crate) id: TabId,
    pub(crate) editor: Editor,
    pub(crate) zoom: f32,
    pub(crate) path: Option<PathBuf>,
    pub(crate) path_key: Option<String>,
    pub(crate) format: Format,
    pub(crate) save_state: TabSaveState,
}

impl TabData {
    pub(crate) fn is_dirty(&self) -> bool {
        if self.path.is_some() {
            !self.editor.is_clean()
        } else {
            !self.editor.is_clean() && self.editor.text().len() > 0
        }
    }

    pub(crate) fn create(
        content: Option<(PathBuf, document::Loaded)>,
        word_wrap: bool,
        paddings: Edges<f32>,
    ) -> Self {
        let (path, path_key, text, format) = match content {
            Some((path, loaded)) => {
                let key = if loaded.path_key.is_empty() {
                    document::quick_path_key(&path)
                } else {
                    loaded.path_key
                };
                (Some(path), Some(key), loaded.text, loaded.format)
            }
            None => (None, None, Rope::new(), Format::default()),
        };
        let editor = Editor::multi_line()
            .with_soft_wrap(word_wrap)
            .with_rope(text)
            .with_paddings(paddings);
        Self {
            id: TabId::next(),
            editor,
            zoom: 1.,
            path,
            path_key,
            format,
            save_state: TabSaveState::Idle,
        }
    }
}

pub(crate) struct Tab {
    pub(crate) data: TabData,
    pub(crate) slot: SlotKey,
}

impl Tab {
    pub(crate) fn adopt(data: TabData) -> Self {
        Self {
            data,
            slot: SlotKey::default(),
        }
    }
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

pub(crate) struct StripFrame {
    shift: f32,
    viewport: f32,
    tabs: Vec<(f32, f32)>,
    ghosts: Vec<(SharedStr, f32, f32, bool)>,
}

pub(crate) struct TabStrip {
    pub(crate) extent: f32,
    pub(crate) scroll: f32,
    pub(crate) snap_layout: bool,
    pub(crate) snap_scroll: bool,
    next_key: usize,
    column: f32,
    pub(crate) width: f32,
    pub(crate) overflow: bool,
    buttons: bool,
    frozen: Option<f32>,
    ghosts: Vec<Ghost>,
    drag: Option<Drag>,
    lifted: Option<TabId>,
    pub(crate) repeat: TimerHandle,
}

impl TabStrip {
    pub(crate) fn new() -> Self {
        Self {
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
        }
    }

    pub(crate) fn measure(&mut self, count: usize) {
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

    pub(crate) fn next_slot(&mut self) -> SlotKey {
        self.next_key += 1;
        SlotKey(self.next_key)
    }

    pub(crate) fn snap(&mut self) {
        self.snap_layout = true;
        self.snap_scroll = true;
    }

    pub(crate) fn layout(&mut self, count: usize) {
        self.measure(count);
        self.extent = count as f32 * self.width;
        self.scroll_to(self.scroll);
    }

    fn scroll_area(&self) -> f32 {
        if self.buttons { SCROLL_AREA } else { 0. }
    }

    pub(crate) fn scroll_viewport(&self) -> f32 {
        self.column - 2. * self.scroll_area()
    }

    pub(crate) fn max_scroll(&self) -> f32 {
        if self.overflow {
            (2. * LIST_PAD + self.extent - self.scroll_viewport()).max(0.)
        } else {
            0.
        }
    }

    pub(crate) fn origin(&self) -> f32 {
        HEADER_WIDTH + self.scroll_area() + LIST_PAD - self.scroll
    }

    fn new_tab_gap(&self) -> f32 {
        if self.buttons {
            NEW_TAB_GAP_SCROLLING
        } else {
            NEW_TAB_GAP
        }
    }

    pub(crate) fn is_frozen(&self) -> bool {
        self.frozen.is_some()
    }

    pub(crate) fn freeze(&mut self, width: f32) {
        self.frozen = Some(width);
    }

    pub(crate) fn thaw(&mut self, count: usize) -> bool {
        if self.frozen.take().is_some() {
            self.layout(count);
            true
        } else {
            false
        }
    }

    pub(crate) fn push_ghost(
        &mut self,
        name: SharedStr,
        key: SlotKey,
        ix: usize,
        width: f32,
        was_active: bool,
    ) {
        self.ghosts.push(Ghost {
            name,
            key,
            x: ix as f32 * width,
            was_active,
        });
    }

    pub(crate) fn depart(&mut self, id: TabId) {
        if self.drag.is_some_and(|drag| drag.id == id) {
            self.drag = None;
        }
        if self.lifted == Some(id) {
            self.lifted = None;
        }
    }

    pub(crate) fn place(&self, x: f32, grab: f32, last: usize) -> (f32, f32, usize) {
        let x = x - self.origin();
        let left = (x - grab).clamp(0., last as f32 * self.width);
        (x, left, slot_index(left, self.width, last))
    }

    pub(crate) fn scroll_to(&mut self, target: f32) {
        self.scroll = target.clamp(0., self.max_scroll()).round();
    }

    pub(crate) fn scroll_by(&mut self, delta: f32) -> bool {
        let before = self.scroll;
        self.scroll_to(before + delta);
        self.scroll != before
    }

    pub(crate) fn reveal(&mut self, left: f32, right: f32) {
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

    pub(crate) fn tab_target(&self, tabs: &[Tab], ix: usize) -> f32 {
        let id = tabs[ix].data.id;
        self.drag
            .filter(|drag| drag.id == id)
            .map_or(ix as f32 * self.width, |drag| drag.left)
    }

    pub(crate) fn reveal_tab(&mut self, tabs: &[Tab], ix: usize) {
        if ix < tabs.len() {
            let left = self.tab_target(tabs, ix);
            self.reveal(left, left + self.width);
        }
    }

    pub(crate) fn drag_tab(
        &mut self,
        tabs: &mut Vec<Tab>,
        active_id: TabId,
        id: TabId,
        x: f32,
        grab: f32,
        start: f32,
    ) -> Option<usize> {
        let from = tabs.iter().position(|t| t.data.id == id)?;
        let origin = self.origin();
        let mut drag = self.drag.filter(|d| d.id == id).unwrap_or(Drag {
            id,
            grab,
            last_move: start - origin,
            left: 0.,
        });
        self.lifted = Some(id);
        let width = self.width;
        let last = tabs.len() - 1;
        let (x, left, to) = self.place(x, drag.grab, last);
        drag.left = left;
        self.snap_scroll = true;
        self.reveal(left, left + width);
        let mut next_active = None;
        if to != from && (x - drag.last_move).abs() > REORDER_THRESHOLD {
            let tab = tabs.remove(from);
            tabs.insert(to, tab);
            next_active = Some(
                tabs.iter()
                    .position(|t| t.data.id == active_id)
                    .unwrap_or(to),
            );
            drag.last_move = x;
        }
        self.drag = Some(drag);
        self.layout(tabs.len());
        next_active
    }

    pub(crate) fn begin_inserted_drag(
        &mut self,
        id: TabId,
        x: f32,
        grab: f32,
        count_before: usize,
    ) -> usize {
        self.measure(count_before + 1);
        let (x, left, ix) = self.place(x, grab, count_before);
        self.drag = Some(Drag {
            id,
            grab,
            last_move: x,
            left,
        });
        self.lifted = Some(id);
        self.snap();
        ix
    }

    pub(crate) fn release_drag(&mut self, count: usize) -> bool {
        if self.drag.take().is_some() {
            self.layout(count);
            true
        } else {
            false
        }
    }

    pub(crate) fn clear_drag(&mut self) {
        self.drag = None;
    }

    pub(crate) fn stop_repeat(&mut self) {
        self.repeat.cancel();
    }

    pub(crate) fn sample(
        &mut self,
        tabs: &[Tab],
        active: usize,
        viewport_width: f32,
        ui: &mut UiState,
    ) -> StripFrame {
        let column = viewport_width - HEADER_WIDTH - STRIP_RESERVED;
        if column != self.column {
            self.column = column;
            self.snap();
            self.layout(tabs.len());
            self.reveal_tab(tabs, active);
        }
        let snap = std::mem::take(&mut self.snap_layout);
        let snap_scroll = std::mem::take(&mut self.snap_scroll);
        let (scroll, _) = slide(AnimKey::TabScroll, 0., self.scroll, snap_scroll, ui);
        let (extent, _) = slide(AnimKey::TabExtent, TAB_WIDTH, self.extent, snap, ui);
        let mut right = 0.;
        let mut lifted_moving = false;
        let mut sampled_tabs = Vec::with_capacity(tabs.len());
        for (ix, tab) in tabs.iter().enumerate() {
            let id = tab.data.id;
            let dragged = self.drag.is_some_and(|drag| drag.id == id);
            let (x, moving) = slide(
                AnimKey::TabX(tab.slot.0),
                right,
                self.tab_target(tabs, ix),
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

    pub(crate) fn render(
        &self,
        tabs: &[Tab],
        active: usize,
        sampled: &StripFrame,
        p: Palette,
        ui: &mut UiState,
    ) -> Node {
        let shift = sampled.shift;
        let viewport = sampled.viewport;
        let lifted = self
            .lifted
            .and_then(|id| tabs.iter().position(|t| t.data.id == id));
        let cover = lifted.map(|ix| {
            let (x, width) = sampled.tabs[ix];
            (shift + x, shift + x + width)
        });
        let hovered_ix = tabs.iter().position(|t| ui.is_hovered(HitId::Tab(t.data.id)));

        let ghosts: Vec<Node> = sampled
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

        let tab_nodes: Vec<Node> = (0..tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted)
            .map(|ix| {
                let (x, width) = sampled.tabs[ix];
                tab_frame(
                    shift + x,
                    width,
                    cover.filter(|_| Some(ix) != lifted),
                    render_tab(tabs, ix, active, hovered_ix, p, ui),
                )
            })
            .collect();

        let scroll_back = self.buttons.then(|| self.scroll_button(false, p, ui));
        let scroll_fwd = self.buttons.then(|| self.scroll_button(true, p, ui));
        let new_tab = strip_button(
            p,
            HitId::NewTab,
            "New tab (Ctrl+N)",
            ADD_GLYPH,
            BUTTON_GLYPH_SIZE,
            true,
            ui,
        )
        .ml(self.new_tab_gap())
        .mt(BUTTON_TOP);

        row()
            .hit(HitId::TabStrip)
            .relative()
            .h(TAB_HEIGHT)
            .when_some(scroll_back, |this, btn| this.child(btn))
            .child(
                box_()
                    .hit(HitId::TabList)
                    .relative()
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

    fn scroll_button(&self, forward: bool, p: Palette, ui: &mut UiState) -> Node {
        let target = self.scroll;
        let enabled = if forward {
            target < self.max_scroll() - 0.1
        } else {
            target > 0.1
        };
        let (hit, tip, glyph) = if forward {
            (
                HitId::TabScrollRight,
                "Scroll tab list forward",
                CARET_RIGHT,
            )
        } else {
            (
                HitId::TabScrollLeft,
                "Scroll tab list backward",
                CARET_LEFT,
            )
        };
        let (left, right) = if forward {
            (SCROLL_INNER_PAD, SCROLL_OUTER_PAD)
        } else {
            (SCROLL_OUTER_PAD, SCROLL_INNER_PAD)
        };
        let button = strip_button(p, hit, tip, glyph, SCROLL_GLYPH_SIZE, enabled, ui)
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

fn slide(key: AnimKey, seed: f32, target: f32, snap: bool, ui: &mut UiState) -> (f32, bool) {
    let duration = if snap { Duration::ZERO } else { SLIDE_DURATION };
    let sample = ui.animate_f32_from(key, seed, target, duration, SLIDE_EASING);
    (sample.value.round(), sample.status == AnimStatus::Running)
}

fn slot_index(left: f32, width: f32, last: usize) -> usize {
    ((left / width - 0.5).ceil().max(0.) as usize).min(last)
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

fn tab_frame(x: f32, width: f32, cover: Option<(f32, f32)>, content: Node) -> Node {
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
    ui: &mut UiState,
) -> Node {
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
    let name = document::display_name(tab.data.path.as_deref());
    let tip: SharedStr = tab
        .data
        .path
        .as_ref()
        .map_or_else(|| name.clone(), |path| path.display().to_string())
        .into();

    let hit = HitId::Tab(id);
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
        strip_button(
            p,
            HitId::TabClose(id),
            "Close tab (Ctrl+W)",
            CLOSE_GLYPH,
            BUTTON_GLYPH_SIZE,
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
        .relative()
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
    bg: crate::color::Rgba,
    trailing: f32,
    name: impl Into<SharedStr>,
) -> Node {
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

fn unsaved_dot(p: Palette, inset: f32) -> Node {
    box_()
        .absolute()
        .top(UNSAVED_DOT_TOP - inset)
        .right(UNSAVED_DOT_RIGHT - inset)
        .size(UNSAVED_DOT)
        .rounded_full()
        .bg(p.text_tertiary)
}

fn strip_button(
    p: Palette,
    hit: HitId,
    tip: &'static str,
    glyph: &'static str,
    glyph_size: f32,
    enabled: bool,
    ui: &mut UiState,
) -> Node {
    icon_button(
        p,
        hit,
        tip,
        glyph,
        IconButtonStyle {
            glyph_size,
            width: BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
            radius: CONTROL_RADIUS,
        },
        enabled,
        ui,
    )
}

fn tab_separator(p: Palette) -> Node {
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
    use super::*;

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
    fn visible_clips_covered_tab_edges() {
        assert_eq!(visible(0., 100., None), (0., 100.));
        assert_eq!(visible(0., 100., Some((120., 200.))), (0., 100.));
        assert_eq!(visible(0., 100., Some((0., 40.))), (40., 100.));
        assert_eq!(visible(0., 100., Some((60., 140.))), (0., 60.));
        assert_eq!(visible(0., 100., Some((-10., 120.))), (100., 100.));
    }

    #[test]
    fn strip_measure_and_reveal_handle_fit_and_overflow() {
        let mut strip = TabStrip::new();
        strip.column = 600.;
        strip.measure(2);
        assert_eq!(strip.width, TAB_WIDTH);
        assert!(!strip.overflow);
        assert!(!strip.buttons);

        strip.measure(10);
        assert!(strip.overflow);
        assert!(strip.buttons);
        assert_eq!(strip.width, TAB_MIN_WIDTH);

        strip.extent = 10. * strip.width;
        strip.reveal(9. * strip.width, 10. * strip.width);
        assert!(strip.scroll > 0.);
        assert!(strip.scroll <= strip.max_scroll());

        strip.reveal(0., strip.width);
        assert_eq!(strip.scroll, 0.);
    }
}
