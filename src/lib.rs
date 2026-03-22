//! **o3jc** — an Objective-C runtime implemented in Rust.
//!
//! Phase 3 adds: retain/release side tables, autorelease pools, and weak references.

pub mod autorelease;
pub mod class_registry;
pub mod method_cache;
pub mod msg_send;
pub mod retain_release;
pub mod sel;
pub mod types;

use std::ptr::NonNull;

pub use class_registry::{
    class_add_method, class_get_instance_method, class_replace_method,
    method_exchange_implementations, objc_allocate_class_pair, objc_get_class_str,
    objc_register_class_pair,
};
pub use method_cache::MethodCache;
pub use msg_send::class_lookup_method;
pub use sel::{sel_get_name, sel_register_name_str};
pub use types::*;

// ---------------------------------------------------------------------------
// C ABI surface (`#[unsafe(no_mangle)]` — matches GNUstep / <objc/runtime.h>)
// ---------------------------------------------------------------------------

/// Intern `name` and return its unique selector.
///
/// # Safety
/// `name` must be a valid, non-null, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sel_registerName(name: *const std::ffi::c_char) -> SEL {
    // SAFETY: caller (C code) guarantees `name` is a valid null-terminated C string.
    unsafe { sel::sel_register_name_cstr(name) }
}

/// Return the null-terminated name string of a selector.
///
/// # Safety
/// `sel` must be a valid interned selector returned by `sel_registerName`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sel_getName(sel: SEL) -> *const std::ffi::c_char {
    sel::sel_get_name(sel)
}

/// Allocate a new (unregistered) class+metaclass pair.
///
/// # Safety
/// `superclass` must be null or point to a live registered `ObjcClass`.
/// `name` must be a valid, non-null, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_allocateClassPair(
    superclass: Class,
    name: *const std::ffi::c_char,
    extra_bytes: usize,
) -> Class {
    // SAFETY: caller (C code) guarantees `superclass` is null or a valid registered
    // ObjcClass, and `name` is a valid null-terminated C string.
    unsafe { class_registry::objc_allocate_class_pair(superclass, name, extra_bytes) }
}

/// Register an allocated class pair into the live class table.
///
/// # Safety
/// `cls` must have been returned by `objc_allocateClassPair`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_registerClassPair(cls: Class) {
    // SAFETY: caller (C code) guarantees `cls` was returned by `objc_allocateClassPair`.
    unsafe { class_registry::objc_register_class_pair(cls) }
}

/// Look up a registered class by C-string name.
///
/// # Safety
/// `name` must be a valid, non-null, null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_getClass(name: *const std::ffi::c_char) -> Class {
    // SAFETY: caller (C code) guarantees `name` is a valid null-terminated C string.
    let s = unsafe { std::ffi::CStr::from_ptr(name) }
        .to_str()
        .unwrap_or("");
    class_registry::objc_get_class_str(s)
}

/// Add a method to a class.
///
/// # Safety
/// `cls` must be a valid non-null `ObjcClass`. `sel` must be an interned selector.
/// `imp` must be a valid function pointer compatible with `types`. `types` must be
/// null or a valid null-terminated type-encoding string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn class_addMethod(
    cls: Class,
    sel: SEL,
    imp: IMP,
    types: *const std::ffi::c_char,
) -> bool {
    // SAFETY: forwarding caller's guarantees.
    unsafe { class_registry::class_add_method(cls, sel, imp, types) }
}

/// Replace a method's implementation, or add it if absent.
///
/// Returns the previous IMP (as a nullable function pointer at the C ABI level),
/// or null if the method did not previously exist.
///
/// # Safety
/// Same requirements as `class_addMethod`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn class_replaceMethod(
    cls: Class,
    sel: SEL,
    imp: IMP,
    types: *const std::ffi::c_char,
) -> Option<IMP> {
    // SAFETY: forwarding caller's guarantees.
    unsafe { class_registry::class_replace_method(cls, sel, imp, types) }
}

/// Return the `Method` (a pointer to the `MethodEntry`) for `sel` in `cls`.
///
/// Only searches `cls` itself, not its superclasses. Returns null if not found.
///
/// # Safety
/// `cls` must be null or point to a live `ObjcClass`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn class_getInstanceMethod(cls: Class, sel: SEL) -> *mut MethodEntry {
    // SAFETY: forwarding caller's guarantees.
    unsafe { class_registry::class_get_instance_method(cls, sel) }
}

