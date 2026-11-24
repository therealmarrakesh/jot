use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::color::Rgba;

pub(crate) fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }

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
    pub(crate) const DECELERATE: Self = Self::CubicBezier {
        x1: 0.0,
        y1: 0.0,
        x2: 0.0,
        y2: 1.0,
    };

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

pub(crate) trait Lerp: Copy + PartialEq {
    fn lerp(self, target: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    #[inline]
    fn lerp(self, target: Self, t: f32) -> Self {
        self + (target - self) * t
    }
}

impl Lerp for Rgba {
    #[inline]
    fn lerp(self, target: Self, t: f32) -> Self {
        let a = self.a + (target.a - self.a) * t;
        let channel = |x: f32, y: f32| {
            let premul = x * self.a + (y * target.a - x * self.a) * t;
            if a > 0.0 {
                (premul / a).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Rgba {
            r: channel(self.r, target.r),
            g: channel(self.g, target.g),
            b: channel(self.b, target.b),
            a,
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedTransition<T> {
    pub(crate) from: T,
    pub(crate) target: T,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
    pub(crate) easing: Easing,
}

impl<T: Lerp> TimedTransition<T> {
    pub(crate) fn settled(value: T, now: Instant) -> Self {
        Self {
            from: value,
            target: value,
            started_at: now,
            duration: Duration::ZERO,
            easing: Easing::Linear,
        }
    }

    pub(crate) fn sample(&self, now: Instant) -> (T, bool) {
        if self.from == self.target || self.duration.is_zero() {
            return (self.target, false);
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed >= self.duration {
            (self.target, false)
        } else {
            let linear = elapsed.as_secs_f32() / self.duration.as_secs_f32();
            (self.from.lerp(self.target, self.easing.sample(linear)), true)
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
        self.start(target, target, Duration::ZERO, Easing::Linear, now);
    }
}

#[derive(Clone, Copy, Debug)]
struct AnimTrack<T> {
    from: T,
    target: T,
    started_at: Instant,
    reversing_scale: f32,
    duration: Duration,
    last_frame: u64,
}

impl<T: Lerp> AnimTrack<T> {
    fn sample_and_update(
        &mut self,
        target: T,
        duration: Duration,
        easing: Easing,
        now: Instant,
        reduce_motion: bool,
        frame: u64,
    ) -> AnimSample<T> {
        self.last_frame = frame;

        if reduce_motion || duration.is_zero() {
            self.from = target;
            self.target = target;
            self.started_at = now;
            self.reversing_scale = 1.0;
            self.duration = duration;
            return AnimSample {
                value: target,
                status: AnimStatus::Finished,
            };
        }

        let elapsed = now.saturating_duration_since(self.started_at);
        let (progress, status) = if self.duration.is_zero() || elapsed >= self.duration {
            (1.0, AnimStatus::Finished)
        } else {
            (
                elapsed.as_secs_f32() / self.duration.as_secs_f32(),
                AnimStatus::Running,
            )
        };
        let eased = easing.sample(progress);
        let current = self.from.lerp(self.target, eased);

        if self.target != target {
            let reversing = target == self.from;
            let reversing_scale = if reversing {
                (eased * self.reversing_scale + (1.0 - self.reversing_scale)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let next_duration = duration.mul_f32(reversing_scale);
            self.from = current;
            self.target = target;
            self.started_at = now;
            self.reversing_scale = reversing_scale;
            self.duration = next_duration;
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
        } else if self.from == self.target {
            AnimSample {
                value: current,
                status: AnimStatus::Idle,
            }
        } else {
            AnimSample {
                value: current,
                status,
            }
        }
    }
}

pub(crate) struct AnimTracks<K: Copy + Eq + Hash> {
    frame: u64,
    running_this_frame: bool,
    scalars: HashMap<K, AnimTrack<f32>>,
    colors: HashMap<K, AnimTrack<Rgba>>,
}

impl<K: Copy + Eq + Hash> Default for AnimTracks<K> {
    fn default() -> Self {
        Self {
            frame: 0,
            running_this_frame: false,
            scalars: HashMap::new(),
            colors: HashMap::new(),
        }
    }
}

impl<K: Copy + Eq + Hash> AnimTracks<K> {
    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        self.running_this_frame = false;
    }

    pub(crate) fn end_frame(&mut self) -> bool {
        let frame = self.frame;
        self.scalars.retain(|_, track| track.last_frame == frame);
        self.colors.retain(|_, track| track.last_frame == frame);
        self.running_this_frame
    }

    pub(crate) fn animate_f32(
        &mut self,
        key: K,
        target: f32,
        duration: Duration,
        easing: Easing,
        now: Instant,
        reduce_motion: bool,
    ) -> f32 {
        self.animate_f32_from(key, target, target, duration, easing, now, reduce_motion)
            .value
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn animate_f32_from(
        &mut self,
        key: K,
        initial: f32,
        target: f32,
        duration: Duration,
        easing: Easing,
        now: Instant,
        reduce_motion: bool,
    ) -> AnimSample<f32> {
        let frame = self.frame;
        let track = self.scalars.entry(key).or_insert_with(|| AnimTrack {
            from: initial,
            target,
            started_at: now,
            reversing_scale: 1.0,
            duration,
            last_frame: frame,
        });
        let sample = track.sample_and_update(target, duration, easing, now, reduce_motion, frame);
        if sample.status == AnimStatus::Running {
            self.running_this_frame = true;
        }
        sample
    }

    pub(crate) fn animate_color(
        &mut self,
        key: K,
        target: Rgba,
        duration: Duration,
        easing: Easing,
        now: Instant,
        reduce_motion: bool,
    ) -> Rgba {
        let frame = self.frame;
        let track = self.colors.entry(key).or_insert_with(|| AnimTrack {
            from: target,
            target,
            started_at: now,
            reversing_scale: 1.0,
            duration,
            last_frame: frame,
        });
        let sample = track.sample_and_update(target, duration, easing, now, reduce_motion, frame);
        if sample.status == AnimStatus::Running {
            self.running_this_frame = true;
        }
        sample.value
    }

    pub(crate) fn remove(&mut self, key: K) {
        self.scalars.remove(&key);
        self.colors.remove(&key);
    }

    #[cfg(test)]
    pub(crate) fn track_count(&self) -> usize {
        self.scalars.len() + self.colors.len()
    }
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

    #[test]
    fn anim_tracks_prune_unused_and_reverse_proportionally() {
        let t0 = Instant::now();
        let dur = Duration::from_millis(100);
        let mut tracks: AnimTracks<u32> = AnimTracks::default();

        tracks.begin_frame();
        let s0 = tracks.animate_f32_from(1, 0.0, 100.0, dur, Easing::Linear, t0, false);
        assert_eq!(s0.status, AnimStatus::Running);
        assert_eq!(s0.value, 0.0);
        assert!(tracks.end_frame());
        assert_eq!(tracks.track_count(), 1);

        let t_mid = t0 + Duration::from_millis(50);
        tracks.begin_frame();
        let s1 = tracks.animate_f32_from(1, 0.0, 0.0, dur, Easing::Linear, t_mid, false);
        assert_eq!(s1.status, AnimStatus::Running);
        assert!((s1.value - 50.0).abs() < 1e-3);
        assert!(tracks.end_frame());

        let t_done = t_mid + Duration::from_millis(50);
        tracks.begin_frame();
        let s2 = tracks.animate_f32_from(1, 0.0, 0.0, dur, Easing::Linear, t_done, false);
        assert_eq!(s2.status, AnimStatus::Finished);
        assert_eq!(s2.value, 0.0);
        assert!(!tracks.end_frame());

        tracks.begin_frame();
        assert!(!tracks.end_frame());
        assert_eq!(tracks.track_count(), 0);
    }
}
