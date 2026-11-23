//! Clean-room core types: `SharedString`, lightweight `Error`/`Result`, safe `SmallVec` wrapper,
//! and standard collection aliases.

use std::borrow::Borrow;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// Immutable string backed by either a `&'static str` or a shared `Arc<str>`.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum SharedString {
    Static(&'static str),
    Owned(Arc<str>),
}

impl Default for SharedString {
    fn default() -> Self {
        Self::Static("")
    }
}

impl SharedString {
    #[inline]
    pub(crate) fn new(s: impl Into<Arc<str>>) -> Self {
        Self::Owned(s.into())
    }

    #[inline]
    pub(crate) const fn new_static(s: &'static str) -> Self {
        Self::Static(s)
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Static(s) => s,
            Self::Owned(s) => s,
        }
    }
}

impl Deref for SharedString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for SharedString {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for SharedString {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SharedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for SharedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl From<&'static str> for SharedString {
    #[inline]
    fn from(s: &'static str) -> Self {
        Self::Static(s)
    }
}

impl From<String> for SharedString {
    #[inline]
    fn from(s: String) -> Self {
        Self::Owned(s.into())
    }
}

impl From<&String> for SharedString {
    #[inline]
    fn from(s: &String) -> Self {
        Self::Owned(s.as_str().into())
    }
}

impl From<Arc<str>> for SharedString {
    #[inline]
    fn from(s: Arc<str>) -> Self {
        Self::Owned(s)
    }
}

impl From<SharedString> for String {
    #[inline]
    fn from(s: SharedString) -> Self {
        s.as_str().to_owned()
    }
}

impl PartialEq<str> for SharedString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for SharedString {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for SharedString {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

/// Minimal clean-room error wrapper matching the `anyhow` subset used by Jot.
pub(crate) mod anyhow {
    use std::fmt;

    pub(crate) struct Error {
        message: String,
    }

    pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

    impl Error {
        pub(crate) fn msg(msg: impl fmt::Display) -> Self {
            Self {
                message: msg.to_string(),
            }
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl fmt::Debug for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
        fn from(err: E) -> Self {
            Self {
                message: err.to_string(),
            }
        }
    }

    pub(crate) trait Context<T> {
        fn context<C: fmt::Display>(self, context: C) -> Result<T>;
    }

    impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
        fn context<C: fmt::Display>(self, context: C) -> Result<T> {
            self.map_err(|err| Error::msg(format!("{context}: {err}")))
        }
    }

    impl<T> Context<T> for Option<T> {
        fn context<C: fmt::Display>(self, context: C) -> Result<T> {
            self.ok_or_else(|| Error::msg(context))
        }
    }

    macro_rules! ensure {
        ($cond:expr, $($arg:tt)*) => {
            if !$cond {
                return Err($crate::ui::anyhow::Error::msg(format!($($arg)*)));
            }
        };
    }

    pub(crate) use ensure;
}

/// Safe `Vec`-backed drop-in for `SmallVec<[T; N]>` (zero `unsafe`).
pub(crate) mod smallvec {
    use super::*;

    pub(crate) trait Array {
        type Item;
    }

    impl<T, const N: usize> Array for [T; N] {
        type Item = T;
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub(crate) struct SmallVec<A: Array>(Vec<A::Item>);

    impl<A: Array> Default for SmallVec<A> {
        #[inline]
        fn default() -> Self {
            Self(Vec::new())
        }
    }

    impl<A: Array> SmallVec<A> {
        #[inline]
        pub(crate) const fn new() -> Self {
            Self(Vec::new())
        }

        #[inline]
        pub(crate) fn with_capacity(capacity: usize) -> Self {
            Self(Vec::with_capacity(capacity))
        }

        #[inline]
        pub(crate) fn from_slice(slice: &[A::Item]) -> Self
        where
            A::Item: Clone,
        {
            Self(slice.to_vec())
        }

        #[inline]
        pub(crate) fn push(&mut self, value: A::Item) {
            self.0.push(value);
        }

        #[inline]
        pub(crate) fn into_vec(self) -> Vec<A::Item> {
            self.0
        }

        #[cfg(test)]
        #[inline]
        pub(crate) fn as_slice(&self) -> &[A::Item] {
            &self.0
        }
    }

    impl<A: Array> Deref for SmallVec<A> {
        type Target = [A::Item];

        #[inline]
        fn deref(&self) -> &[A::Item] {
            &self.0
        }
    }

    impl<A: Array> DerefMut for SmallVec<A> {
        #[inline]
        fn deref_mut(&mut self) -> &mut [A::Item] {
            &mut self.0
        }
    }

    impl<A: Array> FromIterator<A::Item> for SmallVec<A> {
        fn from_iter<I: IntoIterator<Item = A::Item>>(iter: I) -> Self {
            Self(iter.into_iter().collect())
        }
    }

    impl<A: Array> Extend<A::Item> for SmallVec<A> {
        fn extend<I: IntoIterator<Item = A::Item>>(&mut self, iter: I) {
            self.0.extend(iter);
        }
    }

    impl<A: Array> IntoIterator for SmallVec<A> {
        type Item = A::Item;
        type IntoIter = std::vec::IntoIter<A::Item>;

        fn into_iter(self) -> Self::IntoIter {
            self.0.into_iter()
        }
    }

    impl<'a, A: Array> IntoIterator for &'a SmallVec<A> {
        type Item = &'a A::Item;
        type IntoIter = std::slice::Iter<'a, A::Item>;

        fn into_iter(self) -> Self::IntoIter {
            self.0.iter()
        }
    }

    impl<'a, A: Array> IntoIterator for &'a mut SmallVec<A> {
        type Item = &'a mut A::Item;
        type IntoIter = std::slice::IterMut<'a, A::Item>;

        fn into_iter(self) -> Self::IntoIter {
            self.0.iter_mut()
        }
    }

    impl<T, const N: usize> From<[T; N]> for SmallVec<[T; N]> {
        fn from(array: [T; N]) -> Self {
            Self(Vec::from(array))
        }
    }

    impl<A: Array> From<Vec<A::Item>> for SmallVec<A> {
        fn from(vec: Vec<A::Item>) -> Self {
            Self(vec)
        }
    }

    macro_rules! smallvec {
        () => {
            $crate::ui::smallvec::SmallVec::new()
        };
        ($elem:expr; $n:expr) => {
            $crate::ui::smallvec::SmallVec::from(vec![$elem; $n])
        };
        ($($x:expr),+ $(,)?) => {
            $crate::ui::smallvec::SmallVec::from(vec![$($x),+])
        };
    }

    pub(crate) use smallvec;
}