/// Return the IMP stored in a `Method`.
///
/// # Safety
/// `method` must be non-null and point to a live `MethodEntry`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn method_getImplementation(method: *mut MethodEntry) -> IMP {
    // SAFETY: caller guarantees `method` is non-null and valid.
    unsafe { (*method).imp }
}

/// Atomically swap the implementations of two methods and flush all caches.
///
/// # Safety
/// Both `m1` and `m2` must be non-null pointers to live `MethodEntry` values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn method_exchangeImplementations(
    m1: *mut MethodEntry,
    m2: *mut MethodEntry,
) {
    // SAFETY: forwarding caller's guarantees.
    unsafe { class_registry::method_exchange_implementations(m1, m2) }
}

/// GNUstep-style IMP lookup.
///
/// Returns `Some(imp)` on hit, or `None` (null function pointer at the C ABI
/// level) if the receiver is null or no implementation is found.
/// `Option<IMP>` is guaranteed by Rust to have the same layout as a nullable
/// function pointer, so C callers see either a valid IMP or null.
///
/// (Dynamic resolution and forwarding are added in a later phase.)
///
/// # Safety
/// `receiver` must be null or point to a live `ObjcObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_msg_lookup(receiver: Id, sel: SEL) -> Option<IMP> {
    // SAFETY: caller (C code) guarantees `receiver` is null or a valid live ObjcObject.
    unsafe { msg_send::objc_msg_lookup(receiver, sel) }
}

// ---------------------------------------------------------------------------
// C ABI surface — retain / release / autorelease (Phase 3)
// ---------------------------------------------------------------------------

/// Increment `obj`'s retain count and return it.
///
/// # Safety
/// `obj` must be null or point to a live `ObjcObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_retain(obj: Id) -> Id {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_retain(obj) }
}

/// Decrement `obj`'s retain count; deallocate if it reaches zero.
///
/// # Safety
/// `obj` must be null or point to a live `ObjcObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_release(obj: Id) {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_release(obj) }
}

/// Add `obj` to the current autorelease pool and return it.
///
/// # Safety
/// `obj` must be null or point to a live `ObjcObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_autorelease(obj: Id) -> Id {
    // SAFETY: forwarding caller's guarantees.
    unsafe { autorelease::objc_autorelease(obj) }
}

/// Push a new autorelease pool; returns an opaque token for the matching pop.
#[unsafe(no_mangle)]
pub extern "C" fn objc_autoreleasePoolPush() -> *mut () {
    autorelease::objc_autorelease_pool_push()
}

/// Pop autorelease pools back to `token`, releasing all objects added since.
///
/// # Safety
/// `token` must be a value returned by `objc_autoreleasePoolPush` on the same
/// thread, not yet consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_autoreleasePoolPop(token: *mut ()) {
    // SAFETY: forwarding caller's guarantees.
    unsafe { autorelease::objc_autorelease_pool_pop(token) }
}

/// Initialise the weak-pointer location `*location` to `obj`.
///
/// # Safety
/// `location` must be a valid writable pointer. `obj` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_initWeak(location: NonNull<Id>, obj: Id) -> Id {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_init_weak(location, obj) }
}

/// Update the weak-pointer location `*location` to `new_obj`.
///
/// # Safety
/// `location` must have been initialised by `objc_initWeak`. `new_obj` must be
/// null or point to a live `ObjcObject`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_storeWeak(location: NonNull<Id>, new_obj: Id) -> Id {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_store_weak(location, new_obj) }
}

/// Load the weak pointer at `location`, returning a +1 retained reference or
/// null if the object has been deallocated. The caller must release the result.
///
/// # Safety
/// `location` must have been initialised by `objc_initWeak`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_loadWeakRetained(location: NonNull<Id>) -> Id {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_load_weak_retained(location) }
}

/// Load the weak pointer at `location` (autoreleased; null if deallocated).
///
/// # Safety
/// `location` must have been initialised by `objc_initWeak`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_loadWeak(location: NonNull<Id>) -> Id {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_load_weak(location) }
}

