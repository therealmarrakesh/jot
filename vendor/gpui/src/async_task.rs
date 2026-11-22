//! Minimal safe replacement for the `async-task` crate.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use crate::sync::Mutex;

pub trait Schedule<M = ()> {
    fn schedule(&self, runnable: Runnable<M>);
}

impl<M, F: Fn(Runnable<M>)> Schedule<M> for F {
    fn schedule(&self, runnable: Runnable<M>) {
        (self)(runnable);
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

struct Header<M> {
    metadata: M,
    core: Mutex<TaskCore>,
    schedule: Box<dyn Fn(Runnable<M>) + Send + Sync>,
}

impl<M: Send + Sync + 'static> Header<M> {
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
            let runnable = Runnable {
                header: self.clone(),
            };
            (self.schedule)(runnable);
        }
    }
}

impl<M: Send + Sync + 'static> Wake for Header<M> {
    fn wake(self: Arc<Self>) {
        self.schedule_runnable();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.schedule_runnable();
    }
}

pub struct Runnable<M = ()> {
    header: Arc<Header<M>>,
}

impl<M> fmt::Debug for Runnable<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runnable").finish_non_exhaustive()
    }
}

impl<M: Send + Sync + 'static> Runnable<M> {
    pub fn metadata(&self) -> &M {
        &self.header.metadata
    }

    pub fn into_raw(self) -> NonNull<()> {
        let ptr = Arc::into_raw(self.header) as *mut ();
        unsafe { NonNull::new_unchecked(ptr) }
    }

    /// # Safety
    /// `ptr` must have been produced by [`Runnable::into_raw`] with the same metadata type `M`.
    pub unsafe fn from_raw(ptr: NonNull<()>) -> Self {
        Self {
            header: unsafe { Arc::from_raw(ptr.as_ptr() as *const Header<M>) },
        }
    }

    pub fn schedule(self) {
        let header = self.header.clone();
        (header.schedule)(self);
    }

    pub fn run(self) -> bool {
        let mut poll_fn = {
            let mut core = self.header.core.lock();
            core.scheduled = false;
            if core.closed || core.finished {
                core.poll_fn = None;
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

        let (reschedule, awaiter) = {
            let mut core = self.header.core.lock();
            core.running = false;
            if ready {
                core.finished = true;
                drop(poll_fn);
                (false, core.awaiter.take())
            } else if core.closed {
                drop(poll_fn);
                (false, core.awaiter.take())
            } else {
                core.poll_fn = Some(poll_fn);
                let reschedule = core.scheduled;
                (reschedule, None)
            }
        };

        if let Some(awaiter) = awaiter {
            awaiter.wake();
        }
        if reschedule {
            let header = self.header.clone();
            let runnable = Runnable {
                header: self.header,
            };
            (header.schedule)(runnable);
            true
        } else {
            false
        }
    }
}

pub struct Task<T, M = ()> {
    header: Arc<Header<M>>,
    output: Arc<Mutex<Option<T>>>,
    detached: bool,
}

impl<T, M> Unpin for Task<T, M> {}

impl<T, M> fmt::Debug for Task<T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task").finish_non_exhaustive()
    }
}

impl<T, M> Task<T, M> {
    pub fn detach(mut self) {
        self.detached = true;
        self.header.core.lock().detached = true;
    }

    pub fn is_finished(&self) -> bool {
        self.header.core.lock().finished
    }

    pub fn fallible(mut self) -> FallibleTask<T, M> {
        self.detached = true;
        FallibleTask {
            header: self.header.clone(),
            output: self.output.clone(),
            detached: false,
        }
    }

    pub async fn cancel(mut self) -> Option<T> {
        self.detached = true;
        let mut core = self.header.core.lock();
        core.closed = true;
        if !core.running {
            core.poll_fn = None;
        }
        drop(core);
        self.output.lock().take()
    }
}

