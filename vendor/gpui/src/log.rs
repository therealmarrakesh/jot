//! Minimal zero-dependency replacement for the `log` crate (no-op logger).

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

pub struct Record;

pub struct RecordBuilder;

impl Record {
    pub fn builder() -> RecordBuilder {
        RecordBuilder
    }
}

impl RecordBuilder {
    pub fn target(&mut self, _: &str) -> &mut Self {
        self
    }
    pub fn args(&mut self, _: std::fmt::Arguments<'_>) -> &mut Self {
        self
    }
    pub fn file(&mut self, _: Option<&str>) -> &mut Self {
        self
    }
    pub fn line(&mut self, _: Option<u32>) -> &mut Self {
        self
    }
    pub fn level(&mut self, _: Level) -> &mut Self {
        self
    }
    pub fn build(&self) -> Record {
        Record
    }
}

pub struct NopLogger;

impl NopLogger {
    pub fn log(&self, _: &Record) {}
    pub fn flush(&self) {}
}

pub fn logger() -> NopLogger {
    NopLogger
}

#[doc(hidden)]
#[macro_export]
macro_rules! __gpui_log_noop {
    (target: $target:expr, $($arg:tt)+) => {{
        let _ = $target;
        if false {
            let _ = format_args!($($arg)+);
        }
    }};
    ($($arg:tt)+) => {{
        if false {
            let _ = format_args!($($arg)+);
        }
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __gpui_log_enabled {
    ($($arg:tt)*) => {
        false
    };
}

pub use crate::__gpui_log_enabled as log_enabled;
pub use crate::__gpui_log_noop as debug;
pub use crate::__gpui_log_noop as error;
pub use crate::__gpui_log_noop as info;
pub use crate::__gpui_log_noop as trace;
pub use crate::__gpui_log_noop as warn;
