use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    marker::PhantomData,
    mem::ManuallyDrop,
    panic::Location,
    pin::Pin,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

use crate::win32::WindowsDispatcher;
use crate::ui::sync::Mutex;

#[derive(Clone)]
pub(crate) struct MainTaskQueue {
    inner: Arc<MainTaskQueueInner>,
}

struct MainTaskQueueInner {
    queue: Mutex<VecDeque<Runnable>>,
    closed: AtomicBool,
}

impl MainTaskQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(MainTaskQueueInner {
                queue: Mutex::new(VecDeque::new()),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn push(&self, runnable: Runnable) -> Result<(), Runnable> {
        let mut queue = self.inner.queue.lock();
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(runnable);
        }
        queue.push_back(runnable);
        Ok(())
    }

    pub(crate) fn try_pop(&self) -> Option<Runnable> {
        self.inner.queue.lock().pop_front()
    }

    pub(crate) fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        let drained: Vec<Runnable> = self.inner.queue.lock().drain(..).collect();
        drop(drained);
    }
}

struct TaskCore {
    poll_fn: Option<Box<dyn FnMut(&mut Context<'_>) -> bool + Send>>,
    scheduled: bool,
    running: bool,
    closed: bool,
    detached: bool,
    finished: bool,
    awaiter: Option<Waker>,
}

struct TaskHeader {
    core: Mutex<TaskCore>,
    schedule: Box<dyn Fn(Runnable) + Send + Sync>,
}

impl TaskHeader {
    fn schedule_runnable(self: &Arc<Self>) {
        let should_enqueue = {
            let mut core = self.core.lock();
            if core.closed || core.finished || core.scheduled {
                false
            } else {
                core.scheduled = true;
                !core.running
            }
        };
        if should_enqueue {
            (self.schedule)(Runnable {
                header: self.clone(),
            });
        }
    }
}

impl Wake for TaskHeader {
    fn wake(self: Arc<Self>) {
        self.schedule_runnable();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.schedule_runnable();
    }
}

pub(crate) struct Runnable {
    header: Arc<TaskHeader>,
}

impl fmt::Debug for Runnable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runnable").finish_non_exhaustive()
    }
}

impl Runnable {
    pub(crate) fn into_raw(self) -> NonNull<()> {
        let ptr = Arc::into_raw(self.header) as *mut ();
        unsafe { NonNull::new_unchecked(ptr) }
    }

    /// # Safety
    /// `ptr` must have been produced by [`Runnable::into_raw`].
    pub(crate) unsafe fn from_raw(ptr: NonNull<()>) -> Self {
        Self {
            header: unsafe { Arc::from_raw(ptr.as_ptr() as *const TaskHeader) },
        }
    }

    pub(crate) fn schedule(self) {
        let header = self.header.clone();
        (header.schedule)(self);
    }

    pub(crate) fn run(self) -> bool {
        let mut poll_fn = {
            let mut core = self.header.core.lock();
            core.scheduled = false;
            if core.closed || core.finished {
                let old = core.poll_fn.take();
                drop(core);
                drop(old);
                return false;
            }
            let Some(poll_fn) = core.poll_fn.take() else {
                return false;
            };
            core.running = true;
            poll_fn
        };

        let waker = Waker::from(self.header.clone());
        let mut cx = Context::from_waker(&waker);
        let ready = poll_fn(&mut cx);

        let (reschedule, awaiter, to_drop) = {
            let mut core = self.header.core.lock();
            core.running = false;
            if ready {
                core.finished = true;
                (false, core.awaiter.take(), Some(poll_fn))
            } else if core.closed {
                (false, core.awaiter.take(), Some(poll_fn))
            } else {
                core.poll_fn = Some(poll_fn);
                (core.scheduled, None, None)
            }
        };

        drop(to_drop);
        if let Some(awaiter) = awaiter {
            awaiter.wake();
        }
        if reschedule {
            let header = self.header.clone();
            (header.schedule)(Runnable {
                header: self.header,
            });
            true
        } else {
            false
        }
    }
}

struct SpawnedTask<T> {
    header: Arc<TaskHeader>,
    output: Arc<Mutex<Option<T>>>,
    detached: bool,
}

