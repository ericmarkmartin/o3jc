use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::ptr::NonNull;
use std::sync::{LazyLock, RwLock};

use crate::method_cache::{MethodCache, flush_class_cache_tree};
use crate::sel::sel_eq;
use crate::types::*;

/// Newtype that lets `*mut ObjcClass` live in a `RwLock`-guarded map.
///
/// Raw pointers are `!Send`, which would prevent `RwLock<HashMap<_, *mut ObjcClass>>`
/// from being `Sync` (required for a `static`). We assert `Send` manually here:
/// access to the pointer is always guarded by the `RwLock`, so cross-thread
/// transfer is safe.
struct SendClass(*mut ObjcClass);
unsafe impl Send for SendClass {}
unsafe impl Sync for SendClass {}

/// Global class registry: class name → class pointer.
static CLASS_REGISTRY: LazyLock<RwLock<HashMap<Box<str>, SendClass>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Allocate a new class+metaclass pair and return a pointer to the class.
///
/// The returned class is *not* yet in the registry; call
/// `objc_register_class_pair` after adding methods and ivars.
///
/// # Safety
/// * `superclass` must be null or point to a live, registered `ObjcClass`.
/// * `name` must be a valid, non-null, null-terminated C string.
pub unsafe fn objc_allocate_class_pair(
    superclass: Class,
    name: *const c_char,
    _extra_bytes: usize,
) -> Class {
    // SAFETY: caller guarantees `name` is a valid, non-null, null-terminated C string.
    let name_str = unsafe { CStr::from_ptr(name) }
        .to_str()
        .expect("class name must be valid UTF-8");

    // Leak a CString so the name pointer is valid for the process lifetime.
    let name_ptr: *const c_char = CString::new(name_str).unwrap().into_raw();

    let instance_size = if superclass.is_null() {
        std::mem::size_of::<ObjcObject>() as i64
    } else {
        // SAFETY: caller guarantees `superclass` is non-null and points to a live ObjcClass.
        unsafe { (*superclass).instance_size }
    };

    // Metaclass super: the superclass's metaclass (or None for the root).
    let meta_super: Option<NonNull<ObjcClass>> = if superclass.is_null() {
        None
    } else {
        // SAFETY: caller guarantees `superclass` is non-null and points to a live ObjcClass.
        unsafe { (*superclass).isa }
    };

    let cache_meta = NonNull::from(Box::leak(MethodCache::new()));
    let cache_cls = NonNull::from(Box::leak(MethodCache::new()));

    let mut metaclass_obj = ObjcClass {
        isa: None, // root metaclass isa — set during bootstrap
        super_class: meta_super,
        name: name_ptr,
        version: 0,
        info: class_flags::CLASS_IS_METACLASS,
        instance_size: std::mem::size_of::<ObjcClass>() as i64,
        ivars: std::ptr::null(),
        method_list: None,
        dtable: std::ptr::null(),
        cxx_construct: std::ptr::null(),
        cxx_destruct: std::ptr::null(),
        subclass_list: None,
        sibling_class: None,
        protocols: std::ptr::null(),
        extra_data: std::ptr::null(),
        abi_version: 0,
        properties: std::ptr::null(),
    };
    metaclass_obj.set_cache(Some(cache_meta));
    let metaclass = NonNull::from(Box::leak(Box::new(metaclass_obj)));

    let mut class_obj = ObjcClass {
        isa: Some(metaclass),
        super_class: NonNull::new(superclass),
        name: name_ptr,
        version: 0,
        info: 0,
        instance_size,
        ivars: std::ptr::null(),
        method_list: None,
        dtable: std::ptr::null(),
        cxx_construct: std::ptr::null(),
        cxx_destruct: std::ptr::null(),
        subclass_list: None,
        sibling_class: None,
        protocols: std::ptr::null(),
        extra_data: std::ptr::null(),
        abi_version: 0,
        properties: std::ptr::null(),
    };
    class_obj.set_cache(Some(cache_cls));
    let class = NonNull::from(Box::leak(Box::new(class_obj)));

    // Thread new class into the superclass's subclass_list / sibling_class list.
    if !superclass.is_null() {
        // SAFETY: caller guarantees `superclass` is a valid, live ObjcClass.
        let super_ref = unsafe { &mut *superclass };
        let old_first = super_ref.subclass_list;
        // SAFETY: `class` was just created above and is non-null.
        unsafe { class.as_ptr().as_mut().unwrap().sibling_class = old_first };
        super_ref.subclass_list = Some(class);
    }

    class.as_ptr()
}

