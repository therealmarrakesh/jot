//! Clean-room easing curves, linear interpolation, and per-window keyed transitions.

use std::time::{Duration, Instant};

use super::app::App;
use super::color::Rgba;
use super::element::ElementId;
use super::geometry::{Pixels, Point, point, px};
use super::window::Window;

/// Evaluates a standard CSS `cubic-bezier(x1, y1, x2, y2)` timing curve at normalized time `t`.
///
/// Uses Newton-Raphson root finding on `x(s) = t` with a bracketed bisection fallback when the
/// slope flattens near vertical or cusp control points.
pub(crate) fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }

    // Bernstein polynomial expanded into Horner coefficients for P0=(0,0), P3=(1,1):
    // B(s) = ((a * s + b) * s + c) * s
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;

    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;

    let eval_x = |s: f32| ((ax * s + bx) * s + cx) * s;
    let eval_y = |s: f32| ((ay * s + by) * s + cy) * s;
    let deriv_x = |s: f32| (3.0 * ax * s + 2.0 * bx) * s + cx;

    const EPS: f32 = 1e-6;
    let mut s = t;
    let mut solved = false;
    for _ in 0..8 {
        let residual = eval_x(s) - t;
        if residual.abs() < EPS {
            solved = true;
            break;
        }
        let slope = deriv_x(s);
        if slope.abs() < EPS {
            break;
        }
        s = (s - residual / slope).clamp(0.0, 1.0);
    }

    if !solved {
        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        s = t;
        for _ in 0..32 {
            let x = eval_x(s);
            if (x - t).abs() < EPS {
                break;
            }
            if x < t {
                lo = s;
            } else {
                hi = s;
            }
            s = 0.5 * (lo + hi);
        }
    }

    eval_y(s).clamp(0.0, 1.0)
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
    pub(crate) fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::EaseInCubic => t * t * t,
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier(x1, y1, x2, y2, t),
        }
    }
}

/// Linearly interpolatable value for smooth UI transitions.
pub(crate) trait Lerp: Clone {
    fn lerp(&self, target: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    #[inline]
    fn lerp(&self, target: &Self, t: f32) -> Self {
        self + (target - self) * t
    }
}

impl Lerp for Pixels {
    #[inline]
    fn lerp(&self, target: &Self, t: f32) -> Self {
        let a = self.as_f32();
        let b = target.as_f32();
        px(a + (b - a) * t)
    }
}

impl Lerp for Point<Pixels> {
    #[inline]
    fn lerp(&self, target: &Self, t: f32) -> Self {
        point(self.x.lerp(&target.x, t), self.y.lerp(&target.y, t))
    }
}

impl Lerp for Rgba {
    #[inline]
    fn lerp(&self, target: &Self, t: f32) -> Self {
        Rgba {
            r: self.r.lerp(&target.r, t),
            g: self.g.lerp(&target.g, t),
            b: self.b.lerp(&target.b, t),
            a: self.a.lerp(&target.a, t),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnimStatus {
    Idle,
    Running,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AnimSample<T> {
    pub(crate) value: T,
    pub(crate) status: AnimStatus,
}

/// Standalone value transition driven by caller-supplied timestamps.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedTransition<T> {
    pub(crate) from: T,
    pub(crate) target: T,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
    pub(crate) easing: Easing,
}

impl<T: Lerp + PartialEq> TimedTransition<T> {
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
            (self.from.lerp(&self.target, self.easing.sample(linear)), true)
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

struct AnimTrack<T> {
    from: T,
    target: T,
    started_at: Instant,
    reversing_scale: f32,
    duration: Duration,
}

/// Smoothly animates toward `target`, starting already settled at `target` on first render.
pub(crate) fn animate<T>(
    id: impl Into<ElementId>,
    target: T,
    duration: Duration,
    easing: Easing,
    window: &mut Window,
    cx: &App,
) -> T
where
    T: Lerp + PartialEq + 'static,
{
    animate_from(id, target.clone(), target, duration, easing, window, cx).value
}

/// Smoothly animates toward `target`, seeding per-window track state at `initial` on first render.
///
/// Tracks are stored directly in `Window::anim_tracks` and automatically pruned after a frame in
/// which they are no longer queried.
pub(crate) fn animate_from<T>(
    id: impl Into<ElementId>,
    initial: T,
    target: T,
    duration: Duration,
    easing: Easing,
    window: &mut Window,
    cx: &App,
) -> AnimSample<T>
where
    T: Lerp + PartialEq + 'static,
{
    let now = Instant::now();
    let reduce_motion = cx.reduce_motion();
    let sample = window.with_anim_track(id.into(), |prev: Option<AnimTrack<T>>| {
        let mut track = prev.unwrap_or_else(|| AnimTrack {
            from: initial,
            target: target.clone(),
            started_at: now,
            reversing_scale: 1.0,
            duration,
        });

        if reduce_motion || duration.is_zero() {
            track.from = target.clone();
            track.target = target.clone();
            track.started_at = now;
            track.reversing_scale = 1.0;
            track.duration = duration;
            return (
                AnimSample {
                    value: target,
                    status: AnimStatus::Finished,
                },
                track,
            );
        }

        let elapsed = now.saturating_duration_since(track.started_at);
        let (progress, status) = if track.duration.is_zero() || elapsed >= track.duration {
            (1.0, AnimStatus::Finished)
        } else {
            (
                elapsed.as_secs_f32() / track.duration.as_secs_f32(),
                AnimStatus::Running,
            )
        };
        let eased = easing.sample(progress);
        let current = track.from.lerp(&track.target, eased);

        let result = if track.target != target {
            let reversing = target == track.from;
            let reversing_scale = if reversing {
                (eased * track.reversing_scale + (1.0 - track.reversing_scale)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let next_duration = duration.mul_f32(reversing_scale);
            track.from = current.clone();
            track.target = target.clone();
            track.started_at = now;
            track.reversing_scale = reversing_scale;
            track.duration = next_duration;
            if next_duration.is_zero() {
                AnimSample {
                    value: target,
                    status: AnimStatus::Finished,
                }
            } else {
                AnimSample {
                    value: current,
                    status: AnimStatus::Running,
                }
            }
        } else if track.from == track.target {
            AnimSample {
                value: current,
                status: AnimStatus::Idle,
            }
        } else {
            AnimSample {
                value: current,
                status,
            }
        };

        (result, track)
    });

    if sample.status == AnimStatus::Running {
        window.request_animation_frame();
    }
    sample
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn timed_transition_samples_interpolates_and_settles() {
        let t0 = Instant::now();
        let mut tr = TimedTransition::settled(0.0_f32, t0);
        assert_eq!(tr.sample(t0), (0.0, false));

        tr.start(0.0, 10.0, Duration::from_millis(100), Easing::Linear, t0);
        let (mid, running) = tr.sample(t0 + Duration::from_millis(50));
        assert!(running);
        assert!((mid - 5.0).abs() < 1e-4);

        let (end, running) = tr.sample(t0 + Duration::from_millis(100));
        assert!(!running);
        assert_eq!(end, 10.0);

        tr.settle(3.0, t0 + Duration::from_millis(120));
        assert_eq!(tr.sample(t0 + Duration::from_millis(120)), (3.0, false));
    }
}
