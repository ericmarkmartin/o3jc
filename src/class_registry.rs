use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::ptr::NonNull;
use std::sync::{LazyLock, RwLock};

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

    let metaclass = NonNull::from(Box::leak(Box::new(ObjcClass {
        isa: None, // root metaclass isa — set during bootstrap (Phase 1: leave null)
        super_class: meta_super,
        name: name_ptr,
        version: 0,
        info: class_flags::CLASS_IS_METACLASS,
        instance_size: std::mem::size_of::<ObjcClass>() as i64,
        ivars: std::ptr::null(),
        method_list: None,
        dtable: std::ptr::null(),
        protocols: std::ptr::null(),
    })));

    let class = NonNull::from(Box::leak(Box::new(ObjcClass {
        isa: Some(metaclass),
        super_class: NonNull::new(superclass),
        name: name_ptr,
        version: 0,
        info: 0,
        instance_size,
        ivars: std::ptr::null(),
        method_list: None,
        dtable: std::ptr::null(),
        protocols: std::ptr::null(),
    })));

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
/// # Safety
/// * `cls` must point to a valid, non-null `ObjcClass`.
/// * `sel` must be a properly interned selector.
/// * `imp` must be a valid function pointer with a signature compatible with `types`.
/// * `types` must be null or a valid null-terminated type-encoding C string.
pub unsafe fn class_add_method(cls: Class, sel: SEL, imp: IMP, types: *const c_char) -> bool {
    // SAFETY: caller guarantees `cls` is non-null and points to a valid ObjcClass,
    // and that no other thread is concurrently mutating it (pre-registration discipline).
    let cls_ref = unsafe { &mut *cls };
    let list = cls_ref.method_list.get_or_insert_with(MethodList::new);
    // SAFETY: `list` was either just created by `MethodList::new` (valid by construction)
    // or was previously inserted and has not been freed; no other reference exists here.
    let list = unsafe { list.as_mut() };
    if list.entries.iter().any(|entry| entry.sel == sel) {
        false
    } else {
        list.entries.push(MethodEntry { sel, types, imp });
        true
    }
}
