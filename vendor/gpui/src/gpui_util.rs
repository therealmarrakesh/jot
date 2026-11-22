use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt::{self, Debug},
    future::Future,
    hash::{Hash, Hasher},
    ops::AddAssign,
    panic::Location,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

pub mod arc_cow {
    use super::*;

    pub enum ArcCow<'a, T: ?Sized> {
        Borrowed(&'a T),
        Owned(Arc<T>),
    }

    impl<T: ?Sized + PartialEq> PartialEq for ArcCow<'_, T> {
        fn eq(&self, other: &Self) -> bool {
            self.as_ref() == other.as_ref()
        }
    }

    impl<T: ?Sized + PartialOrd> PartialOrd for ArcCow<'_, T> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            self.as_ref().partial_cmp(other.as_ref())
        }
    }

    impl<T: ?Sized + Ord> Ord for ArcCow<'_, T> {
        fn cmp(&self, other: &Self) -> Ordering {
            self.as_ref().cmp(other.as_ref())
        }
    }

    impl<T: ?Sized + Eq> Eq for ArcCow<'_, T> {}

    impl<T: ?Sized + Hash> Hash for ArcCow<'_, T> {
        fn hash<H: Hasher>(&self, state: &mut H) {
            match self {
                Self::Borrowed(borrowed) => Hash::hash(borrowed, state),
                Self::Owned(owned) => Hash::hash(&**owned, state),
            }
        }
    }

    impl<T: ?Sized> Clone for ArcCow<'_, T> {
        fn clone(&self) -> Self {
            match self {
                Self::Borrowed(borrowed) => Self::Borrowed(borrowed),
                Self::Owned(owned) => Self::Owned(owned.clone()),
            }
        }
    }

    impl<'a, T: ?Sized> From<&'a T> for ArcCow<'a, T> {
        fn from(s: &'a T) -> Self {
            Self::Borrowed(s)
        }
    }

    impl<T: ?Sized> From<Arc<T>> for ArcCow<'_, T> {
        fn from(s: Arc<T>) -> Self {
            Self::Owned(s)
        }
    }

    impl<T: ?Sized> From<&'_ Arc<T>> for ArcCow<'_, T> {
        fn from(s: &'_ Arc<T>) -> Self {
            Self::Owned(s.clone())
        }
    }

    impl From<String> for ArcCow<'_, str> {
        fn from(value: String) -> Self {
            Self::Owned(value.into())
        }
    }

    impl From<&String> for ArcCow<'_, str> {
        fn from(value: &String) -> Self {
            Self::Owned(value.clone().into())
        }
    }

    impl<'a> From<Cow<'a, str>> for ArcCow<'a, str> {
        fn from(value: Cow<'a, str>) -> Self {
            match value {
                Cow::Borrowed(borrowed) => Self::Borrowed(borrowed),
                Cow::Owned(owned) => Self::Owned(owned.into()),
            }
        }
    }

    impl<T> From<Vec<T>> for ArcCow<'_, [T]> {
        fn from(vec: Vec<T>) -> Self {
            ArcCow::Owned(Arc::from(vec))
        }
    }

    impl<'a> From<&'a str> for ArcCow<'a, [u8]> {
        fn from(s: &'a str) -> Self {
            ArcCow::Borrowed(s.as_bytes())
        }
    }

    impl<T: ?Sized + ToOwned> std::borrow::Borrow<T> for ArcCow<'_, T> {
        fn borrow(&self) -> &T {
            match self {
                ArcCow::Borrowed(borrowed) => borrowed,
                ArcCow::Owned(owned) => owned.as_ref(),
            }
        }
    }

    impl<T: ?Sized> std::ops::Deref for ArcCow<'_, T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            match self {
                ArcCow::Borrowed(s) => s,
                ArcCow::Owned(s) => s.as_ref(),
            }
        }
    }

    impl<T: ?Sized> AsRef<T> for ArcCow<'_, T> {
        fn as_ref(&self) -> &T {
            match self {
                ArcCow::Borrowed(s) => s,
                ArcCow::Owned(s) => s.as_ref(),
            }
        }
    }

    impl<T: ?Sized + Debug> Debug for ArcCow<'_, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Borrowed(s) => s.fmt(f),
                Self::Owned(s) => s.fmt(f),
            }
        }
    }

    impl<T: ?Sized + fmt::Display> fmt::Display for ArcCow<'_, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Borrowed(s) => s.fmt(f),
                Self::Owned(s) => s.fmt(f),
            }
        }
    }
}

