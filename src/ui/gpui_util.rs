use std::{fmt, hash::Hasher, ops::AddAssign, panic::Location};

pub(crate) fn post_inc<T: From<u8> + AddAssign<T> + Copy>(value: &mut T) -> T {
    let prev = *value;
    *value += T::from(1);
    prev
}

pub(crate) fn measure<R>(_label: &str, f: impl FnOnce() -> R) -> R {
    f()
}

macro_rules! debug_panic {
    ( $($fmt_arg:tt)* ) => {
        if cfg!(debug_assertions) {
            panic!( $($fmt_arg)* );
        } else {
            $crate::ui::log::error!("{}", format_args!($($fmt_arg)*));
        }
    };
}

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

pub(crate) use {debug_panic, maybe};

pub(crate) trait ResultExt<E> {
    type Ok;

    fn log_err(self) -> Option<Self::Ok>;
    fn log_with_level(self, level: crate::ui::log::Level) -> Option<Self::Ok>;
}

impl<T, E> ResultExt<E> for Result<T, E>
where
    E: fmt::Display,
{
    type Ok = T;

    #[track_caller]
    fn log_err(self) -> Option<T> {
        self.log_with_level(crate::ui::log::Level::Error)
    }

    #[track_caller]
    fn log_with_level(self, level: crate::ui::log::Level) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log_error_with_caller(*Location::caller(), format_args!("{error:#}"), level);
                None
            }
        }
    }
}

#[inline(never)]
fn log_error_with_caller(
    caller: core::panic::Location<'_>,
    arguments: fmt::Arguments<'_>,
    level: crate::ui::log::Level,
) {
    crate::ui::log::logger().log(
        &crate::ui::log::Record::builder()
            .target("gpui")
            .args(arguments)
            .file(Some(caller.file()))
            .line(Some(caller.line()))
            .level(level)
            .build(),
    );
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TypeIdHashBuilder;

impl std::hash::BuildHasher for TypeIdHashBuilder {
    type Hasher = TypeIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        TypeIdHasher::default()
    }
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TypeIdHasher {
    value: u64,
}

impl Hasher for TypeIdHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if let Some(bytes) = bytes.get(..8) {
            self.value = u64::from_ne_bytes(bytes.try_into().unwrap());
        } else {
            crate::ui::debug_panic!(
                "expected a 64-bit value, did you use this hasher with something other than a TypeId?"
            );
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.value
    }
}
