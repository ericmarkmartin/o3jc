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
pub struct CompiledSelector {
    name: *const c_char,
    types: *const c_char,
}

const _: () = {
    assert!(
        std::mem::size_of::<CompiledSelector>() == std::mem::size_of::<ObjcSelector>(),
        "CompiledSelector and ObjcSelector must have the same size"
    );
    assert!(
        std::mem::align_of::<CompiledSelector>() == std::mem::align_of::<ObjcSelector>(),
        "CompiledSelector and ObjcSelector must have the same alignment"
    );
};

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
/// All pointer pairs are start/stop bounds into ELF sections. Sections that
/// are not yet processed use `*const u8` as a placeholder type.
#[repr(C)]
pub struct ObjcModuleInfo {
    version: i64,

    // __objc_selectors — array of { *name, *types } structs
    sel_start: *mut CompiledSelector,
    sel_stop: *mut CompiledSelector,

    // __objc_classes — array of pointers to class structs
    classes_start: *const *mut ObjcClass,
    classes_stop: *const *mut ObjcClass,

    // __objc_class_refs (Phase 6+)
    class_refs_start: *const u8,
    class_refs_stop: *const u8,

    // __objc_cats (Phase 8)
    cats_start: *const u8,
    cats_stop: *const u8,

    // __objc_protocols (Phase 9)
    protocols_start: *const u8,
    protocols_stop: *const u8,

    // __objc_protocol_refs (Phase 9)
    protocol_refs_start: *const u8,
    protocol_refs_stop: *const u8,

    // __objc_class_aliases
    class_aliases_start: *const u8,
    class_aliases_stop: *const u8,

    // __objc_constant_string
    constant_strings_start: *const u8,
    constant_strings_stop: *const u8,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a slice from a start/stop pointer pair.
///
/// Returns an empty slice if `start >= stop` or either pointer is null.
///
/// # Safety
/// `start` and `stop` must bracket a valid, contiguous array of `T`.
unsafe fn section_slice<'a, T>(start: *const T, stop: *const T) -> &'a [T] {
    if start.is_null() || stop.is_null() || start >= stop {
        return &[];
    }
    let count = unsafe { stop.offset_from(start) } as usize;
    unsafe { std::slice::from_raw_parts(start, count) }
}

/// Mutable version of `section_slice`.
///
/// # Safety
/// Same as `section_slice`, plus the caller must ensure exclusive access.
unsafe fn section_slice_mut<'a, T>(start: *mut T, stop: *mut T) -> &'a mut [T] {
    if start.is_null() || stop.is_null() || start >= stop {
        return &mut [];
    }
    let count = unsafe { stop.offset_from(start) } as usize;
    unsafe { std::slice::from_raw_parts_mut(start, count) }
}

impl ObjcModuleInfo {
    /// Return the `__objc_selectors` section as a mutable slice.
    ///
    /// # Safety
    /// The `sel_start`/`sel_stop` pointers must bracket valid section data.
    unsafe fn selectors_mut(&mut self) -> &mut [CompiledSelector] {
        unsafe { section_slice_mut(self.sel_start, self.sel_stop) }
    }

