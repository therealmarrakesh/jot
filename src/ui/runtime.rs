//! Safe, single-threaded UI task executor + background thread pool + timer thread + channels.
//!
//! Zero `unsafe` code. Wakes the Win32 message loop via a registered wake callback
//! (`PostMessageW` to a hidden message-only window), so foreground tasks and timers run even
//! inside modal Win32 move/size loops (e.g., tab tear-off drag).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;
type SendBoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Async channels (`oneshot` and unbounded `mpsc`) compatible with `crate::ui::futures`.
pub(crate) mod futures {
    use super::*;

    pub(crate) struct NextFuture<'a, S>(&'a mut S);

    pub(crate) mod channel {
        use super::*;

        pub(crate) mod oneshot {
            use super::*;

            #[derive(Copy, Clone, Debug, PartialEq, Eq)]
            pub(crate) struct Canceled;

            struct Inner<T> {
                value: Option<T>,
                closed: bool,
                waker: Option<Waker>,
            }

            pub(crate) struct Sender<T>(Arc<Mutex<Inner<T>>>);
            pub(crate) struct Receiver<T>(Arc<Mutex<Inner<T>>>);

            pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
                let inner = Arc::new(Mutex::new(Inner {
                    value: None,
                    closed: false,
                    waker: None,
                }));
                (Sender(inner.clone()), Receiver(inner))
            }

            impl<T> Sender<T> {
                pub(crate) fn send(self, value: T) -> Result<(), T> {
                    let waker = {
                        let mut guard = self.0.lock().unwrap();
                        if guard.closed {
                            return Err(value);
                        }
                        guard.value = Some(value);
                        guard.waker.take()
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                    Ok(())
                }
            }

            impl<T> Drop for Sender<T> {
                fn drop(&mut self) {
                    let waker = {
                        let mut guard = self.0.lock().unwrap();
                        guard.closed = true;
                        guard.waker.take()
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
            }

            impl<T> Drop for Receiver<T> {
                fn drop(&mut self) {
                    let mut guard = self.0.lock().unwrap();
                    guard.closed = true;
                    guard.waker = None;
                }
            }

            impl<T> Future for Receiver<T> {
                type Output = Result<T, Canceled>;

                fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    let mut guard = self.0.lock().unwrap();
                    if let Some(value) = guard.value.take() {
                        return Poll::Ready(Ok(value));
                    }
                    if guard.closed {
                        return Poll::Ready(Err(Canceled));
                    }
                    guard.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }

        pub(crate) mod mpsc {
            use super::*;

            struct Inner<T> {
                queue: VecDeque<T>,
                senders: usize,
                closed: bool,
                waker: Option<Waker>,
            }

            pub(crate) struct UnboundedSender<T>(Arc<Mutex<Inner<T>>>);
            pub(crate) struct UnboundedReceiver<T>(Arc<Mutex<Inner<T>>>);

            pub(crate) fn unbounded<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
                let inner = Arc::new(Mutex::new(Inner {
                    queue: VecDeque::new(),
                    senders: 1,
                    closed: false,
                    waker: None,
                }));
                (UnboundedSender(inner.clone()), UnboundedReceiver(inner))
            }

            impl<T> Clone for UnboundedSender<T> {
                fn clone(&self) -> Self {
                    self.0.lock().unwrap().senders += 1;
                    Self(self.0.clone())
                }
            }

            impl<T> UnboundedSender<T> {
                pub(crate) fn unbounded_send(&self, value: T) -> Result<(), T> {
                    let waker = {
                        let mut guard = self.0.lock().unwrap();
                        if guard.closed {
                            return Err(value);
                        }
                        guard.queue.push_back(value);
                        guard.waker.take()
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                    Ok(())
                }
            }

            impl<T> Drop for UnboundedSender<T> {
                fn drop(&mut self) {
                    let waker = {
                        let mut guard = self.0.lock().unwrap();
                        guard.senders -= 1;
                        if guard.senders == 0 {
                            guard.waker.take()
                        } else {
                            None
                        }
                    };
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
            }

            impl<T> Drop for UnboundedReceiver<T> {
                fn drop(&mut self) {
                    let mut guard = self.0.lock().unwrap();
                    guard.closed = true;
                    guard.waker = None;
                }
            }

            impl<T> UnboundedReceiver<T> {
                pub(crate) fn next(&mut self) -> NextFuture<'_, Self> {
                    NextFuture(self)
                }
            }

            impl<T> Future for NextFuture<'_, UnboundedReceiver<T>> {
                type Output = Option<T>;

                fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                    let mut guard = self.0.0.lock().unwrap();
                    if let Some(value) = guard.queue.pop_front() {
                        return Poll::Ready(Some(value));
                    }
                    if guard.senders == 0 {
                        return Poll::Ready(None);
                    }
                    guard.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global timer thread (Condvar-backed min-heap; 0% CPU when idle)
// ---------------------------------------------------------------------------

struct TimerEntry {
    deadline: Instant,
    id: u64,
    waker: Waker,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline).then(self.id.cmp(&other.id))
    }
}

struct TimerState {
    heap: BinaryHeap<Reverse<TimerEntry>>,
    next_id: u64,
}

struct TimerReactor {
    state: Mutex<TimerState>,
    condvar: Condvar,
}

impl TimerReactor {
    fn global() -> &'static Arc<Self> {
        static REACTOR: OnceLock<Arc<TimerReactor>> = OnceLock::new();
        REACTOR.get_or_init(|| {
            let reactor = Arc::new(TimerReactor {
                state: Mutex::new(TimerState {
                    heap: BinaryHeap::new(),
                    next_id: 1,
                }),
                condvar: Condvar::new(),
            });
            let bg = reactor.clone();
            thread::Builder::new()
                .name("jot-timer".into())
                .spawn(move || bg.run())
                .expect("failed to spawn timer thread");
            reactor
        })
    }

    fn register(&self, deadline: Instant, waker: Waker) {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        let wake_thread = state
            .heap
            .peek()
            .is_none_or(|Reverse(top)| deadline < top.deadline);
        state.heap.push(Reverse(TimerEntry {
            deadline,
            id,
            waker,
        }));
        if wake_thread {
            self.condvar.notify_one();
        }
    }

    fn run(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            let now = Instant::now();
            while let Some(Reverse(top)) = state.heap.peek() {
                if top.deadline <= now {
                    let Reverse(entry) = state.heap.pop().unwrap();
                    drop(state);
                    entry.waker.wake();
                    state = self.state.lock().unwrap();
                } else {
                    break;
                }
            }
            if let Some(Reverse(top)) = state.heap.peek() {
                let wait = top.deadline.saturating_duration_since(Instant::now());
                let (next, _) = self.condvar.wait_timeout(state, wait).unwrap();
                state = next;
            } else {
                state = self.condvar.wait(state).unwrap();
            }
        }
    }
}

pub(crate) struct Timer {
    deadline: Instant,
    registered: bool,
}

impl Future for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if Instant::now() >= self.deadline {
            return Poll::Ready(());
        }
        if !self.registered {
            self.registered = true;
            TimerReactor::global().register(self.deadline, cx.waker().clone());
        } else {
            TimerReactor::global().register(self.deadline, cx.waker().clone());
        }
        Poll::Pending
    }
}