impl<T, M> Future for Task<T, M> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if let Some(val) = self.output.lock().take() {
            return Poll::Ready(val);
        }
        let mut core = self.header.core.lock();
        if let Some(val) = self.output.lock().take() {
            return Poll::Ready(val);
        }
        if core.closed {
            panic!("polled a cancelled task");
        }
        core.awaiter = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl<T, M> Drop for Task<T, M> {
    fn drop(&mut self) {
        if !self.detached {
            let mut core = self.header.core.lock();
            if !core.detached {
                core.closed = true;
                if !core.running {
                    core.poll_fn = None;
                }
            }
        }
    }
}

pub struct FallibleTask<T, M = ()> {
    header: Arc<Header<M>>,
    output: Arc<Mutex<Option<T>>>,
    detached: bool,
}

impl<T, M> Unpin for FallibleTask<T, M> {}

impl<T, M> fmt::Debug for FallibleTask<T, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FallibleTask").finish_non_exhaustive()
    }
}

impl<T, M> FallibleTask<T, M> {
    pub fn detach(mut self) {
        self.detached = true;
        self.header.core.lock().detached = true;
    }
}

impl<T, M> Future for FallibleTask<T, M> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        if let Some(val) = self.output.lock().take() {
            return Poll::Ready(Some(val));
        }
        let mut core = self.header.core.lock();
        if let Some(val) = self.output.lock().take() {
            return Poll::Ready(Some(val));
        }
        if core.finished || core.closed {
            return Poll::Ready(None);
        }
        core.awaiter = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl<T, M> Drop for FallibleTask<T, M> {
    fn drop(&mut self) {
        if !self.detached {
            let mut core = self.header.core.lock();
            if !core.detached {
                core.closed = true;
                if !core.running {
                    core.poll_fn = None;
                }
            }
        }
    }
}

pub struct Builder<M = ()> {
    metadata: M,
}

impl Builder<()> {
    pub fn new() -> Self {
        Self { metadata: () }
    }
}

impl Default for Builder<()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> Builder<M> {
    pub fn metadata<M2>(self, metadata: M2) -> Builder<M2> {
        Builder { metadata }
    }

    pub fn spawn<F, Fut, S>(self, future: F, schedule: S) -> (Runnable<M>, Task<Fut::Output, M>)
    where
        F: FnOnce(&M) -> Fut,
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
        S: Schedule<M> + Send + Sync + 'static,
        M: Send + Sync + 'static,
    {
        unsafe { self.spawn_unchecked(future, schedule) }
    }

    /// # Safety
    /// Caller must ensure non-`Send` futures and outputs are only polled and dropped on their originating thread.
    pub unsafe fn spawn_unchecked<F, Fut, S>(
        self,
        future: F,
        schedule: S,
    ) -> (Runnable<M>, Task<Fut::Output, M>)
    where
        F: FnOnce(&M) -> Fut,
        Fut: Future + 'static,
        Fut::Output: 'static,
        S: Schedule<M> + Send + Sync + 'static,
        M: Send + Sync + 'static,
    {
        struct AssertSend<T>(T);
        unsafe impl<T> Send for AssertSend<T> {}

        let fut = future(&self.metadata);
        let output: Arc<Mutex<Option<Fut::Output>>> = Arc::new(Mutex::new(None));
        let output_slot = AssertSend(output.clone());
        let mut pinned = AssertSend(Box::pin(fut));

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

        let header = Arc::new(Header {
            metadata: self.metadata,
            core: Mutex::new(TaskCore {
                poll_fn: Some(poll_fn),
                scheduled: true,
                running: false,
                closed: false,
                detached: false,
                finished: false,
                awaiter: None,
            }),
            schedule: Box::new(move |runnable| schedule.schedule(runnable)),
        });

        let runnable = Runnable {
            header: header.clone(),
        };
        let task = Task {
            header,
            output,
            detached: false,
        };
        (runnable, task)
    }
}
