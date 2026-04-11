use std::ffi::c_char;
use std::ptr::NonNull;

/// The base layout of every Objective-C object.
/// `isa` lives at offset 0 as required by the GNUstep v2 ABI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ObjcObject {
    pub isa: NonNull<ObjcClass>,
}

// SAFETY: The runtime owns all synchronization invariants for ObjC objects.
// Access is serialized through the side table's DashMap shard locks.
unsafe impl Send for ObjcObject {}
unsafe impl Sync for ObjcObject {}

/// Selector descriptor — corresponds to GNUstep's `struct objc_selector`.
///
/// In the GNUstep v2 ABI, selectors are pointers to `{ name, types }` pairs.
/// The `name` field points to an interned C string (guaranteed unique per
/// selector name by the intern table). Two selectors are equal iff their
/// `name` pointers are equal.
///
/// Compiled selectors (emitted by Clang into `__objc_selectors`) start with
/// uninterned name pointers; the loader fixes them up at load time.
#[repr(C)]
pub struct ObjcSelector {
    /// Interned selector name (stable, process-lifetime pointer). Always
    /// non-null after construction or loader fixup.
    pub(crate) name: NonNull<c_char>,
    /// Type encoding string, or `None` if untyped (e.g. from `sel_registerName`).
    /// `Option<NonNull<c_char>>` is ABI-compatible with a nullable `*const c_char`.
    pub(crate) types: Option<NonNull<c_char>>,
}

impl ObjcSelector {
    /// The interned selector name as a `&CStr`.
    ///
    /// # Safety
    /// The `name` pointer must point to a valid, null-terminated C string
    /// that lives for at least as long as the returned reference.
    pub unsafe fn name(&self) -> &std::ffi::CStr {
        unsafe { std::ffi::CStr::from_ptr(self.name.as_ptr()) }
    }

    /// The type encoding string, or `None` if this is an untyped selector.
    ///
    /// # Safety
    /// The `types` pointer (if present) must point to a valid, null-terminated
    /// C string that lives for at least as long as the returned reference.
    pub unsafe fn types(&self) -> Option<&std::ffi::CStr> {
        self.types
            .map(|ptr| unsafe { std::ffi::CStr::from_ptr(ptr.as_ptr()) })
    }
}

// SAFETY: ObjcSelector fields are immutable after construction (or after
// loader fixup). The name pointer is process-lifetime stable.
unsafe impl Send for ObjcSelector {}
unsafe impl Sync for ObjcSelector {}

/// A selector — a non-null pointer to an interned `ObjcSelector`.
///
/// Two selectors are equal iff their pointer values are equal (guaranteed by
/// the intern table in `sel.rs`). Nullable selectors at call boundaries are
/// expressed as `Option<SEL>`.
pub type SEL = NonNull<ObjcSelector>;

/// A non-null, `Send + Sync` pointer to an `ObjcObject`.
///
/// Needed because `NonNull<T>` is unconditionally `!Send + !Sync`, but
/// `ShardedMutex<Id>` requires `Id: Send` for its `Sync` impl.
/// `#[repr(transparent)]` preserves the niche so `Option<ObjcPtr>` is
/// ABI-compatible with a nullable `*mut ObjcObject`.
#[derive(Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ObjcPtr(NonNull<ObjcObject>);

