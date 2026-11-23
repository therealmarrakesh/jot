use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crate::ui::{Bounds, Context, Pixels, Point, TimerHandle, px};

/// Manages timer-based auto-scrolling during drag interactions.
///
/// Delta convention: positive moves toward the bottom and negative moves
/// toward the top.
pub(crate) struct AutoScroll {
    delta: Rc<Cell<Option<Pixels>>>,
    timer: Option<TimerHandle>,
    /// Last drag position, available to the interaction that owns selection.
    pub(crate) last_drag_position: Option<Point<Pixels>>,
}

impl Default for AutoScroll {
    fn default() -> Self {
        Self {
            delta: Rc::new(Cell::new(None)),
            timer: None,
            last_drag_position: None,
        }
    }
}

impl AutoScroll {
    /// Computes the scroll delta for a pointer Y position within the viewport.
    pub(crate) fn compute_delta(y: Pixels, bounds: Bounds<Pixels>) -> Option<Pixels> {
        const MIN_SPEED: f32 = 12.0;
        const MAX_SPEED: f32 = 64.0;
        const INNER_ZONE: f32 = 16.0;
        const OUTER_RAMP: f32 = 80.0;

        let bottom_trigger = bounds.bottom() - px(INNER_ZONE);
        let top_trigger = bounds.top() + px(INNER_ZONE);

        if y > bottom_trigger {
            let t = ((y - bottom_trigger) / px(INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(px(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED)))
        } else if y < top_trigger {
            let t = ((top_trigger - y) / px(INNER_ZONE + OUTER_RAMP)).min(1.0);
            Some(px(-(MIN_SPEED + t * (MAX_SPEED - MIN_SPEED))))
        } else {
            None
        }
    }

    pub(crate) fn set<T, F>(&mut self, delta: Option<Pixels>, cx: &mut Context<T>, tick: F)
    where
        T: 'static,
        F: Fn(Pixels, &mut T, &mut Context<T>) + 'static,
    {
        let was_idle = self.timer.is_none();
        self.delta.set(delta);

        if delta.is_none() {
            self.timer = None;
            return;
        }

        if was_idle {
            let shared = Rc::clone(&self.delta);
            let this = cx.weak_entity();
            let interval = Duration::from_millis(16);
            self.timer = Some(cx.every(interval, interval, move |cx| {
                let Some(delta) = shared.get() else {
                    return false;
                };
                this.update(cx, |state, cx| {
                    tick(delta, state, cx);
                    true
                })
                .unwrap_or(false)
            }));
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.delta.get().is_some()
    }

    pub(crate) fn stop(&mut self) {
        self.delta.set(None);
        self.timer = None;
        self.last_drag_position = None;
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::{Bounds, point, size};

    use super::*;

    #[test]
    fn delta_uses_dead_zone_and_symmetric_edge_ramps() {
        let bounds = Bounds::new(point(px(0.), px(100.)), size(px(200.), px(100.)));

        assert_eq!(AutoScroll::compute_delta(px(150.), bounds), None);

        let top = AutoScroll::compute_delta(px(80.), bounds).unwrap();
        let bottom = AutoScroll::compute_delta(px(220.), bounds).unwrap();
        assert_eq!(top, -bottom);
        assert!(top < px(-12.));
    }

    #[test]
    fn stop_clears_delta_and_drag_position() {
        let mut scroll = AutoScroll::default();
        scroll.delta.set(Some(px(20.)));
        scroll.last_drag_position = Some(point(px(1.), px(2.)));

        scroll.stop();

        assert!(!scroll.is_active());
        assert_eq!(scroll.last_drag_position, None);
    }
}
