//! Clean-room owned overlay scrollbar state, geometry, and painting.

use std::time::{Duration, Instant};

use crate::theme::Palette;
use crate::ui::anim::{Easing, TimedTransition};
use crate::ui::{
    Anchor, Axis, Bounds, ContentMask, CursorStyle, Hitbox, HitboxBehavior, Pixels, Point, Rgba,
    Size, TimerHandle, Window, fill, point, px, rgba, size,
};

const TRACK_THICKNESS: Pixels = px(16.);
const MIN_THUMB_LENGTH: Pixels = px(48.);
const THUMB_MARGIN: Pixels = px(3.);
const THUMB_REST_THICKNESS: Pixels = px(6.);
const THUMB_ACTIVE_THICKNESS: Pixels = px(8.);
const THUMB_REST_RADIUS: Pixels = px(3.);
const THUMB_ACTIVE_RADIUS: Pixels = px(4.);

const IDLE_HOLD: Duration = Duration::from_secs(2);
const FADE_IN_DURATION: Duration = Duration::from_millis(120);
const FADE_OUT_DURATION: Duration = Duration::from_millis(200);
const EXPAND_DURATION: Duration = Duration::from_millis(120);
const DRAG_NOTIFY_INTERVAL: Duration = Duration::from_micros(8_333);

#[inline]
fn clamp_radius(radius: Pixels, bounds: Bounds<Pixels>) -> Pixels {
    radius
        .min(bounds.size.width / 2.)
        .min(bounds.size.height / 2.)
}

/// 1D geometry along a single scrollbar axis shared by painting, track clicks, and dragging.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TrackMetrics {
    origin: Pixels,
    margin: Pixels,
    pub(crate) thumb_len: Pixels,
    travel: Pixels,
    max_scroll: Pixels,
}

impl TrackMetrics {
    pub(crate) fn compute(
        origin: Pixels,
        viewport_span: Pixels,
        content_span: Pixels,
        corner_reserve: Pixels,
    ) -> Self {
        let track_span = (viewport_span - corner_reserve).max(px(0.));
        let raw_thumb = (viewport_span / content_span * viewport_span)
            .max(MIN_THUMB_LENGTH)
            .min(track_span);
        let margin = THUMB_MARGIN.clamp(px(0.), raw_thumb / 2.);
        Self {
            origin,
            margin,
            thumb_len: raw_thumb - margin * 2.,
            travel: track_span - raw_thumb,
            max_scroll: content_span - viewport_span,
        }
    }

    pub(crate) fn thumb_start(self, scroll_offset: Pixels) -> Pixels {
        let ratio = (-scroll_offset / self.max_scroll).clamp(0., 1.);
        self.origin + self.margin + ratio * self.travel
    }

    fn scroll_for_pointer(self, pointer: Pixels, grab: Pixels) -> Pixels {
        if self.travel <= px(0.) {
            return px(0.);
        }
        let ratio = ((pointer - self.origin - self.margin - grab) / self.travel).clamp(0., 1.);
        -self.max_scroll * ratio
    }

    pub(crate) fn apply_drag(
        self,
        axis: Axis,
        pointer: Point<Pixels>,
        grab: Point<Pixels>,
        mut offset: Point<Pixels>,
    ) -> Point<Pixels> {
        if self.travel <= px(0.) {
            return offset;
        }
        match axis {
            Axis::Vertical => offset.y = self.scroll_for_pointer(pointer.y, grab.y),
            Axis::Horizontal => offset.x = self.scroll_for_pointer(pointer.x, grab.x),
        }
        offset
    }
}

/// Viewport inputs supplied by the scrolled content (e.g., `Editor`) each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarViewport {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) content_size: Size<Pixels>,
    pub(crate) horizontal: bool,
}

/// Prepainted geometry for one active scrollbar axis.
#[derive(Clone)]
pub(crate) struct PreparedAxis {
    pub(crate) axis: Axis,
    pub(crate) bar_hitbox: Hitbox,
    pub(crate) track_bounds: Bounds<Pixels>,
    pub(crate) thumb_hit_bounds: Bounds<Pixels>,
    pub(crate) thumb_paint_bounds: Bounds<Pixels>,
    pub(crate) thumb_color: Rgba,
    pub(crate) corner_radius: Pixels,
    pub(crate) metrics: TrackMetrics,
    pub(crate) opacity: f32,
    pub(crate) interactive: bool,
}

