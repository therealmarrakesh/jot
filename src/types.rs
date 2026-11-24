use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

pub(crate) const APP_NAME: &str = "Jot";

#[derive(Clone)]
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
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Static(s) => s,
            Self::Owned(s) => s,
        }
    }
}

impl PartialEq for SharedStr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for SharedStr {}

impl PartialOrd for SharedStr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for SharedStr {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
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

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub(crate) struct $name(pub(crate) u64);

        impl $name {
            pub(crate) fn next() -> Self {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            }
        }
    };
}
pub(crate) use id_type;

pub(crate) enum Error {
    Win(windows::core::Error),
    Io(std::io::Error),
    Msg(String),
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn new(msg: impl fmt::Display) -> Self {
        Self::Msg(msg.to_string())
    }

    pub(crate) fn hresult(&self) -> Option<i32> {
        match self {
            Self::Win(e) => Some(e.code().0),
            Self::Io(_) | Self::Msg(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win(e) => fmt::Display::fmt(e, f),
            Self::Io(e) => fmt::Display::fmt(e, f),
            Self::Msg(m) => f.write_str(m),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win(e) => write!(f, "{e} (0x{:08x})", e.code().0 as u32),
            Self::Io(e) => fmt::Display::fmt(e, f),
            Self::Msg(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<windows::core::Error> for Error {
    fn from(err: windows::core::Error) -> Self {
        Self::Win(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn shared_str_equality_and_hash_match_borrow_str_contract() {
        let a = SharedStr::Static("hello");
        let b = SharedStr::from(String::from("hello"));
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), Ordering::Equal);

        let mut map = HashMap::new();
        map.insert(a, 42);
        assert_eq!(map.get(&b), Some(&42));
        assert_eq!(map.get("hello"), Some(&42));
    }

    #[test]
    fn error_preserves_windows_hresult() {
        let hr = windows::core::HRESULT(0x8899_000Cu32 as i32);
        let err = Error::from(windows::core::Error::from_hresult(hr));
        assert_eq!(err.hresult(), Some(hr.0));
        assert!(format!("{err:?}").contains("8899000c"));
    }
}
