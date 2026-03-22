use std::ffi::{CStr, CString, c_char};
use std::ptr::NonNull;
use std::sync::LazyLock;

use dashmap::DashMap;

use crate::types::{ObjcSelector, SEL};

/// Global selector intern table: canonical name → stable SEL (stored as usize
/// to satisfy DashMap's `Send` bound on values).
///
/// Once stored, the CString backing each SEL is leaked and lives for the
/// process lifetime, ensuring SEL pointer stability.
static SELECTOR_TABLE: LazyLock<DashMap<Box<str>, usize>> = LazyLock::new(DashMap::new);

/// Return (or create) the unique selector for the Rust string `name`.
///
/// Guaranteed: calling this function twice with equal strings returns the
/// same pointer value.
pub fn sel_register_name_str(name: &str) -> SEL {
    let table = &*SELECTOR_TABLE;
    let addr = *table.entry(name.into()).or_insert_with(|| {
        let cs = CString::new(name).expect("selector name must not contain interior NULs");
        cs.into_raw() as usize
    });
    // SAFETY: `addr` is the result of `CString::into_raw`, which is always non-null.
    // The concrete representation of ObjcSelector is an interned C string; we
    // store it as an opaque pointer and cast back in `sel_get_name`.
    unsafe { NonNull::new_unchecked(addr as *mut ObjcSelector) }
}

/// C-ABI entry point: intern `name` (a null-terminated C string) as a selector.
///
/// # Safety
/// `name` must be a valid, non-null, null-terminated C string.
pub unsafe fn sel_register_name_cstr(name: *const c_char) -> SEL {
    // SAFETY: caller guarantees `name` is a valid, non-null, null-terminated C string.
    let s = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    sel_register_name_str(&s)
}

/// Return the interned name of a selector as a C string pointer.
pub fn sel_get_name(sel: SEL) -> *const c_char {
    // SAFETY: every SEL was created from a leaked CString in `sel_register_name_str`;
    // the ObjcSelector pointer is always a valid null-terminated C string in disguise.
    sel.as_ptr() as *const c_char
}
