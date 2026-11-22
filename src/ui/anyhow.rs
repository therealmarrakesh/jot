//! Minimal zero-dependency replacement for the `anyhow` crate.

use std::fmt::{self, Debug, Display};

pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

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

pub(crate) struct Error {
    repr: ErrorRepr,
}

impl Error {
    pub(crate) fn msg<M: Display>(message: M) -> Self {
        Self {
            repr: ErrorRepr::Message(message.to_string()),
        }
    }

    pub(crate) fn new<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self {
            repr: ErrorRepr::Source {
                source: Box::new(error),
            },
        }
    }

    pub(crate) fn context<C: Display>(self, context: C) -> Self {
        Self {
            repr: ErrorRepr::Context {
                context: context.to_string(),
                source: Box::new(self),
            },
        }
    }
}

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

pub(crate) trait Context<T, E> {
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

macro_rules! anyhow {
    ($msg:literal $(,)?) => {
        $crate::ui::anyhow::Error::msg(format!($msg))
    };
    ($err:expr $(,)?) => {
        $crate::ui::anyhow::Error::msg($err)
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::ui::anyhow::Error::msg(format!($fmt, $($arg)*))
    };
}

macro_rules! bail {
    ($($arg:tt)*) => {
        return ::std::result::Result::Err($crate::ui::anyhow::anyhow!($($arg)*))
    };
}

macro_rules! ensure {
    ($cond:expr $(,)?) => {
        if !$cond {
            $crate::ui::anyhow::bail!(concat!("Condition failed: `", stringify!($cond), "`"));
        }
    };
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            $crate::ui::anyhow::bail!($($arg)+);
        }
    };
}

pub(crate) use anyhow;
pub(crate) use bail;
pub(crate) use ensure;