/// Insert the class into the live registry.
///
/// After this call the class is visible to `objc_get_class` and can receive
/// messages. Ivar layout is frozen from this point on.
///
/// # Safety
/// `cls` must point to a valid class allocated with `objc_allocate_class_pair`.
pub unsafe fn objc_register_class_pair(cls: Class) {
    let name = {
        // SAFETY: caller guarantees `cls` points to a valid ObjcClass; `name` is a
        // process-lifetime C string set during `objc_allocate_class_pair`.
        unsafe { CStr::from_ptr((*cls).name) }
            .to_str()
            .expect("class name must be valid UTF-8")
    };
    let mut registry = CLASS_REGISTRY.write().unwrap();
    registry.insert(name.into(), SendClass(cls));
    // SAFETY: caller guarantees `cls` points to a valid ObjcClass; write lock is held
    // so no concurrent access to `info`.
    unsafe { (*cls).info |= class_flags::CLASS_REGISTERED };
}

/// Register a class that was loaded from a compiled binary (via `__objc_load`).
///
/// Unlike `objc_register_class_pair`, this does not wire subclass links
/// (compiled classes may have them pre-set by Clang) and takes a raw pointer
/// to a class that was not allocated by `objc_allocate_class_pair`.
///
/// # Safety
/// `cls` must be non-null and point to a valid, fully-patched `ObjcClass`.
pub unsafe fn register_loaded_class(cls: *mut ObjcClass) {
    let name = {
        // SAFETY: caller guarantees `cls` is valid; `name` is a Clang-emitted string.
        unsafe { CStr::from_ptr((*cls).name) }
            .to_str()
            .expect("class name must be valid UTF-8")
    };
    let mut registry = CLASS_REGISTRY.write().unwrap();
    registry.insert(name.into(), SendClass(cls));
    // SAFETY: caller guarantees `cls` is valid; write lock is held.
    unsafe { (*cls).info |= class_flags::CLASS_REGISTERED };
}

/// Look up a registered class by its Rust `&str` name.
///
/// Returns null if no class with that name is registered.
pub fn objc_get_class_str(name: &str) -> Class {
    let registry = CLASS_REGISTRY.read().unwrap();
    match registry.get(name) {
        Some(send_class) => send_class.0,
        None => std::ptr::null_mut(),
    }
}

/// Add a method to a class's method list.
///
/// Returns `true` if the method was added, `false` if a method with the same
/// selector already exists (matching Apple/GNUstep semantics).
///
/// Before registration: the method is appended to the class's single `MethodList`.
/// After registration: a new single-entry `MethodList` is prepended to the chain
/// (preserving the stability of all existing `MethodEntry` pointers), and the
/// class's cache is flushed.
///
/// # Safety
/// * `cls` must point to a valid, non-null `ObjcClass`.
/// * `sel` must be a properly interned selector.
/// * `imp` must be a valid function pointer with a signature compatible with `types`.
/// * `types` must be null or a valid null-terminated type-encoding C string.
pub unsafe fn class_add_method(cls: Class, sel: SEL, imp: IMP, types: *const c_char) -> bool {
    // SAFETY: caller guarantees `cls` is non-null and points to a valid ObjcClass.
    let cls_ref = unsafe { &mut *cls };

    // Reject duplicate selectors (walk the entire method list chain).
    if method_exists_in_chain(cls_ref.method_list, sel) {
        return false;
    }

    let is_registered = cls_ref.info & class_flags::CLASS_REGISTERED != 0;

    if is_registered {
        // Post-registration: prepend a new single-entry MethodList so that all
        // previously returned `*mut MethodEntry` pointers remain valid.
        let old_head = cls_ref.method_list;
        let new_list = NonNull::from(Box::leak(Box::new(MethodList {
            next: old_head,
            entries: vec![MethodEntry { sel, types, imp }],
        })));
        cls_ref.method_list = Some(new_list);

        // Flush the cache for this class and all subclasses.
        // SAFETY: `cls` is a valid, live ObjcClass (caller contract).
        flush_class_cache_tree(unsafe { ClassRef::from_ptr(cls) });
    } else {
        // Pre-registration: mutate in place (no concurrent access possible).
        let list = cls_ref.method_list.get_or_insert_with(MethodList::new);
        // SAFETY: `list` is valid (just created or pre-existing) and not aliased here.
        unsafe { list.as_mut() }
            .entries
            .push(MethodEntry { sel, types, imp });
    }

    true
}

