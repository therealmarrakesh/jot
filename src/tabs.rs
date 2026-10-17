use super::*;
use std::time::Instant;

pub(crate) const TAB_WIDTH: f32 = 240.;
pub(crate) const TAB_HEIGHT: f32 = 32.;
const TAB_LABEL_INSET: f32 = 8.;
const TAB_LABEL_TRAILING: f32 = 44.;
const UNSAVED_DOT: f32 = 6.;
const UNSAVED_DOT_TOP: f32 = 13.;
const UNSAVED_DOT_RIGHT: f32 = 19.;
const CLOSE_BUTTON_TOP: f32 = 5.;
const CLOSE_BUTTON_RIGHT: f32 = 6.;
pub(crate) const STRIP_LEFT: f32 = 9.;
pub(crate) const STRIP_TOP: f32 = 10.;
const SLIDE_DURATION: Duration = Duration::from_millis(200);
const REORDER_THRESHOLD: f32 = 16.;

#[derive(Clone, Copy)]
enum Ink {
    Lead,
    Line,
    Center,
    Trail,
}

impl Ink {
    fn color(self, p: &Palette) -> u32 {
        match self {
            Self::Lead => p.plus_cap_lead,
            Self::Line => p.plus_line,
            Self::Center => p.plus_center,
            Self::Trail => p.plus_cap_trail,
        }
    }
}

const PLUS_SEGMENTS: [(f32, f32, f32, f32, Ink); 9] = [
    (11., 12., 1., 1., Ink::Lead),
    (12., 12., 4., 1., Ink::Line),
    (16., 12., 1., 1., Ink::Center),
    (17., 12., 4., 1., Ink::Line),
    (21., 12., 1., 1., Ink::Trail),
    (16., 7., 1., 1., Ink::Lead),
    (16., 8., 1., 4., Ink::Line),
    (16., 13., 1., 4., Ink::Line),
    (16., 17., 1., 1., Ink::Trail),
];

#[derive(Clone, Copy)]
struct Motion {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Motion {
    fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
            duration: Duration::ZERO,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(self.start).as_secs_f32() / self.duration.as_secs_f32())
            .min(1.)
    }

    fn value(&self, now: Instant) -> f32 {
        let t = self.progress(now);
        let eased = 1. - (1. - t) * (1. - t);
        (self.from + (self.to - self.from) * eased).round()
    }

    fn moving(&self, now: Instant) -> bool {
        self.progress(now) < 1.
    }

    fn slide(&mut self, to: f32, duration: Duration, now: Instant) {
        if self.to == to {
            return;
        }
        *self = Self {
            from: self.value(now),
            to,
            start: now,
            duration,
        };
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Slot {
    x: Motion,
    width: Motion,
}

impl Slot {
    pub(crate) fn at(x: f32, width: f32) -> Self {
        Self {
            x: Motion::at(x),
            width: Motion::at(width),
        }
    }

    fn right(&self, now: Instant) -> f32 {
        self.x.value(now) + self.width.value(now)
    }

    fn moving(&self, now: Instant) -> bool {
        self.x.moving(now) || self.width.moving(now)
    }
}

struct Ghost {
    name: SharedString,
    slot: Slot,
}

#[derive(Clone, Copy)]
struct Drag {
    id: EntityId,
    grab: f32,
    last_move: f32,
}

pub(crate) struct TabStrip {
    trailing: Motion,
    ghosts: Vec<Ghost>,
    drag: Option<Drag>,
    lifted: Option<EntityId>,
}

impl TabStrip {
    pub(crate) fn new() -> Self {
        Self {
            trailing: Motion::at(TAB_WIDTH),
            ghosts: Vec::new(),
            drag: None,
            lifted: None,
        }
    }
}

fn slide_duration(cx: &App) -> Duration {
    if cx.reduce_motion() {
        Duration::ZERO
    } else {
        SLIDE_DURATION
    }
}

fn slot_index(left: f32, last: usize) -> usize {
    ((left / TAB_WIDTH - 0.5).ceil().max(0.) as usize).min(last)
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

fn frame(x: f32, width: f32, cover: Option<(f32, f32)>, content: impl IntoElement) -> Div {
    let (left, right) = visible(x, x + width, cover);
    div()
        .absolute()
        .top_0()
        .left(px(left))
        .w(px(right - left))
        .h(px(TAB_HEIGHT))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(x - left))
                .w(px(width))
                .h_full()
                .child(content),
        )
}

