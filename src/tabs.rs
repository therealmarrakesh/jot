use super::*;
use std::time::Instant;

pub(crate) const TAB_WIDTH: f32 = 240.;
const TAB_MIN_WIDTH: f32 = 80.;
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
const STRIP_RESERVED: f32 = 3. * 46. + 43. + 3. + 32.;
const SCROLL_AREA: f32 = 43.;
const SCROLL_OUTER_PAD: f32 = 8.;
const SCROLL_INNER_PAD: f32 = 3.;
const SCROLL_STEP: f32 = 50.;
const SCROLL_REPEAT_DELAY: Duration = Duration::from_millis(50);
const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
const WHEEL_SHARE: f32 = 0.15;
const BUTTON_WIDTH: f32 = 32.;
const BUTTON_HEIGHT: f32 = 24.;
const BUTTON_TOP: f32 = 5.;
const NEW_TAB_GAP: f32 = 5.;
const NEW_TAB_GAP_SCROLLING: f32 = 3.;
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
    scroll: Motion,
    column: f32,
    width: f32,
    overflow: bool,
    frozen: Option<f32>,
    ghosts: Vec<Ghost>,
    drag: Option<Drag>,
    lifted: Option<EntityId>,
    repeat: Task<()>,
}

impl TabStrip {
    pub(crate) fn new() -> Self {
        Self {
            trailing: Motion::at(TAB_WIDTH),
            scroll: Motion::at(0.),
            column: f32::INFINITY,
            width: TAB_WIDTH,
            overflow: false,
            frozen: None,
            ghosts: Vec::new(),
            drag: None,
            lifted: None,
            repeat: Task::ready(()),
        }
    }

    fn measure(&mut self, count: usize) {
        let count = count.max(1) as f32;
        let room = self.column - 2. * LIST_PAD;
        let fit = (room / count).clamp(TAB_MIN_WIDTH, TAB_WIDTH);
        self.width = self.frozen.unwrap_or(fit);
        self.overflow = self.width * count > room;
        if self.overflow && self.frozen.is_none() {
            self.width = ((room - 2. * SCROLL_AREA) / count).clamp(TAB_MIN_WIDTH, TAB_WIDTH);
        }
    }

    fn viewport(&self, now: Instant) -> f32 {
        if self.overflow {
            self.column - 2. * SCROLL_AREA
        } else {
            LIST_PAD + self.trailing.value(now)
        }
    }

    fn max_scroll(&self) -> f32 {
        if self.overflow {
            (2. * LIST_PAD + self.trailing.to - self.viewport(Instant::now())).max(0.)
        } else {
            0.
        }
    }

    fn origin(&self, now: Instant) -> f32 {
        let scroll_area = if self.overflow { SCROLL_AREA } else { 0. };
        HEADER_WIDTH + scroll_area + LIST_PAD - self.scroll.value(now)
    }

    fn bounds(&self, now: Instant) -> Bounds<Pixels> {
        let (scroll_areas, gap) = if self.overflow {
            (2. * SCROLL_AREA, NEW_TAB_GAP_SCROLLING)
        } else {
            (0., NEW_TAB_GAP)
        };
        Bounds::new(
            point(px(HEADER_WIDTH), px(STRIP_TOP)),
            size(
                px(scroll_areas + self.viewport(now) + gap + BUTTON_WIDTH),
                px(TAB_HEIGHT),
            ),
        )
    }

    fn scroll_to(&mut self, target: f32, duration: Duration) {
        let target = target.clamp(0., self.max_scroll()).round();
        self.scroll.slide(target, duration, Instant::now());
    }

    fn reveal(&mut self, left: f32, right: f32, duration: Duration) {
        if !self.overflow {
            self.scroll = Motion::at(0.);
            return;
        }
        let viewport = self.viewport(Instant::now());
        let current = self.scroll.to;
        let target = if left < current {
            left
        } else if right + 2. * LIST_PAD > current + viewport {
            right + 2. * LIST_PAD - viewport
        } else {
            current
        };
        self.scroll_to(target, duration);
    }
}

