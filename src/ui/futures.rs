//! Minimal in-house async channels, mutex, and future combinators.

use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    fmt,
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

pub(crate) use future::FutureExt;

pub(crate) mod channel {
    pub(crate) mod oneshot {
        use super::super::*;

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) struct Canceled;

        impl fmt::Display for Canceled {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("oneshot canceled")
            }
        }

        impl std::error::Error for Canceled {}

        struct State<T> {
            value: Option<T>,
            sender_dropped: bool,
            receiver_dropped: bool,
            rx_waker: Option<Waker>,
            cancel_waker: Option<Waker>,
        }

        pub(crate) struct Sender<T> {
            state: Arc<Mutex<State<T>>>,
            sent: bool,
        }

        pub(crate) struct Receiver<T> {
            state: Arc<Mutex<State<T>>>,
            done: bool,
        }

        pub(crate) struct Cancellation<'a, T> {
            sender: &'a mut Sender<T>,
        }

        pub(crate) fn channel<T>() -> (Sender<T>, Receiver<T>) {
            let state = Arc::new(Mutex::new(State {
                value: None,
                sender_dropped: false,
                receiver_dropped: false,
                rx_waker: None,
                cancel_waker: None,
            }));
            (
                Sender {
                    state: state.clone(),
                    sent: false,
                },
                Receiver { state, done: false },
            )
        }

        impl<T> Sender<T> {
            pub(crate) fn send(mut self, value: T) -> Result<(), T> {
                self.sent = true;
                let waker = {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.receiver_dropped {
                        return Err(value);
                    }
                    state.value = Some(value);
                    state.rx_waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
                Ok(())
            }

            pub(crate) fn is_canceled(&self) -> bool {
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .receiver_dropped
            }

            pub(crate) fn cancellation(&mut self) -> Cancellation<'_, T> {
                Cancellation { sender: self }
            }
        }

        impl<T> Drop for Sender<T> {
            fn drop(&mut self) {
                if self.sent {
                    return;
                }
                let waker = {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.sender_dropped = true;
                    state.rx_waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        }

        impl<T> Unpin for Cancellation<'_, T> {}

        impl<T> Future for Cancellation<'_, T> {
            type Output = ();

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                let mut state = self.sender.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.receiver_dropped {
                    Poll::Ready(())
                } else {
                    state.cancel_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }

        impl<T> Receiver<T> {
            pub(crate) fn close(&mut self) {
                let waker = {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.receiver_dropped = true;
                    state.cancel_waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        }

        impl<T> Unpin for Receiver<T> {}

        impl<T> Future for Receiver<T> {
            type Output = Result<T, Canceled>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(val) = state.value.take() {
                    drop(state);
                    self.done = true;
                    Poll::Ready(Ok(val))
                } else if state.sender_dropped {
                    drop(state);
                    self.done = true;
                    Poll::Ready(Err(Canceled))
                } else {
                    state.rx_waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }

        impl<T> Drop for Receiver<T> {
            fn drop(&mut self) {
                if !self.done {
                    self.close();
                }
            }
        }
    }

    pub(crate) mod mpsc {
        use super::super::*;

        struct State<T> {
            queue: VecDeque<T>,
            senders: usize,
            receiver_dropped: bool,
            waker: Option<Waker>,
        }

        pub(crate) struct UnboundedSender<T> {
            state: Arc<Mutex<State<T>>>,
        }

        pub(crate) struct UnboundedReceiver<T> {
            state: Arc<Mutex<State<T>>>,
        }

        pub(crate) fn unbounded<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
            let state = Arc::new(Mutex::new(State {
                queue: VecDeque::new(),
                senders: 1,
                receiver_dropped: false,
                waker: None,
            }));
            (
                UnboundedSender {
                    state: state.clone(),
                },
                UnboundedReceiver { state },
            )
        }

        impl<T> Clone for UnboundedSender<T> {
            fn clone(&self) -> Self {
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .senders += 1;
                Self {
                    state: self.state.clone(),
                }
            }
        }

        impl<T> UnboundedSender<T> {
            pub(crate) fn unbounded_send(&self, msg: T) -> Result<(), T> {
                let waker = {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.receiver_dropped {
                        return Err(msg);
                    }
                    state.queue.push_back(msg);
                    state.waker.take()
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
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.senders -= 1;
                    if state.senders == 0 {
                        state.waker.take()
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
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .receiver_dropped = true;
            }
        }

        pub(crate) struct Next<'a, T> {
            receiver: &'a mut UnboundedReceiver<T>,
        }

        impl<T> Unpin for Next<'_, T> {}

        impl<T> Future for Next<'_, T> {
            type Output = Option<T>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
                let mut state = self
                    .receiver
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(item) = state.queue.pop_front() {
                    Poll::Ready(Some(item))
                } else if state.senders == 0 {
                    Poll::Ready(None)
                } else {
                    state.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }

        impl<T> UnboundedReceiver<T> {
            pub(crate) fn next(&mut self) -> Next<'_, T> {
                Next { receiver: self }
            }
        }
    }
}

pub(crate) mod lock {
    use super::*;

    struct WaitState {
        locked: bool,
        next_id: u64,
        waiters: VecDeque<(u64, Waker)>,
    }

    /// Minimal async FIFO mutex.
    pub(crate) struct Mutex<T> {
        state: std::sync::Mutex<WaitState>,
        value: UnsafeCell<T>,
    }

    unsafe impl<T: Send> Send for Mutex<T> {}
    unsafe impl<T: Send> Sync for Mutex<T> {}

    impl<T> Mutex<T> {
        pub(crate) fn new(value: T) -> Self {
            Self {
                state: std::sync::Mutex::new(WaitState {
                    locked: false,
                    next_id: 1,
                    waiters: VecDeque::new(),
                }),
                value: UnsafeCell::new(value),
            }
        }

        pub(crate) fn lock(&self) -> MutexLockFuture<'_, T> {
            MutexLockFuture {
                mutex: self,
                waiter_id: None,
            }
        }
    }

    pub(crate) struct MutexLockFuture<'a, T> {
        mutex: &'a Mutex<T>,
        waiter_id: Option<u64>,
    }

    impl<T> Unpin for MutexLockFuture<'_, T> {}

    impl<'a, T> Future for MutexLockFuture<'a, T> {
        type Output = MutexGuard<'a, T>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let mut state = self.mutex.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.locked {
                state.locked = true;
                if let Some(id) = self.waiter_id.take() {
                    state.waiters.retain(|(wid, _)| *wid != id);
                }
                return Poll::Ready(MutexGuard { mutex: self.mutex });
            }
            if let Some(id) = self.waiter_id {
                if let Some(entry) = state.waiters.iter_mut().find(|(wid, _)| *wid == id) {
                    entry.1 = cx.waker().clone();
                } else {
                    state.waiters.push_back((id, cx.waker().clone()));
                }
            } else {
                let id = state.next_id;
                state.next_id = id.wrapping_add(1);
                state.waiters.push_back((id, cx.waker().clone()));
                self.waiter_id = Some(id);
            }
            Poll::Pending
        }
    }

    impl<T> Drop for MutexLockFuture<'_, T> {
        fn drop(&mut self) {
            if let Some(id) = self.waiter_id.take() {
                let mut state = self.mutex.state.lock().unwrap_or_else(|e| e.into_inner());
                state.waiters.retain(|(wid, _)| *wid != id);
            }
        }
    }

    pub(crate) struct MutexGuard<'a, T> {
        mutex: &'a Mutex<T>,
    }

    impl<T> Deref for MutexGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            unsafe { &*self.mutex.value.get() }
        }
    }

    impl<T> DerefMut for MutexGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            unsafe { &mut *self.mutex.value.get() }
        }
    }

    impl<T> Drop for MutexGuard<'_, T> {
        fn drop(&mut self) {
            let waker = {
                let mut state = self.mutex.state.lock().unwrap_or_else(|e| e.into_inner());
                state.locked = false;
                state.waiters.pop_front().map(|(_, w)| w)
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }
}

