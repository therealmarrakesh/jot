use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

type WakeCallback = Arc<dyn Fn() + Send + Sync>;
type ArmTimerCallback = Box<dyn Fn(Option<u32>)>;

enum TimerAction<E> {
    Once(Option<E>),
    Repeat(Duration, Box<dyn Fn() -> E>),
}

struct TimerEntry<E> {
    deadline: Instant,
    seq: u64,
    cancelled: Rc<Cell<bool>>,
    action: TimerAction<E>,
}

impl<E> PartialEq for TimerEntry<E> {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}
impl<E> Eq for TimerEntry<E> {}
impl<E> PartialOrd for TimerEntry<E> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<E> Ord for TimerEntry<E> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline
            .cmp(&other.deadline)
            .then(self.seq.cmp(&other.seq))
    }
}

struct TimerHeap<E> {
    heap: BinaryHeap<Reverse<TimerEntry<E>>>,
    next_seq: u64,
    arm: Option<ArmTimerCallback>,
}

impl<E> TimerHeap<E> {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 1,
            arm: None,
        }
    }

    fn schedule(&mut self, delay: Duration, action: TimerAction<E>) -> TimerHandle {
        let now = Instant::now();
        let seq = self.next_seq;
        self.next_seq = seq.wrapping_add(1);
        let cancelled = Rc::new(Cell::new(false));
        self.heap.push(Reverse(TimerEntry {
            deadline: now + delay,
            seq,
            cancelled: cancelled.clone(),
            action,
        }));
        self.rearm(now);
        TimerHandle { cancelled }
    }

    fn pop_due(&mut self, now: Instant) -> Option<E> {
        while let Some(Reverse(top)) = self.heap.peek() {
            if top.cancelled.get() {
                self.heap.pop();
                continue;
            }
            if top.deadline > now {
                break;
            }
            let mut entry = self.heap.pop().unwrap().0;
            match &mut entry.action {
                TimerAction::Once(slot) => {
                    let ev = slot.take();
                    self.rearm(now);
                    return ev;
                }
                TimerAction::Repeat(interval, make) => {
                    let ev = make();
                    let next_deadline = now + *interval;
                    let seq = self.next_seq;
                    self.next_seq = seq.wrapping_add(1);
                    entry.deadline = next_deadline;
                    entry.seq = seq;
                    self.heap.push(Reverse(entry));
                    self.rearm(now);
                    return Some(ev);
                }
            }
        }
        self.rearm(now);
        None
    }

    fn rearm(&mut self, now: Instant) {
        while let Some(Reverse(top)) = self.heap.peek() {
            if top.cancelled.get() {
                self.heap.pop();
            } else {
                break;
            }
        }
        let Some(arm) = &self.arm else {
            return;
        };
        let next = self.heap.peek().map(|Reverse(top)| {
            let wait = top.deadline.saturating_duration_since(now);
            if wait.is_zero() {
                0
            } else {
                wait.as_millis().clamp(1, u32::MAX as u128) as u32
            }
        });
        arm(next);
    }
}

struct BgQueue<E> {
    ready: Mutex<VecDeque<E>>,
    wake: Mutex<Option<WakeCallback>>,
    wake_posted: AtomicBool,
}

impl<E> BgQueue<E> {
    fn new() -> Self {
        Self {
            ready: Mutex::new(VecDeque::new()),
            wake: Mutex::new(None),
            wake_posted: AtomicBool::new(false),
        }
    }

    fn push(&self, event: E) {
        self.ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event);
        if !self.wake_posted.swap(true, Ordering::AcqRel)
            && let Some(cb) = self
                .wake
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .cloned()
        {
            cb();
        }
    }

    fn pop(&self) -> Option<E> {
        let mut q = self.ready.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ev) = q.pop_front() {
            return Some(ev);
        }
        self.wake_posted.store(false, Ordering::Release);
        None
    }
}

pub(crate) struct TimerHandle {
    cancelled: Rc<Cell<bool>>,
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancelled.set(true);
    }
}

pub(crate) struct Jobs<E> {
    bg: Arc<BgQueue<E>>,
    timers: RefCell<TimerHeap<E>>,
}

