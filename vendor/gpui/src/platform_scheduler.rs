use crate::{
    PlatformDispatcher, RunnableMeta,
    scheduler::{
        Clock, Instant, LocalExecutor, Priority, Scheduler, SessionId, SpawnTime, Task, Timer,
        spawn_dedicated_thread,
    },
};
use crate::async_task::Runnable;
use crate::futures::channel::oneshot;
use std::task::{Context, Poll};
use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};

/// A production implementation of [`Scheduler`] that wraps a [`PlatformDispatcher`].
pub struct PlatformScheduler {
    dispatcher: Arc<dyn PlatformDispatcher>,
    clock: Arc<PlatformClock>,
    next_session_id: AtomicU16,
}

impl PlatformScheduler {
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        Self {
            dispatcher: dispatcher.clone(),
            clock: Arc::new(PlatformClock { dispatcher }),
            next_session_id: AtomicU16::new(0),
        }
    }

    pub fn foreground_executor(self: &Arc<Self>) -> LocalExecutor {
        let session_id = self.next_session_id();
        let scheduler = Arc::downgrade(self);
        LocalExecutor::new(session_id, self.clone(), move |runnable| {
            if let Some(scheduler) = scheduler.upgrade() {
                scheduler.schedule_local(session_id, runnable);
            }
        })
    }

    fn next_session_id(&self) -> SessionId {
        SessionId::new(self.next_session_id.fetch_add(1, Ordering::SeqCst))
    }
}

impl Scheduler for PlatformScheduler {
    fn block(
        &self,
        _session_id: Option<SessionId>,
        mut future: Pin<&mut dyn Future<Output = ()>>,
        timeout: Option<Duration>,
    ) -> bool {
        struct ThreadWaker(std::thread::Thread);
        impl std::task::Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }

        let deadline = timeout.map(|t| Instant::now() + t);
        let waker = std::task::Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        if let Poll::Ready(()) = future.as_mut().poll(&mut cx) {
            return true;
        }

        loop {
            match deadline {
                Some(deadline) => {
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        return false;
                    };
                    std::thread::park_timeout(remaining);
                }
                None => std::thread::park(),
            }
            if let Poll::Ready(()) = future.as_mut().poll(&mut cx) {
                break true;
            }
        }
    }

    fn schedule_local(&self, _session_id: SessionId, runnable: Runnable<RunnableMeta>) {
        self.dispatcher
            .dispatch_on_main_thread(runnable, Priority::default());
    }

    fn schedule_background_with_priority(
        &self,
        runnable: Runnable<RunnableMeta>,
        priority: Priority,
    ) {
        self.dispatcher.dispatch(runnable, priority);
    }

    #[track_caller]
    fn timer(&self, duration: Duration) -> Timer {
        let (tx, rx) = oneshot::channel();
        let dispatcher = self.dispatcher.clone();

        let location = std::panic::Location::caller();
        let (runnable, _task) = crate::async_task::Builder::new()
            .metadata(RunnableMeta {
                location,
                spawned: SpawnTime(Instant::now()),
            })
            .spawn(
                move |_| async move {
                    let _ = tx.send(());
                },
                move |runnable| {
                    dispatcher.dispatch_after(duration, runnable);
                },
            );
        runnable.schedule();

        Timer::new(rx)
    }

    fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

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
    ) -> Task<Box<dyn Any + Send + Sync>> {
        let session_id = self.next_session_id();
        spawn_dedicated_thread(session_id, self, move |executor| f(executor))
    }
}

/// A production clock that uses the platform dispatcher's time.
struct PlatformClock {
    dispatcher: Arc<dyn PlatformDispatcher>,
}

impl Clock for PlatformClock {
    fn now(&self) -> Instant {
        self.dispatcher.now()
    }
}
