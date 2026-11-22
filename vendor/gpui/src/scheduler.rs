use crate::futures::channel::oneshot;
use async_task::Runnable;
use std::{
    any::Any,
    future::Future,
    marker::PhantomData,
    mem::ManuallyDrop,
    panic::Location,
    pin::Pin,
    rc::Rc,
    sync::{Arc, mpsc},
    task::{Context, Poll, Waker},
    thread::{self, ThreadId},
    time::Duration,
};

pub use std::time::Instant;

pub fn test_scheduler_created() -> bool {
    false
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Task priority for background tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Priority {
    RealtimeAudio,
    High,
    #[default]
    Medium,
    Low,
}

impl Priority {
    pub const fn weight(self) -> u32 {
        match self {
            Priority::High => 60,
            Priority::Medium => 30,
            Priority::Low => 10,
            Priority::RealtimeAudio => 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpawnTime(pub Instant);

/// Metadata attached to runnables for debugging.
#[derive(Clone, Debug)]
pub struct RunnableMeta {
    pub location: &'static Location<'static>,
    pub spawned: SpawnTime,
}

impl RunnableMeta {
    #[track_caller]
    pub fn new_with_callers_location() -> Self {
        Self {
            location: core::panic::Location::caller(),
            spawned: SpawnTime(Instant::now()),
        }
    }
}

pub trait Scheduler: Send + Sync {
    fn block(
        &self,
        session_id: Option<SessionId>,
        future: Pin<&mut dyn Future<Output = ()>>,
        timeout: Option<Duration>,
    ) -> bool;

    fn schedule_local(&self, session_id: SessionId, runnable: Runnable<RunnableMeta>);

    fn schedule_background_with_priority(
        &self,
        runnable: Runnable<RunnableMeta>,
        priority: Priority,
    );

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>);

    fn schedule_background(&self, runnable: Runnable<RunnableMeta>) {
        self.schedule_background_with_priority(runnable, Priority::default());
    }

    #[track_caller]
    fn timer(&self, timeout: Duration) -> Timer;
    fn clock(&self) -> Arc<dyn Clock>;

    fn spawn_dedicated(
        self: Arc<Self>,
        f: Box<
            dyn FnOnce(
                    LocalExecutor,
                )
                    -> Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + 'static>>
                + Send
                + 'static,
        >,
    ) -> Task<Box<dyn Any + Send + Sync>>;
}

pub fn spawn_dedicated_thread<F, Fut>(
    session_id: SessionId,
    scheduler: Arc<dyn Scheduler>,
    f: F,
) -> Task<Fut::Output>
where
    F: FnOnce(LocalExecutor) -> Fut + Send + 'static,
    Fut: Future + 'static,
    Fut::Output: Send + 'static,
{
    let (task, delivery) = Task::rendezvous();
    let thread_body = move || {
        let (runnable_sender, runnable_receiver) = mpsc::channel::<Runnable<RunnableMeta>>();
        let dispatch = move |runnable: Runnable<RunnableMeta>| {
            let _ = runnable_sender.send(runnable);
        };
        let executor = LocalExecutor::new(session_id, scheduler, dispatch);
        let root_task = executor.spawn(f(executor.clone()));
        delivery.deliver(root_task);
        drop(executor);

        while let Ok(runnable) = runnable_receiver.recv() {
            runnable.run();
        }
    };
    let thread_name = format!("spawn_dedicated session {:?}", session_id);
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(thread_body)
        .expect("failed to spawn dedicated thread");
    task
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SessionId(u16);

impl SessionId {
    pub fn new(id: u16) -> Self {
        SessionId(id)
    }
}

pub struct Timer(oneshot::Receiver<()>);

impl Timer {
    pub fn new(rx: oneshot::Receiver<()>) -> Self {
        Timer(rx)
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        match Pin::new(&mut self.0).poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Clone)]
pub struct LocalExecutor {
    session_id: SessionId,
    scheduler: Arc<dyn Scheduler>,
    dispatch: Arc<dyn Fn(Runnable<RunnableMeta>) + Send + Sync>,
    not_send: PhantomData<Rc<()>>,
}

impl LocalExecutor {
    pub fn new(
        session_id: SessionId,
        scheduler: Arc<dyn Scheduler>,
        dispatch: impl Fn(Runnable<RunnableMeta>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            session_id,
            scheduler,
            dispatch: Arc::new(dispatch),
            not_send: PhantomData,
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn scheduler(&self) -> &Arc<dyn Scheduler> {
        &self.scheduler
    }

    pub fn is_test(&self) -> bool {
        false
    }

    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> Task<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let schedule = self.schedule();
        let location = Location::caller();
        let (runnable, task) = spawn_local_with_source_location(
            future,
            schedule,
            RunnableMeta {
                location,
                spawned: SpawnTime(Instant::now()),
            },
        );
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    #[track_caller]
    pub fn spawn_with_dispatch<F>(
        &self,
        future: F,
        dispatch: impl Fn(Runnable<RunnableMeta>) + Send + Sync + 'static,
    ) -> Task<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let location = Location::caller();
        let (runnable, task) = spawn_local_with_source_location(
            future,
            dispatch,
            RunnableMeta {
                location,
                spawned: SpawnTime(Instant::now()),
            },
        );
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    pub fn block_on<Fut: Future>(&self, future: Fut) -> Fut::Output {
        use std::cell::Cell;

        let output = Cell::new(None);
        let future = async {
            output.set(Some(future.await));
        };
        let mut future = std::pin::pin!(future);

        self.scheduler
            .block(Some(self.session_id), future.as_mut(), None);

        output.take().expect("block_on future did not complete")
    }

    pub fn block_with_timeout<Fut: Future>(
        &self,
        timeout: Duration,
        future: Fut,
    ) -> Result<Fut::Output, impl Future<Output = Fut::Output> + use<Fut>> {
        use std::cell::Cell;

        let output = Cell::new(None);
        let mut future = Box::pin(future);

        {
            let future_ref = &mut future;
            let wrapper = async {
                output.set(Some(future_ref.await));
            };
            let mut wrapper = std::pin::pin!(wrapper);

            self.scheduler
                .block(Some(self.session_id), wrapper.as_mut(), Some(timeout));
        }

        match output.take() {
            Some(value) => Ok(value),
            None => Err(future),
        }
    }

    #[track_caller]
    pub fn timer(&self, duration: Duration) -> Timer {
        self.scheduler.timer(duration)
    }

    pub fn now(&self) -> Instant {
        self.scheduler.clock().now()
    }

    #[track_caller]
    pub fn spawn_dedicated<F, Fut>(&self, f: F) -> Task<Fut::Output>
    where
        F: FnOnce(LocalExecutor) -> Fut + Send + 'static,
        Fut: Future + 'static,
        Fut::Output: Send + Sync + 'static,
    {
        self.scheduler
            .clone()
            .spawn_dedicated(box_dedicated(f))
            .downcast::<Fut::Output>()
    }

    fn schedule(&self) -> impl Fn(Runnable<RunnableMeta>) + Send + Sync + 'static {
        let dispatch = self.dispatch.clone();
        move |runnable| dispatch(runnable)
    }
}

fn box_dedicated<F, Fut>(
    f: F,
) -> Box<
    dyn FnOnce(LocalExecutor) -> Pin<Box<dyn Future<Output = Box<dyn Any + Send + Sync>> + 'static>>
        + Send
        + 'static,
>
where
    F: FnOnce(LocalExecutor) -> Fut + Send + 'static,
    Fut: Future + 'static,
    Fut::Output: Send + Sync + 'static,
{
    Box::new(move |executor| {
        Box::pin(async move { Box::new(f(executor).await) as Box<dyn Any + Send + Sync> })
    })
}

#[derive(Clone)]
pub struct BackgroundExecutor {
    scheduler: Arc<dyn Scheduler>,
}

impl BackgroundExecutor {
    pub fn new(scheduler: Arc<dyn Scheduler>) -> Self {
        Self { scheduler }
    }

    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_with_priority(Priority::default(), future)
    }

    #[track_caller]
    pub fn spawn_with_priority<F>(&self, priority: Priority, future: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let schedule = self.schedule_with_priority(priority);
        let location = Location::caller();
        let (runnable, task) = async_task::Builder::new()
            .metadata(RunnableMeta {
                location,
                spawned: SpawnTime(Instant::now()),
            })
            .spawn(move |_| future, schedule);
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    #[track_caller]
    pub fn spawn_realtime<F>(&self, future: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let location = Location::caller();
        let (tx, rx) = mpsc::sync_channel::<Runnable<RunnableMeta>>(1);

        self.scheduler.spawn_realtime(Box::new(move || {
            while let Ok(runnable) = rx.recv() {
                runnable.run();
            }
        }));

        let (runnable, task) = async_task::Builder::new()
            .metadata(RunnableMeta {
                location,
                spawned: SpawnTime(Instant::now()),
            })
            .spawn(
                move |_| future,
                move |runnable| {
                    let _ = tx.send(runnable);
                },
            );
        runnable.schedule();
        Task(TaskState::Spawned(task))
    }

    #[track_caller]
    pub fn timer(&self, duration: Duration) -> Timer {
        self.scheduler.timer(duration)
    }

    pub fn now(&self) -> Instant {
        self.scheduler.clock().now()
    }

    pub fn scheduler(&self) -> &Arc<dyn Scheduler> {
        &self.scheduler
    }

    pub fn is_test(&self) -> bool {
        false
    }

    #[track_caller]
    pub fn spawn_dedicated<F, Fut>(&self, f: F) -> Task<Fut::Output>
    where
        F: FnOnce(LocalExecutor) -> Fut + Send + 'static,
        Fut: Future + 'static,
        Fut::Output: Send + Sync + 'static,
    {
        self.scheduler
            .clone()
            .spawn_dedicated(box_dedicated(f))
            .downcast::<Fut::Output>()
    }

    fn schedule_with_priority(
        &self,
        priority: Priority,
    ) -> impl Fn(Runnable<RunnableMeta>) + Send + Sync + 'static {
        let scheduler = Arc::downgrade(&self.scheduler);
        move |runnable| {
            if let Some(scheduler) = scheduler.upgrade() {
                scheduler.schedule_background_with_priority(runnable, priority);
            }
        }
    }
}

#[must_use]
pub struct Task<T>(TaskState<T>);

enum TaskState<T> {
    Ready(Option<T>),
    Spawned(async_task::Task<T, RunnableMeta>),
    Downcast {
        inner: Box<Task<Box<dyn Any + Send + Sync>>>,
        marker: PhantomData<fn() -> T>,
    },
    Rendezvous(RendezvousReceiver<T>),
}

enum RendezvousState<T> {
    Pending(Option<Waker>),
    Delivered(Task<T>),
    Cancelled,
    Detached,
    Taken,
}

pub(crate) struct RendezvousReceiver<T> {
    shared: Arc<crate::sync::Mutex<RendezvousState<T>>>,
}

impl<T> RendezvousReceiver<T> {
    fn poll_take(&self, cx: &mut Context) -> Poll<Task<T>> {
        let mut state = self.shared.lock();
        match std::mem::replace(&mut *state, RendezvousState::Taken) {
            RendezvousState::Delivered(task) => Poll::Ready(task),
            RendezvousState::Pending(_) => {
                *state = RendezvousState::Pending(Some(cx.waker().clone()));
                Poll::Pending
            }
            RendezvousState::Cancelled | RendezvousState::Detached | RendezvousState::Taken => {
                unreachable!("a rendezvous task was polled after its receiver was consumed")
            }
        }
    }

    fn is_ready(&self) -> bool {
        match &*self.shared.lock() {
            RendezvousState::Delivered(task) => task.is_ready(),
            _ => false,
        }
    }

    fn detach(self) {
        let mut state = self.shared.lock();
        match std::mem::replace(&mut *state, RendezvousState::Detached) {
            RendezvousState::Delivered(task) => {
                *state = RendezvousState::Taken;
                drop(state);
                task.detach();
            }
            RendezvousState::Pending(_) => {}
            RendezvousState::Cancelled | RendezvousState::Detached | RendezvousState::Taken => {
                unreachable!("a rendezvous task was detached after its receiver was consumed")
            }
        }
    }
}

impl<T> Drop for RendezvousReceiver<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        match &*state {
            RendezvousState::Pending(_) => *state = RendezvousState::Cancelled,
            RendezvousState::Delivered(_) => {
                let delivered = std::mem::replace(&mut *state, RendezvousState::Cancelled);
                drop(state);
                drop(delivered);
            }
            _ => {}
        }
    }
}

pub(crate) struct RendezvousSender<T> {
    shared: Arc<crate::sync::Mutex<RendezvousState<T>>>,
}

impl<T> RendezvousSender<T> {
    pub(crate) fn deliver(self, task: Task<T>) {
        let mut state = self.shared.lock();
        match std::mem::replace(&mut *state, RendezvousState::Delivered(task)) {
            RendezvousState::Pending(waker) => {
                drop(state);
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
            RendezvousState::Cancelled => {
                let delivered = std::mem::replace(&mut *state, RendezvousState::Cancelled);
                drop(state);
                drop(delivered);
            }
            RendezvousState::Detached => {
                let RendezvousState::Delivered(task) =
                    std::mem::replace(&mut *state, RendezvousState::Detached)
                else {
                    unreachable!("the delivered task was just stored");
                };
                drop(state);
                task.detach();
            }
            RendezvousState::Delivered(_) | RendezvousState::Taken => {
                unreachable!("a rendezvous task was delivered twice")
            }
        }
    }
}

impl<T> Task<T> {
    pub fn ready(val: T) -> Self {
        Task(TaskState::Ready(Some(val)))
    }

    pub(crate) fn rendezvous() -> (Self, RendezvousSender<T>) {
        let shared = Arc::new(crate::sync::Mutex::new(RendezvousState::Pending(None)));
        (
            Task(TaskState::Rendezvous(RendezvousReceiver {
                shared: shared.clone(),
            })),
            RendezvousSender { shared },
        )
    }

    pub fn from_async_task(task: async_task::Task<T, RunnableMeta>) -> Self {
        Task(TaskState::Spawned(task))
    }

    pub fn is_ready(&self) -> bool {
        match &self.0 {
            TaskState::Ready(_) => true,
            TaskState::Spawned(task) => task.is_finished(),
            TaskState::Downcast { inner, .. } => inner.is_ready(),
            TaskState::Rendezvous(receiver) => receiver.is_ready(),
        }
    }

    pub fn detach(self) {
        match self {
            Task(TaskState::Ready(_)) => {}
            Task(TaskState::Spawned(task)) => task.detach(),
            Task(TaskState::Downcast { inner, .. }) => inner.detach(),
            Task(TaskState::Rendezvous(receiver)) => receiver.detach(),
        }
    }

    pub fn fallible(self) -> FallibleTask<T> {
        FallibleTask(match self.0 {
            TaskState::Ready(val) => FallibleTaskState::Ready(val),
            TaskState::Spawned(task) => FallibleTaskState::Spawned(task.fallible()),
            TaskState::Downcast { inner, .. } => FallibleTaskState::Downcast {
                inner: Box::new(inner.fallible()),
                marker: PhantomData,
            },
            TaskState::Rendezvous(receiver) => FallibleTaskState::Rendezvous(receiver),
        })
    }
}

impl Task<Box<dyn Any + Send + Sync>> {
    pub fn downcast<T: Send + Sync + 'static>(self) -> Task<T> {
        Task(TaskState::Downcast {
            inner: Box::new(self),
            marker: PhantomData,
        })
    }
}

impl<T> std::fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            TaskState::Ready(_) => f.debug_tuple("Task::Ready").finish(),
            TaskState::Spawned(task) => f.debug_tuple("Task::Spawned").field(task).finish(),
            TaskState::Downcast { inner, .. } => {
                f.debug_tuple("Task::Downcast").field(inner).finish()
            }
            TaskState::Rendezvous(_) => f.debug_tuple("Task::Rendezvous").finish(),
        }
    }
}