impl<E: Send + 'static> Jobs<E> {
    pub(crate) fn new() -> Self {
        Self {
            bg: Arc::new(BgQueue::new()),
            timers: RefCell::new(TimerHeap::new()),
        }
    }

    pub(crate) fn attach_platform(&self, platform: &super::Platform) {
        *self.bg.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(platform.wake_callback());
        let mut timers = self.timers.borrow_mut();
        timers.arm = Some(platform.timer_callback());
        timers.rearm(Instant::now());
    }

    pub(crate) fn post(&self, event: E) {
        self.bg.push(event);
    }

    pub(crate) fn after(&self, delay: Duration, event: E) -> TimerHandle {
        self.timers
            .borrow_mut()
            .schedule(delay, TimerAction::Once(Some(event)))
    }

    pub(crate) fn every(
        &self,
        delay: Duration,
        interval: Duration,
        make: impl Fn() -> E + 'static,
    ) -> TimerHandle {
        self.timers
            .borrow_mut()
            .schedule(delay, TimerAction::Repeat(interval, Box::new(make)))
    }

    pub(crate) fn spawn<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce() -> T + Send + 'static,
        finish: impl FnOnce(std::result::Result<T, String>) -> E + Send + 'static,
    ) {
        let target = self.bg.clone();
        let finish = Arc::new(Mutex::new(Some(finish)));
        let thread_target = target.clone();
        let thread_finish = finish.clone();
        let res = thread::Builder::new().name(name.into()).spawn(move || {
            let outcome = match catch_unwind(AssertUnwindSafe(work)) {
                Ok(val) => Ok(val),
                Err(payload) => Err(super::diag::panic_message(&*payload)),
            };
            if let Some(f) = thread_finish
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                thread_target.push(f(outcome));
            }
        });
        if let Err(err) = res
            && let Some(f) = finish.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            target.push(f(Err(format!("Failed to start worker thread: {err}"))));
        }
    }

    pub(crate) fn pop(&self) -> Option<E> {
        if let Some(ev) = self.timers.borrow_mut().pop_due(Instant::now()) {
            return Some(ev);
        }
        self.bg.pop()
    }

    #[cfg(test)]
    pub(crate) fn pop_at(&self, now: Instant) -> Option<E> {
        if let Some(ev) = self.timers.borrow_mut().pop_due(now) {
            return Some(ev);
        }
        self.bg.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TestEvent {
        Once,
        Cancelled,
        Tick,
        Bg(usize),
        Panicked(String),
    }

    #[test]
    fn timers_fire_cancel_on_drop_and_repeat_until_cancelled() {
        let jobs = Jobs::new();
        let t0 = Instant::now();
        let _keep = jobs.after(Duration::from_millis(20), TestEvent::Once);
        let cancelled = jobs.after(Duration::from_millis(10), TestEvent::Cancelled);
        drop(cancelled);

        assert_eq!(jobs.pop_at(t0 + Duration::from_millis(15)), None);
        assert_eq!(
            jobs.pop_at(t0 + Duration::from_millis(25)),
            Some(TestEvent::Once)
        );
        assert_eq!(jobs.pop_at(t0 + Duration::from_millis(30)), None);

        let repeating = jobs.every(
            Duration::from_millis(15),
            Duration::from_millis(15),
            || TestEvent::Tick,
        );
        let t1 = Instant::now();
        assert_eq!(
            jobs.pop_at(t1 + Duration::from_millis(20)),
            Some(TestEvent::Tick)
        );
        assert_eq!(
            jobs.pop_at(t1 + Duration::from_millis(40)),
            Some(TestEvent::Tick)
        );
        drop(repeating);
        assert_eq!(jobs.pop_at(t1 + Duration::from_millis(100)), None);
    }

    #[test]
    fn cancel_prevents_already_elapsed_timer_from_firing() {
        let jobs = Jobs::new();
        let handle = jobs.after(Duration::from_millis(5), TestEvent::Cancelled);
        let after_due = Instant::now() + Duration::from_millis(20);
        drop(handle);
        assert_eq!(jobs.pop_at(after_due), None);
    }

    #[test]
    fn spawn_delivers_typed_event_and_recovers_from_worker_panic() {
        let jobs = Jobs::new();
        jobs.spawn(
            "test-panic",
            || -> usize { panic!("boom") },
            |res| match res {
                Ok(n) => TestEvent::Bg(n),
                Err(msg) => TestEvent::Panicked(msg),
            },
        );
        jobs.spawn(
            "test-ok",
            || 21 * 2,
            |res| match res {
                Ok(n) => TestEvent::Bg(n),
                Err(msg) => TestEvent::Panicked(msg),
            },
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got = Vec::new();
        while Instant::now() < deadline && got.len() < 2 {
            if let Some(ev) = jobs.pop() {
                got.push(ev);
            } else {
                thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(got.contains(&TestEvent::Bg(42)));
        assert!(
            got.iter()
                .any(|e| matches!(e, TestEvent::Panicked(m) if m.contains("boom")))
        );
    }
}
