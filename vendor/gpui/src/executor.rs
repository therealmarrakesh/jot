use crate::{App, PlatformDispatcher, PlatformScheduler};
use crate::futures::channel::mpsc;
use crate::futures::prelude::*;
use crate::gpui_util::{TryFutureExt, TryFutureExtBacktrace};
use crate::scheduler::{self, Instant, Scheduler};
use std::{future::Future, marker::PhantomData, mem, pin::Pin, rc::Rc, sync::Arc, time::Duration};

pub use crate::scheduler::{
    FallibleTask, LocalExecutor as SchedulerLocalExecutor, Priority, Task,
};

/// A pointer to the executor that is currently running,
/// for spawning background tasks.
#[derive(Clone)]
pub struct BackgroundExecutor {
    inner: scheduler::BackgroundExecutor,
    dispatcher: Arc<dyn PlatformDispatcher>,
}

/// A pointer to the executor that is currently running,
/// for spawning tasks on the main thread.
#[derive(Clone)]
pub struct ForegroundExecutor {
    inner: scheduler::LocalExecutor,
    dispatcher: Arc<dyn PlatformDispatcher>,
    not_send: PhantomData<Rc<()>>,
}

/// Extension trait for `Task<Result<T, E>>` that adds `detach_and_log_err` with an `&App` context.
///
/// This trait is automatically implemented for all `Task<Result<T, E>>` types.
pub trait TaskExt<T, E> {
    /// Run the task to completion in the background and log any errors that occur.
    fn detach_and_log_err(self, cx: &App);
    /// Like [`Self::detach_and_log_err`], but uses `{:?}` formatting on failure so `crate::anyhow::Error`
    /// values emit their full backtrace. Prefer `detach_and_log_err` unless a backtrace is wanted.
    fn detach_and_log_err_with_backtrace(self, cx: &App);
}

impl<T, E> TaskExt<T, E> for Task<Result<T, E>>
where
    T: 'static,
    E: 'static + std::fmt::Display + std::fmt::Debug,
{
    #[track_caller]
    fn detach_and_log_err(self, cx: &App) {
        let location = core::panic::Location::caller();
        cx.foreground_executor()
            .spawn(self.log_tracked_err(*location))
            .detach();
    }

    #[track_caller]
    fn detach_and_log_err_with_backtrace(self, cx: &App) {
        let location = *core::panic::Location::caller();
        cx.foreground_executor()
            .spawn(self.log_tracked_err_with_backtrace(location))
            .detach();
    }
}

impl BackgroundExecutor {
    /// Creates a new BackgroundExecutor from the given PlatformDispatcher.
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        let scheduler: Arc<dyn Scheduler> = Arc::new(PlatformScheduler::new(dispatcher.clone()));

