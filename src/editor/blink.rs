use std::time::{Duration, Instant};

use crate::geom::{Point, Rect};
use crate::platform::TimerHandle;

use super::{EditorTimer, EditorTimers};

pub(crate) struct BlinkState {
    active: bool,
    visible: bool,
    paused: bool,
    started_at: Option<Instant>,
    timer: Option<TimerHandle>,
}

impl Default for BlinkState {
    fn default() -> Self {
        Self::new()
    }
}

impl BlinkState {
    pub(crate) fn new() -> Self {
        Self {
            active: false,
            visible: false,
            paused: false,
            started_at: None,
            timer: None,
        }
    }

    #[inline]
    pub(crate) fn visible(&self) -> bool {
        self.paused || self.visible
    }

    pub(crate) fn stop(&mut self) {
        self.active = false;
        self.paused = false;
        self.visible = false;
        self.started_at = None;
        self.timer = None;
    }

    pub(crate) fn start(&mut self, timers: &mut dyn EditorTimers) {
        self.active = true;
        self.paused = false;
        self.visible = false;
        self.started_at = Some(Instant::now());
        self.step(timers);
    }

    pub(crate) fn pause(&mut self, timers: &mut dyn EditorTimers) {
        if !self.active && !self.visible {
            return;
        }
        self.paused = true;
        self.visible = true;
        self.started_at = Some(Instant::now());
        let Some(delay) = timers.blink_timing().0 else {
            self.timer = None;
            return;
        };
        self.timer = timers.after(delay, EditorTimer::Blink);
    }

    pub(crate) fn on_timer(&mut self, timers: &mut dyn EditorTimers) -> bool {
        if !self.active {
            return false;
        }
        self.paused = false;
        self.step(timers);
        true
    }

    fn step(&mut self, timers: &mut dyn EditorTimers) {
        if self.paused {
            self.visible = true;
            return;
        }
        let (interval, timeout) = timers.blink_timing();
        let Some(interval) = interval else {
            self.visible = true;
            self.timer = None;
            return;
        };
        if let Some(timeout) = timeout
            && self
                .started_at
                .is_some_and(|start| start.elapsed() >= timeout)
        {
            self.visible = true;
            self.timer = None;
            return;
        }
        self.visible = !self.visible;
        self.timer = timers.after(interval, EditorTimer::Blink);
    }
}

#[derive(Default)]
pub(crate) struct AutoScroll {
    pub(super) delta: Option<f32>,
    timer: Option<TimerHandle>,
    pub(crate) last_drag_pos: Option<Point>,
}

impl AutoScroll {
    pub(crate) fn compute_delta(y: f32, bounds: Rect) -> Option<f32> {
        const MIN_SPEED: f32 = 12.0;
        const MAX_SPEED: f32 = 64.0;
        const INNER_ZONE: f32 = 16.0;
        const OUTER_RAMP: f32 = 80.0;

        let bottom = bounds.bottom() - INNER_ZONE;
        let top = bounds.top() + INNER_ZONE;

        if y > bottom {
            let t = ((y - bottom) / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))
        } else if y < top {
            let t = ((top - y) / (INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(-(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED)))
        } else {
            None
        }
    }

    pub(crate) fn stop(&mut self) {
        self.delta = None;
        self.timer = None;
        self.last_drag_pos = None;
    }

    pub(crate) fn update(&mut self, delta: Option<f32>, timers: &mut dyn EditorTimers) {
        let was_active = self.delta.is_some();
        self.delta = delta;
        if delta.is_none() {
            self.timer = None;
            return;
        }
        if !was_active {
            let interval = Duration::from_millis(16);
            self.timer = timers.every(interval, interval, EditorTimer::AutoScroll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::rect;

    #[test]
    fn auto_scroll_delta_has_dead_zone_and_symmetric_ramps() {
        let bounds = rect(0., 100., 200., 100.);
        assert_eq!(AutoScroll::compute_delta(150., bounds), None);

        let top = AutoScroll::compute_delta(80., bounds).unwrap();
        let bottom = AutoScroll::compute_delta(220., bounds).unwrap();
        assert!(top < 0. && bottom > 0.);
        assert!((top.abs() - bottom.abs()).abs() < 1e-3);

        let mut scroll = AutoScroll::default();
        scroll.stop();
        assert!(scroll.delta.is_none());
    }
}
