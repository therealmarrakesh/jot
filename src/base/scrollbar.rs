use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    Anchor, App, Axis, Bounds, ContentMask, CursorStyle, Element, ElementId, EntityId,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, IntoElement, LayoutId, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Position, ScrollHandle, ScrollWheelEvent, Size,
    Style, Window, fill, point, px, relative, rgba, size,
};

use crate::base::motion::{Easing, TimedTransition};
use crate::theme::palette;

const TRACK_WIDTH: Pixels = px(16.);
const MIN_THUMB_SIZE: Pixels = px(48.);
const THUMB_INSET: Pixels = px(3.);
const THUMB_REST_WIDTH: Pixels = px(6.);
const THUMB_ACTIVE_WIDTH: Pixels = px(8.);
const THUMB_REST_RADIUS: Pixels = px(3.);
const THUMB_ACTIVE_RADIUS: Pixels = px(4.);

const IDLE_HOLD: Duration = Duration::from_secs(2);
const ENTER_DURATION: Duration = Duration::from_millis(120);
const EXIT_DURATION: Duration = Duration::from_millis(200);
const EXPAND_DURATION: Duration = Duration::from_millis(120);
const DRAG_MIN_INTERVAL: Duration = Duration::from_micros(8_333);

#[inline]
fn clamp_thumb_radius(radius: Pixels, bounds: Bounds<Pixels>) -> Pixels {
    radius
        .min(bounds.size.width / 2.)
        .min(bounds.size.height / 2.)
}

/// Longitudinal geometry shared by painting, track clicks, and thumb dragging.
#[derive(Clone, Copy)]
struct ThumbGeometry {
    origin: Pixels,
    inset: Pixels,
    length: Pixels,
    travel: Pixels,
    extent: Pixels,
}

impl ThumbGeometry {
    fn new(
        origin: Pixels,
        container: Pixels,
        content: Pixels,
        margin_end: Pixels,
    ) -> Self {
        let track = (container - margin_end).max(px(0.));
        let logical_length = (container / content * container)
            .max(MIN_THUMB_SIZE)
            .min(track);
        let inset = THUMB_INSET.clamp(px(0.), logical_length / 2.);
        Self {
            origin,
            inset,
            length: logical_length - inset * 2.,
            travel: track - logical_length,
            extent: content - container,
        }
    }

    fn start(self, offset: Pixels) -> Pixels {
        self.origin + self.inset + (-offset / self.extent).clamp(0., 1.) * self.travel
    }

    fn offset(self, position: Pixels, grab: Pixels) -> Pixels {
        if self.travel <= px(0.) {
            return px(0.);
        }
        -self.extent * ((position - self.origin - self.inset - grab) / self.travel).clamp(0., 1.)
    }

    fn drag_offset(
        self,
        axis: Axis,
        position: Point<Pixels>,
        grab: Point<Pixels>,
        mut offset: Point<Pixels>,
    ) -> Point<Pixels> {
        if self.travel <= px(0.) {
            return offset;
        }
        match axis {
            Axis::Vertical => offset.y = self.offset(position.y, grab.y),
            Axis::Horizontal => offset.x = self.offset(position.x, grab.x),
        }
        offset
    }
}

#[derive(Clone, Copy)]
struct ScrollbarStateInner {
    hovered_axis: Option<Axis>,
    hovered_thumb: Option<Axis>,
    dragged_axis: Option<Axis>,
    drag_pos: Point<Pixels>,
    last_scroll_offset: Point<Pixels>,
    last_scroll_time: Option<Instant>,
    last_drag_notify: Instant,
    drag_notify_scheduled: bool,
    drag_notify_generation: u64,
    idle_timer_scheduled: bool,
    opacity: TimedTransition<f32>,
    vertical_width: TimedTransition<Pixels>,
    horizontal_width: TimedTransition<Pixels>,
}

impl ScrollbarStateInner {
    fn new(now: Instant) -> Self {
        Self {
            hovered_axis: None,
            hovered_thumb: None,
            dragged_axis: None,
            drag_pos: point(px(0.), px(0.)),
            last_scroll_offset: point(px(0.), px(0.)),
            last_scroll_time: None,
            last_drag_notify: now,
            drag_notify_scheduled: false,
            drag_notify_generation: 0,
            idle_timer_scheduled: false,
            opacity: TimedTransition::settled(0.0, now),
            vertical_width: TimedTransition::settled(THUMB_REST_WIDTH, now),
            horizontal_width: TimedTransition::settled(THUMB_REST_WIDTH, now),
        }
    }