    /// Return the `__objc_classes` section as a slice of class pointers.
    ///
    /// # Safety
    /// The `classes_start`/`classes_stop` pointers must bracket valid section data.
    unsafe fn classes(&self) -> &[*mut ObjcClass] {
        unsafe { section_slice(self.classes_start, self.classes_stop) }
    }
}

// ---------------------------------------------------------------------------
// Selector fixup
// ---------------------------------------------------------------------------

/// Intern each selector name in the slice.
///
/// After this function returns, every `CompiledSelector.name` has been
/// overwritten with the stable interned name pointer, making the struct
/// usable as an `ObjcSelector { name, types }` for dispatch.
fn load_selectors(selectors: &mut [CompiledSelector]) {
    for entry in selectors {
        if entry.name.is_null() {
            continue; // Skip null sentinels.
        }
        // SAFETY: Clang emits valid null-terminated name strings.
        let name_str = unsafe { CStr::from_ptr(entry.name) }
            .to_str()
            .expect("selector name must be valid UTF-8");
        let interned = intern_selector_name(name_str);
        // Overwrite the name pointer with the interned version.
        entry.name = interned.as_ptr();
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
unsafe fn convert_method_list(compiled: *const ()) -> Option<NonNull<MethodList>> {
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
    let entries_start =
        unsafe { (header as *const u8).add(std::mem::size_of::<CompiledMethodList>()) }
            as *const CompiledMethodEntry;

    // SAFETY: the compiled method list has `count` inline entries after the header.
    let compiled_entries = unsafe { std::slice::from_raw_parts(entries_start, count) };

    let entries = compiled_entries
        .iter()
        .map(|ce| {
            // The selector's `name` field was fixed up by load_selectors to hold
            // the interned name pointer. Cast the CompiledSelector* to SEL
            // (NonNull<ObjcSelector>) — the CompiledSelector and ObjcSelector
            // have identical layout: { *name, *types }.
            // SAFETY: ce.sel was written by Clang and is non-null for real methods.
            let sel = unsafe { NonNull::new_unchecked(ce.sel as *mut ObjcSelector) };
            MethodEntry {
                imp: ce.imp,
                sel,
                types: ce.types,
            }
        })
        .collect();

    Some(NonNull::from(Box::leak(Box::new(MethodList {
        next: None,
        entries,
    }))))
}

// ---------------------------------------------------------------------------
// Class loading
// ---------------------------------------------------------------------------

/// Patch a single class struct: fix instance_size, convert method list, init cache.
///
/// # Safety
/// `cls` must point to a valid Clang-emitted `ObjcClass`.
/// `load_selectors` must have been called first.
unsafe fn patch_class(cls: &mut ObjcClass) {
    if cls.instance_size < 0 {
        cls.instance_size = -cls.instance_size;
    }
    if cls.instance_size == 0 && cls.super_class.is_none() {
        cls.instance_size = std::mem::size_of::<ObjcObject>() as i64;
    }

    // Convert compiled method list to runtime format.
    let raw_methods = cls
        .method_list
        .map(|nn| nn.as_ptr() as *const ())
        .unwrap_or(std::ptr::null());
    cls.method_list = unsafe { convert_method_list(raw_methods) };

    // Initialize per-class method cache (stored in dtable).
    let cache = NonNull::from(Box::leak(MethodCache::new()));
    cls.set_cache(Some(cache));
}

/// Register each Clang-emitted class in the slice.
///
/// # Safety
/// Each non-null pointer in `class_ptrs` must point to a valid Clang-emitted
/// `ObjcClass`. `load_selectors` must have been called first.
unsafe fn load_classes(class_ptrs: &[*mut ObjcClass]) {
    for &cls_ptr in class_ptrs {
        if cls_ptr.is_null() {
            continue; // Skip null sentinels.
        }
        // SAFETY: cls_ptr is a non-null Clang-emitted class struct.
        let cls = unsafe { &mut *cls_ptr };

        unsafe { patch_class(cls) };

        // Process the metaclass (pointed to by isa).
        if let Some(meta_nn) = cls.isa {
            let meta = unsafe { meta_nn.as_ptr().as_mut().unwrap() };
            unsafe { patch_class(meta) };

            // Root metaclass ISA: self-loop (metaclass.isa = metaclass).
            if meta.isa.is_none() {
                meta.isa = Some(meta_nn);
            }

            // Root metaclass super_class → root class.
            if meta.super_class.is_none() && cls.super_class.is_none() {
                meta.super_class = NonNull::new(cls_ptr);
            }
        }

        // SAFETY: cls_ptr points to a valid, fully patched ObjcClass.
        unsafe { class_registry::register_loaded_class(cls_ptr) };
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
    let info = unsafe { &mut *info.cast_mut() };

    // Selectors must be interned before classes (method lists reference them).
    load_selectors(unsafe { info.selectors_mut() });
    unsafe { load_classes(info.classes()) };
}
