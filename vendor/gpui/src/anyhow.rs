//! Minimal zero-dependency replacement for the `anyhow` crate.

use std::fmt::{self, Debug, Display};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[allow(non_snake_case)]
#[inline]
pub fn Ok<T>(value: T) -> Result<T> {
    std::result::Result::Ok(value)
}

enum ErrorRepr {
    Message(String),
    Source {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    Context {
        context: String,
        source: Box<Error>,
    },
}

pub struct Error {
    repr: ErrorRepr,
}

impl Error {
    pub fn msg<M: Display>(message: M) -> Self {
        Self {
            repr: ErrorRepr::Message(message.to_string()),
        }
    }

    pub fn new<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self {
            repr: ErrorRepr::Source {
                source: Box::new(error),
            },
        }
    }

    pub fn context<C: Display>(self, context: C) -> Self {
        Self {
            repr: ErrorRepr::Context {
                context: context.to_string(),
                source: Box::new(self),
            },
        }
    }

    pub fn downcast_ref<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match &self.repr {
            ErrorRepr::Message(_) => None,
            ErrorRepr::Source { source } => source.downcast_ref::<E>(),
            ErrorRepr::Context { source, .. } => source.downcast_ref::<E>(),
        }
    }

    pub fn root_cause(&self) -> &(dyn std::error::Error + 'static) {
        match &self.repr {
            ErrorRepr::Message(_) => self.as_dyn_error(),
            ErrorRepr::Source { source } => {
                let mut cur: &(dyn std::error::Error + 'static) = source.as_ref();
                while let Some(next) = cur.source() {
                    cur = next;
                }
                cur
            }
            ErrorRepr::Context { source, .. } => source.root_cause(),
        }
    }

    fn as_dyn_error(&self) -> &(dyn std::error::Error + 'static) {
        static DUMMY: StaticMsgError = StaticMsgError;
        match &self.repr {
            ErrorRepr::Source { source } => source.as_ref(),
            ErrorRepr::Context { source, .. } => source.as_dyn_error(),
            ErrorRepr::Message(_) => &DUMMY,
        }
    }
}

#[derive(Debug)]
struct StaticMsgError;
impl Display for StaticMsgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("error")
    }
}
impl std::error::Error for StaticMsgError {}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(error: E) -> Self {
        Self::new(error)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            ErrorRepr::Message(msg) => f.write_str(msg),
            ErrorRepr::Source { source } => Display::fmt(source, f),
            ErrorRepr::Context { context, source } => {
                if f.alternate() {
                    write!(f, "{context}: {source:#}")
                } else {
                    f.write_str(context)
                }
            }
        }
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            ErrorRepr::Message(msg) => f.write_str(msg),
            ErrorRepr::Source { source } => Display::fmt(source, f),
            ErrorRepr::Context { context, source } => {
                write!(f, "{context}\n\nCaused by:\n    {source:?}")
            }
        }
    }
}

pub trait Context<T, E> {
    fn context<C>(self, context: C) -> Result<T, Error>
    where
        C: Display + Send + Sync + 'static;

    fn with_context<C, F>(self, f: F) -> Result<T, Error>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C;
}

impl<T, E> Context<T, E> for std::result::Result<T, E>
where
    E: Into<Error>,
{
    fn context<C>(self, context: C) -> Result<T, Error>
    where
        C: Display + Send + Sync + 'static,
    {
        self.map_err(|e| e.into().context(context))
    }

    fn with_context<C, F>(self, f: F) -> Result<T, Error>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|e| e.into().context(f()))
    }
}

impl<T> Context<T, std::convert::Infallible> for Option<T> {
    fn context<C>(self, context: C) -> Result<T, Error>
    where
        C: Display + Send + Sync + 'static,
    {
        self.ok_or_else(|| Error::msg(context))
    }

    fn with_context<C, F>(self, f: F) -> Result<T, Error>
    where
        C: Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.ok_or_else(|| Error::msg(f()))
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __gpui_anyhow {
    ($msg:literal $(,)?) => {
        $crate::anyhow::Error::msg(format!($msg))
    };
    ($err:expr $(,)?) => {
        $crate::anyhow::Error::msg($err)
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::anyhow::Error::msg(format!($fmt, $($arg)*))
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __gpui_bail {
    ($($arg:tt)*) => {
        return ::std::result::Result::Err($crate::__gpui_anyhow!($($arg)*))
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __gpui_ensure {
    ($cond:expr $(,)?) => {
        if !$cond {
            $crate::__gpui_bail!(concat!("Condition failed: `", stringify!($cond), "`"));
        }
    };
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            $crate::__gpui_bail!($($arg)+);
        }
    };
}

pub use crate::__gpui_anyhow as anyhow;
pub use crate::__gpui_anyhow as format_err;
pub use crate::__gpui_bail as bail;
pub use crate::__gpui_ensure as ensure;