fn slide_duration(cx: &App) -> Duration {
    if cx.reduce_motion() {
        Duration::ZERO
    } else {
        SLIDE_DURATION
    }
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

fn frame(x: f32, width: f32, cover: Option<(f32, f32)>, content: impl IntoElement) -> Div {
    let (left, right) = visible(x, x + width, cover);
    div()
        .absolute()
        .top_0()
        .left(px(left))
        .w(px((right - left).max(0.)))
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

fn tab_face(p: Palette, active: bool, trailing: f32, name: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .size_full()
        .pl(px(TAB_LABEL_INSET))
        .pr(px(trailing))
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
        let width = self.strip.width;
        let tab = self.tabs.remove(ix);
        let id = tab.editor.entity_id();
        if self.strip.drag.is_some_and(|drag| drag.id == id) {
            self.strip.drag = None;
        }
        detach::forget(id, cx);
        if ix < self.tabs.len() && self.strip.bounds(now).contains(&window.mouse_position()) {
            self.strip.frozen = Some(width);
        }
        let mut slot = tab.slot;
        slot.x.slide(ix as f32 * width, duration, now);
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
        self.strip.measure(self.tabs.len());
        let width = self.strip.width;
        let dragged = self.strip.drag.map(|drag| drag.id);
        let mut x = 0.;
        for tab in &mut self.tabs {
            if Some(tab.editor.entity_id()) != dragged {
                tab.slot.x.slide(x, duration, now);
            }
            tab.slot.width.slide(width, duration, now);
            x += width;
        }
        self.strip.trailing.slide(x, duration, now);
        let scroll = self.strip.scroll.to;
        self.strip.scroll_to(scroll, duration);
    }

    pub(crate) fn reveal_tab(&mut self, ix: usize, cx: &App) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let left = tab.slot.x.to;
        let right = left + tab.slot.width.to;
        self.strip.reveal(left, right, slide_duration(cx));
    }

    fn scroll_tabs(&mut self, delta: f32, cx: &mut Context<Self>) {
        let target = self.strip.scroll.to + delta;
        self.strip.scroll_to(target, slide_duration(cx));
        cx.notify();
    }

    fn thaw_tabs(&mut self, cx: &mut Context<Self>) {
        if self.strip.frozen.take().is_some() {
            self.layout_tabs(cx);
            cx.notify();
        }
    }

    fn thaw_listener(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let bounds = self.strip.bounds(Instant::now());
        let jot = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moved = jot.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && !bounds.contains(&event.position) {
                        moved.update(cx, |jot, cx| jot.thaw_tabs(cx)).ok();
                    }
                });
                let left = jot.clone();
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        left.update(cx, |jot, cx| jot.thaw_tabs(cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .size_full()
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
        let now = Instant::now();
        let origin = self.strip.origin(now);
        let mut drag = self
            .strip
            .drag
            .filter(|drag| drag.id == id)
            .unwrap_or(Drag {
                id,
                grab,
                last_move: start - origin,
            });
        self.strip.lifted = Some(id);
        let width = self.strip.width;
        let x = position.x - origin;
        let last = self.tabs.len() - 1;
        let left = (x - drag.grab).clamp(0., last as f32 * width);
        self.tabs[from].slot.x = Motion::at(left);
        self.strip.reveal(left, left + width, Duration::ZERO);
        let to = slot_index(left, width, last);
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
        let count = self.tabs.len();
        self.strip.measure(count + 1);
        let width = self.strip.width;
        let x = position.x - self.strip.origin(Instant::now());
        let left = (x - grab).clamp(0., count as f32 * width);
        let ix = slot_index(left, width, count);
        tab.slot = Slot::at(left, width);
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
        tab.slot = Slot::at(ix as f32 * self.strip.width, 0.);
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
        }
        self.layout_tabs(cx);
        if let Some(ix) = self.tab_index(id) {
            self.activate_tab(ix, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn sample_tabs(&mut self, window: &mut Window) {
        let now = Instant::now();
        let column = window.viewport_size().width.as_f32() - HEADER_WIDTH - STRIP_RESERVED;
        if column != self.strip.column {
            self.strip.column = column;
            self.layout_tabs_over(Duration::ZERO);
            let active = self.active;
            if let Some(tab) = self.tabs.get(active) {
                let left = tab.slot.x.to;
                let right = left + tab.slot.width.to;
                self.strip.reveal(left, right, Duration::ZERO);
            }
        }
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
            || self.strip.scroll.moving(now)
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
        let trailing = if show_close || show_dot {
            TAB_LABEL_TRAILING
        } else {
            TAB_LABEL_TRAILING_BARE
        };
        let inset = if active { 1. } else { 0. };
        let name = self.tab_name(ix);
        let tip: SharedString = tab
            .path
            .as_ref()
            .map_or(name.clone(), |path| path.display().to_string())
            .into();
        let key = id.as_u64() as usize;

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
                        let now = Instant::now();
                        let grab = point(
                            x - this.strip.origin(now) - this.tabs[ix].slot.x.value(now),
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
                tooltip::hover(("tab", key), 0, tip.clone(), *hovered, cx);
                cx.notify();
            }))
            .child(tab_face(p, active, trailing, name).when(!active, |this| {
                this.hover(move |style| style.bg(rgba(p.subtle_secondary)))
            }))
            .child(tooltip::anchor(("tab", key)))
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
                        .w(px(BUTTON_WIDTH))
                        .h(px(BUTTON_HEIGHT))
                        .rounded(px(4.))
                        .font_family("Segoe Fluent Icons")
                        .text_size(px(12.))
                        .text_color(rgba(p.text_primary))
                        .hover(move |style| style.bg(rgba(p.subtle_secondary)))
                        .active(move |style| style.bg(rgba(p.subtle_tertiary)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_hover(move |hovered, _, cx| {
                            tooltip::hover(
                                ("close-tab", key),
                                1,
                                "Close tab (Ctrl+W)",
                                *hovered,
                                cx,
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(ix) = this.tab_index(id) {
                                this.close_tab(ix, window, cx);
                            }
                        }))
                        .child(tooltip::anchor(("close-tab", key)))
                        .child("\u{E711}"),
                )
            })
    }

    fn new_tab_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let gap = if self.strip.overflow {
            NEW_TAB_GAP_SCROLLING
        } else {
            NEW_TAB_GAP
        };
        div()
            .id("new-tab")
            .flex_none()
            .ml(px(gap))
            .mt(px(BUTTON_TOP))
            .w(px(BUTTON_WIDTH))
            .h(px(BUTTON_HEIGHT))
            .rounded(px(4.))
            .hover(move |style| style.bg(rgba(p.subtle_secondary)))
            .active(move |style| style.bg(rgba(p.subtle_tertiary)))
            .on_hover(|hovered, _, cx| {
                tooltip::hover("new-tab", 0, "New tab (Ctrl+N)", *hovered, cx)
            })
            .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx)))
            .child(tooltip::anchor("new-tab"))
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

    fn scroll_button(&self, forward: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let target = self.strip.scroll.to;
        let enabled = if forward {
            target < self.strip.max_scroll() - 0.1
        } else {
            target > 0.1
        };
        let step = if forward { SCROLL_STEP } else { -SCROLL_STEP };
        let (key, tip) = if forward {
            ("scroll-tabs-forward", "Scroll tab list forward")
        } else {
            ("scroll-tabs-backward", "Scroll tab list backward")
        };
        let (left, right) = if forward {
            (SCROLL_INNER_PAD, SCROLL_OUTER_PAD)
        } else {
            (SCROLL_OUTER_PAD, SCROLL_INNER_PAD)
        };
        let stop = cx.listener(|this, _: &MouseUpEvent, _, _| {
            this.strip.repeat = Task::ready(());
        });
        let stop_out = cx.listener(|this, _: &MouseUpEvent, _, _| {
            this.strip.repeat = Task::ready(());
        });
        div().flex_none().h_full().pl(px(left)).pr(px(right)).child(
            div()
                .id(key)
                .on_hover(move |hovered, _, cx| tooltip::hover(key, 0, tip, *hovered, cx))
                .flex()
                .items_center()
                .justify_center()
                .mt(px(BUTTON_TOP))
                .w(px(BUTTON_WIDTH))
                .h(px(BUTTON_HEIGHT))
                .rounded(px(4.))
                .font_family("Segoe Fluent Icons")
                .text_size(px(8.))
                .text_color(rgba(if enabled {
                    p.text_secondary
                } else {
                    p.text_disabled
                }))
                .when(enabled, |this| {
                    this.hover(move |style| style.bg(rgba(p.subtle_secondary)))
                        .active(move |style| style.bg(rgba(p.subtle_tertiary)))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.scroll_tabs(step, cx);
                        this.strip.repeat = cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(SCROLL_REPEAT_DELAY).await;
                            loop {
                                if this
                                    .update(cx, |jot, cx| jot.scroll_tabs(step, cx))
                                    .is_err()
                                {
                                    break;
                                }
                                cx.background_executor().timer(SCROLL_REPEAT_INTERVAL).await;
                            }
                        });
                    }),
                )
                .child(tooltip::anchor(key))
                .on_mouse_up(MouseButton::Left, stop)
                .on_mouse_up_out(MouseButton::Left, stop_out)
                .child(
                    div()
                        .relative()
                        .left(px(-1.))
                        .top(px(1.))
                        .child(if forward { "\u{EDDA}" } else { "\u{EDD9}" }),
                ),
        )
    }

    pub(crate) fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let now = Instant::now();
        let shift = LIST_PAD - self.strip.scroll.value(now);
        let viewport = self.strip.viewport(now);
        let lifted = self.strip.lifted.and_then(|id| self.tab_index(id));
        let cover = lifted.map(|ix| {
            let slot = &self.tabs[ix].slot;
            (shift + slot.x.value(now), shift + slot.right(now))
        });
        let ghosts: Vec<_> = self
            .strip
            .ghosts
            .iter()
            .map(|ghost| {
                frame(
                    shift + ghost.slot.x.value(now),
                    ghost.slot.width.value(now),
                    cover,
                    tab_face(p, false, TAB_LABEL_TRAILING_BARE, ghost.name.clone()),
                )
            })
            .collect();
        let tabs: Vec<_> = (0..self.tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted)
            .map(|ix| {
                let slot = self.tabs[ix].slot;
                frame(
                    shift + slot.x.value(now),
                    slot.width.value(now),
                    cover.filter(|_| Some(ix) != lifted),
                    self.tab(ix, cx),
                )
            })
            .collect();
        let overflow = self.strip.overflow;

        div()
            .relative()
            .flex()
            .h(px(TAB_HEIGHT))
            .when(self.strip.frozen.is_some(), |this| {
                this.child(self.thaw_listener(cx))
            })
            .when(overflow, |this| this.child(self.scroll_button(false, cx)))
            .child(
                div()
                    .id("tab-list")
                    .relative()
                    .flex_none()
                    .h_full()
                    .w(px(viewport))
                    .overflow_hidden()
                    .when(overflow, |this| {
                        this.on_scroll_wheel(cx.listener(
                            |this, event: &ScrollWheelEvent, _, cx| {
                                let viewport = this.strip.viewport(Instant::now());
                                match event.delta {
                                    ScrollDelta::Lines(lines) => {
                                        let notches = if lines.y != 0. { lines.y } else { lines.x };
                                        this.scroll_tabs(-notches * WHEEL_SHARE * viewport, cx);
                                    }
                                    ScrollDelta::Pixels(pixels) => {
                                        let delta = if pixels.y != px(0.) {
                                            pixels.y
                                        } else {
                                            pixels.x
                                        };
                                        let target = this.strip.scroll.to - delta.as_f32();
                                        this.strip.scroll_to(target, Duration::ZERO);
                                        cx.notify();
                                    }
                                }
                            },
                        ))
                    })
                    .children(ghosts)
                    .children(tabs)
                    .when(detach::active(cx), |this| this.child(detach::listeners())),
            )
            .when(overflow, |this| this.child(self.scroll_button(true, cx)))
            .child(self.new_tab_button(cx))
    }
}