impl<T> Drop for SpawnedTask<T> {
    fn drop(&mut self) {
        if !self.detached {
            let to_drop = {
                let mut core = self.header.core.lock();
                if !core.detached {
                    core.closed = true;
                    if !core.running {
                        core.poll_fn.take()
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            drop(to_drop);
        }
    }
}

/// A handle to a spawned asynchronous task. Dropping a `Task` cancels it unless [`Task::detach`] is called.
#[must_use]
pub(crate) struct Task<T>(TaskState<T>);

enum TaskState<T> {
    Ready(Option<T>),
    Spawned(SpawnedTask<T>),
}

impl<T> Unpin for Task<T> {}

impl<T> Task<T> {
    /// Creates a task that immediately resolves to `val`.
    pub(crate) fn ready(val: T) -> Self {
        Self(TaskState::Ready(Some(val)))
    }

    /// Detaches the task so it keeps running after this handle is dropped.
    pub(crate) fn detach(self) {
        if let Task(TaskState::Spawned(mut task)) = self {
            task.detached = true;
            task.header.core.lock().detached = true;
        }
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        match &mut this.0 {
            TaskState::Ready(val) => Poll::Ready(val.take().expect("Task polled after completion")),
            TaskState::Spawned(task) => {
                if let Some(val) = task.output.lock().take() {
                    return Poll::Ready(val);
                }
                let mut core = task.header.core.lock();
                if let Some(val) = task.output.lock().take() {
                    return Poll::Ready(val);
                }
                if core.closed {
                    panic!("polled a cancelled task");
                }
                core.awaiter = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl<T> fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            TaskState::Ready(_) => f.debug_tuple("Task::Ready").finish(),
            TaskState::Spawned(_) => f.debug_tuple("Task::Spawned").finish(),
        }
    }
}

/// # Safety
/// Caller must ensure non-`Send` futures and outputs are only polled and dropped on their originating thread.
unsafe fn spawn_unchecked<Fut>(
    future: Fut,
    schedule: impl Fn(Runnable) + Send + Sync + 'static,
) -> (Runnable, Task<Fut::Output>)
where
    Fut: Future + 'static,
    Fut::Output: 'static,
{
    struct AssertSend<T>(T);
    unsafe impl<T> Send for AssertSend<T> {}

    let output: Arc<Mutex<Option<Fut::Output>>> = Arc::new(Mutex::new(None));
    let output_slot = AssertSend(output.clone());
    let mut pinned = AssertSend(Box::pin(future));

    let poll_fn = Box::new(move |cx: &mut Context<'_>| {
        let output_slot = &output_slot;
        let pinned = &mut pinned;
        match pinned.0.as_mut().poll(cx) {
            Poll::Ready(val) => {
                *output_slot.0.lock() = Some(val);
                true
            }
            Poll::Pending => false,
        }
    });

    let header = Arc::new(TaskHeader {
        core: Mutex::new(TaskCore {
            poll_fn: Some(poll_fn),
            scheduled: true,
            running: false,
            closed: false,
            detached: false,
            finished: false,
            awaiter: None,
        }),
        schedule: Box::new(schedule),
    });

    let runnable = Runnable {
        header: header.clone(),
    };
    let task = Task(TaskState::Spawned(SpawnedTask {
        header,
        output,
        detached: false,
    }));
    (runnable, task)
}

#[inline]
fn current_thread_id() -> ThreadId {
    std::thread_local! {
        static ID: ThreadId = thread::current().id();
    }
    ID.try_with(|id| *id)
        .unwrap_or_else(|_| thread::current().id())
}

struct ThreadCheckedFuture<F> {
    id: ThreadId,
    inner: ManuallyDrop<F>,
    location: &'static Location<'static>,
}

impl<F> Drop for ThreadCheckedFuture<F> {
    fn drop(&mut self) {
        assert_eq!(
            self.id,
            current_thread_id(),
            "local task dropped by a thread that didn't spawn it. Task spawned at {}",
            self.location
        );
        unsafe { ManuallyDrop::drop(&mut self.inner) };
    }
}

impl<F: Future> Future for ThreadCheckedFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        assert!(
            this.id == current_thread_id(),
            "local task polled by a thread that didn't spawn it. Task spawned at {}",
            this.location
        );
        unsafe { Pin::new_unchecked(&mut *this.inner).poll(cx) }
    }
}

/// Executor for spawning tasks onto the Windows thread pool.
#[derive(Clone)]
pub(crate) struct BackgroundExecutor {
    dispatcher: Arc<WindowsDispatcher>,
}

impl BackgroundExecutor {
    pub(crate) fn new(dispatcher: Arc<WindowsDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Enqueues the given future to be run to completion on a background thread.
    #[track_caller]
    pub(crate) fn spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        let dispatcher = Arc::downgrade(&self.dispatcher);
        let (runnable, task) = unsafe {
            spawn_unchecked(future, move |runnable| {
                if let Some(dispatcher) = dispatcher.upgrade() {
                    dispatcher.dispatch(runnable);
                }
            })
        };
        runnable.schedule();
        task
    }

    /// Returns the current instant.
    pub(crate) fn now(&self) -> Instant {
        Instant::now()
    }

    /// Returns a task that completes after the given duration.
    #[track_caller]
    pub(crate) fn timer(&self, duration: Duration) -> Task<()> {
        if duration.is_zero() {
            return Task::ready(());
        }
        let dispatcher = self.dispatcher.clone();
        let (runnable, task) = unsafe {
            spawn_unchecked(async {}, move |runnable| {
                dispatcher.dispatch_after(duration, runnable);
            })
        };
        runnable.schedule();
        task
    }

    /// Returns whether the current thread is the main UI thread.
    pub(crate) fn is_main_thread(&self) -> bool {
        self.dispatcher.is_main_thread()
    }
}

/// Executor for spawning tasks onto the main UI thread.
#[derive(Clone)]
pub(crate) struct ForegroundExecutor {
    dispatcher: Arc<WindowsDispatcher>,
    not_send: PhantomData<Rc<()>>,
}

impl ForegroundExecutor {
    pub(crate) fn new(dispatcher: Arc<WindowsDispatcher>) -> Self {
        Self {
            dispatcher,
            not_send: PhantomData,
        }
    }

    /// Enqueues the given future to run on the main thread.
    #[track_caller]
    pub(crate) fn spawn<R>(&self, future: impl Future<Output = R> + 'static) -> Task<R>
    where
        R: 'static,
    {
        let location = Location::caller();
        let checked = ThreadCheckedFuture {
            id: current_thread_id(),
            inner: ManuallyDrop::new(future),
            location,
        };
        let dispatcher = Arc::downgrade(&self.dispatcher);
        let (runnable, task) = unsafe {
            spawn_unchecked(checked, move |runnable| {
                if let Some(dispatcher) = dispatcher.upgrade() {
                    dispatcher.dispatch_on_main_thread(runnable);
                }
            })
        };
        runnable.schedule();
        task
    }
}
