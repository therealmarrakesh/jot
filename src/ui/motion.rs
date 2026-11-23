use std::time::{Duration, Instant};

use crate::ui::{App, ElementId, Pixels, Point, Rgba, Window, point, px};

/// A cubic bezier easing curve like CSS `cubic-bezier(x1, y1, x2, y2)`.
pub(crate) fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let (cx, cy) = (3.0 * x1, 3.0 * y1);
    let (bx, by) = (3.0 * (x2 - x1) - cx, 3.0 * (y2 - y1) - cy);
    let (ax, ay) = (1.0 - cx - bx, 1.0 - cy - by);
    let sample_x = |s: f32| ((ax * s + bx) * s + cx) * s;
    let sample_y = |s: f32| ((ay * s + by) * s + cy) * s;
    let slope_x = |s: f32| (3.0 * ax * s + 2.0 * bx) * s + cx;

    let mut s = t;
    let mut converged = false;
    for _ in 0..8 {
        let error = sample_x(s) - t;
        if error.abs() < 1e-6 {
            converged = true;
            break;
        }
        let slope = slope_x(s);
        if slope.abs() < 1e-6 {
            break;
        }
        s = (s - error / slope).clamp(0.0, 1.0);
    }

    if !converged {
        let (mut low, mut high) = (0.0, 1.0);
        s = t;
        for _ in 0..32 {
            let x = sample_x(s);
            if (x - t).abs() < 1e-6 {
                break;
            }
            if x < t {
                low = s;
            } else {
                high = s;
            }
            s = (low + high) / 2.0;
        }
    }

    sample_y(s).clamp(0.0, 1.0)
}

#[inline]
pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[inline]
pub(crate) fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Easing {
    Linear,
    #[default]
    EaseOutCubic,
    EaseInCubic,
    CubicBezier {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

impl Easing {
    #[inline]
    pub(crate) fn sample(self, progress: f32) -> f32 {
        let progress = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear => progress,
            Self::EaseOutCubic => ease_out_cubic(progress),
            Self::EaseInCubic => ease_in_cubic(progress),
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier(x1, y1, x2, y2, progress),
        }
    }
}

pub(crate) trait Interpolate: Clone {
    fn interpolate(&self, target: &Self, progress: f32) -> Self;
}

impl Interpolate for f32 {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        self + (target - self) * progress
    }
}

impl Interpolate for Pixels {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        let a: f32 = (*self).into();
        let b: f32 = (*target).into();
        px(a + (b - a) * progress)
    }
}

impl Interpolate for Point<Pixels> {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        point(
            self.x.interpolate(&target.x, progress),
            self.y.interpolate(&target.y, progress),
        )
    }
}

impl Interpolate for Rgba {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        Rgba {
            r: self.r.interpolate(&target.r, progress),
            g: self.g.interpolate(&target.g, progress),
            b: self.b.interpolate(&target.b, progress),
            a: self.a.interpolate(&target.a, progress),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MotionStatus {
    Idle,
    Running,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MotionValue<T> {
    pub(crate) value: T,
    pub(crate) status: MotionStatus,
}

/// Pure scalar/value transition driven by caller-supplied timestamps.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedTransition<T> {
    pub(crate) from: T,
    pub(crate) target: T,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
    pub(crate) easing: Easing,
}

impl<T: Interpolate + PartialEq> TimedTransition<T> {
    pub(crate) fn settled(value: T, now: Instant) -> Self {
        Self {
            from: value.clone(),
            target: value,
            started_at: now,
            duration: Duration::ZERO,
            easing: Easing::Linear,
        }
    }