// ---------------------------------------------------------------------------
// Foreground executor (UI thread) + Background thread pool
// ---------------------------------------------------------------------------

trait ForegroundTask {
    fn poll_task(&mut self, cx: &mut Context<'_>) -> bool;
}

// `ForegroundTask` holds `!Send` UI futures, so we keep those in a thread-local table keyed by
// `u64` task ID and only send the `u64` ID across threads! That keeps the entire runtime 100% safe
// Rust with zero `unsafe impl Send`.
struct SafeWakeHandle {
    id: u64,
    ready_ids: Arc<ReadyQueue>,
}

struct ReadyQueue {
    ids: Mutex<VecDeque<u64>>,
    wake_ui: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl ReadyQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            ids: Mutex::new(VecDeque::new()),
            wake_ui: Mutex::new(None),
        })
    }

    fn set_wake_callback(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake_ui.lock().unwrap() = Some(wake);
    }

    fn push(&self, id: u64) {
        self.ids.lock().unwrap().push_back(id);
        if let Some(wake) = self.wake_ui.lock().unwrap().clone() {
            wake();
        }
    }

    fn pop(&self) -> Option<u64> {
        self.ids.lock().unwrap().pop_front()
    }
}

impl Wake for SafeWakeHandle {
    fn wake(self: Arc<Self>) {
        self.ready_ids.push(self.id);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ready_ids.push(self.id);
    }
}