impl Clone for ObjcPtr {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for ObjcPtr {}

impl std::ops::Deref for ObjcPtr {
    type Target = NonNull<ObjcObject>;
    fn deref(&self) -> &NonNull<ObjcObject> {
        &self.0
    }
}

impl From<NonNull<ObjcObject>> for ObjcPtr {
    fn from(ptr: NonNull<ObjcObject>) -> Self {
        Self(ptr)
    }
}

// SAFETY: The runtime serializes all access to ObjC objects through
// side-table locks and stripe locks.
unsafe impl Send for ObjcPtr {}
unsafe impl Sync for ObjcPtr {}

/// An opaque object reference (`id` in Objective-C).
///
/// `None` is the equivalent of Objective-C `nil`. `ObjcPtr` is
/// `#[repr(transparent)]` around `NonNull<ObjcObject>`, so `Option<ObjcPtr>`
/// has the null-pointer niche and is ABI-compatible with `*mut ObjcObject`.
///
/// cbindgen can't resolve custom `#[repr(transparent)]` wrappers inside
/// `Option`, so the C header defines `Id` manually via `cbindgen.toml`.
pub type Id = Option<ObjcPtr>;

/// A method implementation.
///
/// Matches the GNUstep ABI signature `id (*IMP)(id, SEL, ...)`. Callers must
/// transmute to the actual parameter types before invoking.
pub type IMP = unsafe extern "C" fn(Id, SEL, ...) -> Id;

/// A class pointer (`Class` in Objective-C).
pub type Class = *mut ObjcClass;

/// A single method descriptor stored in a method list.
///
/// Field order matches Clang's GNUstep v2 ABI: `{ IMP, SEL, types }`.
#[repr(C)]
pub struct MethodEntry {
    pub imp: IMP,
    pub sel: SEL,
    /// Type-encoding string (e.g. `"v24@0:8"`), null-terminated.
    pub types: *const c_char,
}

// SAFETY: MethodEntry fields are only mutated under class-write locks.
unsafe impl Send for MethodEntry {}
unsafe impl Sync for MethodEntry {}

/// A node in the linked chain of method lists attached to a class.
///
/// The `next` pointer lets categories prepend lists without copying.
/// Phase 1: one list per class; category chaining added in Phase 5.
#[repr(C)]
pub struct MethodList {
    /// Next list in the chain (`None` = end of chain).
    pub next: Option<NonNull<MethodList>>,
    pub entries: Vec<MethodEntry>,
}

// SAFETY: MethodList is only mutated while holding the class-write lock.
unsafe impl Send for MethodList {}
unsafe impl Sync for MethodList {}

impl MethodList {
    /// Allocate a new empty `MethodList` on the heap and return its raw pointer.
    /// The caller takes ownership; drop via `Box::from_raw`.
    pub fn new() -> NonNull<Self> {
        NonNull::from(Box::leak(Box::new(MethodList {
            next: None,
            entries: Vec::new(),
        })))
    }
}

/// Class info flag bits.
pub mod class_flags {
    /// This class object is a metaclass.
    /// Bit 0, matching Clang's GNUstep v2 codegen (`info = 1` for metaclass).
    pub const CLASS_IS_METACLASS: u64 = 1 << 0;
    /// The class has been registered and is live in the class table.
    /// Runtime-internal flag at a high bit to avoid conflict with ABI flags.
    pub const CLASS_REGISTERED: u64 = 1 << 16;
}

/// The runtime class object — GNUstep v2 ABI layout (17 fields).
///
/// Field order matches exactly what Clang emits in `CGObjCGNU.cpp` for
/// `-fobjc-runtime=gnustep-2.0`. This allows compiled class structs from
/// `__objc_classes` to be used in-place without reallocation.
///
/// The method cache is stored in the `dtable` field (ABI field #9), which
/// Clang always emits as null. Accessor methods abstract the cast.
#[repr(C)]
pub struct ObjcClass {
    // --- 17 ABI fields (must match Clang's CGObjCGNU.cpp exactly) ---
    /// 1. The metaclass (`isa` of the class object). `None` only for the root
    ///    metaclass before bootstrap.
    pub isa: Option<NonNull<ObjcClass>>,
    /// 2. The superclass; `None` for the root class.
    pub super_class: Option<NonNull<ObjcClass>>,
    /// 3. Null-terminated class name.
    pub name: *const c_char,
    /// 4. Class version (default 0).
    pub version: i64,
    /// 5. Info flags (see `class_flags`).
    pub info: u64,
    /// 6. Size of an instance in bytes. Clang emits negative values for classes
    ///    with ivars; the loader patches these at load time.
    pub instance_size: i64,
    /// 7. Ivar list — null until ivar support is implemented.
    pub ivars: *const (),
    /// 8. Head of the method list chain. `None` if no methods have been added.
    pub method_list: Option<NonNull<MethodList>>,
    /// 9. Dispatch table pointer — repurposed to hold `*mut MethodCache`.
    ///    Use `cache()` / `set_cache()` accessors instead of accessing directly.
    pub dtable: *const (),
    /// 10. C++ constructor function — null for plain ObjC classes.
    pub cxx_construct: *const (),
    /// 11. C++ destructor function — null for plain ObjC classes.
    pub cxx_destruct: *const (),
    /// 12. Head of the direct-subclass linked list, threaded through `sibling_class`.
    pub subclass_list: Option<NonNull<ObjcClass>>,
    /// 13. Next sibling in the parent's subclass list (`None` = end of list).
    pub sibling_class: Option<NonNull<ObjcClass>>,
    /// 14. Protocol conformance list — null until protocol support.
    pub protocols: *const (),
    /// 15. Extra reference data — null (reserved for future use).
    pub extra_data: *const (),
    /// 16. ABI version number.
    pub abi_version: i64,
    /// 17. Property metadata list — null until property support.
    pub properties: *const (),
}

impl ObjcClass {
    /// Read the per-class method cache stored in the `dtable` field.
    pub fn cache(&self) -> Option<NonNull<crate::method_cache::MethodCache>> {
        NonNull::new(self.dtable as *mut crate::method_cache::MethodCache)
    }

