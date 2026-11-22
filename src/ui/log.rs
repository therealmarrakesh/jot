//! Minimal zero-dependency replacement for the `log` crate (no-op logger).

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Level {
    Error = 1,
}

pub(crate) struct Record;

pub(crate) struct RecordBuilder;

impl Record {
    pub(crate) fn builder() -> RecordBuilder {
        RecordBuilder
    }
}

impl RecordBuilder {
    pub(crate) fn target(&mut self, _: &str) -> &mut Self {
        self
    }
    pub(crate) fn args(&mut self, _: std::fmt::Arguments<'_>) -> &mut Self {
        self
    }
    pub(crate) fn file(&mut self, _: Option<&str>) -> &mut Self {
        self
    }
    pub(crate) fn line(&mut self, _: Option<u32>) -> &mut Self {
        self
    }
    pub(crate) fn level(&mut self, _: Level) -> &mut Self {
        self
    }
    pub(crate) fn build(&self) -> Record {
        Record
    }
}

pub(crate) struct NopLogger;

impl NopLogger {
    pub(crate) fn log(&self, _: &Record) {}
    pub(crate) fn flush(&self) {}
}

pub(crate) fn logger() -> NopLogger {
    NopLogger
}

macro_rules! log_noop {
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

pub(crate) use log_noop as debug;
pub(crate) use log_noop as error;
pub(crate) use log_noop as info;
pub(crate) use log_noop as trace;
pub(crate) use log_noop as warn;
