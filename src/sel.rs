use std::ffi::{CStr, CString, c_char};
use std::ptr::NonNull;
use std::sync::LazyLock;

use dashmap::DashMap;

use crate::types::{ObjcSelector, SEL};

/// Maps canonical name → interned `&'static CStr`.
/// Each name string is leaked via `Box::leak` for process-lifetime stability.
static NAME_TABLE: LazyLock<DashMap<Box<str>, &'static CStr>> = LazyLock::new(DashMap::new);

/// Maps canonical name → leaked `&'static ObjcSelector`.
/// Deduplicates selectors created by `sel_registerName` (which have no types).
static SEL_TABLE: LazyLock<DashMap<Box<str>, &'static ObjcSelector>> = LazyLock::new(DashMap::new);

/// Intern a selector name string and return a stable `NonNull<c_char>` pointer.
///
/// Guaranteed: calling this function twice with equal strings returns the
/// same pointer value. Used by both `sel_register_name_str` and the loader's
/// selector fixup.
pub fn intern_selector_name(name: &str) -> NonNull<c_char> {
    let cstr: &'static CStr = *NAME_TABLE.entry(name.into()).or_insert_with(|| {
        let cs = CString::new(name).expect("selector name must not contain interior NULs");
        Box::leak(cs.into_boxed_c_str())
    });
    // SAFETY: `CStr::as_ptr()` always returns a non-null pointer.
    unsafe { NonNull::new_unchecked(cstr.as_ptr().cast_mut()) }
}

/// Return (or create) the unique selector for the Rust string `name`.
///
/// The returned SEL has `types == None` (untyped). Two calls with the same
/// string return the same `NonNull<ObjcSelector>` pointer.
pub fn sel_register_name_str(name: &str) -> SEL {
    let sel_ref: &'static ObjcSelector = *SEL_TABLE.entry(name.into()).or_insert_with(|| {
        let name_ptr = intern_selector_name(name);
        Box::leak(Box::new(ObjcSelector {
            name: name_ptr,
            types: None,
        }))
    });
    NonNull::from(sel_ref)
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
    unsafe { (*sel.as_ptr()).name.as_ptr() }
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