    pub(crate) fn sample(&self, now: Instant) -> (T, bool) {
        if self.from == self.target || self.duration.is_zero() {
            return (self.target.clone(), false);
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed >= self.duration {
            (self.target.clone(), false)
        } else {
            let linear = elapsed.as_secs_f32() / self.duration.as_secs_f32();
            (
                self.from
                    .interpolate(&self.target, self.easing.sample(linear)),
                true,
            )
        }
    }

    pub(crate) fn start(
        &mut self,
        from: T,
        target: T,
        duration: Duration,
        easing: Easing,
        now: Instant,
    ) {
        self.from = from;
        self.target = target;
        self.started_at = now;
        self.duration = duration;
        self.easing = easing;
    }

    pub(crate) fn settle(&mut self, target: T, now: Instant) {
        self.start(target.clone(), target, Duration::ZERO, Easing::Linear, now);
    }
}

#[derive(Clone)]
struct KeyedTransition<T> {
    from: T,
    target: T,
    started_at: Instant,
    reversing_factor: f32,
    duration: Duration,
}

/// Transitions smoothly toward `target`, starting at `target` on first render.
pub(crate) fn transition<T>(
    id: impl Into<ElementId>,
    target: T,
    duration: Duration,
    easing: Easing,
    window: &mut Window,
    cx: &mut App,
) -> T
where
    T: Interpolate + PartialEq + 'static,
{
    transition_from(id, target.clone(), target, duration, easing, window, cx).value
}

/// Transitions smoothly toward `target`, seeding keyed state at `initial` on first render.
pub(crate) fn transition_from<T>(
    id: impl Into<ElementId>,
    initial: T,
    target: T,
    duration: Duration,
    easing: Easing,
    window: &mut Window,
    cx: &mut App,
) -> MotionValue<T>
where
    T: Interpolate + PartialEq + 'static,
{
    let id = ElementId::NamedChild(id.into().into(), "__motion".into());
    let now = cx.background_executor().now();
    let state = window.use_keyed_state(id, cx, |_, _| KeyedTransition {
        from: initial,
        target: target.clone(),
        started_at: now,
        reversing_factor: 1.0,
        duration,
    });

    let snapshot = state.read(cx).clone();

    if cx.reduce_motion() || duration.is_zero() {
        if snapshot.from != target || snapshot.target != target {
            state.update(cx, |state, _| {
                state.from = target.clone();
                state.target = target.clone();
                state.started_at = now;
                state.reversing_factor = 1.0;
                state.duration = duration;
            });
        }
        return MotionValue {
            value: target,
            status: MotionStatus::Finished,
        };
    }

    let elapsed = now.saturating_duration_since(snapshot.started_at);
    let (progress, status) = if snapshot.duration.is_zero() || elapsed >= snapshot.duration {
        (1.0, MotionStatus::Finished)
    } else {
        (
            elapsed.as_secs_f32() / snapshot.duration.as_secs_f32(),
            MotionStatus::Running,
        )
    };
    let sampled = snapshot
        .from
        .interpolate(&snapshot.target, easing.sample(progress));

    let (value, status) = if snapshot.target != target {
        let reversing = target == snapshot.from;
        let reversing_factor = if reversing {
            (easing.sample(progress) * snapshot.reversing_factor
                + (1.0 - snapshot.reversing_factor))
                .clamp(0.0, 1.0)
        } else {
            1.0
        };
        let next_duration = duration.mul_f32(reversing_factor);
        state.update(cx, |state, _| {
            state.from = sampled.clone();
            state.target = target.clone();
            state.started_at = now;
            state.reversing_factor = reversing_factor;
            state.duration = next_duration;
        });
        if next_duration.is_zero() {
            (target, MotionStatus::Finished)
        } else {
            (sampled, MotionStatus::Running)
        }
    } else if snapshot.from == snapshot.target {
        (sampled, MotionStatus::Idle)
    } else {
        (sampled, status)
    };

    if status == MotionStatus::Running {
        window.request_animation_frame();
    }
    MotionValue { value, status }
}

#[cfg(test)]
mod tests {
    use super::cubic_bezier;

    #[test]
    fn cubic_bezier_matches_engine_published_values() {
        for (t, expected) in [
            (0.0, 0.0),
            (0.2, 0.295),
            (0.5, 0.802),
            (0.8, 0.976),
            (1.0, 1.0),
        ] {
            let actual = cubic_bezier(0.25, 0.1, 0.25, 1., t);
            assert!(
                (actual - expected).abs() < 1e-3,
                "ease({t}) = {actual}, expected {expected}"
            );
        }

        for (t, expected) in [
            (0.05, 0.01136),
            (0.1, 0.03978),
            (0.25, 0.18235),
            (0.5, 0.5),
            (0.75, 0.81764),
            (0.95, 0.98863),
        ] {
            let actual = cubic_bezier(0.25, 0., 0.75, 1., t);
            assert!(
                (actual - expected).abs() < 3e-4,
                "curve({t}) = {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn cubic_bezier_with_thirds_x_maps_time_identically() {
        for step in 0..=100 {
            let t = step as f32 / 100.;
            let one_t = 1. - t;
            let expected = 3. * 0.72 * one_t * one_t * t + 3. * one_t * t * t + t * t * t;
            let actual = cubic_bezier(1. / 3., 0.72, 2. / 3., 1., t);
            assert!(
                (actual - expected).abs() < 1e-4,
                "ease({t}) = {actual}, expected {expected}"
            );
        }
    }

    #[test]
    fn cubic_bezier_matches_the_css_definition() {
        fn css_reference(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
            let sample = |p1: f64, p2: f64, s: f64| {
                3. * p1 * (1. - s) * (1. - s) * s + 3. * p2 * (1. - s) * s * s + s * s * s
            };
            let (mut low, mut high) = (0f64, 1f64);
            for _ in 0..64 {
                let mid = (low + high) / 2.;
                if sample(x1, x2, mid) < t {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            sample(y1, y2, (low + high) / 2.)
        }

        for (x1, y1, x2, y2) in [
            (0.25, 0.1, 0.25, 1.),
            (1. / 3., 0.72, 2. / 3., 1.),
            (0.42, 0., 1., 1.),
            (0., 0., 0.58, 1.),
            (0.42, 0., 0.58, 1.),
            (1., 0., 0., 1.),
            (0., 0., 0., 1.),
            (1., 0.5, 1., 0.5),
        ] {
            for step in 0..=1000 {
                let t = step as f32 / 1000.;
                let y = cubic_bezier(x1, y1, x2, y2, t);
                let expected =
                    css_reference(x1 as f64, y1 as f64, x2 as f64, y2 as f64, t as f64) as f32;
                assert!(
                    (y - expected).abs() < 5e-4,
                    "cubic_bezier({x1}, {y1}, {x2}, {y2})({t}) = {y}, CSS = {expected}"
                );
                assert!((0.0..=1.0).contains(&y), "ease({t}) = {y} out of range");
            }
        }
    }

    #[test]
    fn cubic_bezier_is_monotonic_and_clamped() {
        assert_eq!(cubic_bezier(0.32, 0.72, 0., 1., -1.), 0.);
        assert_eq!(cubic_bezier(0.32, 0.72, 0., 1., 2.), 1.);

        let mut previous = 0.;
        for step in 0..=100 {
            let current = cubic_bezier(0.32, 0.72, 0., 1., step as f32 / 100.);
            assert!(current >= previous - 1e-4, "not monotonic at {step}");
            previous = current;
        }
    }
}
