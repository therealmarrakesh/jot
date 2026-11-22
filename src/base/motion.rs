use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, Bounds, ElementId, Pixels, SharedString, Size, Window};

use crate::base::animation::{Lerp, ease_out_cubic};

mod easing;
mod presence;
mod sequence;

pub use easing::Easing;
pub use presence::Presence;
pub use sequence::Sequence;

/// A duration that can delay the start (`Positive`) or seek into the transition (`Negative`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignedDuration {
    Positive(Duration),
    Negative(Duration),
}

impl SignedDuration {
    pub const ZERO: Self = Self::Positive(Duration::ZERO);

    pub const fn positive(duration: Duration) -> Self {
        Self::Positive(duration)
    }

    pub const fn negative(duration: Duration) -> Self {
        Self::Negative(duration)
    }

    fn active_elapsed(self, elapsed: Duration) -> Option<Duration> {
        match self {
            Self::Positive(delay) => elapsed.checked_sub(delay),
            Self::Negative(offset) => Some(elapsed.saturating_add(offset)),
        }
    }
}

impl Default for SignedDuration {
    fn default() -> Self {
        Self::ZERO
    }
}

impl From<Duration> for SignedDuration {
    fn from(duration: Duration) -> Self {
        Self::positive(duration)
    }
}

/// A value that can be interpolated between two application-owned targets.
pub trait Interpolate: Clone {
    fn interpolate(&self, target: &Self, progress: f32) -> Self;
}

impl<T: Lerp> Interpolate for T {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        self.lerp(target, progress)
    }
}

impl Interpolate for Size<Pixels> {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        Size::new(
            self.width.lerp(&target.width, progress),
            self.height.lerp(&target.height, progress),
        )
    }
}

impl Interpolate for Bounds<Pixels> {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        Bounds::new(
            self.origin.lerp(&target.origin, progress),
            self.size.interpolate(&target.size, progress),
        )
    }
}

/// CSS-like timing policy for a target-value transition.
#[derive(Clone)]
pub struct Transition {
    duration: Duration,
    delay: SignedDuration,
    easing: Easing,
}

impl Transition {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            delay: SignedDuration::ZERO,
            easing: Easing::Custom(Rc::new(ease_out_cubic)),
        }
    }

    pub fn delay(mut self, delay: impl Into<SignedDuration>) -> Self {
        self.delay = delay.into();
        self
    }

    pub fn ease(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Easing::Custom(Rc::new(easing));
        self
    }

    pub fn easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    fn sample(&self, progress: f32) -> f32 {
        self.easing.sample(progress)
    }

    fn progress(&self, elapsed: Duration, duration: Duration) -> (f32, MotionStatus) {
        let Some(active_elapsed) = self.delay.active_elapsed(elapsed) else {
            return (0.0, MotionStatus::Delayed);
        };
        if duration.is_zero() || active_elapsed >= duration {
            return (1.0, MotionStatus::Finished);
        }
        (
            active_elapsed.as_secs_f32() / duration.as_secs_f32(),
            MotionStatus::Running,
        )
    }

    fn finishes_after(&self) -> Duration {
        match self.delay {
            SignedDuration::Positive(delay) => delay.saturating_add(self.duration),
            SignedDuration::Negative(delay) => self.duration.saturating_sub(delay),
        }
    }
}

/// Identifies one independently transitioning value.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TransitionId(ElementId);

impl From<ElementId> for TransitionId {
    fn from(id: ElementId) -> Self {
        Self(id)
    }
}

impl From<&'static str> for TransitionId {
    fn from(id: &'static str) -> Self {
        Self(id.into())
    }
}

impl From<String> for TransitionId {
    fn from(id: String) -> Self {
        Self(id.into())
    }
}

impl From<SharedString> for TransitionId {
    fn from(id: SharedString) -> Self {
        Self(id.into())
    }
}

impl From<usize> for TransitionId {
    fn from(id: usize) -> Self {
        Self(id.into())
    }
}

impl From<i32> for TransitionId {
    fn from(id: i32) -> Self {
        Self(id.into())
    }
}

impl From<TransitionId> for ElementId {
    fn from(id: TransitionId) -> Self {
        ElementId::NamedChild(id.0.into(), "__base-transition-state".into())
    }
}

