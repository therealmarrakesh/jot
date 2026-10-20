use super::*;
use gpui_kit::base::{Sequence, Transition, TransitionId};

const TAB_WIDTH: f32 = 240.;
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
pub(crate) const CAPTION_WIDTH: f32 = 3. * CAPTION_BUTTON_WIDTH;
const DRAG_RESERVE: f32 = 43.;
const STRIP_RESERVED: f32 = CAPTION_WIDTH + DRAG_RESERVE + NEW_TAB_GAP_SCROLLING + BUTTON_WIDTH;
pub(crate) const TITLE_BAR_MIN_WIDTH: f32 = HEADER_WIDTH + 2. * LIST_PAD + STRIP_RESERVED;
const SCROLL_AREA: f32 = SCROLL_OUTER_PAD + BUTTON_WIDTH + SCROLL_INNER_PAD;
const SCROLL_OUTER_PAD: f32 = 8.;
const SCROLL_INNER_PAD: f32 = 3.;
const SCROLL_STEP: f32 = 50.;
const SCROLL_REPEAT_DELAY: Duration = Duration::from_millis(50);
const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(100);
const WHEEL_SHARE: f32 = 0.15;
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
const BUTTON_GLYPH_SIZE: f32 = 12.;

#[derive(Clone, Copy, Default)]
pub(crate) struct SlotKey(usize);

impl SlotKey {
    fn x(self) -> ElementId {
        ElementId::from(("tab-x", self.0))
    }

    fn width(self) -> ElementId {
        ElementId::from(("tab-width", self.0))
    }
}

struct Ghost {
    name: SharedString,
    key: SlotKey,
    x: f32,
}

#[derive(Clone, Copy)]
struct Drag {
    id: EntityId,
    grab: f32,
    last_move: f32,
    left: f32,
}

pub(crate) struct StripFrame {
    shift: f32,
    viewport: f32,
    tabs: Vec<(f32, f32)>,
    ghosts: Vec<(SharedString, f32, f32)>,
}

