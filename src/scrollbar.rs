//! Owned overlay scrollbar state, track geometry, hover/drag handling, and Direct2D painting.

use std::time::{Duration, Instant};

use crate::color::{Rgba, rgba};
use crate::geom::{Axis, Corners, Point, Rect, Size, point, size};
use crate::gfx::{D2DRenderer, fill};
use crate::platform::TimerHandle;
use crate::theme::Palette;
use crate::ui::{Easing, TimedTransition};

const TRACK_THICKNESS: f32 = 16.;
const MIN_THUMB_LENGTH: f32 = 48.;
const THUMB_MARGIN: f32 = 3.;
const THUMB_REST_THICKNESS: f32 = 6.;
const THUMB_ACTIVE_THICKNESS: f32 = 8.;
const THUMB_REST_RADIUS: f32 = 3.;
const THUMB_ACTIVE_RADIUS: f32 = 4.;

const IDLE_HOLD: Duration = Duration::from_secs(2);
const FADE_IN_DURATION: Duration = Duration::from_millis(120);
const FADE_OUT_DURATION: Duration = Duration::from_millis(200);
const EXPAND_DURATION: Duration = Duration::from_millis(120);
const DRAG_NOTIFY_INTERVAL: Duration = Duration::from_micros(8_333);

#[inline]
fn clamp_radius(radius: f32, bounds: Rect) -> f32 {
    radius
        .min(bounds.size.width * 0.5)
        .min(bounds.size.height * 0.5)
        .max(0.0)
}

/// 1D geometry along a single scrollbar axis shared by painting, track clicks, and dragging.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TrackMetrics {
    origin: f32,
    margin: f32,
    pub(crate) thumb_len: f32,
    travel: f32,
    max_scroll: f32,
}

impl TrackMetrics {
    pub(crate) fn compute(
        origin: f32,
        viewport_span: f32,
        content_span: f32,
        corner_reserve: f32,
    ) -> Self {
        let track_span = (viewport_span - corner_reserve).max(0.);
        let raw_thumb = (viewport_span / content_span * viewport_span)
            .max(MIN_THUMB_LENGTH)
            .min(track_span);
        let margin = THUMB_MARGIN.clamp(0., raw_thumb * 0.5);
        Self {
            origin,
            margin,
            thumb_len: raw_thumb - margin * 2.,
            travel: track_span - raw_thumb,
            max_scroll: content_span - viewport_span,
        }
    }

    pub(crate) fn thumb_start(self, scroll_offset: f32) -> f32 {
        let ratio = (-scroll_offset / self.max_scroll).clamp(0., 1.);
        self.origin + self.margin + ratio * self.travel
    }

    fn scroll_for_pointer(self, pointer: f32, grab: f32) -> f32 {
        if self.travel <= 0. {
            return 0.;
        }
        let ratio = ((pointer - self.origin - self.margin - grab) / self.travel).clamp(0., 1.);
        -self.max_scroll * ratio
    }

    pub(crate) fn apply_drag(
        self,
        axis: Axis,
        pointer: Point,
        grab: Point,
        mut offset: Point,
    ) -> Point {
        if self.travel <= 0. {
            return offset;
        }
        match axis {
            Axis::Vertical => offset.y = self.scroll_for_pointer(pointer.y, grab.y),
            Axis::Horizontal => offset.x = self.scroll_for_pointer(pointer.x, grab.x),
        }
        offset
    }
}

/// Viewport inputs supplied by the scrolled content (`Editor`) each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarViewport {
    pub(crate) bounds: Rect,
    pub(crate) content_size: Size,
    pub(crate) horizontal: bool,
}

/// Prepared geometry for one active scrollbar axis retained on `ScrollbarState` for direct
/// hit-testing, dragging, and painting.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreparedAxis {
    pub(crate) axis: Axis,
    pub(crate) track_bounds: Rect,
    pub(crate) thumb_hit_bounds: Rect,
    pub(crate) thumb_paint_bounds: Rect,
    pub(crate) thumb_color: Rgba,
    pub(crate) corner_radius: f32,
    pub(crate) metrics: TrackMetrics,
    pub(crate) opacity: f32,
    pub(crate) interactive: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollbarMoveOutcome {
    pub(crate) new_offset: Option<Point>,
    pub(crate) notify_now: bool,
    pub(crate) consumed: bool,
}