/// Result of routing a mouse move through `ScrollbarState`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollbarMoveOutcome {
    pub(crate) new_offset: Option<Point<Pixels>>,
    pub(crate) notify_now: bool,
    pub(crate) stop_propagation: bool,
}

/// Owned scrollbar interaction and animation state (stored directly on `Editor`).
pub(crate) struct ScrollbarState {
    hovered_axis: Option<Axis>,
    hovered_thumb: Option<Axis>,
    dragged_axis: Option<Axis>,
    drag_grab: Point<Pixels>,
    last_offset: Point<Pixels>,
    last_activity: Option<Instant>,
    last_drag_notify: Instant,
    drag_timer_pending: bool,
    drag_timer: TimerHandle,
    idle_timer_pending: bool,
    idle_timer: TimerHandle,
    opacity: TimedTransition<f32>,
    vertical_thickness: TimedTransition<Pixels>,
    horizontal_thickness: TimedTransition<Pixels>,
}

impl Default for ScrollbarState {
    fn default() -> Self {
        Self::new_at(Instant::now())
    }
}

impl ScrollbarState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn new_at(now: Instant) -> Self {
        Self {
            hovered_axis: None,
            hovered_thumb: None,
            dragged_axis: None,
            drag_grab: point(px(0.), px(0.)),
            last_offset: point(px(0.), px(0.)),
            last_activity: None,
            last_drag_notify: now,
            drag_timer_pending: false,
            drag_timer: TimerHandle::default(),
            idle_timer_pending: false,
            idle_timer: TimerHandle::default(),
            opacity: TimedTransition::settled(0.0, now),
            vertical_thickness: TimedTransition::settled(THUMB_REST_THICKNESS, now),
            horizontal_thickness: TimedTransition::settled(THUMB_REST_THICKNESS, now),
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
                .last_activity
                .is_some_and(|t| now.saturating_duration_since(t) < IDLE_HOLD)
    }

    fn step_opacity(&mut self, visible: bool, reduce_motion: bool, now: Instant) -> (f32, bool) {
        let target = if visible { 1.0 } else { 0.0 };
        if reduce_motion {
            self.opacity.settle(target, now);
        } else if self.opacity.target != target {
            let from = self.opacity.sample(now).0;
            let (duration, easing) = if visible {
                (FADE_IN_DURATION, Easing::Linear)
            } else {
                (FADE_OUT_DURATION, Easing::EaseInCubic)
            };
            self.opacity.start(from, target, duration, easing, now);
        }
        self.opacity.sample(now)
    }

    fn step_thickness(
        &mut self,
        axis: Axis,
        target: Pixels,
        reduce_motion: bool,
        now: Instant,
    ) -> (Pixels, bool) {
        let transition = match axis {
            Axis::Vertical => &mut self.vertical_thickness,
            Axis::Horizontal => &mut self.horizontal_thickness,
        };
        if reduce_motion {
            transition.settle(target, now);
        } else if transition.target != target {
            let from = transition.sample(now).0;
            transition.start(from, target, EXPAND_DURATION, Easing::EaseOutCubic, now);
        }
        transition.sample(now)
    }

    pub(crate) fn on_idle_timer_fired(&mut self) {
        self.idle_timer_pending = false;
    }

    pub(crate) fn on_drag_timer_fired(&mut self, now: Instant) -> bool {
        if !self.drag_timer_pending {
            return false;
        }
        self.drag_timer_pending = false;
        self.last_drag_notify = now;
        true
    }

    pub(crate) fn on_wheel(&mut self, offset: Point<Pixels>, now: Instant) -> bool {
        if offset == self.last_offset {
            return false;
        }
        self.last_offset = offset;
        self.last_activity = Some(now);
        self.idle_timer.cancel();
        self.idle_timer_pending = false;
        true
    }