        Self {
            inner: scheduler::BackgroundExecutor::new(scheduler),
            dispatcher,
        }
    }

    /// Returns the underlying scheduler::BackgroundExecutor.
    ///
    /// This is used by Ex to pass the executor to thread/worktree code.
    pub fn scheduler_executor(&self) -> scheduler::BackgroundExecutor {
        self.inner.clone()
    }

    /// Enqueues the given future to be run to completion on a background thread.
    #[track_caller]
    pub fn spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.spawn_with_priority(Priority::default(), future.boxed())
    }

    /// Enqueues the given future to be run to completion on a background thread with the given priority.
    #[track_caller]
    pub fn spawn_with_priority<R>(
        &self,
        priority: Priority,
        future: impl Future<Output = R> + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static,
    {
        self.inner.spawn_with_priority(priority, future)
    }

    /// Runs background tasks that may borrow from their environment and waits for all of them to complete.
    ///
    /// Dropping the returned future cancels its tasks and synchronously waits for their futures to
    /// be destroyed before returning.
    pub async fn scoped<'scope, F>(&self, scheduler: F)
    where
        F: FnOnce(&mut Scope<'scope>),
    {
        let mut scope = Scope::new(self.clone(), Priority::default());
        (scheduler)(&mut scope);
        let spawned = mem::take(&mut scope.futures)
            .into_iter()
            .map(|f| self.spawn_with_priority(scope.priority, f))
            .collect::<Vec<_>>();
        for task in spawned {
            task.await;
        }
    }

    pub async fn scoped_priority<'scope, F>(&self, priority: Priority, scheduler: F)
    where
        F: FnOnce(&mut Scope<'scope>),
    {
        let mut scope = Scope::new(self.clone(), priority);
        (scheduler)(&mut scope);
        let spawned = mem::take(&mut scope.futures)
            .into_iter()
            .map(|f| self.spawn_with_priority(scope.priority, f))
            .collect::<Vec<_>>();
        for task in spawned {
            task.await;
        }
    }

    /// Get the current time.
    pub fn now(&self) -> Instant {
        self.inner.scheduler().clock().now()
    }

    /// Returns a task that will complete after the given duration.
    #[track_caller]
    pub fn timer(&self, duration: Duration) -> Task<()> {
        if duration.is_zero() {
            return Task::ready(());
        }
        self.spawn(self.inner.scheduler().timer(duration))
    }

    /// How many CPUs are available to the dispatcher.
    pub fn num_cpus(&self) -> usize {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }

    /// Whether we're on the main thread.
    pub fn is_main_thread(&self) -> bool {
        self.dispatcher.is_main_thread()
    }

    /// Whether this executor runs on the deterministic test scheduler.
    pub fn is_test(&self) -> bool {
        self.inner.is_test()
    }

    #[doc(hidden)]
    pub fn dispatcher(&self) -> &Arc<dyn PlatformDispatcher> {
        &self.dispatcher
    }
}

impl ForegroundExecutor {
    /// Creates a new ForegroundExecutor from the given PlatformDispatcher.
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        let platform_scheduler = Arc::new(PlatformScheduler::new(dispatcher.clone()));
        let inner = platform_scheduler.foreground_executor();
        Self {
            inner,
            dispatcher,
            not_send: PhantomData,
        }
    }

    /// Enqueues the given Task to run on the main thread.
    #[track_caller]
    pub fn spawn<R>(&self, future: impl Future<Output = R> + 'static) -> Task<R>
    where
        R: 'static,
    {
        self.inner.spawn(future.boxed_local())
    }

    /// Enqueues the given Task to run on the main thread with the given priority.
    #[track_caller]
    pub fn spawn_with_priority<R>(
        &self,
        _priority: Priority,
        future: impl Future<Output = R> + 'static,
    ) -> Task<R>
    where
        R: 'static,
    {
        self.inner.spawn(future)
    }

    #[track_caller]
    pub fn spawn_when_idle<R>(
        &self,
        timeout: Option<Duration>,
        future: impl Future<Output = R> + 'static,
    ) -> Task<R>
    where
        R: 'static,
    {
        let dispatcher = self.dispatcher.clone();
        self.inner
            .spawn_with_dispatch(future.boxed_local(), move |runnable| {
                dispatcher.dispatch_on_main_thread_when_idle(runnable, timeout);
            })
    }

    pub fn idle_time_remaining(&self) -> Option<Duration> {
        self.dispatcher.idle_time_remaining()
    }

    /// Block the current thread until the given future resolves.
    pub fn block_on<R>(&self, future: impl Future<Output = R>) -> R {
        self.inner.block_on(future)
    }

    /// Block the current thread until the given future resolves or the timeout elapses.
    pub fn block_with_timeout<R, Fut: Future<Output = R>>(
        &self,
        duration: Duration,
        future: Fut,
    ) -> Result<R, impl Future<Output = R> + use<R, Fut>> {
        self.inner.block_with_timeout(duration, future)
    }

    #[doc(hidden)]
    pub fn dispatcher(&self) -> &Arc<dyn PlatformDispatcher> {
        &self.dispatcher
    }

    #[doc(hidden)]
    pub fn scheduler_executor(&self) -> SchedulerLocalExecutor {
        self.inner.clone()
    }
}

/// Scope manages a set of tasks that are enqueued and waited on together. See [`BackgroundExecutor::scoped`].
pub struct Scope<'a> {
    executor: BackgroundExecutor,
    priority: Priority,
    futures: Vec<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    tx: Option<mpsc::Sender<()>>,
    rx: mpsc::Receiver<()>,
    lifetime: PhantomData<&'a ()>,
}

impl<'a> Scope<'a> {
    fn new(executor: BackgroundExecutor, priority: Priority) -> Self {
        let (tx, rx) = mpsc::channel(1);
        Self {
            executor,
            priority,
            tx: Some(tx),
            rx,
            futures: Default::default(),
            lifetime: PhantomData,
        }
    }

    /// How many CPUs are available to the dispatcher.
    pub fn num_cpus(&self) -> usize {
        self.executor.num_cpus()
    }

    /// Spawn a future into this scope.
    #[track_caller]
    pub fn spawn<F>(&mut self, f: F)
    where
        F: Future<Output = ()> + Send + 'a,
    {
        let tx = self.tx.clone().unwrap();

        // SAFETY: The 'a lifetime is guaranteed to outlive any of these futures because
        // dropping this `Scope` blocks until all of the futures have resolved.
        let f = unsafe {
            mem::transmute::<
                Pin<Box<dyn Future<Output = ()> + Send + 'a>>,
                Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
            >(Box::pin(async move {
                f.await;
                drop(tx);
            }))
        };
        self.futures.push(f);
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        self.tx.take().unwrap();

        // Wait until the channel is closed, which means that all of the spawned
        // futures have resolved.
        let future = async {
            self.rx.next().await;
        };
        let mut future = std::pin::pin!(future);
        self.executor
            .inner
            .scheduler()
            .block(None, future.as_mut(), None);
    }
}