    /// Store a method cache pointer in the `dtable` field.
    pub fn set_cache(&mut self, cache: Option<NonNull<crate::method_cache::MethodCache>>) {
        self.dtable = cache.map_or(std::ptr::null(), |p| p.as_ptr() as *const ());
    }
}

// SAFETY: The runtime owns all synchronization for class objects.
unsafe impl Send for ObjcClass {}
unsafe impl Sync for ObjcClass {}

/// A read-only reference to a live `ObjcClass`.
///
/// Concentrates the pointer-validity unsafe at construction time so that
/// traversal of class hierarchies (superclass chain, subclass tree,
/// method-list chain) can be done with safe code.
///
/// # Invariant
/// The inner pointer is non-null and points to a valid `ObjcClass`.
/// In practice, classes are allocated via `Box::leak` or loaded from
/// compiled binaries and never freed, so the pointer is valid for the
/// process lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassRef(NonNull<ObjcClass>);

impl ClassRef {
    /// Wrap a raw `NonNull` class pointer.
    ///
    /// # Safety
    /// `ptr` must point to a valid, live `ObjcClass`.
    pub unsafe fn from_raw(ptr: NonNull<ObjcClass>) -> Self {
        Self(ptr)
    }

    /// Wrap a raw `*mut ObjcClass` pointer.
    ///
    /// # Safety
    /// `ptr` must be non-null and point to a valid, live `ObjcClass`.
    pub unsafe fn from_ptr(ptr: *mut ObjcClass) -> Self {
        unsafe { Self(NonNull::new_unchecked(ptr)) }
    }

    /// The raw mutable pointer.
    pub fn as_ptr(self) -> *mut ObjcClass {
        self.0.as_ptr()
    }

    /// The raw `NonNull` pointer.
    pub fn as_non_null(self) -> NonNull<ObjcClass> {
        self.0
    }

    /// The metaclass (isa).
    pub fn isa(self) -> Option<ClassRef> {
        // SAFETY: ClassRef invariant guarantees the pointer is valid.
        unsafe { self.0.as_ref().isa.map(ClassRef) }
    }

    /// The superclass, or `None` for root classes.
    pub fn superclass(self) -> Option<ClassRef> {
        // SAFETY: ClassRef invariant.
        unsafe { self.0.as_ref().super_class.map(ClassRef) }
    }

    /// The head of the method-list chain.
    pub fn method_list(self) -> Option<NonNull<MethodList>> {
        // SAFETY: ClassRef invariant.
        unsafe { self.0.as_ref().method_list }
    }

    /// The per-class method cache.
    pub fn cache(&self) -> Option<&crate::method_cache::MethodCache> {
        // SAFETY: ClassRef invariant; cache allocated via Box::leak (process-lifetime).
        unsafe { self.0.as_ref().cache().map(|p| p.as_ref()) }
    }

    /// Head of the direct-subclass linked list.
    pub fn subclass_list(self) -> Option<ClassRef> {
        // SAFETY: ClassRef invariant.
        unsafe { self.0.as_ref().subclass_list.map(ClassRef) }
    }

    /// Next sibling in the parent's subclass list.
    pub fn sibling_class(self) -> Option<ClassRef> {
        // SAFETY: ClassRef invariant.
        unsafe { self.0.as_ref().sibling_class.map(ClassRef) }
    }

    /// Iterator over the class hierarchy (self, superclass, superclass's super, …).
    pub fn ancestors(self) -> impl Iterator<Item = ClassRef> {
        std::iter::successors(Some(self), |cls| cls.superclass())
    }

    /// Iterator over direct subclasses.
    pub fn subclasses(self) -> impl Iterator<Item = ClassRef> {
        std::iter::successors(self.subclass_list(), |cls| cls.sibling_class())
    }
}

/// Iterate over a linked chain of `MethodList` nodes.
///
/// All method-list nodes are allocated via `Box::leak` and never freed,
/// so the returned references are valid for the process lifetime.
pub fn method_list_iter(
    head: Option<NonNull<MethodList>>,
) -> impl Iterator<Item = &'static MethodList> {
    std::iter::successors(head, |&ptr| {
        // SAFETY: all MethodList nodes are process-lifetime-stable (Box::leak).
        let list: &MethodList = unsafe { &*ptr.as_ptr() };
        list.next
    })
    .map(|ptr| -> &'static MethodList {
        // SAFETY: same guarantee.
        unsafe { &*ptr.as_ptr() }
    })
}