pub(crate) mod future {
    use super::*;

    pub(crate) type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Either<A, B> {
        Left(A),
        Right(B),
    }

    pub(crate) struct Select<A, B>(Option<(A, B)>);

    impl<A: Future + Unpin, B: Future + Unpin> Unpin for Select<A, B> {}

    impl<A: Future + Unpin, B: Future + Unpin> Future for Select<A, B> {
        type Output = Either<(A::Output, B), (B::Output, A)>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let (mut a, mut b) = self.0.take().expect("Select polled after completion");
            if let Poll::Ready(val) = Pin::new(&mut a).poll(cx) {
                return Poll::Ready(Either::Left((val, b)));
            }
            if let Poll::Ready(val) = Pin::new(&mut b).poll(cx) {
                return Poll::Ready(Either::Right((val, a)));
            }
            self.0 = Some((a, b));
            Poll::Pending
        }
    }

    pub(crate) fn select<A: Future + Unpin, B: Future + Unpin>(a: A, b: B) -> Select<A, B> {
        Select(Some((a, b)))
    }

    pub(crate) trait FutureExt: Future {
        fn boxed_local<'a>(self) -> LocalBoxFuture<'a, Self::Output>
        where
            Self: Sized + 'a,
        {
            Box::pin(self)
        }
    }

    impl<F: Future + ?Sized> FutureExt for F {}
}