#[must_use]
pub struct FallibleTask<T>(FallibleTaskState<T>);

enum FallibleTaskState<T> {
    Ready(Option<T>),
    Spawned(async_task::FallibleTask<T, RunnableMeta>),
    Downcast {
        inner: Box<FallibleTask<Box<dyn Any + Send + Sync>>>,
        marker: PhantomData<fn() -> T>,
    },
    Rendezvous(RendezvousReceiver<T>),
}

impl<T> FallibleTask<T> {
    pub fn ready(val: T) -> Self {
        FallibleTask(FallibleTaskState::Ready(Some(val)))
    }

    pub fn detach(self) {
        match self.0 {
            FallibleTaskState::Ready(_) => {}
            FallibleTaskState::Spawned(task) => task.detach(),
            FallibleTaskState::Downcast { inner, .. } => inner.detach(),
            FallibleTaskState::Rendezvous(receiver) => receiver.detach(),
        }
    }
}

impl<T: 'static> Future for FallibleTask<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        loop {
            match &mut this.0 {
                FallibleTaskState::Ready(val) => return Poll::Ready(val.take()),
                FallibleTaskState::Spawned(task) => return Pin::new(task).poll(cx),
                FallibleTaskState::Downcast { inner, .. } => {
                    return match Pin::new(inner.as_mut()).poll(cx) {
                        Poll::Ready(Some(boxed_any)) => Poll::Ready(Some(
                            *boxed_any
                                .downcast::<T>()
                                .expect("FallibleTask::poll: downcast type mismatch"),
                        )),
                        Poll::Ready(None) => Poll::Ready(None),
                        Poll::Pending => Poll::Pending,
                    };
                }
                FallibleTaskState::Rendezvous(receiver) => match receiver.poll_take(cx) {
                    Poll::Ready(task) => {
                        this.0 = task.fallible().0;
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}

impl<T> std::fmt::Debug for FallibleTask<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            FallibleTaskState::Ready(_) => f.debug_tuple("FallibleTask::Ready").finish(),
            FallibleTaskState::Spawned(task) => {
                f.debug_tuple("FallibleTask::Spawned").field(task).finish()
            }
            FallibleTaskState::Downcast { inner, .. } => f
                .debug_tuple("FallibleTask::Downcast")
                .field(inner)
                .finish(),
            FallibleTaskState::Rendezvous(_) => f.debug_tuple("FallibleTask::Rendezvous").finish(),
        }
    }
}

