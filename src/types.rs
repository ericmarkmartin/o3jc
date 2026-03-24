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

/// Opaque selector handle — corresponds to GNUstep's `struct objc_selector`.
///
/// cbindgen:opaque
///
/// Never constructed directly; exists only as a pointee for `SEL`. The
/// concrete representation (an interned C string address) is a runtime
/// implementation detail invisible to Clang-compiled code.
#[repr(C)]
pub struct ObjcSelector {
    _private: [u8; 0],
}

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
    fn clone(&self) -> Self { *self }
}
impl Copy for ObjcPtr {}

impl std::ops::Deref for ObjcPtr {
    type Target = NonNull<ObjcObject>;
    fn deref(&self) -> &NonNull<ObjcObject> { &self.0 }
}

impl From<NonNull<ObjcObject>> for ObjcPtr {
    fn from(ptr: NonNull<ObjcObject>) -> Self { Self(ptr) }
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
#[repr(C)]
pub struct MethodEntry {
    pub sel: SEL,
    /// Type-encoding string (e.g. `"v24@0:8"`), null-terminated.
    pub types: *const c_char,
    pub imp: IMP,
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
    /// The class has been registered and is live in the class table.
    pub const CLASS_REGISTERED: u64 = 1 << 0;
    /// This class object is a metaclass.
    pub const CLASS_IS_METACLASS: u64 = 1 << 1;
}

/// The runtime class object.
///
/// The field layout here will eventually need to match what Clang emits for
/// compiler-generated static classes (e.g. `@implementation MyClass`), but we
/// haven't yet pinned down the exact GNUstep v2 layout. For now this is
/// whatever the runtime needs internally.
#[repr(C)]
pub struct ObjcClass {
    /// The metaclass (`isa` of the class object). `None` only for the root
    /// metaclass (set during bootstrap).
    pub isa: Option<NonNull<ObjcClass>>,
    /// The superclass; `None` for the root class.
    pub super_class: Option<NonNull<ObjcClass>>,
    /// Null-terminated class name. Heap-allocated; owned by this struct.
    pub name: *const c_char,
    /// Class version (default 0).
    pub version: i64,
    /// Info flags (see `class_flags`).
    pub info: u64,
    /// Size of an instance in bytes.
    pub instance_size: i64,
    /// Ivar list — null until Phase 6.
    pub ivars: *const (),
    /// Head of the method list chain. `None` if no methods have been added yet.
    pub method_list: Option<NonNull<MethodList>>,
    /// GNUstep dispatch table pointer. Null until we implement the dtable mechanism.
    pub dtable: *const (),
    /// Protocol list — null until Phase 5.
    pub protocols: *const (),
    /// Head of the direct-subclass linked list, threaded through `next_sibling`.
    /// Used to propagate cache invalidation down the hierarchy.
    pub first_subclass: Option<NonNull<ObjcClass>>,
    /// Next sibling in the parent's subclass list (`None` = end of list).
    pub next_sibling: Option<NonNull<ObjcClass>>,
    /// Per-class method cache. `None` until `objc_allocate_class_pair` initialises it.
    pub cache: Option<NonNull<crate::method_cache::MethodCache>>,
}

// SAFETY: The runtime owns all synchronization for class objects.
unsafe impl Send for ObjcClass {}
unsafe impl Sync for ObjcClass {}
