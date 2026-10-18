use super::*;
use gpui_kit::base::{Transition, transition};
use tabs::{CARET_LEFT, CARET_RIGHT, repeat, window_listeners};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

const BAR_BREADTH: f32 = 12.;
const MARGIN: f32 = 1.;
const THUMB_COLLAPSED: f32 = 8.;
const THUMB_OFFSET: f32 = 2.;
const THUMB_STROKE_INSET: f32 = 3.;
const THUMB_MIN_LENGTH: f32 = 30.;
const THUMB_RADIUS: f32 = 3.;
const TRACK_RADIUS: f32 = 6.;
const ARROW: f32 = 12.;
const ARROW_GLYPH: f32 = 8.;
const ARROW_PADDING: f32 = 4.;
const ARROW_PRESSED_SCALE: f32 = 0.875;
const LINE_DELTA: f32 = 16.;
const EXPAND: Duration = Duration::from_millis(167);
const OPACITY_CHANGE: Duration = Duration::from_millis(83);
const EXPAND_BEGIN: Duration = Duration::from_millis(400);
const CONTRACT_BEGIN: Duration = Duration::from_millis(500);
const HIDE_DELAY: Duration = Duration::from_secs(2);
const REPEAT_DELAY: Duration = Duration::from_millis(250);
const REPEAT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Decrease,
    PageDecrease,
    Thumb,
    PageIncrease,
    Increase,
}

#[derive(Default)]
struct Bar {
    hovered: bool,
    hovered_part: Option<Part>,
}

#[derive(Clone, Copy)]
struct Press {
    axis: Axis,
    part: Part,
    pointer: f32,
    offset: f32,
}

pub(crate) struct ScrollChrome {
    bars: [Bar; 2],
    pub(crate) area: Rc<Cell<Bounds<Pixels>>>,
    shown: bool,
    press: Option<Press>,
    last_offset: Option<Point<Pixels>>,
    hiding: Task<()>,
    repeating: Task<()>,
}

impl ScrollChrome {
    pub(crate) fn new() -> Self {
        Self {
            bars: Default::default(),
            area: Rc::new(Cell::new(Bounds::default())),
            shown: false,
            press: None,
            last_offset: None,
            hiding: Task::ready(()),
            repeating: Task::ready(()),
        }
    }

    fn bar(&self, axis: Axis) -> &Bar {
        &self.bars[axis as usize]
    }

    fn bar_mut(&mut self, axis: Axis) -> &mut Bar {
        &mut self.bars[axis as usize]
    }
}

fn auto_hide() -> bool {
    let mut data = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows_sys::w!(r"Control Panel\Accessibility"),
            windows_sys::w!("DynamicScrollbars"),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    status != 0 || data != 0
}

struct Metrics {
    viewport: f32,
    content: f32,
    offset: f32,
}

impl Metrics {
    fn scrollable(&self) -> bool {
        self.content > self.viewport + 0.5
    }

    fn range(&self) -> f32 {
        (self.content - self.viewport).max(0.)
    }
}

fn metrics(editor: &TextareaState, axis: Axis) -> Metrics {
    let (viewport, content, offset) = editor.scroll_extent();
    Metrics {
        viewport: viewport.along(axis).as_f32(),
        content: content.along(axis).as_f32(),
        offset: -offset.along(axis).as_f32(),
    }
}

struct Track {
    length: f32,
    thumb_start: f32,
    thumb_length: f32,
}

fn track(length: f32, metrics: &Metrics) -> Track {
    let track = (length - 2. * ARROW).max(0.);
    let thumb_length = (track * metrics.viewport / metrics.content.max(1.))
        .max(THUMB_MIN_LENGTH)
        .min(track);
    let progress = if metrics.range() > 0. {
        (metrics.offset / metrics.range()).clamp(0., 1.)
    } else {
        0.
    };
    Track {
        length: track,
        thumb_start: ARROW + (track - thumb_length) * progress,
        thumb_length,
    }
}

