//! Core UI string and error types: `SharedString`, `UiError`, and `UiResult`.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
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

/// Lightweight string-backed error type for internal Win32/Direct2D/window operations.
pub(crate) struct UiError {
    message: String,
}

pub(crate) type UiResult<T> = std::result::Result<T, UiError>;

impl UiError {
    pub(crate) fn new(msg: impl fmt::Display) -> Self {
        Self {
            message: msg.to_string(),
        }
    }
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for UiError {
    fn from(err: E) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}