impl<I, C> From<(I, C)> for TransitionId
where
    I: Into<ElementId>,
    C: Into<SharedString>,
{
    fn from((id, channel): (I, C)) -> Self {
        Self(ElementId::NamedChild(id.into().into(), channel.into()))
    }
}

#[derive(Clone)]
struct ValueTransition<T> {
    from: T,
    target: T,
    started_at: Instant,
    reversing_factor: f32,
    duration: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionStatus {
    Idle,
    Delayed,
    Running,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionValue<T> {
    pub value: T,
    pub status: MotionStatus,
}

/// Returns the current value for a CSS-like transition toward `target`.
pub fn transition<T>(
    id: impl Into<TransitionId>,
    target: T,
    policy: Transition,
    window: &mut Window,
    cx: &mut App,
) -> T
where
    T: Interpolate + PartialEq + 'static,
{
    transition_with_status(id, target, policy, window, cx).value
}

pub fn transition_with_status<T>(
    id: impl Into<TransitionId>,
    target: T,
    policy: Transition,
    window: &mut Window,
    cx: &mut App,
) -> MotionValue<T>
where
    T: Interpolate + PartialEq + 'static,
{
    let id: ElementId = id.into().into();
    let now = cx.background_executor().now();
    let state = window.use_keyed_state(id, cx, |_, _| ValueTransition {
        from: target.clone(),
        target: target.clone(),
        started_at: now,
        reversing_factor: 1.0,
        duration: policy.duration,
    });

    let snapshot = state.read(cx).clone();

    if cx.reduce_motion() || policy.duration.is_zero() {
        if snapshot.from != target || snapshot.target != target {
            state.update(cx, |state, _| {
                state.from = target.clone();
                state.target = target.clone();
                state.started_at = now;
                state.reversing_factor = 1.0;
                state.duration = policy.duration;
            });
        }
        return MotionValue {
            value: target,
            status: MotionStatus::Finished,
        };
    }

    let elapsed = now.saturating_duration_since(snapshot.started_at);
    let (progress, status) = policy.progress(elapsed, snapshot.duration);
    let sampled = snapshot
        .from
        .interpolate(&snapshot.target, policy.sample(progress));

    let (value, status) = if snapshot.target != target {
        let reversing = target == snapshot.from;
        let reversing_factor = if reversing {
            (policy.sample(progress) * snapshot.reversing_factor
                + (1.0 - snapshot.reversing_factor))
                .clamp(0.0, 1.0)
        } else {
            1.0
        };
        let duration = policy.duration.mul_f32(reversing_factor);
        state.update(cx, |state, _| {
            state.from = sampled.clone();
            state.target = target.clone();
            state.started_at = now;
            state.reversing_factor = reversing_factor;
            state.duration = duration;
        });
        let (initial_progress, initial_status) = policy.progress(Duration::ZERO, duration);
        (
            sampled.interpolate(&target, policy.sample(initial_progress)),
            initial_status,
        )
    } else {
        (
            sampled,
            if snapshot.from == snapshot.target {
                MotionStatus::Idle
            } else {
                status
            },
        )
    };
    if matches!(status, MotionStatus::Delayed | MotionStatus::Running) {
        window.request_animation_frame();
    }
    MotionValue { value, status }
}

#[cfg(test)]
mod tests {
    use super::Easing;

    #[test]
    fn css_keyword_easing_matches_reference_samples() {
        for (easing, samples) in [
            (Easing::Ease, [(0.2, 0.295), (0.5, 0.802), (0.8, 0.976)]),
            (Easing::EaseIn, [(0.2, 0.062), (0.5, 0.315), (0.8, 0.692)]),
            (Easing::EaseOut, [(0.2, 0.308), (0.5, 0.685), (0.8, 0.938)]),
            (Easing::EaseInOut, [(0.2, 0.082), (0.5, 0.5), (0.8, 0.918)]),
        ] {
            for (t, expected) in samples {
                let actual = easing.sample(t);
                assert!(
                    (actual - expected).abs() < 0.01,
                    "{easing:?} at {t}: expected {expected}, got {actual}"
                );
            }
        }
    }
}
