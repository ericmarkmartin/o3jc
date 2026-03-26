//! GNUstep v2 ELF section loader — `__objc_load`.
//!
//! When Clang compiles any `.m` file with `-fobjc-runtime=gnustep-2.0` it
//! generates a hidden `.objcv2_load_function` placed in `.init_array`.  That
//! function calls `__objc_load` with a pointer to an `ObjcModuleInfo` struct
//! whose fields are start/stop pointers into the ELF sections Clang emits
//! (`__objc_selectors`, `__objc_classes`, `__objc_cats`, …).
//!
//! # Phase 5
//! Walks `__objc_selectors` to intern/fix-up selector pointers, then walks
//! `__objc_classes` to register Clang-emitted class structs.

use std::ffi::{CStr, c_char};
use std::ptr::NonNull;

use crate::class_registry;
use crate::method_cache::MethodCache;
use crate::sel::intern_selector_name;
use crate::types::*;

// ---------------------------------------------------------------------------
// Compiled ABI types — match Clang's GNUstep v2 output exactly
// ---------------------------------------------------------------------------

/// A selector entry in `__objc_selectors`: `{ name, types }`.
///
/// After fixup, `name` is overwritten with the interned name pointer so that
/// the struct can serve as an `ObjcSelector` for dispatch.
#[repr(C)]
struct CompiledSelector {
    name: *mut c_char,
    types: *const c_char,
}

/// Header of a compiled method list: `{ next, count, size, entries[] }`.
///
/// The `next` field is always null from the compiler; the runtime may use it
/// to chain additional lists (e.g. from categories).
#[repr(C)]
struct CompiledMethodList {
    next: *mut CompiledMethodList,
    count: i32,
    // Padding inserted by #[repr(C)] between i32 and i64.
    size: i64,
    // Followed by `count` CompiledMethodEntry structs inline.
}

/// A method entry as Clang emits it: `{ IMP, *selector_struct, *types }`.
#[repr(C)]
struct CompiledMethodEntry {
    imp: IMP,
    sel: *mut CompiledSelector,
    types: *const c_char,
}

// ---------------------------------------------------------------------------
// ObjcModuleInfo — the struct passed to __objc_load
// ---------------------------------------------------------------------------

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
#[repr(C)]
pub struct ObjcModuleInfo {
    pub version: i64,

    // __objc_selectors — array of { *name, *types } structs
    pub sel_start: *mut *mut c_char,
    pub sel_stop:  *mut *mut c_char,

    // __objc_classes — array of class pointers
    pub classes_start: *mut *mut u8,
    pub classes_stop:  *mut *mut u8,

    // __objc_class_refs
    pub class_refs_start: *mut *mut u8,
    pub class_refs_stop:  *mut *mut u8,

    // __objc_cats
    pub cats_start: *mut *mut u8,
    pub cats_stop:  *mut *mut u8,

    // __objc_protocols
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

// ---------------------------------------------------------------------------
// Selector fixup
// ---------------------------------------------------------------------------

/// Walk `__objc_selectors` and intern each selector name.
///
/// After this function returns, every `CompiledSelector.name` in the section
/// has been overwritten with the stable interned name pointer, making the
/// struct usable as an `ObjcSelector { name, types }` for dispatch.
///
/// # Safety
/// `start` and `stop` must bracket a valid `__objc_selectors` ELF section.
unsafe fn load_selectors(start: *mut *mut c_char, stop: *mut *mut c_char) {
    let mut ptr = start as *mut CompiledSelector;
    let end = stop as *mut CompiledSelector;
    while ptr < end {
        // SAFETY: ptr is within the section bounds.
        let entry = unsafe { &mut *ptr };
        if !entry.name.is_null() {
            // SAFETY: Clang emits valid null-terminated name strings.
            let name_str = unsafe { CStr::from_ptr(entry.name) }
                .to_str()
                .expect("selector name must be valid UTF-8");
            let interned = intern_selector_name(name_str);
            // Overwrite the name pointer with the interned version.
            entry.name = interned as *mut c_char;
        }
        // SAFETY: advancing within section bounds.
        ptr = unsafe { ptr.add(1) };
    }
}

// ---------------------------------------------------------------------------
// Method list conversion
// ---------------------------------------------------------------------------

/// Convert a Clang-emitted compiled method list into a runtime `MethodList`.
///
/// Reads the inline `CompiledMethodEntry` array, converts each selector
/// reference to an interned SEL (the name field was already fixed up by
/// `load_selectors`), and returns a heap-allocated `MethodList`.
///
/// Returns `None` if `compiled` is null or has zero entries.
///
/// # Safety
/// `compiled` must be null or point to a valid `CompiledMethodList` with
/// `count` inline `CompiledMethodEntry` structs following the header.
/// `load_selectors` must have already been called on this module.
unsafe fn convert_method_list(
    compiled: *const (),
) -> Option<NonNull<MethodList>> {
    if compiled.is_null() {
        return None;
    }
    let header = compiled as *const CompiledMethodList;
    // SAFETY: caller guarantees the pointer is valid.
    let count = unsafe { (*header).count } as usize;
    if count == 0 {
        return None;
    }

    // The entries array starts right after the header.
    let entries_ptr = unsafe {
        (header as *const u8).add(std::mem::size_of::<CompiledMethodList>())
    } as *const CompiledMethodEntry;

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        // SAFETY: `i < count` and the compiled method list has `count` inline entries.
        let ce = unsafe { &*entries_ptr.add(i) };

        // The selector's `name` field was fixed up by load_selectors to hold
        // the interned name pointer. Cast the CompiledSelector* to SEL
        // (NonNull<ObjcSelector>) — the CompiledSelector and ObjcSelector
        // have identical layout: { *name, *types }.
        // SAFETY: ce.sel was written by Clang and is non-null for real methods.
        let sel = unsafe { NonNull::new_unchecked(ce.sel as *mut ObjcSelector) };

        entries.push(MethodEntry {
            imp: ce.imp,
            sel,
            types: ce.types,
        });
    }