    pub(crate) fn prepare(
        &mut self,
        viewport: ScrollbarViewport,
        scroll_offset: Point<Pixels>,
        reduce_motion: bool,
        p: Palette,
        window: &mut Window,
        schedule_idle: impl FnOnce(Duration) -> TimerHandle,
    ) -> Vec<PreparedAxis> {
        let now = Instant::now();
        if scroll_offset != self.last_offset {
            self.last_offset = scroll_offset;
            self.last_activity = Some(now);
            self.idle_timer.cancel();
            self.idle_timer_pending = false;
        }

        let visible = self.wants_visible(now);
        let (opacity, opacity_running) = self.step_opacity(visible, reduce_motion, now);
        if opacity_running {
            window.request_animation_frame();
        }

        let is_hovered = self.hovered_axis.is_some() || self.hovered_thumb.is_some();
        if !is_hovered
            && self.dragged_axis.is_none()
            && !self.idle_timer_pending
            && let Some(last) = self.last_activity
        {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < IDLE_HOLD {
                self.idle_timer_pending = true;
                self.idle_timer = schedule_idle(IDLE_HOLD - elapsed);
            }
        }

        let vp = viewport.bounds;
        let vertical_needed = viewport.content_size.height > vp.size.height;
        let horizontal_needed =
            viewport.horizontal && viewport.content_size.width > vp.size.width;

        let mut axes_needed = Vec::with_capacity(2);
        if horizontal_needed {
            axes_needed.push(Axis::Horizontal);
        }
        if vertical_needed {
            axes_needed.push(Axis::Vertical);
        }

        let mut prepared = Vec::with_capacity(axes_needed.len());
        for axis in axes_needed {
            let is_vertical = matches!(axis, Axis::Vertical);
            let (content_span, viewport_span, axis_offset) = if is_vertical {
                (viewport.content_size.height, vp.size.height, scroll_offset.y)
            } else {
                (viewport.content_size.width, vp.size.width, scroll_offset.x)
            };
            let corner_reserve = if !is_vertical && vertical_needed {
                TRACK_THICKNESS
            } else {
                px(0.)
            };

            let track_bounds = if is_vertical {
                Bounds::new(
                    point(vp.right() - TRACK_THICKNESS, vp.top()),
                    size(TRACK_THICKNESS, vp.size.height),
                )
            } else {
                Bounds::new(
                    point(vp.left(), vp.bottom() - TRACK_THICKNESS),
                    size(vp.size.width, TRACK_THICKNESS),
                )
            };

            let is_dragged = self.dragged_axis == Some(axis);
            let is_thumb_hovered = self.hovered_thumb == Some(axis);
            let (thumb_color, target_thickness, radius) = if is_dragged {
                (
                    rgba(p.text_primary),
                    THUMB_ACTIVE_THICKNESS,
                    THUMB_ACTIVE_RADIUS,
                )
            } else if is_thumb_hovered {
                (
                    rgba(p.text_secondary),
                    THUMB_ACTIVE_THICKNESS,
                    THUMB_ACTIVE_RADIUS,
                )
            } else {
                (
                    rgba(p.text_tertiary),
                    THUMB_REST_THICKNESS,
                    THUMB_REST_RADIUS,
                )
            };

            let (thickness, thickness_running) =
                self.step_thickness(axis, target_thickness, reduce_motion, now);
            if thickness_running {
                window.request_animation_frame();
            }

            let origin = if is_vertical {
                track_bounds.origin.y
            } else {
                track_bounds.origin.x
            };
            let metrics =
                TrackMetrics::compute(origin, viewport_span, content_span, corner_reserve);
            let thumb_offset = metrics.thumb_start(axis_offset) - origin;

            let (thumb_hit_bounds, thumb_paint_bounds) = if is_vertical {
                (
                    Bounds::from_anchor_and_size(
                        Anchor::TopRight,
                        track_bounds.top_right() + point(-THUMB_MARGIN, thumb_offset),
                        size(TRACK_THICKNESS, metrics.thumb_len),
                    ),
                    Bounds::from_anchor_and_size(
                        Anchor::TopRight,
                        track_bounds.top_right() + point(-THUMB_MARGIN, thumb_offset),
                        size(thickness, metrics.thumb_len),
                    ),
                )
            } else {
                (
                    Bounds::from_anchor_and_size(
                        Anchor::BottomLeft,
                        track_bounds.bottom_left() + point(thumb_offset, -THUMB_MARGIN),
                        size(metrics.thumb_len, TRACK_THICKNESS),
                    ),
                    Bounds::from_anchor_and_size(
                        Anchor::BottomLeft,
                        track_bounds.bottom_left() + point(thumb_offset, -THUMB_MARGIN),
                        size(metrics.thumb_len, thickness),
                    ),
                )
            };

            let bar_hitbox = window.with_content_mask(
                Some(ContentMask {
                    bounds: track_bounds,
                }),
                |window| window.insert_hitbox(track_bounds, HitboxBehavior::Normal),
            );

            prepared.push(PreparedAxis {
                axis,
                bar_hitbox,
                track_bounds,
                thumb_hit_bounds,
                thumb_paint_bounds,
                thumb_color,
                corner_radius: clamp_radius(radius, thumb_paint_bounds),
                metrics,
                opacity,
                interactive: visible || opacity > 0.0,
            });
        }

        prepared
    }