pub fn post_inc<T: From<u8> + AddAssign<T> + Copy>(value: &mut T) -> T {
    let prev = *value;
    *value += T::from(1);
    prev
}

pub fn measure<R>(_label: &str, f: impl FnOnce() -> R) -> R {
    f()
}

#[macro_export]
macro_rules! debug_panic {
    ( $($fmt_arg:tt)* ) => {
        if cfg!(debug_assertions) {
            panic!( $($fmt_arg)* );
        } else {
            crate::log::error!("{}", format_args!($($fmt_arg)*));
        }
    };
}

#[macro_export]
macro_rules! maybe {
    ($block:block) => {
        (|| $block)()
    };
    (async $block:block) => {
        (async || $block)()
    };
    (async move $block:block) => {
        (async move || $block)()
    };
}

pub use crate::{debug_panic, maybe};

pub trait ResultExt<E> {
    type Ok;

    fn log_err(self) -> Option<Self::Ok>;
    fn log_err_with_backtrace(self) -> Option<Self::Ok>
    where
        E: fmt::Debug;
    fn debug_assert_ok(self, reason: &str) -> Self;
    fn warn_on_err(self) -> Option<Self::Ok>;
    fn log_with_level(self, level: crate::log::Level) -> Option<Self::Ok>;
    fn anyhow(self) -> crate::anyhow::Result<Self::Ok>
    where
        E: Into<crate::anyhow::Error>;
}

impl<T, E> ResultExt<E> for Result<T, E>
where
    E: fmt::Display,
{
    type Ok = T;

    #[track_caller]
    fn log_err(self) -> Option<T> {
        self.log_with_level(crate::log::Level::Error)
    }

    #[track_caller]
    fn log_err_with_backtrace(self) -> Option<T>
    where
        E: fmt::Debug,
    {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log_error_with_caller(
                    *Location::caller(),
                    format_args!("{:#}", DebugAsDisplay(&error)),
                    crate::log::Level::Error,
                );
                None
            }
        }
    }

    #[track_caller]
    fn debug_assert_ok(self, reason: &str) -> Self {
        if let Err(error) = &self {
            crate::debug_panic!("{reason} - {error:#}");
        }
        self
    }

    #[track_caller]
    fn warn_on_err(self) -> Option<T> {
        self.log_with_level(crate::log::Level::Warn)
    }

    #[track_caller]
    fn log_with_level(self, level: crate::log::Level) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log_error_with_caller(*Location::caller(), format_args!("{error:#}"), level);
                None
            }
        }
    }

    fn anyhow(self) -> crate::anyhow::Result<T>
    where
        E: Into<crate::anyhow::Error>,
    {
        self.map_err(Into::into)
    }
}

#[inline(never)]
fn log_error_with_caller(
    caller: core::panic::Location<'_>,
    arguments: fmt::Arguments<'_>,
    level: crate::log::Level,
) {
    crate::log::logger().log(
        &crate::log::Record::builder()
            .target("gpui")
            .args(arguments)
            .file(Some(caller.file()))
            .line(Some(caller.line()))
            .level(level)
            .build(),
    );
}

struct DebugAsDisplay<'a, E>(&'a E);

impl<E: fmt::Debug> fmt::Display for DebugAsDisplay<'_, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

pub trait TryFutureExt {
    fn log_err(self) -> LogErrorFuture<Self>
    where
        Self: Sized;