/// Replace an existing method's IMP, or add it if not found.
///
/// Returns the old IMP if the method existed, or null if it was freshly added.
///
/// # Safety
/// Same requirements as `class_add_method`.
pub unsafe fn class_replace_method(
    cls: Class,
    sel: SEL,
    imp: IMP,
    types: *const c_char,
) -> Option<IMP> {
    // SAFETY: caller guarantees `cls` is non-null and valid.
    let cls_ref = unsafe { &mut *cls };
    if let Some(entry) = find_method_in_chain(cls_ref.method_list, sel).map(|p| unsafe { &mut *p })
    {
        let old = entry.imp;
        entry.imp = imp;
        // SAFETY: `cls` is valid.
        flush_class_cache_tree(unsafe { ClassRef::from_ptr(cls) });
        Some(old)
    } else {
        // SAFETY: forwarding our own safety requirements.
        unsafe { class_add_method(cls, sel, imp, types) };
        None
    }
}

/// Swap the implementations of two methods and flush all caches.
///
/// # Safety
/// `m1` and `m2` must be non-null pointers to live `MethodEntry` values
/// obtained from `class_get_instance_method`.
pub unsafe fn method_exchange_implementations(m1: *mut MethodEntry, m2: *mut MethodEntry) {
    // SAFETY: caller guarantees both pointers are valid and non-null.
    let old = unsafe { (*m1).imp };
    unsafe {
        (*m1).imp = (*m2).imp;
        (*m2).imp = old;
    }
    // We don't have back-pointers from MethodEntry to its class, so we flush
    // every registered class's cache. Swizzling is rare; correctness beats
    // performance here.
    flush_all_caches();
}

/// Return a pointer to the `MethodEntry` for `sel` in `cls`'s own method list
/// (does **not** walk the superclass chain).
///
/// Returns null if the class has no such method.
///
/// # Safety
/// `cls` must be non-null and point to a live `ObjcClass`.
pub unsafe fn class_get_instance_method(cls: Class, sel: SEL) -> *mut MethodEntry {
    if cls.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: caller guarantees `cls` is non-null and valid.
    let cls_ref = unsafe { ClassRef::from_ptr(cls) };
    find_method_in_chain(cls_ref.method_list(), sel).unwrap_or(std::ptr::null_mut())
}

/// Flush the caches of every class currently in the registry (and their subclasses).
pub fn flush_all_caches() {
    let registry = CLASS_REGISTRY.read().unwrap();
    for send_class in registry.values() {
        // SAFETY: registered class pointers are valid for the process lifetime.
        flush_class_cache_tree(unsafe { ClassRef::from_ptr(send_class.0) });
    }
}

// ---------------------------------------------------------------------------
// Private helpers

/// Return `true` if any node in the method list chain rooted at `head`
/// contains an entry with selector `sel`.
fn method_exists_in_chain(head: Option<NonNull<MethodList>>, sel: SEL) -> bool {
    method_list_iter(head)
        .flat_map(|list| list.entries.iter())
        .any(|e| sel_eq(e.sel, sel))
}

/// Return a raw pointer to the first `MethodEntry` with selector `sel` in the
/// method list chain rooted at `head`, or `None` if not found.
fn find_method_in_chain(head: Option<NonNull<MethodList>>, sel: SEL) -> Option<*mut MethodEntry> {
    method_list_iter(head).find_map(|list| {
        list.entries
            .iter()
            .find(|e| sel_eq(e.sel, sel))
            .map(|e| e as *const MethodEntry as *mut MethodEntry)
    })
}
