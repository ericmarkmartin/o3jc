use std::ptr::NonNull;

use crate::types::*;

/// Walk the method list chain of a single class looking for `sel`.
///
/// Searches `list → list.next → ...` (head of chain = highest priority,
/// as required for category override semantics).
///
/// # Safety
/// `list` and all `next` pointers reachable from it must be valid `MethodList`
/// references for the duration of the call.
unsafe fn search_method_lists(list: Option<NonNull<MethodList>>, sel: SEL) -> Option<IMP> {
    std::iter::successors(list, |&ptr| {
        // SAFETY: function's safety contract guarantees all `next` pointers reachable
        // from `list` are valid MethodList references.
        unsafe { ptr.as_ref().next }
    })
    .flat_map(|ptr| {
        // SAFETY: same guarantee — every node in the chain is a valid MethodList.
        unsafe { ptr.as_ref() }.entries.iter()
    })
    .find(|entry| entry.sel == sel)
    .map(|entry| entry.imp)
}

/// Slow-path method lookup: walk the class hierarchy from `cls` upward.
///
/// Returns the first `IMP` found, or `None` if no class in the chain
/// implements `sel`.
///
/// # Safety
/// `cls` and all `super_class` pointers reachable from it must be valid
/// `ObjcClass` references for the duration of the call.
pub unsafe fn class_lookup_method(cls: Option<NonNull<ObjcClass>>, sel: SEL) -> Option<IMP> {
    std::iter::successors(cls, |&ptr| {
        // SAFETY: function's safety contract guarantees all `super_class` pointers
        // reachable from `cls` are valid ObjcClass references.
        unsafe { ptr.as_ref().super_class }
    })
    .find_map(|ptr| {
        // SAFETY: same guarantee — every class in the hierarchy is a valid ObjcClass.
        unsafe { search_method_lists(ptr.as_ref().method_list, sel) }
    })
}

/// Core message lookup: given a receiver and selector, return the `IMP` to call.
///
/// Returns `None` if `receiver` is null or the method is not found anywhere
/// in the class hierarchy. (Dynamic resolution and forwarding are Phase 4.)
///
/// # Safety
/// `receiver` must be null or point to a live `ObjcObject`.
pub unsafe fn objc_msg_lookup(receiver: Id, sel: SEL) -> Option<IMP> {
    let receiver = NonNull::new(receiver)?;
    // SAFETY: caller guarantees `receiver` is non-null and points to a live ObjcObject;
    // NonNull::new above confirmed non-null, so dereferencing is valid.
    let cls = unsafe { receiver.as_ref().isa };
    // SAFETY: `cls` came from a live ObjcObject's `isa` field, which is always set to a
    // valid ObjcClass by `objc_allocate_class_pair` and never mutated after construction.
    unsafe { class_lookup_method(Some(cls), sel) }
}