fn tab_face(p: Palette, active: bool, name: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .size_full()
        .pl(px(TAB_LABEL_INSET))
        .pr(px(TAB_LABEL_TRAILING))
        .font_family(UI_FONT)
        .text_size(px(12.))
        .map(|this| {
            if active {
                this.rounded_t(px(7.))
                    .bg(rgba(p.layer))
                    .text_color(rgba(p.text_primary))
                    .font_weight(FontWeight::SEMIBOLD)
            } else {
                this.rounded_t(px(8.)).text_color(rgba(p.text_secondary))
            }
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name.into()),
        )
}

impl Jot {
    pub(crate) fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = Self::create_tab(None, window, cx);
        self.push_tab(tab, window, cx);
    }

    pub(crate) fn push_tab(&mut self, mut tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let x = self.tabs.last().map_or(0., |last| last.slot.right(now));
        tab.slot = Slot::at(x, 0.);
        self.tabs.push(tab);
        self.layout_tabs(cx);
        self.activate_tab(self.tabs.len() - 1, window, cx);
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 {
            window.remove_window();
            return;
        }
        let now = Instant::now();
        let duration = slide_duration(cx);
        let name = self.tab_name(ix);
        let tab = self.tabs.remove(ix);
        let id = tab.editor.entity_id();
        if self.strip.drag.is_some_and(|drag| drag.id == id) {
            self.strip.drag = None;
        }
        detach::forget(id, cx);
        let mut slot = tab.slot;
        slot.x.slide(ix as f32 * TAB_WIDTH, duration, now);
        slot.width.slide(0., duration, now);
        if slot.moving(now) {
            self.strip.ghosts.push(Ghost {
                name: name.into(),
                slot,
            });
        }
        self.hovered_tab = None;
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.layout_tabs(cx);
        self.activate_tab(self.active, window, cx);
    }

    pub(crate) fn owner(&self, window: &Window, cx: &Context<Self>) -> detach::Owner {
        detach::Owner {
            handle: window.window_handle(),
            jot: cx.weak_entity(),
            hwnd: self.hwnd,
        }
    }

    fn layout_tabs(&mut self, cx: &App) {
        self.layout_tabs_over(slide_duration(cx));
    }

    fn layout_tabs_over(&mut self, duration: Duration) {
        let now = Instant::now();
        let dragged = self.strip.drag.map(|drag| drag.id);
        let mut x = 0.;
        for tab in &mut self.tabs {
            if Some(tab.editor.entity_id()) != dragged {
                tab.slot.x.slide(x, duration, now);
            }
            tab.slot.width.slide(TAB_WIDTH, duration, now);
            x += TAB_WIDTH;
        }
        self.strip.trailing.slide(x, duration, now);
    }

    pub(crate) fn drag_tab(
        &mut self,
        id: EntityId,
        position: Point<f32>,
        grab: f32,
        start: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(from) = self.tab_index(id) else {
            return;
        };
        let mut drag = self
            .strip
            .drag
            .filter(|drag| drag.id == id)
            .unwrap_or(Drag {
                id,
                grab,
                last_move: start - STRIP_LEFT,
            });
        self.strip.lifted = Some(id);
        let x = position.x - STRIP_LEFT;
        let last = self.tabs.len() - 1;
        let left = (x - drag.grab).clamp(0., last as f32 * TAB_WIDTH);
        self.tabs[from].slot.x = Motion::at(left);
        let to = slot_index(left, last);
        if to != from && (x - drag.last_move).abs() > REORDER_THRESHOLD {
            let active = self.active_editor().entity_id();
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            self.active = self.tab_index(active).unwrap_or(to);
            drag.last_move = x;
        }
        self.strip.drag = Some(drag);
        self.layout_tabs(cx);
        cx.notify();
    }

    pub(crate) fn release_tab(&mut self, cx: &mut Context<Self>) {
        if self.strip.drag.take().is_some() {
            self.layout_tabs(cx);
            cx.notify();
        }
    }

    pub(crate) fn take_tab(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TabData> {
        let ix = self.tab_index(id)?;
        let tab = self.tabs.remove(ix);
        self.strip.drag = None;
        self.strip.lifted = None;
        self.hovered_tab = None;
        if self.tabs.is_empty() {
            window.remove_window();
            return Some(tab.into_data());
        }
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.layout_tabs(cx);
        self.activate_tab(self.active, window, cx);
        Some(tab.into_data())
    }

    pub(crate) fn insert_dragged_tab(
        &mut self,
        data: TabData,
        position: Point<f32>,
        grab: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut tab = Self::adopt_tab(data, window, cx);
        let id = tab.editor.entity_id();
        let x = position.x - STRIP_LEFT;
        let count = self.tabs.len();
        let left = (x - grab).clamp(0., count as f32 * TAB_WIDTH);
        let ix = slot_index(left, count);
        tab.slot = Slot::at(left, TAB_WIDTH);
        self.tabs.insert(ix, tab);
        self.strip.drag = Some(Drag {
            id,
            grab,
            last_move: x,
        });
        self.strip.lifted = Some(id);
        self.layout_tabs_over(Duration::ZERO);
        self.activate_tab(ix, window, cx);
    }

    pub(crate) fn restore_tab(
        &mut self,
        data: TabData,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut tab = Self::adopt_tab(data, window, cx);
        let ix = ix.min(self.tabs.len());
        tab.slot = Slot::at(ix as f32 * TAB_WIDTH, 0.);
        self.tabs.insert(ix, tab);
        self.layout_tabs(cx);
        self.activate_tab(ix, window, cx);
    }

    pub(crate) fn return_tab(
        &mut self,
        id: EntityId,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.strip.drag = None;
        if let Some(from) = self.tab_index(id) {
            let tab = self.tabs.remove(from);
            let ix = ix.min(self.tabs.len());
            self.tabs.insert(ix, tab);
            self.activate_tab(ix, window, cx);
        }
        self.layout_tabs(cx);
        cx.notify();
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn sample_tabs(&mut self, window: &mut Window) {
        let now = Instant::now();
        self.strip.ghosts.retain(|ghost| ghost.slot.moving(now));
        if let Some(id) = self.strip.lifted
            && self.strip.drag.is_none()
            && !self
                .tab_index(id)
                .is_some_and(|ix| self.tabs[ix].slot.x.moving(now))
        {
            self.strip.lifted = None;
        }
        if !self.strip.ghosts.is_empty()
            || self.strip.trailing.moving(now)
            || self.tabs.iter().any(|tab| tab.slot.moving(now))
        {
            window.request_animation_frame();
        }
    }

    fn tab(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let tab = &self.tabs[ix];
        let id = tab.editor.entity_id();
        let active = ix == self.active;
        let separator = !active && ix + 1 != self.active && ix + 1 < self.tabs.len();
        let dirty = tab.dirty;
        let show_close = self.hovered_tab == Some(id) || (active && !dirty);
        let show_dot = dirty && !show_close;
        let inset = if active { 1. } else { 0. };
        let name = self.tab_name(ix);

        div()
            .id(("tab", id.as_u64() as usize))
            .relative()
            .size_full()
            .overflow_hidden()
            .when(active, |this| {
                this.border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgba(p.stroke))
                    .rounded_t(px(8.))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if let Some(ix) = this.tab_index(id) {
                        this.activate_tab(ix, window, cx);
                        let x = event.position.x.as_f32();
                        let grab = point(
                            x - STRIP_LEFT - this.tabs[ix].slot.x.value(Instant::now()),
                            event.position.y.as_f32() - STRIP_TOP,
                        );
                        detach::press(this.owner(window, cx), id, ix, grab, x, cx);
                        cx.notify();
                    }
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_tab = Some(id);
                } else if this.hovered_tab == Some(id) {
                    this.hovered_tab = None;
                }
                cx.notify();
            }))
            .child(tab_face(p, active, name).when(!active, |this| {
                this.hover(move |style| style.bg(rgba(p.subtle_secondary)))
            }))
            .when(show_dot, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(UNSAVED_DOT_TOP - inset))
                        .right(px(UNSAVED_DOT_RIGHT - inset))
                        .size(px(UNSAVED_DOT))
                        .rounded_full()
                        .bg(rgba(p.text_tertiary)),
                )
            })
            .when(separator, |this| {
                this.child(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(8.))
                        .w(px(1.))
                        .h(px(16.))
                        .bg(rgba(p.stroke)),
                )
            })
            .when(show_close, |this| {
                this.child(
                    div()
                        .id(("close-tab", id.as_u64() as usize))
                        .absolute()
                        .top(px(CLOSE_BUTTON_TOP - inset))
                        .right(px(CLOSE_BUTTON_RIGHT - inset))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(32.))
                        .h(px(24.))
                        .rounded(px(4.))
                        .font_family("Segoe Fluent Icons")
                        .text_size(px(12.))
                        .text_color(rgba(p.text_primary))
                        .hover(move |style| style.bg(rgba(p.subtle_secondary)))
                        .active(move |style| style.bg(rgba(p.subtle_tertiary)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(ix) = this.tab_index(id) {
                                this.close_tab(ix, window, cx);
                            }
                        }))
                        .child("\u{E711}"),
                )
            })
    }

    fn new_tab_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        div()
            .id("new-tab")
            .flex_none()
            .ml(px(5.))
            .mt(px(5.))
            .w(px(32.))
            .h(px(24.))
            .rounded(px(4.))
            .hover(move |style| style.bg(rgba(p.subtle_secondary)))
            .active(move |style| style.bg(rgba(p.subtle_tertiary)))
            .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx)))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        for (x, y, w, h, ink) in PLUS_SEGMENTS {
                            window.paint_quad(fill(
                                Bounds::new(
                                    bounds.origin + point(px(x), px(y)),
                                    size(px(w), px(h)),
                                ),
                                rgba(ink.color(&p)),
                            ));
                        }
                    },
                )
                .size_full(),
            )
    }

    pub(crate) fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let now = Instant::now();
        let lifted = self.strip.lifted.and_then(|id| self.tab_index(id));
        let cover = lifted.map(|ix| {
            let slot = &self.tabs[ix].slot;
            (slot.x.value(now), slot.right(now))
        });
        let ghosts: Vec<_> = self
            .strip
            .ghosts
            .iter()
            .map(|ghost| {
                frame(
                    ghost.slot.x.value(now),
                    ghost.slot.width.value(now),
                    cover,
                    tab_face(p, false, ghost.name.clone()),
                )
            })
            .collect();
        let tabs: Vec<_> = (0..self.tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted)
            .map(|ix| {
                let slot = self.tabs[ix].slot;
                frame(
                    slot.x.value(now),
                    slot.width.value(now),
                    cover.filter(|_| Some(ix) != lifted),
                    self.tab(ix, cx),
                )
            })
            .collect();

        div()
            .flex()
            .h(px(TAB_HEIGHT))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .h_full()
                    .w(px(self.strip.trailing.value(now)))
                    .children(ghosts)
                    .children(tabs)
                    .when(detach::active(cx), |this| this.child(detach::listeners())),
            )
            .child(self.new_tab_button(cx))
    }
}