pub(crate) struct TabStrip {
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
    lifted: Option<EntityId>,
    hovered: Option<EntityId>,
    area: Rc<Cell<Bounds<Pixels>>>,
    repeat: Task<()>,
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
            hovered: None,
            area: Rc::new(Cell::new(Bounds::default())),
            repeat: Task::ready(()),
        }
    }

    fn measure(&mut self, count: usize) {
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

    fn key(&mut self) -> SlotKey {
        self.next_key += 1;
        SlotKey(self.next_key)
    }

    fn snap(&mut self) {
        self.snap_layout = true;
        self.snap_scroll = true;
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

    fn origin(&self) -> f32 {
        HEADER_WIDTH + self.scroll_area() + LIST_PAD - self.scroll
    }

    fn new_tab_gap(&self) -> f32 {
        if self.buttons {
            NEW_TAB_GAP_SCROLLING
        } else {
            NEW_TAB_GAP
        }
    }

    fn depart(&mut self, id: EntityId) {
        if self.drag.is_some_and(|drag| drag.id == id) {
            self.drag = None;
        }
        if self.lifted == Some(id) {
            self.lifted = None;
        }
        if self.hovered == Some(id) {
            self.hovered = None;
        }
    }

    fn place(&self, x: f32, grab: f32, last: usize) -> (f32, f32, usize) {
        let x = x - self.origin();
        let left = (x - grab).clamp(0., last as f32 * self.width);
        (x, left, slot_index(left, self.width, last))
    }

    fn scroll_to(&mut self, target: f32) {
        self.scroll = target.clamp(0., self.max_scroll()).round();
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
}

fn slide(
    id: impl Into<TransitionId>,
    seed: f32,
    target: f32,
    snap: bool,
    window: &mut Window,
    cx: &mut App,
) -> (f32, bool) {
    let duration = if snap { Duration::ZERO } else { SLIDE_DURATION };
    let sample = Sequence::new(id, seed)
        .with_step(target, Transition::new(duration).easing(SLIDE_EASING))
        .sample(window, cx);
    (sample.value().round(), !sample.is_finished())
}

pub(crate) fn window_listeners(register: impl FnOnce(&mut Window) + 'static) -> impl IntoElement {
    canvas(|_, _, _| {}, move |_, _, window, _| register(window))
        .absolute()
        .size_full()
}

fn repeat(
    cx: &mut Context<Jot>,
    delay: Duration,
    interval: Duration,
    mut tick: impl FnMut(&mut Jot, &mut Context<Jot>) -> bool + 'static,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(delay).await;
        while this.update(cx, |jot, cx| tick(jot, cx)).unwrap_or(false) {
            cx.background_executor().timer(interval).await;
        }
    })
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
        .font_family(SMALL_FONT)
        .text_size(px(12.))
        .map(|this| {
            if active {
                this.rounded_t(px(field::OVERLAY_RADIUS - 1.))
                    .bg(rgba(p.layer))
                    .text_color(rgba(p.text_primary))
                    .font_weight(FontWeight::SEMIBOLD)
            } else {
                this.rounded_t(px(field::OVERLAY_RADIUS))
                    .text_color(rgba(p.text_secondary))
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

fn unsaved_dot(p: Palette, inset: f32) -> Div {
    div()
        .absolute()
        .top(px(UNSAVED_DOT_TOP - inset))
        .right(px(UNSAVED_DOT_RIGHT - inset))
        .size(px(UNSAVED_DOT))
        .rounded_full()
        .bg(rgba(p.text_tertiary))
}

fn strip_button(
    p: Palette,
    id: impl Into<ElementId>,
    tip: &'static str,
    glyph: &'static str,
    glyph_size: f32,
    enabled: bool,
) -> brush::BrushTransition {
    field::icon_button(
        p,
        id,
        tip,
        glyph,
        px(glyph_size),
        size(px(BUTTON_WIDTH), px(BUTTON_HEIGHT)),
        px(field::CONTROL_RADIUS),
        enabled,
    )
}

fn tab_separator(p: Palette) -> Div {
    div()
        .absolute()
        .right_0()
        .top(px(8.))
        .w(px(1.))
        .h(px(16.))
        .bg(rgba(p.divider))
}

impl Jot {
    pub(crate) fn create_tab(
        content: Option<(PathBuf, document::Loaded)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Tab {
        let word_wrap = cx.global::<ViewSettings>().word_wrap;
        let (path, text, format) = match content {
            Some((path, loaded)) => (Some(path), loaded.text, loaded.format),
            None => (None, String::new(), document::Format::default()),
        };
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(word_wrap)
                .viewport_wheel_step()
                .default_value(text)
        });
        Self::adopt_tab(
            TabData {
                editor,
                zoom: 1.,
                path,
                format,
                dirty: false,
            },
            window,
            cx,
        )
    }

    pub(crate) fn adopt_tab(data: TabData, window: &mut Window, cx: &mut Context<Self>) -> Tab {
        data.editor
            .update(cx, |editor, cx| editor.attach_window(window, cx));
        let _changes = cx.subscribe(&data.editor, |this, editor, event: &InputEvent, cx| {
            if let InputEvent::Change = event
                && let Some(tab) = this.tabs.iter_mut().find(|tab| tab.data.editor == editor)
            {
                let dirty = tab.data.path.is_some() || editor.read(cx).text().len() > 0;
                if tab.data.dirty != dirty {
                    tab.data.dirty = dirty;
                    cx.notify();
                }
            }
        });
        Tab {
            data,
            slot: SlotKey::default(),
            _changes,
        }
    }

    pub(crate) fn tab_name(&self, ix: usize) -> String {
        document::display_name(self.tabs[ix].data.path.as_deref())
    }

    pub(crate) fn tab_index(&self, id: EntityId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.data.editor.entity_id() == id)
    }

    pub(crate) fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = Self::create_tab(None, window, cx);
        self.push_tab(tab, window, cx);
    }

    pub(crate) fn push_tab(&mut self, mut tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        tab.slot = self.strip.key();
        self.tabs.push(tab);
        self.layout_tabs();
        self.activate_tab(self.tabs.len() - 1, window, cx);
    }

    pub(crate) fn replace_tab(
        &mut self,
        ix: usize,
        mut tab: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tab.slot = self.tabs[ix].slot;
        let replaced = std::mem::replace(&mut self.tabs[ix], tab);
        self.forget_tab(replaced.data.editor.entity_id(), cx);
        self.activate_tab(ix, window, cx);
    }

    fn forget_tab(&mut self, id: EntityId, cx: &mut Context<Self>) {
        self.strip.depart(id);
        tab_drag::forget(id, cx);
    }

    pub(crate) fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 {
            window.remove_window();
            return;
        }
        let width = self.strip.width;
        let name = self.tab_name(ix).into();
        let tab = self.tabs.remove(ix);
        self.forget_tab(tab.data.editor.entity_id(), cx);
        if ix < self.tabs.len() && self.strip.area.get().contains(&window.mouse_position()) {
            self.strip.frozen = Some(width);
        }
        self.strip.ghosts.push(Ghost {
            name,
            key: tab.slot,
            x: ix as f32 * width,
        });
        self.settle_after_removal(ix, window, cx);
    }

    fn settle_after_removal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        self.layout_tabs();
        self.activate_tab(self.active, window, cx);
    }

    fn layout_tabs(&mut self) {
        self.strip.measure(self.tabs.len());
        self.strip.extent = self.tabs.len() as f32 * self.strip.width;
        self.strip.scroll_to(self.strip.scroll);
    }

    fn tab_target(&self, ix: usize) -> f32 {
        let id = self.tabs[ix].data.editor.entity_id();
        self.strip
            .drag
            .filter(|drag| drag.id == id)
            .map_or(ix as f32 * self.strip.width, |drag| drag.left)
    }

    pub(crate) fn reveal_tab(&mut self, ix: usize) {
        if ix < self.tabs.len() {
            let left = self.tab_target(ix);
            self.strip.reveal(left, left + self.strip.width);
        }
    }

    fn scroll_tabs(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        let before = self.strip.scroll;
        self.strip.scroll_to(before + delta);
        cx.notify();
        self.strip.scroll != before
    }

    fn thaw_tabs(&mut self, cx: &mut Context<Self>) {
        if self.strip.frozen.take().is_some() {
            self.layout_tabs();
            cx.notify();
        }
    }

    fn thaw_listener(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let area = self.strip.area.clone();
        let jot = cx.weak_entity();
        window_listeners(move |window| {
            let moved = jot.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && !area.get().contains(&event.position) {
                    moved.update(cx, |jot, cx| jot.thaw_tabs(cx)).ok();
                }
            });
            window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble {
                    jot.update(cx, |jot, cx| jot.thaw_tabs(cx)).ok();
                }
            });
        })
    }

    pub(crate) fn drag_tab(
        &mut self,
        id: EntityId,
        x: f32,
        grab: f32,
        start: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(from) = self.tab_index(id) else {
            return;
        };
        let origin = self.strip.origin();
        let mut drag = self
            .strip
            .drag
            .filter(|drag| drag.id == id)
            .unwrap_or(Drag {
                id,
                grab,
                last_move: start - origin,
                left: 0.,
            });
        self.strip.lifted = Some(id);
        let width = self.strip.width;
        let last = self.tabs.len() - 1;
        let (x, left, to) = self.strip.place(x, drag.grab, last);
        drag.left = left;
        self.strip.snap_scroll = true;
        self.strip.reveal(left, left + width);
        if to != from && (x - drag.last_move).abs() > REORDER_THRESHOLD {
            let active = self.active_editor().entity_id();
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            self.active = self.tab_index(active).unwrap_or(to);
            drag.last_move = x;
        }
        self.strip.drag = Some(drag);
        self.layout_tabs();
        cx.notify();
    }

    pub(crate) fn release_tab(&mut self, cx: &mut Context<Self>) {
        if self.strip.drag.take().is_some() {
            self.layout_tabs();
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
        self.strip.depart(id);
        if self.tabs.is_empty() {
            window.remove_window();
        } else {
            self.settle_after_removal(ix, window, cx);
        }
        Some(tab.data)
    }

    pub(crate) fn insert_dragged_tab(
        &mut self,
        data: TabData,
        x: f32,
        grab: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut tab = Self::adopt_tab(data, window, cx);
        tab.slot = self.strip.key();
        let id = tab.data.editor.entity_id();
        let count = self.tabs.len();
        self.strip.measure(count + 1);
        let (x, left, ix) = self.strip.place(x, grab, count);
        self.tabs.insert(ix, tab);
        self.strip.drag = Some(Drag {
            id,
            grab,
            last_move: x,
            left,
        });
        self.strip.lifted = Some(id);
        self.strip.snap();
        self.layout_tabs();
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
        tab.slot = self.strip.key();
        self.tabs.insert(ix, tab);
        self.layout_tabs();
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
        self.layout_tabs();
        if let Some(ix) = self.tab_index(id) {
            self.activate_tab(ix, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn sample_tabs(&mut self, window: &mut Window, cx: &mut App) -> StripFrame {
        let column = window.viewport_size().width.as_f32() - HEADER_WIDTH - STRIP_RESERVED;
        if column != self.strip.column {
            self.strip.column = column;
            self.strip.snap();
            self.layout_tabs();
            self.reveal_tab(self.active);
        }
        let snap = std::mem::take(&mut self.strip.snap_layout);
        let snap_scroll = std::mem::take(&mut self.strip.snap_scroll);
        let (scroll, _) = slide("tab-scroll", 0., self.strip.scroll, snap_scroll, window, cx);
        let (extent, _) = slide("tab-extent", TAB_WIDTH, self.strip.extent, snap, window, cx);
        let mut right = 0.;
        let mut lifted_moving = false;
        let mut tabs = Vec::with_capacity(self.tabs.len());
        for (ix, tab) in self.tabs.iter().enumerate() {
            let id = tab.data.editor.entity_id();
            let dragged = self.strip.drag.is_some_and(|drag| drag.id == id);
            let (x, moving) = slide(
                tab.slot.x(),
                right,
                self.tab_target(ix),
                snap || dragged,
                window,
                cx,
            );
            let (width, _) = slide(tab.slot.width(), 0., self.strip.width, snap, window, cx);
            if self.strip.lifted == Some(id) {
                lifted_moving = moving;
            }
            right = x + width;
            tabs.push((x, width));
        }
        let mut ghosts = Vec::new();
        self.strip.ghosts.retain(|ghost| {
            let (x, sliding) = slide(ghost.key.x(), ghost.x, ghost.x, false, window, cx);
            let (width, shrinking) = slide(ghost.key.width(), 0., 0., false, window, cx);
            let moving = sliding || shrinking;
            if moving {
                ghosts.push((ghost.name.clone(), x, width));
            }
            moving
        });
        if self.strip.drag.is_none() && !lifted_moving {
            self.strip.lifted = None;
        }
        let viewport = if self.strip.overflow {
            self.strip.scroll_viewport()
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

    fn tab(&self, ix: usize, left: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let tab = &self.tabs[ix];
        let id = tab.data.editor.entity_id();
        let active = ix == self.active;
        let hovered = self
            .strip
            .hovered
            .and_then(|hovered| self.tab_index(hovered));
        let separator =
            !active && ix + 1 != self.active && hovered != Some(ix) && hovered != Some(ix + 1);
        let dirty = tab.data.dirty;
        let show_close = self.strip.hovered == Some(id) || (active && !dirty);
        let show_dot = dirty && !show_close;
        let trailing = if show_close || show_dot {
            TAB_LABEL_TRAILING
        } else {
            TAB_LABEL_TRAILING_BARE
        };
        let inset = if active { 1. } else { 0. };
        let name = self.tab_name(ix);
        let tip: SharedString = tab
            .data
            .path
            .as_ref()
            .map_or(name.clone(), |path| path.display().to_string())
            .into();

        let tab = div()
            .id(("tab", id))
            .relative()
            .size_full()
            .overflow_hidden()
            .when(active, |this| {
                this.border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgba(p.stroke))
                    .rounded_t(px(field::OVERLAY_RADIUS))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if let Some(ix) = this.tab_index(id)
                        && let Some(owner) = tab_drag::owner(window)
                    {
                        this.activate_tab(ix, window, cx);
                        let grab = point(
                            event.position.x.as_f32() - left,
                            event.position.y.as_f32() - STRIP_TOP,
                        );
                        let alone = this.tabs.len() == 1;
                        tab_drag::press(owner, id, ix, grab, event.position, alone, cx);
                        cx.notify();
                    }
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.strip.hovered = Some(id);
                } else if this.strip.hovered == Some(id) {
                    this.strip.hovered = None;
                }
                cx.notify();
            }))
            .child(
                tab_face(p, active, trailing, name)
                    .id(("tab-face", id))
                    .when(!active, |this| {
                        this.hover(move |style| style.bg(rgba(p.layer_hover)))
                    })
                    .on_aux_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.is_middle_click()
                            && let Some(ix) = this.tab_index(id)
                        {
                            this.close_tab(ix, window, cx);
                        }
                    })),
            )
            .when(show_dot, |this| this.child(unsaved_dot(p, inset)))
            .when(separator, |this| this.child(tab_separator(p)))
            .when(show_close, |this| {
                this.child(Self::close_button(p, id, inset, cx))
            });
        tooltip::owner(("tab", id), tip, tab)
    }

    fn close_button(
        p: Palette,
        id: EntityId,
        inset: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        strip_button(
            p,
            ("close-tab", id),
            "Close tab (Ctrl+W)",
            CLOSE_GLYPH,
            BUTTON_GLYPH_SIZE,
            true,
        )
        .absolute()
        .top(px(CLOSE_BUTTON_TOP - inset))
        .right(px(CLOSE_BUTTON_RIGHT - inset))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, window, cx| {
            if let Some(ix) = this.tab_index(id) {
                this.close_tab(ix, window, cx);
            }
        }))
    }

    fn stop_repeat(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.strip.repeat = Task::ready(());
    }

    fn new_tab_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        strip_button(
            p,
            "new-tab",
            "New tab (Ctrl+N)",
            ADD_GLYPH,
            BUTTON_GLYPH_SIZE,
            true,
        )
        .ml(px(self.strip.new_tab_gap()))
        .mt(px(BUTTON_TOP))
        .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx)))
    }

    fn scroll_button(&self, forward: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = palette(cx);
        let target = self.strip.scroll;
        let enabled = if forward {
            target < self.strip.max_scroll() - 0.1
        } else {
            target > 0.1
        };
        let step = if forward { SCROLL_STEP } else { -SCROLL_STEP };
        let (key, tip, glyph) = if forward {
            (
                "scroll-tabs-forward",
                "Scroll tab list forward",
                CARET_RIGHT,
            )
        } else {
            (
                "scroll-tabs-backward",
                "Scroll tab list backward",
                CARET_LEFT,
            )
        };
        let (left, right) = if forward {
            (SCROLL_INNER_PAD, SCROLL_OUTER_PAD)
        } else {
            (SCROLL_OUTER_PAD, SCROLL_INNER_PAD)
        };
        let button = strip_button(p, key, tip, glyph, SCROLL_GLYPH_SIZE, enabled)
            .mt(px(BUTTON_TOP))
            .when(enabled, |this| {
                this.text_color(rgba(p.text_secondary)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if this.scroll_tabs(step, cx) {
                            this.strip.repeat = repeat(
                                cx,
                                SCROLL_REPEAT_DELAY,
                                SCROLL_REPEAT_INTERVAL,
                                move |jot, cx| jot.scroll_tabs(step, cx),
                            );
                        }
                    }),
                )
            })
            .on_mouse_up(MouseButton::Left, cx.listener(Self::stop_repeat))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::stop_repeat));
        div()
            .flex_none()
            .h_full()
            .pl(px(left))
            .pr(px(right))
            .child(button)
    }

    pub(crate) fn tab_strip(
        &self,
        sampled: &StripFrame,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let shift = sampled.shift;
        let viewport = sampled.viewport;
        let lifted = self.strip.lifted.and_then(|id| self.tab_index(id));
        let cover = lifted.map(|ix| {
            let (x, width) = sampled.tabs[ix];
            (shift + x, shift + x + width)
        });
        let ghosts = sampled.ghosts.iter().map(|(name, x, width)| {
            frame(
                shift + x,
                *width,
                cover,
                tab_face(p, false, TAB_LABEL_TRAILING_BARE, name.clone()),
            )
        });
        let origin = HEADER_WIDTH + self.strip.scroll_area() + shift;
        let tabs: Vec<_> = (0..self.tabs.len())
            .filter(|&ix| Some(ix) != lifted)
            .chain(lifted)
            .map(|ix| {
                let (x, width) = sampled.tabs[ix];
                frame(
                    shift + x,
                    width,
                    cover.filter(|_| Some(ix) != lifted),
                    self.tab(ix, origin + x, cx),
                )
            })
            .collect();
        let overflow = self.strip.overflow;
        let buttons = self.strip.buttons;

        let area = self.strip.area.clone();
        let strip = div()
            .relative()
            .flex()
            .h(px(TAB_HEIGHT))
            .when(self.strip.frozen.is_some(), |this| {
                this.child(self.thaw_listener(cx))
            })
            .when(buttons, |this| this.child(self.scroll_button(false, cx)))
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
                                let viewport = this.strip.scroll_viewport();
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
                                        let target = this.strip.scroll - delta.as_f32();
                                        this.strip.snap_scroll = true;
                                        this.strip.scroll_to(target);
                                        cx.notify();
                                    }
                                }
                            },
                        ))
                    })
                    .children(ghosts)
                    .children(tabs)
                    .when(tab_drag::active(cx), |this| {
                        this.child(tab_drag::listeners())
                    }),
            )
            .when(buttons, |this| this.child(self.scroll_button(true, cx)))
            .child(self.new_tab_button(cx));
        measure::measured(strip, move |bounds, _, _| area.set(bounds))
    }
}