    fn log_tracked_err(self, location: core::panic::Location<'static>) -> LogErrorFuture<Self>
    where
        Self: Sized;

    fn warn_on_err(self) -> LogErrorFuture<Self>
    where
        Self: Sized;

    fn unwrap(self) -> UnwrapFuture<Self>
    where
        Self: Sized;
}

pub trait TryFutureExtBacktrace {
    fn log_err_with_backtrace(self) -> LogErrorWithBacktraceFuture<Self>
    where
        Self: Sized;

    fn log_tracked_err_with_backtrace(
        self,
        location: core::panic::Location<'static>,
    ) -> LogErrorWithBacktraceFuture<Self>
    where
        Self: Sized;
}

impl<F, T, E> TryFutureExt for F
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    #[track_caller]
    fn log_err(self) -> LogErrorFuture<Self> {
        LogErrorFuture(self, crate::log::Level::Error, *Location::caller())
    }

    fn log_tracked_err(self, location: core::panic::Location<'static>) -> LogErrorFuture<Self> {
        LogErrorFuture(self, crate::log::Level::Error, location)
    }

    #[track_caller]
    fn warn_on_err(self) -> LogErrorFuture<Self> {
        LogErrorFuture(self, crate::log::Level::Warn, *Location::caller())
    }

    fn unwrap(self) -> UnwrapFuture<Self> {
        UnwrapFuture(self)
    }
}

impl<F, T, E> TryFutureExtBacktrace for F
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Debug,
{
    #[track_caller]
    fn log_err_with_backtrace(self) -> LogErrorWithBacktraceFuture<Self> {
        LogErrorWithBacktraceFuture(self, crate::log::Level::Error, *Location::caller())
    }

    fn log_tracked_err_with_backtrace(
        self,
        location: core::panic::Location<'static>,
    ) -> LogErrorWithBacktraceFuture<Self> {
        LogErrorWithBacktraceFuture(self, crate::log::Level::Error, location)
    }
}

#[must_use]
pub struct LogErrorFuture<F>(F, crate::log::Level, core::panic::Location<'static>);

impl<F, T, E> Future for LogErrorFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let level = self.1;
        let location = self.2;
        let inner = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        match inner.poll(cx) {
            Poll::Ready(output) => Poll::Ready(match output {
                Ok(output) => Some(output),
                Err(error) => {
                    log_error_with_caller(location, format_args!("{error:#}"), level);
                    None
                }
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[must_use]
pub struct LogErrorWithBacktraceFuture<F>(F, crate::log::Level, core::panic::Location<'static>);

impl<F, T, E> Future for LogErrorWithBacktraceFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Debug,
{
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let level = self.1;
        let location = self.2;
        let inner = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        match inner.poll(cx) {
            Poll::Ready(output) => Poll::Ready(match output {
                Ok(output) => Some(output),
                Err(error) => {
                    log_error_with_caller(
                        location,
                        format_args!("{:#}", DebugAsDisplay(&error)),
                        level,
                    );
                    None
                }
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct UnwrapFuture<F>(F);

impl<F, T, E> Future for UnwrapFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Debug,
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let inner = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        match inner.poll(cx) {
            Poll::Ready(result) => Poll::Ready(result.unwrap()),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct Deferred<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Deferred<F> {
    pub fn abort(mut self) {
        self.0.take();
    }
}

impl<F: FnOnce()> Drop for Deferred<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f()
        }
    }
}

#[must_use]
pub fn defer<F: FnOnce()>(f: F) -> Deferred<F> {
    Deferred(Some(f))
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeIdHashBuilder;

impl std::hash::BuildHasher for TypeIdHashBuilder {
    type Hasher = TypeIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        TypeIdHasher::default()
    }
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeIdHasher {
    value: u64,
}

impl Hasher for TypeIdHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if let Some(bytes) = bytes.get(..8) {
            self.value = u64::from_ne_bytes(bytes.try_into().unwrap());
        } else {
            crate::debug_panic!(
                "expected a 64-bit value, did you use this hasher with something other than a TypeId?"
            );
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.value
    }
}