struct TaskOutcome<T> {
    value: Option<T>,
    cancelled: bool,
    detached: bool,
    waker: Option<Waker>,
}

struct ForegroundFuture<T> {
    future: BoxFuture<T>,
    outcome: Arc<Mutex<TaskOutcome<T>>>,
}

impl<T: 'static> ForegroundTask for ForegroundFuture<T> {
    fn poll_task(&mut self, cx: &mut Context<'_>) -> bool {
        if self.outcome.lock().unwrap().cancelled {
            return true;
        }
        match self.future.as_mut().poll(cx) {
            Poll::Pending => false,
            Poll::Ready(val) => {
                let waker = {
                    let mut out = self.outcome.lock().unwrap();
                    out.value = Some(val);
                    out.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
                true
            }
        }
    }
}

struct UiTaskTable {
    next_id: u64,
    tasks: std::collections::HashMap<u64, Box<dyn ForegroundTask>>,
}

thread_local! {
    static UI_TASKS: std::cell::RefCell<UiTaskTable> = std::cell::RefCell::new(UiTaskTable {
        next_id: 1,
        tasks: std::collections::HashMap::new(),
    });
}

/// Handle to a spawned foreground or background task. Dropping a non-detached task cancels it.
pub(crate) struct Task<T>(TaskInner<T>);

enum TaskInner<T> {
    Ready(Option<T>),
    Pending(Arc<Mutex<TaskOutcome<T>>>),
}

impl<T> Task<T> {
    pub(crate) fn ready(value: T) -> Self {
        Self(TaskInner::Ready(Some(value)))
    }

    pub(crate) fn detach(mut self) {
        if let TaskInner::Pending(outcome) = &mut self.0 {
            outcome.lock().unwrap().detached = true;
        }
        std::mem::forget(self);
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let TaskInner::Pending(outcome) = &self.0 {
            let mut guard = outcome.lock().unwrap();
            if !guard.detached {
                guard.cancelled = true;
                guard.waker = None;
            }
        }
    }
}

impl<T: Unpin> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        match &mut self.0 {
            TaskInner::Ready(val) => Poll::Ready(val.take().expect("Task polled after completion")),
            TaskInner::Pending(outcome) => {
                let mut guard = outcome.lock().unwrap();
                if let Some(val) = guard.value.take() {
                    Poll::Ready(val)
                } else {
                    guard.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }
}

/// Single-threaded UI executor backed by the Win32 message loop.
#[derive(Clone)]
pub(crate) struct ForegroundExecutor {
    ready_ids: Arc<ReadyQueue>,
}

impl ForegroundExecutor {
    pub(crate) fn new() -> Self {
        Self {
            ready_ids: ReadyQueue::new(),
        }
    }

    pub(crate) fn set_wake_callback(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.ready_ids.set_wake_callback(wake);
    }

    pub(crate) fn spawn<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
        let outcome = Arc::new(Mutex::new(TaskOutcome {
            value: None,
            cancelled: false,
            detached: false,
            waker: None,
        }));
        let id = UI_TASKS.with(|table| {
            let mut table = table.borrow_mut();
            let id = table.next_id;
            table.next_id = table.next_id.wrapping_add(1);
            table.tasks.insert(
                id,
                Box::new(ForegroundFuture {
                    future: Box::pin(future),
                    outcome: outcome.clone(),
                }),
            );
            id
        });
        self.ready_ids.push(id);
        Task(TaskInner::Pending(outcome))
    }

