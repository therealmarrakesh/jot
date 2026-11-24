use std::time::{Duration, Instant};

use crate::geom::{Axis, Point, Rect, Size};
use crate::ids::{AnimKey, EditorSlot, HitId, Node, UiState};
use crate::platform::{CursorStyle, TimerHandle};
use crate::theme::Palette;
use crate::ui::{Easing, box_};

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
fn axis_ix(axis: Axis) -> usize {
    match axis {
        Axis::Vertical => 0,
        Axis::Horizontal => 1,
    }
}

#[inline]
fn axis_val(p: Point, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => p.y,
        Axis::Horizontal => p.x,
    }
}

#[inline]
fn set_axis_val(p: &mut Point, axis: Axis, v: f32) {
    match axis {
        Axis::Vertical => p.y = v,
        Axis::Horizontal => p.x = v,
    }
}

#[inline]
fn axis_size(s: Size, axis: Axis) -> f32 {
    match axis {
        Axis::Vertical => s.height,
        Axis::Horizontal => s.width,
    }
}

#[inline]
fn clamp_radius(radius: f32, width: f32, height: f32) -> f32 {
    radius.min(width * 0.5).min(height * 0.5).max(0.0)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TrackMetrics {
    margin: f32,
    pub(crate) thumb_len: f32,
    travel: f32,
    max_scroll: f32,
}

impl TrackMetrics {
    pub(crate) fn compute(viewport_span: f32, content_span: f32, corner_reserve: f32) -> Self {
        let track_span = (viewport_span - corner_reserve).max(0.);
        let raw_thumb = (viewport_span / content_span * viewport_span)
            .max(MIN_THUMB_LENGTH)
            .min(track_span);
        let margin = THUMB_MARGIN.clamp(0., raw_thumb * 0.5);
        Self {
            margin,
            thumb_len: raw_thumb - margin * 2.,
            travel: track_span - raw_thumb,
            max_scroll: content_span - viewport_span,
        }
    }

    pub(crate) fn thumb_local(self, scroll_offset: f32) -> f32 {
        let ratio = (-scroll_offset / self.max_scroll).clamp(0., 1.);
        self.margin + ratio * self.travel
    }

    pub(crate) fn apply_drag(
        self,
        axis: Axis,
        track_origin: f32,
        pointer: Point,
        grab: f32,
        mut offset: Point,
    ) -> Point {
        if self.travel <= 0. {
            return offset;
        }
        let pos = axis_val(pointer, axis);
        let ratio = ((pos - track_origin - self.margin - grab) / self.travel).clamp(0., 1.);
        set_axis_val(&mut offset, axis, -self.max_scroll * ratio);
        offset
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarViewport {
    pub(crate) size: Size,
    pub(crate) content_size: Size,
    pub(crate) scroll_offset: Point,
    pub(crate) horizontal_left: f32,
    pub(crate) horizontal: bool,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    axis: Axis,
    grab: f32,
}

#[derive(Clone, Copy, Debug)]
struct ActiveAxis {
    metrics: TrackMetrics,
    viewport_span: f32,
    thumb_local: f32,
    interactive: bool,
}

pub(crate) struct ScrollbarState {
    drag: Option<Drag>,
    last_offset: Point,
    last_activity: Option<Instant>,
    was_hovered: bool,
    last_drag_notify: Instant,
    drag_timer: Option<TimerHandle>,
    idle_timer: Option<TimerHandle>,
    axes: [Option<ActiveAxis>; 2],
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
            drag: None,
            last_offset: Point::ZERO,
            last_activity: None,
            was_hovered: false,
            last_drag_notify: now,
            drag_timer: None,
            idle_timer: None,
            axes: [None, None],
        }
    }

    pub(crate) fn drag_axis(&self) -> Option<Axis> {
        self.drag.map(|d| d.axis)
    }

    pub(crate) fn is_interactive(&self, axis: Axis) -> bool {
        self.axes[axis_ix(axis)].is_some_and(|a| a.interactive)
    }

    pub(crate) fn on_idle_timer_fired(&mut self) {
        self.idle_timer = None;
    }

    pub(crate) fn on_drag_timer_fired(&mut self, now: Instant) -> bool {
        if self.drag_timer.take().is_none() {
            return false;
        }
        self.last_drag_notify = now;
        true
    }

    pub(crate) fn on_wheel(&mut self, offset: Point, now: Instant) -> bool {
        if offset == self.last_offset {
            return false;
        }
        self.last_offset = offset;
        self.last_activity = Some(now);
        self.idle_timer = None;
        true
    }

    pub(crate) fn render(
        &mut self,
        slot: EditorSlot,
        vp: ScrollbarViewport,
        p: Palette,
        now: Instant,
        ui: &mut UiState,
        schedule_idle: impl FnOnce(Duration) -> Option<TimerHandle>,
    ) -> Vec<Node> {
        let scroll_offset = vp.scroll_offset;
        if scroll_offset != self.last_offset {
            self.last_offset = scroll_offset;
            self.last_activity = Some(now);
            self.idle_timer = None;
        }

        let is_axis_hovered = |axis: Axis, ui: &UiState| {
            ui.is_hovered(HitId::ScrollTrack(slot, axis))
                || ui.is_hovered(HitId::ScrollThumb(slot, axis))
        };
        let hovered =
            is_axis_hovered(Axis::Vertical, ui) || is_axis_hovered(Axis::Horizontal, ui);
        if hovered || (self.was_hovered && !hovered) {
            self.last_activity = Some(now);
            self.idle_timer = None;
        }
        self.was_hovered = hovered;

        let visible = hovered
            || self.drag.is_some()
            || self
                .last_activity
                .is_some_and(|t| now.saturating_duration_since(t) < IDLE_HOLD);

        if !hovered
            && self.drag.is_none()
            && self.idle_timer.is_none()
            && let Some(last) = self.last_activity
        {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < IDLE_HOLD {
                self.idle_timer = schedule_idle(IDLE_HOLD - elapsed);
            }
        }

        let vertical_needed = vp.size.height > 0. && vp.content_size.height > vp.size.height;
        let horizontal_needed =
            vp.horizontal && vp.size.width > 0. && vp.content_size.width > vp.size.width;
        self.axes = [None, None];
        if !vertical_needed && !horizontal_needed {
            return Vec::new();
        }

        let (fade_dur, fade_ease) = if visible {
            (FADE_IN_DURATION, Easing::Linear)
        } else {
            (FADE_OUT_DURATION, Easing::EaseInCubic)
        };
        let opacity = ui
            .animate_f32_from(
                AnimKey::ScrollOpacity(slot),
                0.0,
                if visible { 1.0 } else { 0.0 },
                fade_dur,
                fade_ease,
            )
            .value;
        let interactive = visible || opacity > 0.0;

        let mut nodes = Vec::with_capacity(2);
        for (needed, axis) in [
            (horizontal_needed, Axis::Horizontal),
            (vertical_needed, Axis::Vertical),
        ] {
            if !needed {
                continue;
            }
            let viewport_span = axis_size(vp.size, axis);
            let content_span = axis_size(vp.content_size, axis);
            let corner_reserve = if axis == Axis::Horizontal && vertical_needed {
                TRACK_THICKNESS
            } else {
                0.
            };
            let metrics = TrackMetrics::compute(viewport_span, content_span, corner_reserve);
            let thumb_local = metrics.thumb_local(axis_val(scroll_offset, axis));
            self.axes[axis_ix(axis)] = Some(ActiveAxis {
                metrics,
                viewport_span,
                thumb_local,
                interactive,
            });

            let is_dragged = self.drag.is_some_and(|d| d.axis == axis);
            let is_thumb_hovered = interactive && ui.is_hovered(HitId::ScrollThumb(slot, axis));
            let (thumb_color, target_thickness, target_radius) = if is_dragged {
                (p.text_primary, THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else if is_thumb_hovered {
                (p.text_secondary, THUMB_ACTIVE_THICKNESS, THUMB_ACTIVE_RADIUS)
            } else {
                (p.text_tertiary, THUMB_REST_THICKNESS, THUMB_REST_RADIUS)
            };
            let thickness = ui
                .animate_f32_from(
                    AnimKey::ScrollThickness(slot, axis),
                    THUMB_REST_THICKNESS,
                    target_thickness,
                    EXPAND_DURATION,
                    Easing::EaseOutCubic,
                )
                .value;

            let thumb_node = (opacity > 0.0).then(|| {
                let (pill_w, pill_h) = match axis {
                    Axis::Vertical => (thickness, metrics.thumb_len),
                    Axis::Horizontal => (metrics.thumb_len, thickness),
                };
                let radius = clamp_radius(target_radius, pill_w, pill_h);
                let pill = match axis {
                    Axis::Vertical => box_()
                        .absolute()
                        .right(THUMB_MARGIN)
                        .top(0.)
                        .bottom(0.)
                        .w(thickness),
                    Axis::Horizontal => box_()
                        .absolute()
                        .bottom(THUMB_MARGIN)
                        .left(0.)
                        .right(0.)
                        .h(thickness),
                }
                .rounded(radius)
                .bg(thumb_color.opacity(opacity));

                let thumb = match axis {
                    Axis::Vertical => box_()
                        .absolute()
                        .left(0.)
                        .right(0.)
                        .top(thumb_local)
                        .h(metrics.thumb_len),
                    Axis::Horizontal => box_()
                        .absolute()
                        .top(0.)
                        .bottom(0.)
                        .left(thumb_local)
                        .w(metrics.thumb_len),
                };
                thumb
                    .hit(HitId::ScrollThumb(slot, axis))
                    .cursor(CursorStyle::Arrow)
                    .child(pill)
            });

            let track = match axis {
                Axis::Vertical => box_()
                    .absolute()
                    .right(0.)
                    .top(0.)
                    .bottom(0.)
                    .w(TRACK_THICKNESS),
                Axis::Horizontal => box_()
                    .absolute()
                    .left(vp.horizontal_left)
                    .right(corner_reserve)
                    .bottom(0.)
                    .h(TRACK_THICKNESS),
            };
            nodes.push(
                track
                    .hit(HitId::ScrollTrack(slot, axis))
                    .when(interactive, |n| n.cursor(CursorStyle::Arrow))
                    .when_some(thumb_node, |n, t| n.child(t)),
            );
        }
        nodes
    }

    pub(crate) fn on_mouse_down(
        &mut self,
        axis: Axis,
        hit_thumb: bool,
        position: Point,
        track_rect: Rect,
        current_offset: Point,
    ) -> Option<Point> {
        let active = self.axes[axis_ix(axis)]?;
        if !active.interactive {
            return None;
        }
        let track_origin = axis_val(track_rect.origin, axis);
        let pointer = axis_val(position, axis);
        let thumb_origin = track_origin + active.thumb_local;
        if hit_thumb {
            let grab = pointer - thumb_origin;
            self.drag = Some(Drag { axis, grab });
            Some(
                active
                    .metrics
                    .apply_drag(axis, track_origin, position, grab, current_offset),
            )
        } else {
            let page = (active.viewport_span - 24.0).max(24.0);
            let delta = if pointer < thumb_origin { page } else { -page };
            let mut next = current_offset;
            let val = axis_val(next, axis) + delta;
            set_axis_val(&mut next, axis, val);
            Some(next)
        }
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        position: Point,
        dragging_primary: bool,
        track_rect: Option<Rect>,
        current_offset: Point,
        schedule_drag: impl FnOnce(Duration) -> Option<TimerHandle>,
    ) -> Option<(Option<Point>, bool)> {
        let drag = self.drag.filter(|_| dragging_primary)?;
        let active = self.axes[axis_ix(drag.axis)]?;
        let track_origin = axis_val(track_rect?.origin, drag.axis);
        let now = Instant::now();
        let next = active
            .metrics
            .apply_drag(drag.axis, track_origin, position, drag.grab, current_offset);
        let moved = next != current_offset;
        if moved {
            let elapsed = now.saturating_duration_since(self.last_drag_notify);
            if elapsed >= DRAG_NOTIFY_INTERVAL {
                self.last_drag_notify = now;
                self.drag_timer = None;
                return Some((Some(next), true));
            }
            if self.drag_timer.is_none() {
                self.drag_timer = schedule_drag(DRAG_NOTIFY_INTERVAL - elapsed);
            }
        }
        Some((moved.then_some(next), false))
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        position: Point,
        track_rect: Option<Rect>,
        current_offset: Point,
    ) -> Option<Point> {
        let drag = self.drag.take()?;
        self.drag_timer = None;
        self.last_activity = Some(Instant::now());
        let active = self.axes[axis_ix(drag.axis)]?;
        let track_origin = axis_val(track_rect?.origin, drag.axis);
        Some(
            active
                .metrics
                .apply_drag(drag.axis, track_origin, position, drag.grab, current_offset),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{point, size};
    use crate::ids::TabId;

    #[test]
    fn thumb_radius_is_limited_by_its_actual_bounds() {
        assert_eq!(clamp_radius(6., 8., 80.), 4.);
        assert_eq!(clamp_radius(6., 80., 6.), 3.);
        assert_eq!(clamp_radius(0., 8., 80.), 0.);
    }

    #[test]
    fn full_track_thumb_preserves_scroll_offset() {
        let metrics = TrackMetrics::compute(200., 250., 0.);
        assert!(metrics.travel > 0.);
        let dragged = metrics.apply_drag(Axis::Vertical, 0., point(0., 100.), 10., point(-5., 0.));
        assert_eq!(dragged.x, -5.);
        assert!(dragged.y < 0.);

        let zero_travel = TrackMetrics::compute(40., 100., 0.);
        assert_eq!(zero_travel.travel, 0.);
        let kept = zero_travel.apply_drag(Axis::Vertical, 0., point(0., 20.), 5., point(0., -12.));
        assert_eq!(kept.y, -12.);
    }

    #[test]
    fn scene_scrollbar_renders_fades_and_snaps_on_reduce_motion() {
        let t0 = Instant::now();
        let mut sb = ScrollbarState::new_at(t0);
        let mut ui = UiState::default();
        let p = Palette::resolve(false, None, None);
        let slot = EditorSlot::Tab(TabId::next());
        let vp = ScrollbarViewport {
            size: size(400., 200.),
            content_size: size(400., 1000.),
            scroll_offset: point(0., -40.),
            horizontal_left: 0.,
            horizontal: false,
        };

        sb.on_wheel(point(0., -40.), t0);
        ui.begin_frame(t0, false);
        let nodes = sb.render(slot, vp, p, t0, &mut ui, |_| None);
        assert_eq!(nodes.len(), 1);
        assert!(sb.is_interactive(Axis::Vertical));
        assert!(ui.finish_frame(crate::ids::Frame::default()));

        ui.begin_frame(t0 + FADE_IN_DURATION, false);
        let _ = sb.render(
            slot,
            vp,
            p,
            t0 + FADE_IN_DURATION,
            &mut ui,
            |_| None,
        );
        assert!(!ui.finish_frame(crate::ids::Frame::default()));

        let after_idle = t0 + IDLE_HOLD + Duration::from_millis(10);
        ui.begin_frame(after_idle, true);
        let _ = sb.render(slot, vp, p, after_idle, &mut ui, |_| None);
        assert!(!sb.is_interactive(Axis::Vertical));
    }
}
