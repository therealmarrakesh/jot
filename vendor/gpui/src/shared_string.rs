use std::{
    borrow::{Borrow, Cow},
    fmt,
    ops::Deref,
    sync::Arc,
};

/// An immutable string that can be cheaply cloned across GPUI tasks and UI elements.
#[derive(Eq, PartialEq, PartialOrd, Ord, Hash, Clone)]
pub struct SharedString(Repr);

#[derive(Clone)]
enum Repr {
    Static(&'static str),
    Heap(Arc<str>),
}

impl Repr {
    #[inline]
    fn as_str(&self) -> &str {
        match self {
            Repr::Static(s) => s,
            Repr::Heap(s) => s,
        }
    }
}

impl PartialEq for Repr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Repr {}

impl PartialOrd for Repr {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Repr {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl std::hash::Hash for Repr {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Deref for SharedString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

impl SharedString {
    /// Creates a static [`SharedString`] from a `&'static str`.
    #[inline]
    pub const fn new_static(str: &'static str) -> Self {
        Self(Repr::Static(str))
    }

    /// Creates a [`SharedString`].
    #[inline]
    pub fn new(str: impl AsRef<str>) -> Self {
        Self(Repr::Heap(Arc::from(str.as_ref())))
    }

    /// Get a `&str` from the underlying string.
    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Default for SharedString {
    #[inline]
    fn default() -> Self {
        Self::new_static("")
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

impl fmt::Debug for SharedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for SharedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<String> for SharedString {
    #[inline]
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<SharedString> for String {
    #[inline]
    fn eq(&self, other: &SharedString) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<str> for SharedString {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<'a> PartialEq<&'a str> for SharedString {
    #[inline]
    fn eq(&self, other: &&'a str) -> bool {
        self.as_str() == *other
    }
}

impl From<&SharedString> for SharedString {
    #[inline]
    fn from(s: &SharedString) -> SharedString {
        s.clone()
    }
}

impl From<&str> for SharedString {
    #[inline]
    fn from(s: &str) -> SharedString {
        Self::new(s)
    }
}

impl From<char> for SharedString {
    #[inline]
    fn from(c: char) -> SharedString {
        let mut buf = [0u8; 4];
        Self::new(c.encode_utf8(&mut buf))
    }
}

impl From<&mut str> for SharedString {
    #[inline]
    fn from(s: &mut str) -> SharedString {
        Self::new(&*s)
    }
}

impl From<&String> for SharedString {
    #[inline]
    fn from(s: &String) -> SharedString {
        Self::new(s.as_str())
    }
}

impl From<String> for SharedString {
    #[inline]
    fn from(text: String) -> Self {
        Self(Repr::Heap(Arc::from(text)))
    }
}

impl From<Box<str>> for SharedString {
    #[inline]
    fn from(s: Box<str>) -> SharedString {
        Self(Repr::Heap(Arc::from(s)))
    }
}

impl From<Arc<str>> for SharedString {
    #[inline]
    fn from(s: Arc<str>) -> SharedString {
        Self(Repr::Heap(s))
    }
}

impl From<&Arc<str>> for SharedString {
    #[inline]
    fn from(s: &Arc<str>) -> SharedString {
        Self(Repr::Heap(s.clone()))
    }
}

impl<'a> From<Cow<'a, str>> for SharedString {
    #[inline]
    fn from(s: Cow<'a, str>) -> SharedString {
        Self::new(s.as_ref())
    }
}

impl From<SharedString> for Arc<str> {
    #[inline]
    fn from(text: SharedString) -> Self {
        match text.0 {
            Repr::Static(s) => Arc::from(s),
            Repr::Heap(s) => s,
        }
    }
}

impl From<SharedString> for String {
    #[inline]
    fn from(text: SharedString) -> Self {
        text.as_str().to_owned()
    }
}