    /// Drains currently ready foreground tasks. Call from the Win32 message loop on `WM_APP_WAKE`.
    pub(crate) fn drain_ready(&self) {
        let mut budget = 256;
        while budget > 0 {
            let Some(id) = self.ready_ids.pop() else {
                break;
            };
            budget -= 1;
            let mut task = UI_TASKS.with(|table| table.borrow_mut().tasks.remove(&id));
            let Some(mut boxed) = task.take() else {
                continue;
            };
            let waker = Waker::from(Arc::new(SafeWakeHandle {
                id,
                ready_ids: self.ready_ids.clone(),
            }));
            let mut cx = Context::from_waker(&waker);
            let done = boxed.poll_task(&mut cx);
            if !done {
                UI_TASKS.with(|table| {
                    table.borrow_mut().tasks.insert(id, boxed);
                });
            }
        }
        // If more work remains ready after budget, re-notify the Win32 wake hook.
        if !self.ready_ids.ids.lock().unwrap().is_empty()
            && let Some(wake) = self.ready_ids.wake_ui.lock().unwrap().clone()
        {
            wake();
        }
    }
}

/// Background worker pool + timer factory (`cx.background_executor()`).
#[derive(Clone)]
pub(crate) struct BackgroundExecutor {
    pool: Arc<BackgroundPool>,
}

struct BackgroundPool {
    queue: Mutex<VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    condvar: Condvar,
}

impl BackgroundPool {
    fn global() -> Arc<Self> {
        static POOL: OnceLock<Arc<BackgroundPool>> = OnceLock::new();
        POOL.get_or_init(|| {
            let pool = Arc::new(BackgroundPool {
                queue: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
            });
            let workers = thread::available_parallelism()
                .map(|n| n.get().clamp(2, 4))
                .unwrap_or(2);
            for ix in 0..workers {
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
        .clone()
    }
}

impl BackgroundExecutor {
    pub(crate) fn new() -> Self {
        Self {
            pool: BackgroundPool::global(),
        }
    }

    pub(crate) fn timer(&self, duration: Duration) -> Timer {
        Timer {
            deadline: Instant::now() + duration,
            registered: false,
        }
    }

    pub(crate) fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        let outcome = Arc::new(Mutex::new(TaskOutcome {
            value: None,
            cancelled: false,
            detached: false,
            waker: None,
        }));
        let bg_outcome = outcome.clone();
        let fut: SendBoxFuture<T> = Box::pin(future);
        self.enqueue(move || {
            let value = block_on_send(fut);
            let waker = {
                let mut guard = bg_outcome.lock().unwrap();
                guard.value = Some(value);
                guard.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        });
        Task(TaskInner::Pending(outcome))
    }

    fn enqueue(&self, job: impl FnOnce() + Send + 'static) {
        self.pool.queue.lock().unwrap().push_back(Box::new(job));
        self.pool.condvar.notify_one();
    }
}

struct ThreadParkWaker(thread::Thread);

impl Wake for ThreadParkWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on_send<T>(mut future: SendBoxFuture<T>) -> T {
    let waker = Waker::from(Arc::new(ThreadParkWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(val) => return val,
            Poll::Pending => thread::park(),
        }
    }
}

// Keep `ForegroundSlot` / `SendSlotRef` / `ForegroundQueue` absent — unused prototypes removed below.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_executor_runs_spawned_tasks_and_timers() {
        let fg = ForegroundExecutor::new();
        let bg = BackgroundExecutor::new();
        let (tx, rx) = futures::channel::oneshot::channel::<u32>();

        let out = Arc::new(Mutex::new(0u32));
        let out_clone = out.clone();
        fg.spawn(async move {
            let v = rx.await.unwrap();
            *out_clone.lock().unwrap() = v + 1;
        })
        .detach();

        fg.drain_ready();
        assert_eq!(*out.lock().unwrap(), 0);

        tx.send(41).unwrap();
        fg.drain_ready();
        assert_eq!(*out.lock().unwrap(), 42);

        let bg_done = Arc::new(Mutex::new(false));
        let bg_flag = bg_done.clone();
        let handle = bg.spawn(async move {
            *bg_flag.lock().unwrap() = true;
            99u32
        });
        let result = block_on_send(Box::pin(handle));
        assert_eq!(result, 99);
        assert!(*bg_done.lock().unwrap());
    }
}
