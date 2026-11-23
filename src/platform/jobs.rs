//! Typed UI-thread event delivery for drop-canceling timers, background pool jobs, STA threads,
//! and cross-thread posts — zero `Any`, zero thread-local callback tables, zero `unsafe`.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

type WakeCallback = Arc<dyn Fn() + Send + Sync>;

// ---------------------------------------------------------------------------
// Global Condvar-backed timer min-heap (0% CPU when idle)
// ---------------------------------------------------------------------------

struct ScheduledTimer {
    deadline: Instant,
    seq: u64,
    cancelled: Arc<AtomicBool>,
    repeat: Option<Duration>,
    fire: Box<dyn Fn() + Send + 'static>,
}

impl PartialEq for ScheduledTimer {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}
impl Eq for ScheduledTimer {}
impl PartialOrd for ScheduledTimer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScheduledTimer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline
            .cmp(&other.deadline)
            .then(self.seq.cmp(&other.seq))
    }
}

struct TimerWheel {
    state: Mutex<(BinaryHeap<Reverse<ScheduledTimer>>, u64)>,
    condvar: Condvar,
}

impl TimerWheel {
    fn global() -> &'static Arc<Self> {
        static WHEEL: OnceLock<Arc<TimerWheel>> = OnceLock::new();
        WHEEL.get_or_init(|| {
            let wheel = Arc::new(TimerWheel {
                state: Mutex::new((BinaryHeap::new(), 1)),
                condvar: Condvar::new(),
            });
            let bg = wheel.clone();
            thread::Builder::new()
                .name("jot-timer".into())
                .spawn(move || bg.run())
                .expect("failed to spawn timer thread");
            wheel
        })
    }

    fn schedule(
        &self,
        deadline: Instant,
        cancelled: Arc<AtomicBool>,
        repeat: Option<Duration>,
        fire: Box<dyn Fn() + Send + 'static>,
    ) {
        let mut guard = self.state.lock().unwrap();
        let seq = guard.1;
        guard.1 = seq.wrapping_add(1);
        let wake = guard
            .0
            .peek()
            .is_none_or(|Reverse(top)| deadline < top.deadline);
        guard.0.push(Reverse(ScheduledTimer {
            deadline,
            seq,
            cancelled,
            repeat,
            fire,
        }));
        if wake {
            self.condvar.notify_one();
        }
    }

    fn run(&self) {
        let mut guard = self.state.lock().unwrap();
        loop {
            let now = Instant::now();
            while let Some(Reverse(top)) = guard.0.peek() {
                if top.cancelled.load(Ordering::Relaxed) {
                    guard.0.pop();
                    continue;
                }
                if top.deadline <= now {
                    let Reverse(mut entry) = guard.0.pop().unwrap();
                    if !entry.cancelled.load(Ordering::Relaxed) {
                        (entry.fire)();
                        if let Some(interval) = entry.repeat
                            && !entry.cancelled.load(Ordering::Relaxed)
                        {
                            let seq = guard.1;
                            guard.1 = seq.wrapping_add(1);
                            entry.deadline = Instant::now() + interval;
                            entry.seq = seq;
                            guard.0.push(Reverse(entry));
                        }
                    }
                } else {
                    break;
                }
            }
            if let Some(Reverse(top)) = guard.0.peek() {
                let wait = top.deadline.saturating_duration_since(Instant::now());
                if !wait.is_zero() {
                    let (g, _) = self.condvar.wait_timeout(guard, wait).unwrap();
                    guard = g;
                }
            } else {
                guard = self.condvar.wait(guard).unwrap();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global 2-thread background worker pool (0% CPU when idle)
// ---------------------------------------------------------------------------

type PoolJob = Box<dyn FnOnce() + Send + 'static>;

struct WorkerPool {
    queue: Mutex<VecDeque<PoolJob>>,
    condvar: Condvar,
}

impl WorkerPool {
    fn global() -> &'static Arc<Self> {
        static POOL: OnceLock<Arc<WorkerPool>> = OnceLock::new();
        POOL.get_or_init(|| {
            let pool = Arc::new(WorkerPool {
                queue: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
            });
            for ix in 0..2 {
                let p = pool.clone();
                thread::Builder::new()
                    .name(format!("jot-bg-{ix}"))
                    .spawn(move || loop {
                        let job = {
                            let mut q = p.queue.lock().unwrap();
                            while q.is_empty() {
                                q = p.condvar.wait(q).unwrap();
                            }
                            q.pop_front().unwrap()
                        };
                        job();
                    })
                    .expect("failed to spawn background worker");
            }
            pool
        })
    }

    fn submit(&self, job: PoolJob) {
        self.queue.lock().unwrap().push_back(job);
        self.condvar.notify_one();
    }
}

// ---------------------------------------------------------------------------
// Typed job/timer queue owned by `App`
// ---------------------------------------------------------------------------

struct JobsInner<E> {
    ready: Mutex<VecDeque<E>>,
    wake: Mutex<Option<WakeCallback>>,
}

impl<E> JobsInner<E> {
    fn new() -> Self {
        Self {
            ready: Mutex::new(VecDeque::new()),
            wake: Mutex::new(None),
        }
    }

    fn push(&self, event: E) {
        self.ready.lock().unwrap().push_back(event);
        if let Some(cb) = self.wake.lock().unwrap().as_ref().cloned() {
            cb();
        }
    }

    fn pop(&self) -> Option<E> {
        self.ready.lock().unwrap().pop_front()
    }
}

/// Thread-safe handle allowing background threads (e.g., named-pipe listener) to post typed events.
pub(crate) struct EventPoster<E> {
    inner: Arc<JobsInner<E>>,
}

impl<E> Clone for EventPoster<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<E> EventPoster<E> {
    pub(crate) fn post(&self, event: E) {
        self.inner.push(event);
    }
}

/// RAII handle to a scheduled one-shot or repeating timer. Dropping cancels it unless `.detach()`ed.
#[derive(Default)]
pub(crate) struct TimerHandle {
    cancelled: Option<Arc<AtomicBool>>,
}

impl TimerHandle {
    pub(crate) fn cancel(&mut self) {
        if let Some(flag) = self.cancelled.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(crate) fn detach(mut self) {
        self.cancelled = None;
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Typed timer and background-job manager delivering `E` events to the UI thread.
pub(crate) struct Jobs<E> {
    inner: Arc<JobsInner<E>>,
}

impl<E: Send + 'static> Jobs<E> {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(JobsInner::new()),
        }
    }

    pub(crate) fn set_wake_callback(&self, wake: WakeCallback) {
        *self.inner.wake.lock().unwrap() = Some(wake);
    }

    pub(crate) fn poster(&self) -> EventPoster<E> {
        EventPoster {
            inner: self.inner.clone(),
        }
    }

    pub(crate) fn after(&self, delay: Duration, event: E) -> TimerHandle {
        let cancelled = Arc::new(AtomicBool::new(false));
        let target = self.inner.clone();
        let slot = Mutex::new(Some(event));
        TimerWheel::global().schedule(
            Instant::now() + delay,
            cancelled.clone(),
            None,
            Box::new(move || {
                if let Some(ev) = slot.lock().unwrap().take() {
                    target.push(ev);
                }
            }),
        );
        TimerHandle {
            cancelled: Some(cancelled),
        }
    }

    pub(crate) fn every(
        &self,
        delay: Duration,
        interval: Duration,
        make: impl Fn() -> E + Send + 'static,
    ) -> TimerHandle {
        let cancelled = Arc::new(AtomicBool::new(false));
        let target = self.inner.clone();
        TimerWheel::global().schedule(
            Instant::now() + delay,
            cancelled.clone(),
            Some(interval),
            Box::new(move || target.push(make())),
        );
        TimerHandle {
            cancelled: Some(cancelled),
        }
    }

    pub(crate) fn spawn_bg(&self, work: impl FnOnce() -> E + Send + 'static) {
        let target = self.inner.clone();
        WorkerPool::global().submit(Box::new(move || {
            target.push(work());
        }));
    }

    pub(crate) fn spawn_dedicated(&self, name: &str, work: impl FnOnce() -> E + Send + 'static) {
        let target = self.inner.clone();
        thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                target.push(work());
            })
            .expect("failed to spawn dedicated worker thread");
    }

    pub(crate) fn pop(&self) -> Option<E> {
        self.inner.pop()
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
    }

    #[test]
    fn timers_fire_cancel_on_drop_and_repeat_until_cancelled() {
        let jobs = Jobs::<TestEvent>::new();

        let _keep_once = jobs.after(Duration::from_millis(15), TestEvent::Once);
        let dropped = jobs.after(Duration::from_millis(10), TestEvent::Cancelled);
        drop(dropped);
        let mut repeat = jobs.every(
            Duration::from_millis(10),
            Duration::from_millis(10),
            || TestEvent::Tick,
        );

        let mut saw_once = false;
        let mut saw_cancelled = false;
        let mut ticks = 0usize;

        for _ in 0..40 {
            thread::sleep(Duration::from_millis(10));
            while let Some(ev) = jobs.pop() {
                match ev {
                    TestEvent::Once => saw_once = true,
                    TestEvent::Cancelled => saw_cancelled = true,
                    TestEvent::Tick => {
                        ticks += 1;
                        if ticks >= 3 {
                            repeat.cancel();
                        }
                    }
                    TestEvent::Bg(_) => {}
                }
            }
            if saw_once && ticks >= 3 {
                break;
            }
        }

        assert!(saw_once, "one-shot timer must fire");
        assert!(!saw_cancelled, "dropped TimerHandle must cancel before firing");
        assert!(ticks >= 3, "repeating timer must tick until cancelled");
    }

    #[test]
    fn spawn_bg_delivers_typed_event() {
        let jobs = Jobs::<TestEvent>::new();
        jobs.spawn_bg(|| TestEvent::Bg(21 * 2));

        let mut received = None;
        for _ in 0..50 {
            if let Some(ev) = jobs.pop() {
                received = Some(ev);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(received, Some(TestEvent::Bg(42)));
    }
}