    Some(NonNull::from(Box::leak(Box::new(MethodList {
        next: None,
        entries,
    }))))
}

// ---------------------------------------------------------------------------
// Class loading
// ---------------------------------------------------------------------------

/// Walk `__objc_classes` and register each Clang-emitted class.
///
/// For each class:
/// 1. Patch `instance_size` (negate if negative; set minimum for root classes)
/// 2. Convert compiled method lists to runtime format
/// 3. Set up root metaclass ISA chain (self-loop)
/// 4. Initialize the method cache (stored in `dtable`)
/// 5. Register in the global class table
///
/// # Safety
/// `start` and `stop` must bracket a valid `__objc_classes` ELF section.
/// `load_selectors` must have been called first.
unsafe fn load_classes(start: *mut *mut u8, stop: *mut *mut u8) {
    let mut ptr = start as *mut *mut ObjcClass;
    let end = stop as *mut *mut ObjcClass;
    while ptr < end {
        // SAFETY: ptr is within section bounds.
        let cls_ptr: *mut ObjcClass = unsafe { *ptr };
        if cls_ptr.is_null() {
            // Skip null sentinels.
            ptr = unsafe { ptr.add(1) };
            continue;
        }
        // SAFETY: cls_ptr is a non-null Clang-emitted class struct.
        let cls = unsafe { &mut *cls_ptr };

        // --- Patch instance_size ---
        if cls.instance_size < 0 {
            cls.instance_size = -cls.instance_size;
        }
        if cls.instance_size == 0 && cls.super_class.is_none() {
            cls.instance_size = std::mem::size_of::<ObjcObject>() as i64;
        }

        // --- Convert method list ---
        let compiled_methods = cls.method_list;
        // The method_list field currently holds a raw pointer to a CompiledMethodList.
        // We need to read it as *const () and convert.
        let raw_methods = compiled_methods
            .map(|nn| nn.as_ptr() as *const ())
            .unwrap_or(std::ptr::null());
        cls.method_list = unsafe { convert_method_list(raw_methods) };

        // --- Initialize cache ---
        let cache = NonNull::from(Box::leak(MethodCache::new()));
        cls.set_cache(Some(cache));

        // --- Process metaclass ---
        if let Some(meta_nn) = cls.isa {
            let meta = unsafe { meta_nn.as_ptr().as_mut().unwrap() };

            // Patch metaclass instance_size
            if meta.instance_size < 0 {
                meta.instance_size = -meta.instance_size;
            }

            // Convert metaclass method list
            let meta_raw_methods = meta.method_list
                .map(|nn| nn.as_ptr() as *const ())
                .unwrap_or(std::ptr::null());
            meta.method_list = unsafe { convert_method_list(meta_raw_methods) };

            // Initialize metaclass cache
            let meta_cache = NonNull::from(Box::leak(MethodCache::new()));
            meta.set_cache(Some(meta_cache));

            // Root metaclass ISA: self-loop (metaclass.isa = metaclass)
            if meta.isa.is_none() {
                meta.isa = Some(meta_nn);
            }

            // Root metaclass super_class: points to the root class
            if meta.super_class.is_none() && cls.super_class.is_none() {
                meta.super_class = NonNull::new(cls_ptr);
            }
        }

        // --- Register class ---
        // SAFETY: cls_ptr points to a valid, fully patched ObjcClass.
        unsafe { class_registry::register_loaded_class(cls_ptr) };

        ptr = unsafe { ptr.add(1) };
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point called by every Clang-compiled ObjC module at program startup.
///
/// # Safety
/// `info` must be non-null and point to a valid `ObjcModuleInfo` emitted by
/// Clang.  The start/stop range pointers must bracket valid ELF section data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __objc_load(info: *const ObjcModuleInfo) {
    debug_assert!(!info.is_null(), "__objc_load called with null info");
    // SAFETY: Clang guarantees `info` is non-null and correctly laid out.
    let info = unsafe { &*info };

    // Selectors must be interned before classes (method lists reference them).
    unsafe { load_selectors(info.sel_start, info.sel_stop) };
    unsafe { load_classes(info.classes_start, info.classes_stop) };
}