/// Owned scrollbar interaction, prepared geometry, and animation state (stored on `Editor`).
pub(crate) struct ScrollbarState {
    hovered_axis: Option<Axis>,
    hovered_thumb: Option<Axis>,
    dragged_axis: Option<Axis>,
    drag_grab: Point,
    last_offset: Point,
    last_activity: Option<Instant>,
    last_drag_notify: Instant,
    drag_timer_pending: bool,
    drag_timer: TimerHandle,
    idle_timer_pending: bool,
    idle_timer: TimerHandle,
    opacity: TimedTransition<f32>,
    vertical_thickness: TimedTransition<f32>,
    horizontal_thickness: TimedTransition<f32>,
    viewport_bounds: Rect,
    axes: Vec<PreparedAxis>,
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
            drag_grab: Point::ZERO,
            last_offset: Point::ZERO,
            last_activity: None,
            last_drag_notify: now,
            drag_timer_pending: false,
            drag_timer: TimerHandle::default(),
            idle_timer_pending: false,
            idle_timer: TimerHandle::default(),
            opacity: TimedTransition::settled(0.0, now),
            vertical_thickness: TimedTransition::settled(THUMB_REST_THICKNESS, now),
            horizontal_thickness: TimedTransition::settled(THUMB_REST_THICKNESS, now),
            viewport_bounds: Rect::ZERO,
            axes: Vec::new(),
        }
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragged_axis.is_some()
    }

    pub(crate) fn hits_track(&self, pos: Point) -> bool {
        self.axes
            .iter()
            .any(|a| a.interactive && a.track_bounds.contains(pos))
    }

    fn wants_visible(&self, now: Instant) -> bool {
        let hovered = self.hovered_axis.is_some() || self.hovered_thumb.is_some();
        let dragging = self.dragged_axis.is_some();
        // Hovering the track edge reveals the overlay scrollbar even when faded out (W-9).
        if hovered {
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
        target: f32,
        reduce_motion: bool,
        now: Instant,
    ) -> (f32, bool) {
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

    pub(crate) fn on_wheel(&mut self, offset: Point, now: Instant) -> bool {
        if offset == self.last_offset {
            return false;
        }
        self.last_offset = offset;
        self.last_activity = Some(now);
        self.idle_timer.cancel();
        self.idle_timer_pending = false;
        true
    }

    /// Updates scrollbar layout and animation state. Returns `true` if another vsync frame is needed.
    pub(crate) fn prepare(
        &mut self,
        viewport: ScrollbarViewport,
        scroll_offset: Point,
        reduce_motion: bool,
        p: Palette,
        now: Instant,
        schedule_idle: impl FnOnce(Duration) -> TimerHandle,
    ) -> bool {
        let mut animating = false;
        if scroll_offset != self.last_offset {
            self.last_offset = scroll_offset;
            self.last_activity = Some(now);
            self.idle_timer.cancel();
            self.idle_timer_pending = false;
        }

        let visible = self.wants_visible(now);
        let (opacity, opacity_running) = self.step_opacity(visible, reduce_motion, now);
        animating |= opacity_running;

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
        self.viewport_bounds = vp;
        let vertical_needed = viewport.content_size.height > vp.size.height;
        let horizontal_needed =
            viewport.horizontal && viewport.content_size.width > vp.size.width;

        self.axes.clear();
        let mut axes_needed = [None, None];
        let mut count = 0;
        if horizontal_needed {
            axes_needed[count] = Some(Axis::Horizontal);
            count += 1;
        }
        if vertical_needed {
            axes_needed[count] = Some(Axis::Vertical);
        }

        for axis in axes_needed.into_iter().flatten() {
            let is_vertical = matches!(axis, Axis::Vertical);
            let (content_span, viewport_span, axis_offset) = if is_vertical {
                (viewport.content_size.height, vp.size.height, scroll_offset.y)
            } else {
                (viewport.content_size.width, vp.size.width, scroll_offset.x)
            };
            let corner_reserve = if !is_vertical && vertical_needed {
                TRACK_THICKNESS
            } else {
                0.
            };

            let track_bounds = if is_vertical {
                Rect::new(
                    point(vp.right() - TRACK_THICKNESS, vp.top()),
                    size(TRACK_THICKNESS, vp.size.height),
                )
            } else {
                Rect::new(
                    point(vp.left(), vp.bottom() - TRACK_THICKNESS),
                    size(vp.size.width, TRACK_THICKNESS),
                )
            };

            let is_dragged = self.dragged_axis == Some(axis);
            let is_thumb_hovered = self.hovered_thumb == Some(axis);
            let (thumb_color, target_thickness, radius) = if is_dragged {
                (rgba(p.text_primary), THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else if is_thumb_hovered {
                (rgba(p.text_secondary), THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else {
                (rgba(p.text_tertiary), THUMB_REST_THICKNESS, THUMB_REST_RADIUS)
            };

            let (thickness, thickness_running) =
                self.step_thickness(axis, target_thickness, reduce_motion, now);
            animating |= thickness_running;

            let origin = if is_vertical {
                track_bounds.origin.y
            } else {
                track_bounds.origin.x
            };
            let metrics = TrackMetrics::compute(origin, viewport_span, content_span, corner_reserve);
            let thumb_offset = metrics.thumb_start(axis_offset) - origin;

            let (thumb_hit_bounds, thumb_paint_bounds) = if is_vertical {
                let right = track_bounds.right() - THUMB_MARGIN;
                let top = track_bounds.top() + thumb_offset;
                (
                    Rect::new(
                        point(right - TRACK_THICKNESS, top),
                        size(TRACK_THICKNESS, metrics.thumb_len),
                    ),
                    Rect::new(point(right - thickness, top), size(thickness, metrics.thumb_len)),
                )
            } else {
                let left = track_bounds.left() + thumb_offset;
                let bottom = track_bounds.bottom() - THUMB_MARGIN;
                (
                    Rect::new(
                        point(left, bottom - TRACK_THICKNESS),
                        size(metrics.thumb_len, TRACK_THICKNESS),
                    ),
                    Rect::new(
                        point(left, bottom - thickness),
                        size(metrics.thumb_len, thickness),
                    ),
                )
            };

            self.axes.push(PreparedAxis {
                axis,
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
        animating
    }

    pub(crate) fn paint(&self, renderer: &mut D2DRenderer) {
        if self.axes.is_empty() || self.viewport_bounds.is_empty() {
            return;
        }
        renderer.push_clip(self.viewport_bounds);
        for item in &self.axes {
            if item.opacity > 0.0 {
                renderer.paint_quad(
                    fill(item.thumb_paint_bounds, item.thumb_color.opacity(item.opacity))
                        .corner_radii(Corners::all(item.corner_radius)),
                );
            }
        }
        renderer.pop_clip();
    }

    /// Handles a left mouse-down; returns `Some(new_scroll_offset)` if a scrollbar track/thumb
    /// consumed the click.
    pub(crate) fn on_mouse_down(&mut self, position: Point, current_offset: Point) -> Option<Point> {
        let item = self
            .axes
            .iter()
            .rev()
            .find(|a| a.interactive && a.track_bounds.contains(position))
            .copied()?;
        let next = if item.thumb_hit_bounds.contains(position) {
            let grab = position - item.thumb_hit_bounds.origin;
            self.dragged_axis = Some(item.axis);
            self.drag_grab = grab;
            item.metrics
                .apply_drag(item.axis, position, grab, current_offset)
        } else {
            // Standard Windows scrollbar track click pages by one viewport (W-9).
            let mut paged = current_offset;
            match item.axis {
                Axis::Vertical => {
                    let page = (self.viewport_bounds.size.height - 24.0).max(24.0);
                    if position.y < item.thumb_hit_bounds.top() {
                        paged.y += page;
                    } else {
                        paged.y -= page;
                    }
                }
                Axis::Horizontal => {
                    let page = (self.viewport_bounds.size.width - 24.0).max(24.0);
                    if position.x < item.thumb_hit_bounds.left() {
                        paged.x += page;
                    } else {
                        paged.x -= page;
                    }
                }
            }
            paged
        };
        Some(next)
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_primary: bool,
        current_offset: Point,
        mut schedule_drag: impl FnMut(Duration) -> TimerHandle,
    ) -> ScrollbarMoveOutcome {
        let now = Instant::now();
        let mut notify = false;
        let axes = self.axes.clone();

        for item in &axes {
            let axis = item.axis;
            // Hovering the track reveals a faded scrollbar (W-9).
            if item.track_bounds.contains(position) {
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

            let thumb_hovered = item.interactive && item.thumb_hit_bounds.contains(position);
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
                            consumed: true,
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
                    consumed: true,
                };
            }
        }

        ScrollbarMoveOutcome {
            new_offset: None,
            notify_now: notify,
            consumed: false,
        }
    }

    pub(crate) fn on_mouse_up(&mut self, position: Point, current_offset: Point) -> Option<Point> {
        let axis = self.dragged_axis.take()?;
        self.drag_timer.cancel();
        self.drag_timer_pending = false;
        self.last_activity = Some(Instant::now());
        let item = self.axes.iter().find(|a| a.axis == axis)?;
        Some(
            item.metrics
                .apply_drag(axis, position, self.drag_grab, current_offset),
        )
    }

    pub(crate) fn on_mouse_leave(&mut self) -> bool {
        let had = self.hovered_axis.is_some() || self.hovered_thumb.is_some();
        self.hovered_axis = None;
        self.hovered_thumb = None;
        if had {
            self.last_activity = Some(Instant::now());
        }
        had
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_radius_is_limited_by_its_actual_bounds() {
        let vertical = Rect::new(Point::ZERO, size(8., 80.));
        let horizontal = Rect::new(Point::ZERO, size(80., 6.));

        assert_eq!(clamp_radius(6., vertical), 4.);
        assert_eq!(clamp_radius(6., horizontal), 3.);
        assert_eq!(clamp_radius(0., vertical), 0.);
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
        assert!(mid_exit > 0.5);
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

        let (w0, running) =
            state.step_thickness(Axis::Vertical, THUMB_ACTIVE_THICKNESS, false, start);
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
        let metrics = TrackMetrics::compute(0., 200., 250., 0.);
        assert!(metrics.travel > 0.);
        let dragged = metrics.apply_drag(
            Axis::Vertical,
            point(0., 100.),
            point(0., 10.),
            point(-5., 0.),
        );
        assert_eq!(dragged.x, -5.);
        assert!(dragged.y < 0.);

        let zero_travel = TrackMetrics::compute(0., 40., 100., 0.);
        assert_eq!(zero_travel.travel, 0.);
        let kept = zero_travel.apply_drag(
            Axis::Vertical,
            point(0., 20.),
            point(0., 5.),
            point(0., -12.),
        );
        assert_eq!(kept.y, -12.);
    }
}
