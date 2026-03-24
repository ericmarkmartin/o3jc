//! GNUstep v2 ELF section loader — `__objc_load`.
//!
//! When Clang compiles any `.m` file with `-fobjc-runtime=gnustep-2.0` it
//! generates a hidden `.objcv2_load_function` placed in `.init_array`.  That
//! function calls `__objc_load` with a pointer to an `ObjcModuleInfo` struct
//! whose fields are start/stop pointers into the ELF sections Clang emits
//! (`__objc_selectors`, `__objc_classes`, `__objc_cats`, …).
//!
//! # Phase 4 scope
//! The Phase 4 test uses only runtime APIs (no `@implementation`), so all
//! sections contain only the null-sentinel entries Clang always emits.
//! `__objc_load` just needs to exist and return cleanly.
//!
//! # Future phases
//! - Phase 5: walk `__objc_selectors` to intern/fix-up selector pointers;
//!   walk `__objc_classes` to register Clang-emitted class structs.
//! - Phase 8: walk `__objc_cats` to attach categories.
//! - Phase 9: walk `__objc_protocols` to register protocols.

use std::ffi::c_char;

/// The struct Clang emits as `.objc_init` and passes to `__objc_load`.
///
/// Layout (17 fields, all `#[repr(C)]`):
/// ```text
/// { i64 version,
///   i8** sel_start,  i8** sel_stop,
///   i8** cls_start,  i8** cls_stop,
///   i8** ref_start,  i8** ref_stop,
///   i8** cat_start,  i8** cat_stop,
///   i8** pro_start,  i8** pro_stop,
///   i8** prf_start,  i8** prf_stop,
///   i8** ali_start,  i8** ali_stop,
///   i8** str_start,  i8** str_stop }
/// ```
/// Matches the LLVM IR emitted by clang 14 with `-fobjc-runtime=gnustep-2.0`.
#[repr(C)]
pub struct ObjcModuleInfo {
    /// Module format version (0 for GNUstep v2).
    pub version: i64,

    // __objc_selectors — array of { *name, *types } structs
    pub sel_start: *mut *mut c_char,
    pub sel_stop:  *mut *mut c_char,

    // __objc_classes — array of class init pointers
    pub classes_start: *mut *mut u8,
    pub classes_stop:  *mut *mut u8,

    // __objc_class_refs — stubs used by the compiler for class-name lookups
    pub class_refs_start: *mut *mut u8,
    pub class_refs_stop:  *mut *mut u8,

    // __objc_cats — category descriptors
    pub cats_start: *mut *mut u8,
    pub cats_stop:  *mut *mut u8,

    // __objc_protocols — protocol descriptors
    pub protocols_start: *mut *mut u8,
    pub protocols_stop:  *mut *mut u8,

    // __objc_protocol_refs
    pub protocol_refs_start: *mut *mut u8,
    pub protocol_refs_stop:  *mut *mut u8,

    // __objc_class_aliases
    pub class_aliases_start: *mut *mut u8,
    pub class_aliases_stop:  *mut *mut u8,

    // __objc_constant_string
    pub constant_strings_start: *mut *mut u8,
    pub constant_strings_stop:  *mut *mut u8,
}

/// Entry point called by every Clang-compiled ObjC module at program startup.
///
/// # Safety
/// `info` must be non-null and point to a valid `ObjcModuleInfo` emitted by
/// Clang.  The start/stop range pointers must bracket valid ELF section data.
///
/// # Phase 4
/// All sections contain only null sentinels because the test binary has no
/// `@implementation` blocks.  This stub validates the pointer and returns.
/// Real section walking is added in Phase 5.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __objc_load(info: *const ObjcModuleInfo) {
    // SAFETY: Clang guarantees `info` is non-null and correctly laid out.
    // For Phase 4 the sections are empty; we just validate we got a pointer.
    debug_assert!(!info.is_null(), "__objc_load called with null info");
    let _ = info;
}