impl<T: 'static> Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        loop {
            match &mut this.0 {
                TaskState::Ready(val) => return Poll::Ready(val.take().unwrap()),
                TaskState::Spawned(task) => return Pin::new(task).poll(cx),
                TaskState::Downcast { inner, .. } => {
                    return match Pin::new(inner.as_mut()).poll(cx) {
                        Poll::Ready(boxed_any) => Poll::Ready(
                            *boxed_any
                                .downcast::<T>()
                                .expect("Task::poll: downcast type mismatch"),
                        ),
                        Poll::Pending => Poll::Pending,
                    };
                }
                TaskState::Rendezvous(receiver) => match receiver.poll_take(cx) {
                    Poll::Ready(task) => {
                        this.0 = task.0;
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                },
            }
        }
    }
}

#[track_caller]
fn spawn_local_with_source_location<Fut, S>(
    future: Fut,
    schedule: S,
    metadata: RunnableMeta,
) -> (
    async_task::Runnable<RunnableMeta>,
    async_task::Task<Fut::Output, RunnableMeta>,
)
where
    Fut: Future + 'static,
    Fut::Output: 'static,
    S: async_task::Schedule<RunnableMeta> + Send + Sync + 'static,
{
    #[inline]
    fn thread_id() -> ThreadId {
        std::thread_local! {
            static ID: ThreadId = thread::current().id();
        }
        ID.try_with(|id| *id)
            .unwrap_or_else(|_| thread::current().id())
    }

    struct Checked<F> {
        id: ThreadId,
        inner: ManuallyDrop<F>,
        location: &'static Location<'static>,
    }

    impl<F> Drop for Checked<F> {
        fn drop(&mut self) {
            assert_eq!(
                self.id,
                thread_id(),
                "local task dropped by a thread that didn't spawn it. Task spawned at {}",
                self.location
            );
            unsafe { ManuallyDrop::drop(&mut self.inner) };
        }
    }

    impl<F: Future> Future for Checked<F> {
        type Output = F::Output;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = unsafe { self.get_unchecked_mut() };
            assert!(
                this.id == thread_id(),
                "local task polled by a thread that didn't spawn it. Task spawned at {}",
                this.location
            );
            unsafe { Pin::new_unchecked(&mut *this.inner).poll(cx) }
        }
    }

    let location = metadata.location;

    let future = move |_| Checked {
        id: thread_id(),
        inner: ManuallyDrop::new(future),
        location,
    };

    let builder = async_task::Builder::new().metadata(metadata);
    unsafe { builder.spawn_unchecked(future, schedule) }
}
