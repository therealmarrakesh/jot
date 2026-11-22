//! Thin unpoisoning wrappers over `std::sync::{Mutex, RwLock}`.
//!
//! On Windows, `std::sync::Mutex` and `std::sync::RwLock` are backed directly
//! by `SRWLOCK` (8 bytes, zero heap allocation).

use std::fmt;
pub(crate) use std::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard};

#[derive(Default)]
pub(crate) struct Mutex<T: ?Sized>(std::sync::Mutex<T>);

impl<T> Mutex<T> {
    #[inline]
    pub(crate) const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Default)]
pub(crate) struct RwLock<T: ?Sized>(std::sync::RwLock<T>);

impl<T> RwLock<T> {
    #[inline]
    pub(crate) const fn new(value: T) -> Self {
        Self(std::sync::RwLock::new(value))
    }
}

impl<T: ?Sized> RwLock<T> {
    #[inline]
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, T> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    #[inline]
    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.0.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
