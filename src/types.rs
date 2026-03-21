use std::ffi::c_char;
use std::ptr::NonNull;

/// The base layout of every Objective-C object.
/// `isa` lives at offset 0 as required by the GNUstep v2 ABI.
#[repr(C)]
pub struct ObjcObject {
    pub isa: NonNull<ObjcClass>,
}

// SAFETY: The runtime owns all synchronization invariants for ObjC objects.
unsafe impl Send for ObjcObject {}
unsafe impl Sync for ObjcObject {}

/// A method implementation. Callers must cast to the actual signature before
/// invoking; the declared type matches the C ABI "opaque function pointer".
pub type IMP = unsafe extern "C" fn();

/// A selector — an interned `*const c_char`. Two selectors are equal iff their
/// pointer values are equal (guaranteed by the intern table in `sel.rs`).
pub type SEL = *const c_char;

/// An opaque object reference (`id` in Objective-C).
pub type Id = *mut ObjcObject;

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

/// The Objective-C class structure (broadly GNUstep v2 ABI-compatible).
///
/// `#[repr(C)]` ensures `isa` is at offset 0, so an `*mut ObjcClass` can
/// safely be cast to `*mut ObjcObject`.
#[repr(C)]
pub struct ObjcClass {
    /// The metaclass (isa of the class object). `None` only for the root
    /// metaclass, which is set during bootstrap (Phase 1: left null).
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
    /// Ivar list — null in Phase 1.
    pub ivars: *const (),
    /// Head of the method list chain for this class. `None` if no methods have
    /// been added yet; lazily initialized by `class_add_method`.
    pub method_list: Option<NonNull<MethodList>>,
    /// Method cache / dispatch table — null in Phase 1.
    pub dtable: *const (),
    /// Protocol list — null in Phase 1.
    pub protocols: *const (),
}

// SAFETY: The runtime owns all synchronization for class objects.
unsafe impl Send for ObjcClass {}
unsafe impl Sync for ObjcClass {}