    pub(crate) fn paint_axes(
        axes: &[PreparedAxis],
        viewport_bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        window.with_content_mask(
            Some(ContentMask {
                bounds: viewport_bounds,
            }),
            |window| {
                for item in axes {
                    window.set_cursor_style(CursorStyle::default(), &item.bar_hitbox);
                    if item.opacity > 0.0 {
                        window.paint_layer(viewport_bounds, |window| {
                            window.paint_quad(
                                fill(
                                    item.thumb_paint_bounds,
                                    item.thumb_color.opacity(item.opacity),
                                )
                                .corner_radii(item.corner_radius),
                            );
                        });
                    }
                }
            },
        );
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        item: &PreparedAxis,
        position: Point<Pixels>,
        current_offset: Point<Pixels>,
    ) -> Point<Pixels> {
        if item.thumb_hit_bounds.contains(&position) {
            let grab = position - item.thumb_hit_bounds.origin;
            self.dragged_axis = Some(item.axis);
            self.drag_grab = grab;
            item.metrics
                .apply_drag(item.axis, position, grab, current_offset)
        } else {
            let center = point(item.metrics.thumb_len / 2., item.metrics.thumb_len / 2.);
            item.metrics
                .apply_drag(item.axis, position, center, current_offset)
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        item: &PreparedAxis,
        position: Point<Pixels>,
        dragging_primary: bool,
        current_offset: Point<Pixels>,
        schedule_drag: impl FnOnce(Duration) -> TimerHandle,
    ) -> ScrollbarMoveOutcome {
        let now = Instant::now();
        let axis = item.axis;
        let mut notify = false;

        if item.track_bounds.contains(&position) && item.interactive {
            if self.hovered_axis != Some(axis) {
                self.hovered_axis = Some(axis);
                notify = true;
            }
            self.last_activity = Some(now);
        } else if self.hovered_axis == Some(axis) {
            self.hovered_axis = None;
            self.last_activity = Some(now);
            notify = true;
        }

        let thumb_hovered = item.interactive && item.thumb_hit_bounds.contains(&position);
        if thumb_hovered {
            if self.hovered_thumb != Some(axis) {
                self.hovered_thumb = Some(axis);
                notify = true;
            }
        } else if self.hovered_thumb == Some(axis) {
            self.hovered_thumb = None;
            notify = true;
        }

        if self.dragged_axis == Some(axis) && dragging_primary {
            let next = item
                .metrics
                .apply_drag(axis, position, self.drag_grab, current_offset);
            let moved = next != current_offset;
            if moved || notify {
                let elapsed = now.saturating_duration_since(self.last_drag_notify);
                if elapsed >= DRAG_NOTIFY_INTERVAL {
                    self.last_drag_notify = now;
                    self.drag_timer.cancel();
                    self.drag_timer_pending = false;
                    return ScrollbarMoveOutcome {
                        new_offset: moved.then_some(next),
                        notify_now: true,
                        stop_propagation: true,
                    };
                }
                if !self.drag_timer_pending {
                    self.drag_timer_pending = true;
                    self.drag_timer = schedule_drag(DRAG_NOTIFY_INTERVAL - elapsed);
                }
            }
            return ScrollbarMoveOutcome {
                new_offset: moved.then_some(next),
                notify_now: false,
                stop_propagation: true,
            };
        }

        ScrollbarMoveOutcome {
            new_offset: None,
            notify_now: notify,
            stop_propagation: false,
        }
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        item: &PreparedAxis,
        position: Point<Pixels>,
        current_offset: Point<Pixels>,
    ) -> Option<Point<Pixels>> {
        if self.dragged_axis != Some(item.axis) {
            return None;
        }
        let next = item
            .metrics
            .apply_drag(item.axis, position, self.drag_grab, current_offset);
        self.dragged_axis = None;
        self.drag_timer.cancel();
        self.drag_timer_pending = false;
        self.last_activity = Some(Instant::now());
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_radius_is_limited_by_its_actual_bounds() {
        let vertical = Bounds::new(Point::default(), size(px(8.), px(80.)));
        let horizontal = Bounds::new(Point::default(), size(px(80.), px(6.)));

        assert_eq!(clamp_radius(px(6.), vertical), px(4.));
        assert_eq!(clamp_radius(px(6.), horizontal), px(3.));
        assert_eq!(clamp_radius(Pixels::ZERO, vertical), Pixels::ZERO);
    }

    #[test]
    fn visibility_animates_enter_exit_and_snaps_on_reduce_motion() {
        let start = Instant::now();
        let mut state = ScrollbarState::new_at(start);

        let (op, running) = state.step_opacity(true, false, start);
        assert_eq!(op, 0.0);
        assert!(running);
        let (mid, _) = state.opacity.sample(start + FADE_IN_DURATION / 2);
        assert!((mid - 0.5).abs() < 0.01);
        assert_eq!(state.opacity.sample(start + FADE_IN_DURATION), (1.0, false));

        let (op_exit, running) = state.step_opacity(false, false, start + FADE_IN_DURATION);
        assert_eq!(op_exit, 1.0);
        assert!(running);
        let (mid_exit, _) = state
            .opacity
            .sample(start + FADE_IN_DURATION + FADE_OUT_DURATION / 2);
        assert!(mid_exit > 0.5, "ease-in must remain visible early in exit");
        assert_eq!(
            state
                .opacity
                .sample(start + FADE_IN_DURATION + FADE_OUT_DURATION),
            (0.0, false)
        );

        let (snapped, running) = state.step_opacity(true, true, start);
        assert_eq!(snapped, 1.0);
        assert!(!running);
    }

    #[test]
    fn thumb_width_expands_and_snaps_under_reduced_motion() {
        let start = Instant::now();
        let mut state = ScrollbarState::new_at(start);

        let (w0, running) = state.step_thickness(Axis::Vertical, THUMB_ACTIVE_THICKNESS, false, start);
        assert_eq!(w0, THUMB_REST_THICKNESS);
        assert!(running);
        let (w_mid, _) = state.vertical_thickness.sample(start + EXPAND_DURATION / 2);
        assert!(w_mid > THUMB_REST_THICKNESS && w_mid < THUMB_ACTIVE_THICKNESS);
        assert_eq!(
            state.vertical_thickness.sample(start + EXPAND_DURATION),
            (THUMB_ACTIVE_THICKNESS, false)
        );

        let (snapped, running) =
            state.step_thickness(Axis::Vertical, THUMB_REST_THICKNESS, true, start);
        assert_eq!(snapped, THUMB_REST_THICKNESS);
        assert!(!running);
    }

    #[test]
    fn full_track_thumb_preserves_scroll_offset() {
        let metrics = TrackMetrics::compute(px(0.), px(200.), px(250.), px(0.));
        assert!(metrics.travel > px(0.));
        let dragged = metrics.apply_drag(
            Axis::Vertical,
            point(px(0.), px(100.)),
            point(px(0.), px(10.)),
            point(px(-5.), px(0.)),
        );
        assert_eq!(dragged.x, px(-5.));
        assert!(dragged.y < px(0.));

        let zero_travel = TrackMetrics::compute(px(0.), px(40.), px(100.), px(0.));
        assert_eq!(zero_travel.travel, px(0.));
        let kept = zero_travel.apply_drag(
            Axis::Vertical,
            point(px(0.), px(20.)),
            point(px(0.), px(5.)),
            point(px(0.), px(-12.)),
        );
        assert_eq!(kept.y, px(-12.));
    }
}