    fn wants_visible(&self, now: Instant) -> bool {
        let hovered = self.hovered_axis.is_some() || self.hovered_thumb.is_some();
        let dragging = self.dragged_axis.is_some();
        let currently_visible = self.opacity.sample(now).0 > 0.0;
        if hovered && currently_visible {
            return true;
        }
        dragging
            || self
                .last_scroll_time
                .is_some_and(|t| now.saturating_duration_since(t) < IDLE_HOLD)
    }

    fn update_visibility(&mut self, visible: bool, reduce_motion: bool, now: Instant) -> (f32, bool) {
        let target = if visible { 1.0 } else { 0.0 };
        if reduce_motion {
            self.opacity.settle(target, now);
        } else if self.opacity.target != target {
            let from = self.opacity.sample(now).0;
            let (duration, easing) = if visible {
                (ENTER_DURATION, Easing::Linear)
            } else {
                (EXIT_DURATION, Easing::EaseInCubic)
            };
            self.opacity.start(from, target, duration, easing, now);
        }
        self.opacity.sample(now)
    }

    fn update_width(
        &mut self,
        axis: Axis,
        target: Pixels,
        reduce_motion: bool,
        now: Instant,
    ) -> (Pixels, bool) {
        let transition = match axis {
            Axis::Vertical => &mut self.vertical_width,
            Axis::Horizontal => &mut self.horizontal_width,
        };
        if reduce_motion {
            transition.settle(target, now);
        } else if transition.target != target {
            let from = transition.sample(now).0;
            transition.start(from, target, EXPAND_DURATION, Easing::EaseOutCubic, now);
        }
        transition.sample(now)
    }
}

#[derive(Clone)]
struct ScrollbarState(Rc<Cell<ScrollbarStateInner>>);

impl Default for ScrollbarState {
    fn default() -> Self {
        Self(Rc::new(Cell::new(ScrollbarStateInner::new(Instant::now()))))
    }
}

impl ScrollbarState {
    fn get(&self) -> ScrollbarStateInner {
        self.0.get()
    }

    fn set(&self, inner: ScrollbarStateInner) {
        self.0.set(inner);
    }

    fn notify_drag(&self, now: Instant, view_id: EntityId, window: &mut Window, cx: &mut App) {
        let mut inner = self.get();
        let elapsed = now.saturating_duration_since(inner.last_drag_notify);
        if elapsed >= DRAG_MIN_INTERVAL {
            inner.last_drag_notify = now;
            inner.drag_notify_scheduled = false;
            inner.drag_notify_generation = inner.drag_notify_generation.wrapping_add(1);
            self.set(inner);
            cx.notify(view_id);
            return;
        }
        if inner.drag_notify_scheduled {
            return;
        }
        inner.drag_notify_scheduled = true;
        let generation = inner.drag_notify_generation;
        self.set(inner);
        let state = self.clone();
        let delay = DRAG_MIN_INTERVAL - elapsed;
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(delay).await;
                let mut current = state.get();
                if !current.drag_notify_scheduled || current.drag_notify_generation != generation {
                    return;
                }
                current.drag_notify_scheduled = false;
                current.last_drag_notify = Instant::now();
                state.set(current);
                cx.update(|_, cx| cx.notify(view_id)).ok();
            })
            .detach();
    }
}

/// Overlay scrollbar for Jot's multi-line text editor.
pub(crate) struct Scrollbar {
    scroll_handle: ScrollHandle,
    viewport_bounds: Bounds<Pixels>,
    scroll_size: Size<Pixels>,
    horizontal: bool,
}

impl Scrollbar {
    pub(crate) fn new(
        scroll_handle: ScrollHandle,
        viewport_bounds: Bounds<Pixels>,
        scroll_size: Size<Pixels>,
        horizontal: bool,
    ) -> Self {
        Self {
            scroll_handle,
            viewport_bounds,
            scroll_size,
            horizontal,
        }
    }
}

impl IntoElement for Scrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct AxisLayout {
    axis: Axis,
    bar_hitbox: Hitbox,
    bounds: Bounds<Pixels>,
    thumb_bounds: Bounds<Pixels>,
    thumb_fill: Bounds<Pixels>,
    thumb_bg: Hsla,
    radius: Pixels,
    geometry: ThumbGeometry,
    opacity: f32,
    visible: bool,
}

pub(crate) struct PrepaintState {
    viewport_bounds: Bounds<Pixels>,
    state: ScrollbarState,
    axes: Vec<AxisLayout>,
}

