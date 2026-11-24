use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

type WakeCallback = Arc<dyn Fn() + Send + Sync>;

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
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut due = Vec::new();
        loop {
            {
                let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                while let Some(Reverse(top)) = guard.0.peek() {
                    if top.cancelled.load(Ordering::Acquire) {
                        guard.0.pop();
                        continue;
                    }
                    if top.deadline <= now {
                        due.push(guard.0.pop().unwrap().0);
                    } else {
                        break;
                    }
                }
                if due.is_empty() {
                    if let Some(Reverse(top)) = guard.0.peek() {
                        let wait = top.deadline.saturating_duration_since(Instant::now());
                        if !wait.is_zero() {
                            drop(self.condvar.wait_timeout(guard, wait));
                        }
                    } else {
                        drop(self.condvar.wait(guard));
                    }
                    continue;
                }
            }

            let mut reschedule = Vec::new();
            for mut entry in due.drain(..) {
                if !entry.cancelled.load(Ordering::Acquire) {
                    let _ = catch_unwind(AssertUnwindSafe(|| (entry.fire)()));
                    if let Some(interval) = entry.repeat
                        && !entry.cancelled.load(Ordering::Acquire)
                    {
                        entry.deadline = Instant::now() + interval;
                        reschedule.push(entry);
                    }
                }
            }
            if !reschedule.is_empty() {
                let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
                for mut entry in reschedule {
                    let seq = guard.1;
                    guard.1 = seq.wrapping_add(1);
                    entry.seq = seq;
                    guard.0.push(Reverse(entry));
                }
            }
        }
    }
}

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
                            let mut q = p.queue.lock().unwrap_or_else(|e| e.into_inner());
                            while q.is_empty() {
                                q = p.condvar.wait(q).unwrap_or_else(|e| e.into_inner());
                            }
                            q.pop_front().unwrap()
                        };
                        let _ = catch_unwind(AssertUnwindSafe(job));
                    })
                    .expect("failed to spawn background worker");
            }
            pool
        })
    }

    fn submit(&self, job: PoolJob) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(job);
        self.condvar.notify_one();
    }
}

struct QueuedEvent<E> {
    event: E,
    cancelled: Option<Arc<AtomicBool>>,
}

struct JobsInner<E> {
    ready: Mutex<VecDeque<QueuedEvent<E>>>,
    wake: Mutex<Option<WakeCallback>>,
    wake_posted: AtomicBool,
}

impl<E> JobsInner<E> {
    fn new() -> Self {
        Self {
            ready: Mutex::new(VecDeque::new()),
            wake: Mutex::new(None),
            wake_posted: AtomicBool::new(false),
        }
    }

    fn push(&self, event: E, cancelled: Option<Arc<AtomicBool>>) {
        self.ready
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(QueuedEvent { event, cancelled });
        if !self.wake_posted.swap(true, Ordering::AcqRel)
            && let Some(cb) = self.wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned()
        {
            cb();
        }
    }

    fn pop(&self) -> Option<E> {
        let mut q = self.ready.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(item) = q.pop_front() {
            if item
                .cancelled
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                continue;
            }
            return Some(item.event);
        }
        self.wake_posted.store(false, Ordering::Release);
        None
    }
}

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
        self.inner.push(event, None);
    }
}

#[derive(Default)]
pub(crate) struct TimerHandle {
    cancelled: Option<Arc<AtomicBool>>,
}

impl TimerHandle {
    pub(crate) fn cancel(&mut self) {
        if let Some(flag) = self.cancelled.take() {
            flag.store(true, Ordering::Release);
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
        *self.inner.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }

    pub(crate) fn poster(&self) -> EventPoster<E> {
        EventPoster {
            inner: self.inner.clone(),
        }
    }

    pub(crate) fn after(&self, delay: Duration, event: E) -> TimerHandle {
        let cancelled = Arc::new(AtomicBool::new(false));
        let target = self.inner.clone();
        let flag_for_queue = cancelled.clone();
        let slot = Mutex::new(Some(event));
        TimerWheel::global().schedule(
            Instant::now() + delay,
            cancelled.clone(),
            None,
            Box::new(move || {
                if let Some(ev) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    target.push(ev, Some(flag_for_queue.clone()));
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
        let flag_for_queue = cancelled.clone();
        TimerWheel::global().schedule(
            Instant::now() + delay,
            cancelled.clone(),
            Some(interval),
            Box::new(move || target.push(make(), Some(flag_for_queue.clone()))),
        );
        TimerHandle {
            cancelled: Some(cancelled),
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_bg(&self, work: impl FnOnce() -> E + Send + 'static) {
        let target = self.inner.clone();
        WorkerPool::global().submit(Box::new(move || {
            target.push(work(), None);
        }));
    }

    pub(crate) fn spawn_bg_guarded(
        &self,
        work: impl FnOnce() -> E + Send + 'static,
        on_panic: impl FnOnce(String) -> E + Send + 'static,
    ) {
        let target = self.inner.clone();
        WorkerPool::global().submit(Box::new(move || {
            let ev = match catch_unwind(AssertUnwindSafe(work)) {
                Ok(ev) => ev,
                Err(payload) => on_panic(panic_message(&*payload)),
            };
            target.push(ev, None);
        }));
    }

    pub(crate) fn spawn_dedicated_guarded(
        &self,
        name: &str,
        work: impl FnOnce() -> E + Send + 'static,
        on_panic: impl FnOnce(String) -> E + Send + 'static,
    ) {
        let target = self.inner.clone();
        let on_panic = Arc::new(Mutex::new(Some(on_panic)));
        let thread_target = target.clone();
        let thread_on_panic = on_panic.clone();
        let res = thread::Builder::new().name(name.into()).spawn(move || {
            let ev = match catch_unwind(AssertUnwindSafe(work)) {
                Ok(ev) => ev,
                Err(payload) => {
                    let Some(handler) = thread_on_panic
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take()
                    else {
                        return;
                    };
                    handler(panic_message(&*payload))
                }
            };
            thread_target.push(ev, None);
        });
        if let Err(err) = res
            && let Some(handler) = on_panic.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            target.push(handler(format!("Failed to start worker thread: {err}")), None);
        }
    }

    pub(crate) fn pop(&self) -> Option<E> {
        self.inner.pop()
    }
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Internal error".to_owned()
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
                    TestEvent::Bg(_) | TestEvent::Panicked(_) => {}
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
    fn hard_cancel_drops_already_queued_timer_event() {
        let jobs = Jobs::<TestEvent>::new();
        let mut handle = jobs.after(Duration::from_millis(5), TestEvent::Cancelled);
        thread::sleep(Duration::from_millis(25));
        handle.cancel();
        assert_eq!(jobs.pop(), None);
    }

    #[test]
    fn spawn_bg_delivers_typed_event_and_recovers_from_worker_panic() {
        let jobs = Jobs::<TestEvent>::new();
        jobs.spawn_bg_guarded(
            || panic!("simulated worker failure"),
            TestEvent::Panicked,
        );
        jobs.spawn_bg(|| TestEvent::Bg(21 * 2));

        let mut saw_panic = false;
        let mut saw_bg = false;
        for _ in 0..50 {
            while let Some(ev) = jobs.pop() {
                match ev {
                    TestEvent::Panicked(msg) => {
                        assert!(msg.contains("simulated worker failure"));
                        saw_panic = true;
                    }
                    TestEvent::Bg(42) => saw_bg = true,
                    _ => {}
                }
            }
            if saw_panic && saw_bg {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(saw_panic, "guarded worker panic must still deliver error event");
        assert!(saw_bg, "worker pool must keep serving jobs after a panic");
    }
}