/// Unregister and clear the weak-pointer location `*location`.
///
/// # Safety
/// `location` must have been initialised by `objc_initWeak`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_destroyWeak(location: NonNull<Id>) {
    // SAFETY: forwarding caller's guarantees.
    unsafe { retain_release::objc_destroy_weak(location) }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use std::ptr::NonNull;

    use super::*;

    // -----------------------------------------------------------------------
    // Selector interning

    #[test]
    fn selector_interning_same_pointer() {
        let n1 = CString::new("testMethod").unwrap();
        let n2 = CString::new("testMethod").unwrap();
        unsafe {
            let s1 = sel_registerName(n1.as_ptr());
            let s2 = sel_registerName(n2.as_ptr());
            assert_eq!(s1, s2, "identical names must intern to the same pointer");
        }
    }

    #[test]
    fn selector_interning_different_pointers() {
        let a = CString::new("alpha").unwrap();
        let b = CString::new("beta").unwrap();
        unsafe {
            let sa = sel_registerName(a.as_ptr());
            let sb = sel_registerName(b.as_ptr());
            assert_ne!(sa, sb, "different names must have different SELs");
        }
    }

    // -----------------------------------------------------------------------
    // Class allocation and registration

    #[test]
    fn class_allocate_and_find() {
        let name = CString::new("FindMeClass").unwrap();
        unsafe {
            let cls = objc_allocateClassPair(std::ptr::null_mut(), name.as_ptr(), 0);
            assert!(!cls.is_null());

            // Not findable before registration
            assert!(
                objc_getClass(name.as_ptr()).is_null(),
                "class must not be visible before registration"
            );

            objc_registerClassPair(cls);

            let found = objc_getClass(name.as_ptr());
            assert_eq!(found, cls, "registered class must be retrievable by name");
        }
    }

    // -----------------------------------------------------------------------
    // Direct method dispatch

    static DIRECT_CALLED: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn direct_impl() {
        DIRECT_CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn direct_method_dispatch() {
        let class_name = CString::new("DirectClass").unwrap();
        let sel_name = CString::new("directMethod").unwrap();
        let type_enc = CString::new("v16@0:8").unwrap();

        unsafe {
            let sel = sel_registerName(sel_name.as_ptr());
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);

            // SAFETY: `direct_impl` is an `unsafe extern "C" fn()`, which has the same
            // ABI and layout as IMP (`unsafe extern "C" fn()`); transmute is a no-op.
            let imp: IMP = std::mem::transmute(direct_impl as unsafe extern "C" fn());
            assert!(
                class_addMethod(cls, sel, imp, type_enc.as_ptr()),
                "first addMethod must succeed"
            );
            assert!(
                !class_addMethod(cls, sel, imp, type_enc.as_ptr()),
                "duplicate addMethod must return false"
            );

            objc_registerClassPair(cls);

            let mut obj = ObjcObject {
                isa: NonNull::new(cls).unwrap(),
            };
            let id: Id = Some(NonNull::from(&mut obj));

            let found = objc_msg_lookup(id, sel);
            assert!(found.is_some(), "IMP must be found for registered method");

            // SAFETY: we registered `direct_impl` as a `fn()` with no arguments;
            // the type encoding "v16@0:8" matches a void function, so this cast is valid.
            let f: unsafe extern "C" fn() = std::mem::transmute(found.unwrap());
            f();
            assert!(DIRECT_CALLED.load(Ordering::SeqCst));
        }
    }

    // -----------------------------------------------------------------------
    // Inherited method dispatch

    static INHERITED_CALLED: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn inherited_impl() {
        INHERITED_CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn inherited_method_dispatch() {
        let parent_name = CString::new("InheritParent").unwrap();
        let child_name = CString::new("InheritChild").unwrap();
        let sel_name = CString::new("inheritedMethod").unwrap();
        let type_enc = CString::new("v16@0:8").unwrap();

        unsafe {
            let sel = sel_registerName(sel_name.as_ptr());

            let parent = objc_allocateClassPair(std::ptr::null_mut(), parent_name.as_ptr(), 0);
            // SAFETY: same as direct_method_dispatch — `inherited_impl` is `extern "C" fn()`,
            // layout-identical to IMP.
            let imp: IMP = std::mem::transmute(inherited_impl as unsafe extern "C" fn());
            class_addMethod(parent, sel, imp, type_enc.as_ptr());
            objc_registerClassPair(parent);

            let child = objc_allocateClassPair(parent, child_name.as_ptr(), 0);
            objc_registerClassPair(child);

            let mut obj = ObjcObject {
                isa: NonNull::new(child).unwrap(),
            };
            let id: Id = Some(NonNull::from(&mut obj));

            let found = objc_msg_lookup(id, sel);
            assert!(
                found.is_some(),
                "inherited IMP must be found via superclass walk"
            );

            // SAFETY: `inherited_impl` was registered as `extern "C" fn()`.
            let f: unsafe extern "C" fn() = std::mem::transmute(found.unwrap());
            f();
            assert!(INHERITED_CALLED.load(Ordering::SeqCst));
        }
    }

    // -----------------------------------------------------------------------
    // Null receiver

    #[test]
    fn null_receiver_returns_null_imp() {
        let sel_name = CString::new("anyMethod").unwrap();
        unsafe {
            let sel = sel_registerName(sel_name.as_ptr());
            let imp = objc_msg_lookup(None, sel);
            assert!(imp.is_none(), "null receiver must yield null IMP");
        }
    }

    // -----------------------------------------------------------------------
    // Child overrides parent method

    static OVERRIDE_PARENT_CALLED: AtomicBool = AtomicBool::new(false);
    static OVERRIDE_CHILD_CALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn override_parent_impl() {
        OVERRIDE_PARENT_CALLED.store(true, Ordering::SeqCst);
    }
    unsafe extern "C" fn override_child_impl() {
        OVERRIDE_CHILD_CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn child_overrides_parent_method() {
        let parent_name = CString::new("OverrideParent").unwrap();
        let child_name = CString::new("OverrideChild").unwrap();
        let sel_name = CString::new("overriddenMethod").unwrap();
        let type_enc = CString::new("v16@0:8").unwrap();

        unsafe {
            let sel = sel_registerName(sel_name.as_ptr());

            let parent = objc_allocateClassPair(std::ptr::null_mut(), parent_name.as_ptr(), 0);
            // SAFETY: same as other tests — `extern "C" fn()` is layout-identical to IMP.
            let parent_imp: IMP =
                std::mem::transmute(override_parent_impl as unsafe extern "C" fn());
            class_addMethod(parent, sel, parent_imp, type_enc.as_ptr());
            objc_registerClassPair(parent);

            let child = objc_allocateClassPair(parent, child_name.as_ptr(), 0);
            let child_imp: IMP = std::mem::transmute(override_child_impl as unsafe extern "C" fn());
            class_addMethod(child, sel, child_imp, type_enc.as_ptr());
            objc_registerClassPair(child);

            let mut obj = ObjcObject {
                isa: NonNull::new(child).unwrap(),
            };
            let id: Id = Some(NonNull::from(&mut obj));

            let found = objc_msg_lookup(id, sel);
            assert!(found.is_some());

            // SAFETY: `override_child_impl` was registered as `extern "C" fn()`.
            let f: unsafe extern "C" fn() = std::mem::transmute(found.unwrap());
            f();

            assert!(
                OVERRIDE_CHILD_CALLED.load(Ordering::SeqCst),
                "child override must be called"
            );
            assert!(
                !OVERRIDE_PARENT_CALLED.load(Ordering::SeqCst),
                "parent impl must not be called when child overrides"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: cache hit after first dispatch

    static CACHE_IMPL_CALLED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn cache_impl() {
        CACHE_IMPL_CALLED.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn cache_hit_after_first_dispatch() {
        let class_name = CString::new("CacheTestClass").unwrap();
        let sel_name = CString::new("cachedMethod").unwrap();
        let type_enc = CString::new("v16@0:8").unwrap();

        unsafe {
            let sel = sel_registerName(sel_name.as_ptr());
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);
            let imp: IMP = std::mem::transmute(cache_impl as unsafe extern "C" fn());
            class_addMethod(cls, sel, imp, type_enc.as_ptr());
            objc_registerClassPair(cls);

            let mut obj = ObjcObject {
                isa: NonNull::new(cls).unwrap(),
            };
            let id: Id = Some(NonNull::from(&mut obj));

            // First lookup: slow path, fills cache.
            let found1 = objc_msg_lookup(id, sel);
            assert!(found1.is_some());

            // Second lookup: should hit the cache and return the same IMP.
            let found2 = objc_msg_lookup(id, sel);
            assert_eq!(
                found1.unwrap() as usize,
                found2.unwrap() as usize,
                "cached and uncached lookups must return the same IMP"
            );

            // Call it to verify it works.
            let f: unsafe extern "C" fn() = std::mem::transmute(found2.unwrap());
            f();
            assert_eq!(CACHE_IMPL_CALLED.load(Ordering::SeqCst), 1);
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: method_exchangeImplementations (swizzling)

    static SWIZZLE_A_CALLED: AtomicBool = AtomicBool::new(false);
    static SWIZZLE_B_CALLED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn swizzle_a() {
        SWIZZLE_A_CALLED.store(true, Ordering::SeqCst);
    }
    unsafe extern "C" fn swizzle_b() {
        SWIZZLE_B_CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn method_swizzle_works() {
        let class_name = CString::new("SwizzleClass").unwrap();
        let sel_a_name = CString::new("swizzleA").unwrap();
        let sel_b_name = CString::new("swizzleB").unwrap();
        let type_enc = CString::new("v16@0:8").unwrap();

        unsafe {
            let sel_a = sel_registerName(sel_a_name.as_ptr());
            let sel_b = sel_registerName(sel_b_name.as_ptr());
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);

            let imp_a: IMP = std::mem::transmute(swizzle_a as unsafe extern "C" fn());
            let imp_b: IMP = std::mem::transmute(swizzle_b as unsafe extern "C" fn());
            class_addMethod(cls, sel_a, imp_a, type_enc.as_ptr());
            class_addMethod(cls, sel_b, imp_b, type_enc.as_ptr());
            objc_registerClassPair(cls);

            let mut obj = ObjcObject {
                isa: NonNull::new(cls).unwrap(),
            };
            let id: Id = Some(NonNull::from(&mut obj));

            // Warm the cache for sel_a.
            let _ = objc_msg_lookup(id, sel_a);

            // Swap A ↔ B.
            let m_a = class_getInstanceMethod(cls, sel_a);
            let m_b = class_getInstanceMethod(cls, sel_b);
            assert!(!m_a.is_null() && !m_b.is_null());
            method_exchangeImplementations(m_a, m_b);

            // After swizzle, looking up sel_a should return imp_b (which calls swizzle_b).
            let found = objc_msg_lookup(id, sel_a);
            assert!(found.is_some());
            let f: unsafe extern "C" fn() = std::mem::transmute(found.unwrap());
            f();

            assert!(
                SWIZZLE_B_CALLED.load(Ordering::SeqCst),
                "swizzle_b must be called via sel_a after exchange"
            );
            assert!(
                !SWIZZLE_A_CALLED.load(Ordering::SeqCst),
                "swizzle_a must not be called via sel_a after exchange"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Phase 3: retain / release

    #[test]
    fn retain_release_count() {
        let class_name = CString::new("RetainCountClass").unwrap();
        unsafe {
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);
            objc_registerClassPair(cls);
            let mut obj = ObjcObject { isa: NonNull::new(cls).unwrap() };
            let id: Id = Some(NonNull::from(&mut obj));

            // Fresh object has implicit count of 1.
            assert_eq!(retain_release::objc_retain_count(id), 1);

            objc_retain(id);
            assert_eq!(retain_release::objc_retain_count(id), 2);

            objc_release(id);
            assert_eq!(retain_release::objc_retain_count(id), 1);

            // Release to zero — object is "deallocated" (side table cleaned up).
            // (Memory is not freed since we allocated on the stack.)
            objc_release(id);
            // Count is gone from table; accessing it again is UB in real code,
            // but for our stack-allocated test object it's safe.
        }
    }

    // -----------------------------------------------------------------------
    // Phase 3: dealloc method called on release-to-zero

    static DEALLOC_CALLED: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn dealloc_impl(_obj: Id, _sel: SEL) -> Id {
        DEALLOC_CALLED.store(true, Ordering::SeqCst);
        None
    }

    #[test]
    fn release_to_zero_calls_dealloc() {
        let class_name = CString::new("DeallocClass").unwrap();
        let sel_name = CString::new("dealloc").unwrap();

        unsafe {
            let dealloc_sel = sel_registerName(sel_name.as_ptr());
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);
            let imp: IMP = std::mem::transmute(dealloc_impl as unsafe extern "C" fn(Id, SEL) -> Id);
            class_addMethod(cls, dealloc_sel, imp, std::ptr::null());
            objc_registerClassPair(cls);

            let mut obj = ObjcObject { isa: NonNull::new(cls).unwrap() };
            let id: Id = Some(NonNull::from(&mut obj));

            objc_release(id);
            assert!(DEALLOC_CALLED.load(Ordering::SeqCst), "-dealloc must be called on release-to-zero");
        }
    }

    // -----------------------------------------------------------------------
    // Phase 3: autorelease pool

    static AUTORELEASE_RELEASED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn autorelease_dealloc_impl(_obj: Id, _sel: SEL) -> Id {
        AUTORELEASE_RELEASED.fetch_add(1, Ordering::SeqCst);
        None
    }

    #[test]
    fn autorelease_pool_releases_on_pop() {
        let class_name = CString::new("AutoreleaseClass").unwrap();
        let sel_name = CString::new("autoreleaseDealloc").unwrap();

        unsafe {
            let dealloc_sel = sel_registerName(CString::new("dealloc").unwrap().as_ptr());
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);
            let imp: IMP = std::mem::transmute(
                autorelease_dealloc_impl as unsafe extern "C" fn(Id, SEL) -> Id,
            );
            class_addMethod(cls, dealloc_sel, imp, std::ptr::null());
            objc_registerClassPair(cls);
            drop(sel_name);

            let mut obj1 = ObjcObject { isa: NonNull::new(cls).unwrap() };
            let mut obj2 = ObjcObject { isa: NonNull::new(cls).unwrap() };

            let token = objc_autoreleasePoolPush();
            objc_autorelease(Some(NonNull::from(&mut obj1)));
            objc_autorelease(Some(NonNull::from(&mut obj2)));

            assert_eq!(AUTORELEASE_RELEASED.load(Ordering::SeqCst), 0, "no releases before pop");
            objc_autoreleasePoolPop(token);
            assert_eq!(AUTORELEASE_RELEASED.load(Ordering::SeqCst), 2, "both objects released on pop");
        }
    }

    // -----------------------------------------------------------------------
    // Phase 3: weak references zeroed on dealloc

    #[test]
    fn weak_reference_zeroed_on_dealloc() {
        let class_name = CString::new("WeakTestClass").unwrap();
        unsafe {
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);
            objc_registerClassPair(cls);

            let mut obj = ObjcObject { isa: NonNull::new(cls).unwrap() };
            let id: Id = Some(NonNull::from(&mut obj));

            let mut weak_slot: Id = None;
            let weak_slot_ptr = NonNull::from(&mut weak_slot);
            objc_initWeak(weak_slot_ptr, id);

            // Load inside a pool so the autoreleased retain is balanced before
            // we call the final release.
            let token = objc_autoreleasePoolPush();
            assert_eq!(objc_loadWeak(weak_slot_ptr), id, "weak ref must point to live object");
            objc_autoreleasePoolPop(token);

            // Release to zero → dealloc → weak slot zeroed.
            objc_release(id);
            assert!(objc_loadWeak(weak_slot_ptr).is_none(), "weak ref must be nil after dealloc");
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: post-registration class_addMethod

    static POST_REG_CALLED: AtomicBool = AtomicBool::new(false);
    unsafe extern "C" fn post_reg_impl() {
        POST_REG_CALLED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn post_registration_add_method() {
        let class_name = CString::new("PostRegClass").unwrap();
        let sel_name = CString::new("postRegMethod").unwrap();
        let type_enc = CString::new("v16@0:8").unwrap();

        unsafe {
            let sel = sel_registerName(sel_name.as_ptr());
            let cls = objc_allocateClassPair(std::ptr::null_mut(), class_name.as_ptr(), 0);
            objc_registerClassPair(cls);

            // Method does not exist yet.
            let mut obj = ObjcObject {
                isa: NonNull::new(cls).unwrap(),
            };
            let id: Id = Some(NonNull::from(&mut obj));
            assert!(
                objc_msg_lookup(id, sel).is_none(),
                "method must not exist before post-registration add"
            );

            // Add post-registration.
            let imp: IMP = std::mem::transmute(post_reg_impl as unsafe extern "C" fn());
            let added = class_addMethod(cls, sel, imp, type_enc.as_ptr());
            assert!(added, "post-registration add must return true");

            // Now dispatch must find it.
            let found = objc_msg_lookup(id, sel);
            assert!(found.is_some(), "method must be found after post-registration add");
            let f: unsafe extern "C" fn() = std::mem::transmute(found.unwrap());
            f();
            assert!(POST_REG_CALLED.load(Ordering::SeqCst));
        }
    }
}
