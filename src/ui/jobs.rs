//! Clean-room UI-thread job queue, drop-canceling timers, background thread pool, and dedicated
//! STA/blocking task delivery — zero `Future`/`Waker`/`Poll` machinery and zero `unsafe`.

use std::any::Any;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use super::app::App;

type WakeCallback = Arc<dyn Fn() + Send + Sync>;
type PostedCallback = Box<dyn FnOnce(&mut App) + Send>;
type BgDoneCallback = Box<dyn FnOnce(Box<dyn Any + Send>, &mut App)>;

enum UiTimerCallback {
    OneShot(Box<dyn FnOnce(&mut App)>),
    Repeating {
        interval: Duration,
        cancelled: Arc<AtomicBool>,
        tick: Box<dyn FnMut(&mut App) -> bool>,
    },
}

#[derive(Default)]
struct UiLocalTables {
    timers: HashMap<(u64, u64), UiTimerCallback>,
    bg_done: HashMap<(u64, u64), BgDoneCallback>,
}

thread_local! {
    static UI_TABLES: RefCell<UiLocalTables> = RefCell::new(UiLocalTables::default());
}

static NEXT_JOBS_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_ITEM_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// Global Condvar-backed timer min-heap (0% CPU when idle)
// ---------------------------------------------------------------------------