fn thumb(axis: Axis, track: &Track, expansion: f32) -> (Bounds<Pixels>, f32) {
    let breadth = THUMB_COLLAPSED + (BAR_BREADTH - THUMB_COLLAPSED) * expansion;
    let shift = (BAR_BREADTH - breadth) / 2. + THUMB_OFFSET * (1. - expansion);
    let fill_breadth = (breadth - 2. * THUMB_STROKE_INSET).max(0.);
    let fill_length = (track.thumb_length - 2. * THUMB_STROKE_INSET).max(0.);
    let across = px(shift + THUMB_STROKE_INSET);
    let bounds = Bounds::new(
        point(across, across).apply_along(axis, |_| px(track.thumb_start + THUMB_STROKE_INSET)),
        size(px(fill_breadth), px(fill_breadth)).apply_along(axis, |_| px(fill_length)),
    );
    (bounds, THUMB_RADIUS.min(fill_breadth / 2.))
}

impl Jot {
    pub(crate) fn scroll_activity(&mut self, cx: &mut Context<Self>) {
        let chrome = &mut self.scroll;
        chrome.shown = true;
        chrome.hiding = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIDE_DELAY).await;
            this.update(cx, |jot, cx| jot.hide_scrollbars(cx)).ok();
        });
        cx.notify();
    }

    fn hide_scrollbars(&mut self, cx: &mut Context<Self>) {
        let chrome = &mut self.scroll;
        if chrome.press.is_some() || chrome.bars.iter().any(|bar| bar.hovered) {
            return;
        }
        chrome.shown = false;
        cx.notify();
    }

    fn hover_bar(&mut self, axis: Axis, hovered: bool, cx: &mut Context<Self>) {
        let bar = self.scroll.bar_mut(axis);
        bar.hovered = hovered;
        if !hovered {
            bar.hovered_part = None;
            self.scroll_activity(cx);
        }
        cx.notify();
    }

    fn scroll_by(&mut self, axis: Axis, delta: f32, cx: &mut Context<Self>) {
        self.active_editor().update(cx, |editor, cx| {
            let offset = editor
                .scroll_offset()
                .apply_along(axis, |offset| offset - px(delta));
            editor.set_scroll_offset(offset, cx);
        });
    }

    fn step(&mut self, axis: Axis, part: Part, cx: &mut Context<Self>) {
        let metrics = metrics(self.active_editor().read(cx), axis);
        let delta = match part {
            Part::Decrease => -LINE_DELTA,
            Part::Increase => LINE_DELTA,
            Part::PageDecrease => -metrics.viewport,
            Part::PageIncrease => metrics.viewport,
            Part::Thumb => 0.,
        };
        self.scroll_by(axis, delta, cx);
    }

    fn press_bar(&mut self, axis: Axis, part: Part, pointer: f32, cx: &mut Context<Self>) {
        let metrics = metrics(self.active_editor().read(cx), axis);
        self.scroll.press = Some(Press {
            axis,
            part,
            pointer,
            offset: metrics.offset,
        });
        if part != Part::Thumb {
            self.step(axis, part, cx);
            self.scroll.repeating = repeat(cx, REPEAT_DELAY, REPEAT_INTERVAL, move |jot, cx| {
                let held = jot.scroll.press.is_some_and(|press| press.part == part);
                if held {
                    jot.step(axis, part, cx);
                }
                held
            });
        }
        self.scroll_activity(cx);
    }

    fn drag_bar(&mut self, position: Point<Pixels>, length: f32, cx: &mut Context<Self>) {
        let Some(press) = self.scroll.press.filter(|press| press.part == Part::Thumb) else {
            return;
        };
        let metrics = metrics(self.active_editor().read(cx), press.axis);
        let track = track(length, &metrics);
        let travel = (track.length - track.thumb_length).max(1.);
        let pointer = position.along(press.axis).as_f32();
        let target = press.offset + (pointer - press.pointer) * metrics.range() / travel;
        let delta = target - metrics.offset;
        self.scroll_by(press.axis, delta, cx);
    }

    fn release_bar(&mut self, cx: &mut Context<Self>) {
        if self.scroll.press.take().is_some() {
            self.scroll.repeating = Task::ready(());
            self.scroll_activity(cx);
        }
    }

    pub(crate) fn sample_scrollbars(&mut self, cx: &mut Context<Self>) {
        let offset = self.active_editor().read(cx).scroll_offset();
        if self.scroll.last_offset.is_some_and(|last| last != offset) {
            self.scroll_activity(cx);
        }
        self.scroll.last_offset = Some(offset);
    }

    fn arrow(
        &self,
        axis: Axis,
        part: Part,
        opacity: f32,
        length: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = palette(cx);
        let bar = self.scroll.bar(axis);
        let pressed = self
            .scroll
            .press
            .is_some_and(|press| press.axis == axis && press.part == part);
        let hovered = bar.hovered_part == Some(part);
        let color = if pressed || hovered {
            p.text_secondary
        } else {
            p.scrollbar_thumb
        };
        let glyph = match (axis, part) {
            (Axis::Vertical, Part::Decrease) => "\u{EDDB}",
            (Axis::Vertical, _) => "\u{EDDC}",
            (Axis::Horizontal, Part::Decrease) => CARET_LEFT,
            (Axis::Horizontal, _) => CARET_RIGHT,
        };
        let glyph_size = if pressed {
            ARROW_GLYPH * ARROW_PRESSED_SCALE
        } else {
            ARROW_GLYPH
        };
        let decrease = part == Part::Decrease;
        let start = if decrease { 0. } else { length - ARROW };
        let origin = point(0., 0.).apply_along(axis, |_| start);
        let extent = size(BAR_BREADTH, BAR_BREADTH).apply_along(axis, |_| ARROW);
        let padding = |edge: bool| if edge { ARROW_PADDING } else { 0. };
        div()
            .absolute()
            .left(px(origin.x))
            .top(px(origin.y))
            .w(px(extent.width))
            .h(px(extent.height))
            .when(axis == Axis::Vertical, |this| {
                this.pt(px(padding(decrease))).pb(px(padding(!decrease)))
            })
            .when(axis == Axis::Horizontal, |this| {
                this.pl(px(padding(decrease))).pr(px(padding(!decrease)))
            })
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity)
            .font_family(ICON_FONT)
            .text_size(px(glyph_size))
            .line_height(px(glyph_size))
            .text_color(rgba(color))
            .child(glyph)
    }

    fn bar_motion(
        &self,
        axis: Axis,
        always: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(f32, f32, f32)> {
        let shown = always || self.scroll.shown;
        let visibility = transition(
            ElementId::from(("scrollbar-visibility", axis as usize)),
            if shown { 1. } else { 0. },
            Transition::new(if shown {
                Duration::ZERO
            } else {
                OPACITY_CHANGE
            })
            .easing(Easing::Linear),
            window,
            cx,
        );
        if visibility <= 0. && !shown {
            return None;
        }
        let (target, delay) = if always {
            (1., Duration::ZERO)
        } else if self.scroll.bar(axis).hovered && shown {
            (1., EXPAND_BEGIN)
        } else {
            (0., CONTRACT_BEGIN)
        };
        let expansion = transition(
            ElementId::from(("scrollbar-expansion", axis as usize)),
            target,
            Transition::new(EXPAND).delay(delay).easing(DECELERATE),
            window,
            cx,
        );
        let chrome = transition(
            ElementId::from(("scrollbar-chrome", axis as usize)),
            target,
            Transition::new(OPACITY_CHANGE)
                .delay(delay)
                .easing(Easing::Linear),
            window,
            cx,
        );
        Some((visibility, expansion, chrome))
    }

    fn scrollbar(
        &self,
        axis: Axis,
        metrics: Metrics,
        bounds: Bounds<Pixels>,
        always: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let p = palette(cx);
        if !metrics.scrollable() {
            return None;
        }
        let (visibility, expansion, chrome) = self.bar_motion(axis, always, window, cx)?;
        let origin = self.scroll.area.get().origin + bounds.origin;
        let length = bounds.size.along(axis).as_f32();
        let track = track(length, &metrics);
        let (thumb, radius) = thumb(axis, &track, expansion);
        let noise = acrylic_noise();
        let tiles = (length / NOISE_SIZE).ceil() as usize;
        let id = match axis {
            Axis::Vertical => "scrollbar-vertical",
            Axis::Horizontal => "scrollbar-horizontal",
        };
        let along =
            move |position: Point<Pixels>| (position.along(axis) - origin.along(axis)).as_f32();
        let thumb_range = track.thumb_start..track.thumb_start + track.thumb_length;
        let part_at = move |at: f32| {
            if at < ARROW {
                Part::Decrease
            } else if at >= length - ARROW {
                Part::Increase
            } else if at < thumb_range.start {
                Part::PageDecrease
            } else if at < thumb_range.end {
                Part::Thumb
            } else {
                Part::PageIncrease
            }
        };
        Some(
            div()
                .id(id)
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .opacity(visibility)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.hover_bar(axis, *hovered, cx)
                }))
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    let part = Some(part_at(along(event.position)));
                    let bar = this.scroll.bar_mut(axis);
                    if bar.hovered_part != part {
                        bar.hovered_part = part;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        let at = along(event.position);
                        this.press_bar(axis, part_at(at), at, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .size_full()
                        .rounded(px(TRACK_RADIUS))
                        .overflow_hidden()
                        .opacity(chrome)
                        .bg(rgb(p.editor))
                        .child(div().absolute().size_full().bg(rgba(p.acrylic_luminosity)))
                        .children((0..tiles).map(|tile| {
                            let offset = tile as f32 * NOISE_SIZE;
                            img(noise.clone())
                                .absolute()
                                .when(axis == Axis::Vertical, |this| this.top(px(offset)))
                                .when(axis == Axis::Horizontal, |this| this.left(px(offset)))
                                .size(px(NOISE_SIZE))
                                .opacity(NOISE_OPACITY)
                        })),
                )
                .child(self.arrow(axis, Part::Decrease, chrome, length, cx))
                .child(self.arrow(axis, Part::Increase, chrome, length, cx))
                .child(
                    div()
                        .absolute()
                        .left(thumb.origin.x)
                        .top(thumb.origin.y)
                        .w(thumb.size.width)
                        .h(thumb.size.height)
                        .rounded(px(radius))
                        .bg(rgba(p.scrollbar_thumb)),
                ),
        )
    }

    pub(crate) fn scrollbars(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let area = self.scroll.area.get().size;
        let always = !auto_hide();
        let editor = self.active_editor().read(cx);
        let vertical = metrics(editor, Axis::Vertical);
        let horizontal = metrics(editor, Axis::Horizontal);
        let (width, height) = (area.width.as_f32(), area.height.as_f32());
        let reserve = |other: bool| if other { BAR_BREADTH + 2. * MARGIN } else { 0. };
        let vertical_bounds = Bounds::new(
            point(px(width - MARGIN - BAR_BREADTH), px(MARGIN)),
            size(
                px(BAR_BREADTH),
                px(height - 2. * MARGIN - reserve(horizontal.scrollable())),
            ),
        );
        let horizontal_bounds = Bounds::new(
            point(px(MARGIN), px(height - MARGIN - BAR_BREADTH)),
            size(
                px(width - 2. * MARGIN - reserve(vertical.scrollable())),
                px(BAR_BREADTH),
            ),
        );
        let pressed = self.scroll.press.map(|press| press.axis);
        let drag_length = match pressed {
            Some(Axis::Vertical) => vertical_bounds.size.height.as_f32(),
            Some(Axis::Horizontal) => horizontal_bounds.size.width.as_f32(),
            None => 0.,
        };
        let jot = cx.weak_entity();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(self.scrollbar(
                Axis::Vertical,
                vertical,
                vertical_bounds,
                always,
                window,
                cx,
            ))
            .children(self.scrollbar(
                Axis::Horizontal,
                horizontal,
                horizontal_bounds,
                always,
                window,
                cx,
            ))
            .when(pressed.is_some(), |this| {
                this.child(window_listeners(move |window| {
                    let moved = jot.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            moved
                                .update(cx, |jot, cx| jot.drag_bar(event.position, drag_length, cx))
                                .ok();
                        }
                    });
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                            jot.update(cx, |jot, cx| jot.release_bar(cx)).ok();
                        }
                    });
                }))
            })
    }
}