impl Element for Scrollbar {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("jot-scrollbar".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let viewport = self.viewport_bounds;
        let state = window
            .use_state(cx, |_, _| ScrollbarState::default())
            .read(cx)
            .clone();

        let now = Instant::now();
        let reduce_motion = cx.reduce_motion();
        let mut inner = state.get();

        let current_offset = self.scroll_handle.offset();
        if current_offset != inner.last_scroll_offset {
            inner.last_scroll_offset = current_offset;
            inner.last_scroll_time = Some(now);
        }

        let visible = inner.wants_visible(now);
        let (opacity, opacity_running) = inner.update_visibility(visible, reduce_motion, now);
        if opacity_running {
            window.request_animation_frame();
        }

        let is_hovered = inner.hovered_axis.is_some() || inner.hovered_thumb.is_some();
        if !is_hovered && inner.dragged_axis.is_none() {
            if let Some(last_time) = inner.last_scroll_time {
                let elapsed = now.saturating_duration_since(last_time);
                if elapsed < IDLE_HOLD && !inner.idle_timer_scheduled {
                    inner.idle_timer_scheduled = true;
                    let state = state.clone();
                    let view_id = window.current_view();
                    let delay = IDLE_HOLD - elapsed;
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(delay).await;
                            let mut s = state.get();
                            s.idle_timer_scheduled = false;
                            state.set(s);
                            cx.update(|_, cx| cx.notify(view_id)).ok();
                        })
                        .detach();
                }
            }
        }

        let p = palette(cx);
        let vertical_needed = self.scroll_size.height > viewport.size.height;
        let mut axes_to_check = Vec::with_capacity(2);
        if self.horizontal && self.scroll_size.width > viewport.size.width {
            axes_to_check.push(Axis::Horizontal);
        }
        if vertical_needed {
            axes_to_check.push(Axis::Vertical);
        }

        let mut axes = Vec::with_capacity(axes_to_check.len());
        for axis in axes_to_check {
            let is_vertical = matches!(axis, Axis::Vertical);
            let (content, container, scroll_pos) = if is_vertical {
                (self.scroll_size.height, viewport.size.height, current_offset.y)
            } else {
                (self.scroll_size.width, viewport.size.width, current_offset.x)
            };
            let margin_end = if !is_vertical && vertical_needed {
                TRACK_WIDTH
            } else {
                px(0.)
            };

            let bounds = if is_vertical {
                Bounds::new(
                    point(viewport.right() - TRACK_WIDTH, viewport.top()),
                    size(TRACK_WIDTH, viewport.size.height),
                )
            } else {
                Bounds::new(
                    point(viewport.left(), viewport.bottom() - TRACK_WIDTH),
                    size(viewport.size.width, TRACK_WIDTH),
                )
            };

            let is_dragged = inner.dragged_axis == Some(axis);
            let is_thumb_hovered = inner.hovered_thumb == Some(axis);
            let (thumb_bg, target_width, radius) = if is_dragged {
                (
                    Hsla::from(rgba(p.text_primary)),
                    THUMB_ACTIVE_WIDTH,
                    THUMB_ACTIVE_RADIUS,
                )
            } else if is_thumb_hovered {
                (
                    Hsla::from(rgba(p.text_secondary)),
                    THUMB_ACTIVE_WIDTH,
                    THUMB_ACTIVE_RADIUS,
                )
            } else {
                (
                    Hsla::from(rgba(p.text_tertiary)),
                    THUMB_REST_WIDTH,
                    THUMB_REST_RADIUS,
                )
            };

            let (thumb_width, width_running) =
                inner.update_width(axis, target_width, reduce_motion, now);
            if width_running {
                window.request_animation_frame();
            }

            let origin = if is_vertical {
                bounds.origin.y
            } else {
                bounds.origin.x
            };
            let geometry = ThumbGeometry::new(origin, container, content, margin_end);
            let thumb_start = geometry.start(scroll_pos) - origin;
            let thumb_len = geometry.length;

            let (thumb_bounds, thumb_fill) = if is_vertical {
                (
                    Bounds::from_anchor_and_size(
                        Anchor::TopRight,
                        bounds.top_right() + point(-THUMB_INSET, thumb_start),
                        size(TRACK_WIDTH, thumb_len),
                    ),
                    Bounds::from_anchor_and_size(
                        Anchor::TopRight,
                        bounds.top_right() + point(-THUMB_INSET, thumb_start),
                        size(thumb_width, thumb_len),
                    ),
                )
            } else {
                (
                    Bounds::from_anchor_and_size(
                        Anchor::BottomLeft,
                        bounds.bottom_left() + point(thumb_start, -THUMB_INSET),
                        size(thumb_len, TRACK_WIDTH),
                    ),
                    Bounds::from_anchor_and_size(
                        Anchor::BottomLeft,
                        bounds.bottom_left() + point(thumb_start, -THUMB_INSET),
                        size(thumb_len, thumb_width),
                    ),
                )
            };

            let bar_hitbox = window.with_content_mask(Some(ContentMask { bounds }), |window| {
                window.insert_hitbox(bounds, HitboxBehavior::Normal)
            });

            axes.push(AxisLayout {
                axis,
                bar_hitbox,
                bounds,
                thumb_bounds,
                thumb_fill,
                thumb_bg,
                radius: clamp_thumb_radius(radius, thumb_fill),
                geometry,
                opacity,
                visible: visible || opacity > 0.0,
            });
        }

        state.set(inner);

        PrepaintState {
            viewport_bounds: viewport,
            state,
            axes,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let view_id = window.current_view();
        let viewport = prepaint.viewport_bounds;

        window.with_content_mask(Some(ContentMask { bounds: viewport }), |window| {
            for item in &prepaint.axes {
                let axis = item.axis;
                let bounds = item.bounds;
                let thumb_bounds = item.thumb_bounds;
                let geometry = item.geometry;
                let is_visible = item.visible;

                window.set_cursor_style(CursorStyle::default(), &item.bar_hitbox);
                if item.opacity > 0.0 {
                    window.paint_layer(viewport, |window| {
                        window.paint_quad(
                            fill(item.thumb_fill, item.thumb_bg.opacity(item.opacity))
                                .corner_radii(item.radius),
                        );
                    });
                }

                window.on_mouse_event({
                    let state = prepaint.state.clone();
                    let handle = self.scroll_handle.clone();
                    move |event: &ScrollWheelEvent, phase, _, cx| {
                        if phase.bubble() && viewport.contains(&event.position) {
                            let offset = handle.offset();
                            let mut inner = state.get();
                            if offset != inner.last_scroll_offset {
                                inner.last_scroll_offset = offset;
                                inner.last_scroll_time = Some(Instant::now());
                                state.set(inner);
                                cx.notify(view_id);
                            }
                        }
                    }
                });

                if is_visible {
                    window.on_mouse_event({
                        let state = prepaint.state.clone();
                        let handle = self.scroll_handle.clone();
                        move |event: &MouseDownEvent, phase, _, cx| {
                            if phase.bubble() && bounds.contains(&event.position) {
                                cx.stop_propagation();
                                if thumb_bounds.contains(&event.position) {
                                    let grab = event.position - thumb_bounds.origin;
                                    let mut inner = state.get();
                                    inner.dragged_axis = Some(axis);
                                    inner.drag_pos = grab;
                                    state.set(inner);
                                    handle.set_offset(geometry.drag_offset(
                                        axis,
                                        event.position,
                                        grab,
                                        handle.offset(),
                                    ));
                                } else {
                                    let center = point(geometry.length / 2., geometry.length / 2.);
                                    handle.set_offset(geometry.drag_offset(
                                        axis,
                                        event.position,
                                        center,
                                        handle.offset(),
                                    ));
                                }
                                cx.notify(view_id);
                            }
                        }
                    });
                }

                window.on_mouse_event({
                    let state = prepaint.state.clone();
                    let handle = self.scroll_handle.clone();
                    move |event: &MouseMoveEvent, _, window, cx| {
                        let mut inner = state.get();
                        let mut notify = false;

                        if bounds.contains(&event.position) && is_visible {
                            if inner.hovered_axis != Some(axis) {
                                inner.hovered_axis = Some(axis);
                                notify = true;
                            }
                            inner.last_scroll_time = Some(Instant::now());
                        } else if inner.hovered_axis == Some(axis) {
                            inner.hovered_axis = None;
                            inner.last_scroll_time = Some(Instant::now());
                            notify = true;
                        }

                        let thumb_hovered = is_visible && thumb_bounds.contains(&event.position);
                        if thumb_hovered {
                            if inner.hovered_thumb != Some(axis) {
                                inner.hovered_thumb = Some(axis);
                                notify = true;
                            }
                        } else if inner.hovered_thumb == Some(axis) {
                            inner.hovered_thumb = None;
                            notify = true;
                        }

                        state.set(inner);

                        if inner.dragged_axis == Some(axis) && event.dragging() {
                            cx.stop_propagation();
                            let next = geometry.drag_offset(
                                axis,
                                event.position,
                                inner.drag_pos,
                                handle.offset(),
                            );
                            if handle.offset() != next {
                                handle.set_offset(next);
                                notify = true;
                            }
                            if notify {
                                state.notify_drag(Instant::now(), view_id, window, cx);
                            }
                            return;
                        }

                        if notify {
                            cx.notify(view_id);
                        }
                    }
                });

                window.on_mouse_event({
                    let state = prepaint.state.clone();
                    let handle = self.scroll_handle.clone();
                    move |event: &MouseUpEvent, phase, _, cx| {
                        let mut inner = state.get();
                        if phase.bubble() && inner.dragged_axis == Some(axis) {
                            handle.set_offset(geometry.drag_offset(
                                axis,
                                event.position,
                                inner.drag_pos,
                                handle.offset(),
                            ));
                            inner.dragged_axis = None;
                            inner.last_scroll_time = Some(Instant::now());
                            state.set(inner);
                            cx.notify(view_id);
                        }
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_radius_is_limited_by_its_actual_bounds() {
        let vertical = Bounds::new(Point::default(), size(px(8.), px(80.)));
        let horizontal = Bounds::new(Point::default(), size(px(80.), px(6.)));

        assert_eq!(clamp_thumb_radius(px(6.), vertical), px(4.));
        assert_eq!(clamp_thumb_radius(px(6.), horizontal), px(3.));
        assert_eq!(clamp_thumb_radius(Pixels::ZERO, vertical), Pixels::ZERO);
    }

    #[test]
    fn visibility_animates_enter_exit_and_snaps_on_reduce_motion() {
        let start = Instant::now();
        let mut inner = ScrollbarStateInner::new(start);

        let (op, running) = inner.update_visibility(true, false, start);
        assert_eq!(op, 0.0);
        assert!(running);
        let (mid, _) = inner.opacity.sample(start + ENTER_DURATION / 2);
        assert!((mid - 0.5).abs() < 0.01);
        assert_eq!(inner.opacity.sample(start + ENTER_DURATION), (1.0, false));

        let (op_exit, running) = inner.update_visibility(false, false, start + ENTER_DURATION);
        assert_eq!(op_exit, 1.0);
        assert!(running);
        let (mid_exit, _) = inner
            .opacity
            .sample(start + ENTER_DURATION + EXIT_DURATION / 2);
        assert!(mid_exit > 0.5, "ease-in must remain visible early in exit");
        assert_eq!(
            inner.opacity.sample(start + ENTER_DURATION + EXIT_DURATION),
            (0.0, false)
        );

        let (snapped, running) = inner.update_visibility(true, true, start);
        assert_eq!(snapped, 1.0);
        assert!(!running);
    }

    #[test]
    fn thumb_width_expands_and_snaps_under_reduced_motion() {
        let start = Instant::now();
        let mut inner = ScrollbarStateInner::new(start);

        let (w0, running) = inner.update_width(Axis::Vertical, THUMB_ACTIVE_WIDTH, false, start);
        assert_eq!(w0, THUMB_REST_WIDTH);
        assert!(running);
        let (w_mid, _) = inner.vertical_width.sample(start + EXPAND_DURATION / 2);
        assert!(w_mid > THUMB_REST_WIDTH && w_mid < THUMB_ACTIVE_WIDTH);
        assert_eq!(
            inner.vertical_width.sample(start + EXPAND_DURATION),
            (THUMB_ACTIVE_WIDTH, false)
        );

        let (snapped, running) = inner.update_width(Axis::Vertical, THUMB_REST_WIDTH, true, start);
        assert_eq!(snapped, THUMB_REST_WIDTH);
        assert!(!running);
    }

    #[test]
    fn full_track_thumb_preserves_scroll_offset() {
        let geometry = ThumbGeometry::new(px(0.), px(200.), px(250.), px(0.));
        assert!(geometry.travel > px(0.));
        let dragged = geometry.drag_offset(
            Axis::Vertical,
            point(px(0.), px(100.)),
            point(px(0.), px(10.)),
            point(px(-5.), px(0.)),
        );
        assert_eq!(dragged.x, px(-5.));
        assert!(dragged.y < px(0.));

        let zero_travel = ThumbGeometry::new(px(0.), px(40.), px(100.), px(0.));
        assert_eq!(zero_travel.travel, px(0.));
        let kept = zero_travel.drag_offset(
            Axis::Vertical,
            point(px(0.), px(20.)),
            point(px(0.), px(5.)),
            point(px(0.), px(-12.)),
        );
        assert_eq!(kept.y, px(-12.));
    }
}