struct ScheduledTimer {
    deadline: Instant,
    seq: u64,
    id: u64,
    cancelled: Arc<AtomicBool>,
    target: Arc<JobsInner>,
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
        id: u64,
        cancelled: Arc<AtomicBool>,
        target: Arc<JobsInner>,
    ) {
        let mut guard = self.state.lock().unwrap();
        let seq = guard.1;
        guard.1 = seq.wrapping_add(1);
        let wake_thread = guard
            .0
            .peek()
            .is_none_or(|Reverse(top)| deadline < top.deadline);
        guard.0.push(Reverse(ScheduledTimer {
            deadline,
            seq,
            id,
            cancelled,
            target,
        }));
        if wake_thread {
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
                    let Reverse(entry) = guard.0.pop().unwrap();
                    if !entry.cancelled.load(Ordering::Relaxed) {
                        entry.target.push_timer(entry.id);
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
// Global 2-thread background worker pool (Condvar-backed; 0% CPU when idle)
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
// Per-`App` job/timer delivery queues
// ---------------------------------------------------------------------------

enum ReadyWork {
    Timer(u64),
    BgDone(u64, Box<dyn Any + Send>),
    Posted(PostedCallback),
}

struct JobsInner {
    ready: Mutex<VecDeque<ReadyWork>>,
    wake: Mutex<Option<WakeCallback>>,
}

impl JobsInner {
    fn new() -> Self {
        Self {
            ready: Mutex::new(VecDeque::new()),
            wake: Mutex::new(None),
        }
    }

    fn wake(&self) {
        if let Some(cb) = self.wake.lock().unwrap().as_ref().cloned() {
            cb();
        }
    }

    fn push_timer(&self, id: u64) {
        self.ready.lock().unwrap().push_back(ReadyWork::Timer(id));
        self.wake();
    }

    fn push_bg_done(&self, id: u64, value: Box<dyn Any + Send>) {
        self.ready
            .lock()
            .unwrap()
            .push_back(ReadyWork::BgDone(id, value));
        self.wake();
    }

    fn push_posted(&self, cb: PostedCallback) {
        self.ready.lock().unwrap().push_back(ReadyWork::Posted(cb));
        self.wake();
    }

    fn pop(&self) -> Option<ReadyWork> {
        self.ready.lock().unwrap().pop_front()
    }
}

/// Thread-safe handle allowing background threads (e.g., named-pipe listener) to post callbacks
/// onto the UI thread and wake the Win32 message loop.
#[derive(Clone)]
pub(crate) struct UiPoster {
    inner: Arc<JobsInner>,
}

impl UiPoster {
    pub(crate) fn post(&self, cb: impl FnOnce(&mut App) + Send + 'static) {
        self.inner.push_posted(Box::new(cb));
    }
}

/// RAII handle to a scheduled one-shot or repeating timer. Dropping cancels the timer unless
/// `.detach()` was called.
#[derive(Default)]
pub(crate) struct TimerHandle {
    slot: Option<(u64, u64, Arc<AtomicBool>)>,
}

impl TimerHandle {
    pub(crate) fn cancel(&mut self) {
        if let Some((jobs_id, id, cancelled)) = self.slot.take() {
            cancelled.store(true, Ordering::Relaxed);
            UI_TABLES.with(|tables| {
                tables.borrow_mut().timers.remove(&(jobs_id, id));
            });
        }
    }

    pub(crate) fn detach(mut self) {
        self.slot = None;
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Owned by `App`; manages timers, background worker completions, and cross-thread UI posts.
pub(crate) struct Jobs {
    jobs_id: u64,
    inner: Arc<JobsInner>,
}

impl Jobs {
    pub(crate) fn new() -> Self {
        Self {
            jobs_id: NEXT_JOBS_ID.fetch_add(1, Ordering::Relaxed),
            inner: Arc::new(JobsInner::new()),
        }
    }

    pub(crate) fn set_wake_callback(&self, wake: WakeCallback) {
        *self.inner.wake.lock().unwrap() = Some(wake);
    }

    pub(crate) fn poster(&self) -> UiPoster {
        UiPoster {
            inner: self.inner.clone(),
        }
    }

    pub(crate) fn after(
        &self,
        delay: Duration,
        on_fire: impl FnOnce(&mut App) + 'static,
    ) -> TimerHandle {
        let id = NEXT_ITEM_ID.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        UI_TABLES.with(|tables| {
            tables
                .borrow_mut()
                .timers
                .insert((self.jobs_id, id), UiTimerCallback::OneShot(Box::new(on_fire)));
        });
        TimerWheel::global().schedule(
            Instant::now() + delay,
            id,
            cancelled.clone(),
            self.inner.clone(),
        );
        TimerHandle {
            slot: Some((self.jobs_id, id, cancelled)),
        }
    }

    pub(crate) fn every(
        &self,
        delay: Duration,
        interval: Duration,
        on_tick: impl FnMut(&mut App) -> bool + 'static,
    ) -> TimerHandle {
        let id = NEXT_ITEM_ID.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        UI_TABLES.with(|tables| {
            tables.borrow_mut().timers.insert(
                (self.jobs_id, id),
                UiTimerCallback::Repeating {
                    interval,
                    cancelled: cancelled.clone(),
                    tick: Box::new(on_tick),
                },
            );
        });
        TimerWheel::global().schedule(
            Instant::now() + delay,
            id,
            cancelled.clone(),
            self.inner.clone(),
        );
        TimerHandle {
            slot: Some((self.jobs_id, id, cancelled)),
        }
    }

    pub(crate) fn spawn_bg<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        on_done: impl FnOnce(R, &mut App) + 'static,
    ) {
        let id = NEXT_ITEM_ID.fetch_add(1, Ordering::Relaxed);
        UI_TABLES.with(|tables| {
            tables.borrow_mut().bg_done.insert(
                (self.jobs_id, id),
                Box::new(move |boxed, cx| {
                    let value = *boxed.downcast::<R>().expect("spawn_bg result type mismatch");
                    on_done(value, cx);
                }),
            );
        });
        let target = self.inner.clone();
        WorkerPool::global().submit(Box::new(move || {
            let result = work();
            target.push_bg_done(id, Box::new(result));
        }));
    }

    pub(crate) fn spawn_dedicated<R: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce() -> R + Send + 'static,
        on_done: impl FnOnce(R, &mut App) + 'static,
    ) {
        let id = NEXT_ITEM_ID.fetch_add(1, Ordering::Relaxed);
        UI_TABLES.with(|tables| {
            tables.borrow_mut().bg_done.insert(
                (self.jobs_id, id),
                Box::new(move |boxed, cx| {
                    let value = *boxed
                        .downcast::<R>()
                        .expect("spawn_dedicated result type mismatch");
                    on_done(value, cx);
                }),
            );
        });
        let target = self.inner.clone();
        thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = work();
                target.push_bg_done(id, Box::new(result));
            })
            .expect("failed to spawn dedicated worker thread");
    }

    /// Drains all ready timers, background completions, and cross-thread posts on the UI thread.
    pub(crate) fn drain_into(app: &mut App) {
        let jobs_id = app.jobs.jobs_id;
        let inner = app.jobs.inner.clone();
        while let Some(item) = inner.pop() {
            match item {
                ReadyWork::Timer(id) => {
                    let cb = UI_TABLES.with(|t| t.borrow_mut().timers.remove(&(jobs_id, id)));
                    match cb {
                        Some(UiTimerCallback::OneShot(f)) => f(app),
                        Some(UiTimerCallback::Repeating {
                            interval,
                            cancelled,
                            mut tick,
                        }) => {
                            if !cancelled.load(Ordering::Relaxed)
                                && tick(app)
                                && !cancelled.load(Ordering::Relaxed)
                            {
                                UI_TABLES.with(|t| {
                                    t.borrow_mut().timers.insert(
                                        (jobs_id, id),
                                        UiTimerCallback::Repeating {
                                            interval,
                                            cancelled: cancelled.clone(),
                                            tick,
                                        },
                                    );
                                });
                                TimerWheel::global().schedule(
                                    Instant::now() + interval,
                                    id,
                                    cancelled,
                                    inner.clone(),
                                );
                            }
                        }
                        None => {}
                    }
                }
                ReadyWork::BgDone(id, value) => {
                    if let Some(cb) =
                        UI_TABLES.with(|t| t.borrow_mut().bg_done.remove(&(jobs_id, id)))
                    {
                        cb(value, app);
                    }
                }
                ReadyWork::Posted(cb) => cb(app),
            }
            app.flush_effects();
        }
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        let jobs_id = self.jobs_id;
        UI_TABLES.with(|tables| {
            let mut t = tables.borrow_mut();
            t.timers.retain(|&(jid, _), _| jid != jobs_id);
            t.bg_done.retain(|&(jid, _), _| jid != jobs_id);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::WindowsPlatform;
    use std::rc::Rc;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn timers_fire_cancel_on_drop_and_repeat_until_false() {
        let platform = Rc::new(WindowsPlatform::new().expect("WindowsPlatform::new failed"));
        let app_cell = App::new_rc(platform);

        let fired_once = Arc::new(AtomicBool::new(false));
        let fired_cancelled = Arc::new(AtomicBool::new(false));
        let repeat_count = Arc::new(AtomicUsize::new(0));

        let _keep_once;
        let _keep_repeat;
        {
            let app = app_cell.inner.borrow();
            let f1 = fired_once.clone();
            _keep_once = app.after(Duration::from_millis(15), move |_| {
                f1.store(true, Ordering::SeqCst);
            });

            let f2 = fired_cancelled.clone();
            let dropped_timer = app.after(Duration::from_millis(10), move |_| {
                f2.store(true, Ordering::SeqCst);
            });
            drop(dropped_timer);

            let rc = repeat_count.clone();
            _keep_repeat = app.every(
                Duration::from_millis(10),
                Duration::from_millis(10),
                move |_| rc.fetch_add(1, Ordering::SeqCst) + 1 < 3,
            );
        }

        for _ in 0..40 {
            thread::sleep(Duration::from_millis(10));
            app_cell.inner.borrow_mut().drain_jobs();
            if fired_once.load(Ordering::SeqCst) && repeat_count.load(Ordering::SeqCst) >= 3 {
                break;
            }
        }

        assert!(fired_once.load(Ordering::SeqCst), "one-shot timer must fire");
        assert!(
            !fired_cancelled.load(Ordering::SeqCst),
            "dropped TimerHandle must cancel before firing"
        );
        assert_eq!(
            repeat_count.load(Ordering::SeqCst),
            3,
            "repeating timer must stop once callback returns false"
        );
    }

    #[test]
    fn spawn_bg_delivers_result_to_ui_thread() {
        let platform = Rc::new(WindowsPlatform::new().expect("WindowsPlatform::new failed"));
        let app_cell = App::new_rc(platform);
        let delivered = Arc::new(AtomicUsize::new(0));
        {
            let d = delivered.clone();
            let app = app_cell.inner.borrow();
            app.spawn_bg(
                || 21usize * 2,
                move |val, _| {
                    d.store(val, Ordering::SeqCst);
                },
            );
        }
        for _ in 0..50 {
            app_cell.inner.borrow_mut().drain_jobs();
            if delivered.load(Ordering::SeqCst) == 42 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(delivered.load(Ordering::SeqCst), 42);
    }
}
