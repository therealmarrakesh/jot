//! Shared string and error types used across Jot.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// Immutable string backed by either a `&'static str` or a cheap-to-clone `Arc<str>`.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum SharedStr {
    Static(&'static str),
    Owned(Arc<str>),
}

impl Default for SharedStr {
    fn default() -> Self {
        Self::Static("")
    }
}

impl SharedStr {
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

impl Deref for SharedStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for SharedStr {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for SharedStr {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for SharedStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl From<&'static str> for SharedStr {
    #[inline]
    fn from(s: &'static str) -> Self {
        Self::Static(s)
    }
}

impl From<String> for SharedStr {
    #[inline]
    fn from(s: String) -> Self {
        Self::Owned(s.into())
    }
}

impl From<&String> for SharedStr {
    #[inline]
    fn from(s: &String) -> Self {
        Self::Owned(s.as_str().into())
    }
}

impl From<Arc<str>> for SharedStr {
    #[inline]
    fn from(s: Arc<str>) -> Self {
        Self::Owned(s)
    }
}

impl From<SharedStr> for String {
    #[inline]
    fn from(s: SharedStr) -> Self {
        s.as_str().to_owned()
    }
}

impl PartialEq<str> for SharedStr {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for SharedStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for SharedStr {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

/// Lightweight string-backed error for Win32, Direct2D, and DirectWrite operations.
pub(crate) struct Error {
    message: String,
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn new(msg: impl fmt::Display) -> Self {
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
