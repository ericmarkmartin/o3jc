//! A `Send + Sync` newtype around `NonNull<T>`.

use std::ptr::NonNull;

/// A non-null pointer that is `Send + Sync`.
///
/// # Safety
/// The caller must guarantee that all access through this pointer is
/// properly synchronised (e.g. by an external lock or by the ObjC
/// runtime's ownership rules).
#[derive(Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SendPtr<T>(NonNull<T>);

// Manual Copy/Clone: `NonNull<T>` is always `Copy`, but `derive(Copy)`
// would add a `T: Copy` bound we don't want.
impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SendPtr<T> {}

impl<T> SendPtr<T> {
    pub fn new(ptr: NonNull<T>) -> Self {
        Self(ptr)
    }
}

impl<T> std::ops::Deref for SendPtr<T> {
    type Target = NonNull<T>;
    fn deref(&self) -> &NonNull<T> {
        &self.0
    }
}

impl<T> From<NonNull<T>> for SendPtr<T> {
    fn from(ptr: NonNull<T>) -> Self {
        Self(ptr)
    }
}

// SAFETY: SendPtr is only constructed in contexts where the runtime
// serialises access to the pointee (side-table locks, stripe locks, etc.).
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}
