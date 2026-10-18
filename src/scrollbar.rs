use super::*;
use gpui_kit::base::{Transition, transition_with_status};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

const SIZE: f32 = 12.;
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Decrease,
    PageDecrease,
    Thumb,
    PageIncrease,
    Increase,
}

struct Bar {
    hovered: bool,
    hovered_part: Option<Part>,
}

impl Bar {
    fn new() -> Self {
        Self {
            hovered: false,
            hovered_part: None,
        }
    }
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
    area: Rc<Cell<Bounds<Pixels>>>,
    shown: bool,
    press: Option<Press>,
    last_offset: Option<Point<Pixels>>,
    hiding: Task<()>,
    repeating: Task<()>,
}

impl ScrollChrome {
    pub(crate) fn new() -> Self {
        Self {
            bars: [Bar::new(), Bar::new()],
            area: Rc::new(Cell::new(Bounds::default())),
            shown: false,
            press: None,
            last_offset: None,
            hiding: Task::ready(()),
            repeating: Task::ready(()),
        }
    }

    fn bar(&mut self, axis: Axis) -> &mut Bar {
        &mut self.bars[axis as usize]
    }
}

fn auto_hide() -> bool {
    let key: Vec<u16> = r"Control Panel\Accessibility"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let value: Vec<u16> = "DynamicScrollbars".encode_utf16().chain(Some(0)).collect();
    let mut data = 1u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
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
    match axis {
        Axis::Vertical => Metrics {
            viewport: viewport.height.as_f32(),
            content: content.height.as_f32(),
            offset: -offset.y.as_f32(),
        },
        Axis::Horizontal => Metrics {
            viewport: viewport.width.as_f32(),
            content: content.width.as_f32(),
            offset: -offset.x.as_f32(),
        },
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
        let bar = self.scroll.bar(axis);
        bar.hovered = hovered;
        if !hovered {
            bar.hovered_part = None;
        }
        if !hovered {
            self.scroll_activity(cx);
        }
        cx.notify();
    }

    fn scroll_by(&mut self, axis: Axis, delta: f32, cx: &mut Context<Self>) {
        let editor = self.active_editor().clone();
        editor.update(cx, |editor, cx| {
            let offset = editor.scroll_offset();
            let offset = match axis {
                Axis::Vertical => point(offset.x, offset.y - px(delta)),
                Axis::Horizontal => point(offset.x - px(delta), offset.y),
            };
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
            self.scroll.repeating = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(REPEAT_DELAY).await;
                loop {
                    let held = this
                        .update(cx, |jot, cx| {
                            let held = jot.scroll.press.is_some_and(|press| press.part == part);
                            if held {
                                jot.step(axis, part, cx);
                            }
                            held
                        })
                        .unwrap_or(false);
                    if !held {
                        break;
                    }
                    cx.background_executor().timer(REPEAT_INTERVAL).await;
                }
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
        let pointer = match press.axis {
            Axis::Vertical => position.y.as_f32(),
            Axis::Horizontal => position.x.as_f32(),
        };
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

    pub(crate) fn scroll_area(&self) -> impl IntoElement + use<> {
        let area = self.scroll.area.clone();
        canvas(move |bounds, _, _| area.set(bounds), |_, _, _, _| {})
            .absolute()
            .size_full()
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
        let bar = &self.scroll.bars[axis as usize];
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
            (Axis::Horizontal, Part::Decrease) => "\u{EDD9}",
            (Axis::Horizontal, _) => "\u{EDDA}",
        };
        let size = if pressed {
            ARROW_GLYPH * ARROW_PRESSED_SCALE
        } else {
            ARROW_GLYPH
        };
        let start = if part == Part::Decrease {
            0.
        } else {
            length - ARROW
        };
        let (left, top, w, h) = match axis {
            Axis::Vertical => (0., start, SIZE, ARROW),
            Axis::Horizontal => (start, 0., ARROW, SIZE),
        };
        let padding = |edge: bool| if edge { ARROW_PADDING } else { 0. };
        let decrease = part == Part::Decrease;
        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(w))
            .h(px(h))
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
            .text_size(px(size))
            .line_height(px(size))
            .text_color(rgba(color))
            .child(glyph)
    }

    fn scrollbar(
        &self,
        axis: Axis,
        bounds: Bounds<Pixels>,
        always: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let p = palette(cx);
        let metrics = metrics(self.active_editor().read(cx), axis);
        if !metrics.scrollable() {
            return None;
        }
        let bar = &self.scroll.bars[axis as usize];
        let shown = always || self.scroll.shown;
        let visibility = transition_with_status(
            ElementId::from(("scrollbar-visibility", axis as usize)),
            if shown { 1. } else { 0. },
            Transition::new(if shown {
                Duration::ZERO
            } else {
                OPACITY_CHANGE
            })
            .ease(|t| t),
            window,
            cx,
        );
        if visibility.value <= 0. && !shown {
            return None;
        }
        let expanded = always || (bar.hovered && shown);
        let (target, delay) = if expanded {
            (1., EXPAND_BEGIN)
        } else {
            (0., CONTRACT_BEGIN)
        };
        let delay = if always { Duration::ZERO } else { delay };
        let expansion = transition_with_status(
            ElementId::from(("scrollbar-expansion", axis as usize)),
            target,
            Transition::new(EXPAND)
                .delay(delay)
                .ease(cubic_bezier(0., 0., 0., 1.)),
            window,
            cx,
        )
        .value;
        let chrome = transition_with_status(
            ElementId::from(("scrollbar-chrome", axis as usize)),
            target,
            Transition::new(OPACITY_CHANGE).delay(delay).ease(|t| t),
            window,
            cx,
        )
        .value;
        let visibility = visibility.value;
        let origin = self.scroll.area.get().origin + bounds.origin;
        let length = match axis {
            Axis::Vertical => bounds.size.height.as_f32(),
            Axis::Horizontal => bounds.size.width.as_f32(),
        };
        let track = track(length, &metrics);
        let breadth = THUMB_COLLAPSED + (SIZE - THUMB_COLLAPSED) * expansion;
        let shift = (SIZE - breadth) / 2. + THUMB_OFFSET * (1. - expansion);
        let fill_breadth = (breadth - 2. * THUMB_STROKE_INSET).max(0.);
        let fill_length = (track.thumb_length - 2. * THUMB_STROKE_INSET).max(0.);
        let radius = THUMB_RADIUS.min(fill_breadth / 2.);
        let (thumb_left, thumb_top, thumb_w, thumb_h) = match axis {
            Axis::Vertical => (
                shift + THUMB_STROKE_INSET,
                track.thumb_start + THUMB_STROKE_INSET,
                fill_breadth,
                fill_length,
            ),
            Axis::Horizontal => (
                track.thumb_start + THUMB_STROKE_INSET,
                shift + THUMB_STROKE_INSET,
                fill_length,
                fill_breadth,
            ),
        };
        let noise = tooltip::acrylic_noise();
        let tiles = (length / tooltip::NOISE_SIZE).ceil() as usize;
        let id = match axis {
            Axis::Vertical => "scrollbar-vertical",
            Axis::Horizontal => "scrollbar-horizontal",
        };
        let along = move |position: Point<Pixels>| match axis {
            Axis::Vertical => (position.y - origin.y).as_f32(),
            Axis::Horizontal => (position.x - origin.x).as_f32(),
        };
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
                    let bar = this.scroll.bar(axis);
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
                        .child(div().absolute().size_full().bg(rgba(p.tooltip_luminosity)))
                        .children((0..tiles).map(|tile| {
                            let offset = tile as f32 * tooltip::NOISE_SIZE;
                            img(noise.clone())
                                .absolute()
                                .when(axis == Axis::Vertical, |this| this.top(px(offset)))
                                .when(axis == Axis::Horizontal, |this| this.left(px(offset)))
                                .size(px(tooltip::NOISE_SIZE))
                                .opacity(tooltip::NOISE_OPACITY)
                        })),
                )
                .child(self.arrow(axis, Part::Decrease, chrome, length, cx))
                .child(self.arrow(axis, Part::Increase, chrome, length, cx))
                .child(
                    div()
                        .absolute()
                        .left(px(thumb_left))
                        .top(px(thumb_top))
                        .w(px(thumb_w))
                        .h(px(thumb_h))
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
        let size = self.scroll.area.get().size;
        let always = !auto_hide();
        let editor = self.active_editor().read(cx);
        let vertical = metrics(editor, Axis::Vertical).scrollable();
        let horizontal = metrics(editor, Axis::Horizontal).scrollable();
        let (width, height) = (size.width.as_f32(), size.height.as_f32());
        let reserve = |other: bool| if other { SIZE + 2. * MARGIN } else { 0. };
        let vertical_bounds = Bounds::new(
            point(px(width - MARGIN - SIZE), px(MARGIN)),
            gpui_kit::size(px(SIZE), px(height - 2. * MARGIN - reserve(horizontal))),
        );
        let horizontal_bounds = Bounds::new(
            point(px(MARGIN), px(height - MARGIN - SIZE)),
            gpui_kit::size(px(width - 2. * MARGIN - reserve(vertical)), px(SIZE)),
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
            .children(self.scrollbar(Axis::Vertical, vertical_bounds, always, window, cx))
            .children(self.scrollbar(Axis::Horizontal, horizontal_bounds, always, window, cx))
            .when(pressed.is_some(), |this| {
                this.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let moved = jot.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    moved
                                        .update(cx, |jot, cx| {
                                            jot.drag_bar(event.position, drag_length, cx)
                                        })
                                        .ok();
                                }
                            });
                            let released = jot.clone();
                            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble
                                    && event.button == MouseButton::Left
                                {
                                    released.update(cx, |jot, cx| jot.release_bar(cx)).ok();
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
    }
}
