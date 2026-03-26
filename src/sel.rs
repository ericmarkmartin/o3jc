use std::ffi::{CStr, CString, c_char};
use std::ptr::NonNull;
use std::sync::LazyLock;

use dashmap::DashMap;

use crate::types::{ObjcSelector, SEL};

/// Maps canonical name → interned `*const c_char` (stored as usize for `Send`).
/// Each name string is leaked via `CString::into_raw` for process-lifetime stability.
static NAME_TABLE: LazyLock<DashMap<Box<str>, usize>> = LazyLock::new(DashMap::new);

/// Maps canonical name → leaked `*mut ObjcSelector` (stored as usize for `Send`).
/// Deduplicates selectors created by `sel_registerName` (which have no types).
static SEL_TABLE: LazyLock<DashMap<Box<str>, usize>> = LazyLock::new(DashMap::new);

/// Intern a selector name string and return a stable `*const c_char` pointer.
///
/// Guaranteed: calling this function twice with equal strings returns the
/// same pointer value. Used by both `sel_register_name_str` and the loader's
/// selector fixup.
pub fn intern_selector_name(name: &str) -> *const c_char {
    let addr = *NAME_TABLE.entry(name.into()).or_insert_with(|| {
        let cs = CString::new(name).expect("selector name must not contain interior NULs");
        cs.into_raw() as usize
    });
    addr as *const c_char
}

/// Return (or create) the unique selector for the Rust string `name`.
///
/// The returned SEL has `types == null` (untyped). Two calls with the same
/// string return the same `NonNull<ObjcSelector>` pointer.
pub fn sel_register_name_str(name: &str) -> SEL {
    let addr = *SEL_TABLE.entry(name.into()).or_insert_with(|| {
        let name_ptr = intern_selector_name(name);
        let sel = Box::new(ObjcSelector {
            name: name_ptr,
            types: std::ptr::null(),
        });
        Box::into_raw(sel) as usize
    });
    // SAFETY: `addr` is from `Box::into_raw`, always non-null.
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
    // SAFETY: every SEL points to a valid ObjcSelector whose `name` field
    // is either an interned CString (from sel_register_name_str) or a
    // loader-fixedup pointer (from __objc_selectors).
    unsafe { (*sel.as_ptr()).name }
}

/// Compare two selectors for name equality.
///
/// Returns true iff both selectors refer to the same method name (their
/// interned `name` pointers are equal). This is the correct comparison for
/// GNUstep v2 ABI where different typed selectors with the same name share
/// the same interned name pointer.
pub fn sel_eq(a: SEL, b: SEL) -> bool {
    // SAFETY: both SELs point to valid ObjcSelector structs.
    unsafe { (*a.as_ptr()).name == (*b.as_ptr()).name }
}
