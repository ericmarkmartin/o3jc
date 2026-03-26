use std::ptr::NonNull;

use crate::sel::sel_eq;
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
    .find(|entry| sel_eq(entry.sel, sel))
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
/// **Fast path**: checks the per-class `MethodCache` first (one read-lock
/// acquisition + hash lookup). On a miss the slow path walks the class
/// hierarchy, then fills the cache before returning.
///
/// Returns `None` if `receiver` is null or the method is not found anywhere
/// in the class hierarchy. (Dynamic resolution and forwarding are Phase 4.)
///
/// # Safety
/// `receiver` must be null or point to a live `ObjcObject`.
pub unsafe fn objc_msg_lookup(receiver: Id, sel: SEL) -> Option<IMP> {
    let receiver = receiver?;
    // SAFETY: caller guarantees `receiver` is non-null and points to a live ObjcObject;
    // NonNull::new above confirmed non-null, so dereferencing is valid.
    let cls = unsafe { receiver.as_ref().isa };

    // --- Fast path: check the per-class cache ---
    // SAFETY: `cls` came from a live ObjcObject's `isa`, which is always set to a
    // valid ObjcClass. The `cache` field is set in `objc_allocate_class_pair` and
    // never mutated after construction.
    let cache = unsafe { cls.as_ref().cache() };
    if let Some(cache) = cache {
        // SAFETY: cache was allocated by `MethodCache::new` in `objc_allocate_class_pair`.
        if let Some(imp) = unsafe { cache.as_ref().lookup(sel) } {
            return Some(imp);
        }
    }

    // --- Slow path: walk the hierarchy ---
    // SAFETY: `cls` is always a valid ObjcClass (see above).
    let imp = unsafe { class_lookup_method(Some(cls), sel) }?;

    // Fill the cache so the next call takes the fast path.
    if let Some(cache) = cache {
        // SAFETY: same as above.
        unsafe { cache.as_ref().insert(sel, imp) };
    }

    Some(imp)
}

/// Non-nullable IMP lookup — aborts on method-not-found.
///
/// Used by the `objc_msgSend` trampoline which needs a raw function pointer
/// (not `Option<IMP>`) to tail-call.
///
/// # Safety
/// `receiver` must be non-null and point to a live `ObjcObject`.
/// `sel` must be a valid interned selector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objc_msg_lookup_nonnull(receiver: Id, sel: SEL) -> IMP {
    // SAFETY: caller guarantees receiver is non-null and live.
    unsafe { objc_msg_lookup(receiver, sel) }
        .expect("objc_msgSend: unrecognized selector sent to instance")
}

/// GNUstep v2 message-send trampoline (x86_64 SysV ABI).
///
/// Clang emits `objc_msgSend(receiver, sel, ...)` for every ObjC message send.
/// This naked function saves all argument registers, calls `objc_msg_lookup_nonnull`
/// to resolve the IMP, restores registers, and tail-calls the IMP so that the
/// callee sees the original arguments.
///
/// Nil receiver returns 0 (NULL) without dispatching.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn objc_msgSend() {
    std::arch::naked_asm!(
        // Nil check: receiver is in rdi
        "test rdi, rdi",
        "jz 2f",
        // Save all argument-passing registers (SysV ABI)
        // 6 GPRs + 8 SSE regs = 6*8 + 8*16 = 176 bytes
        // Align to 16: 176 + 8 (return addr already pushed) = 184; need 8 more
        "sub rsp, 0xb8",        // 184 bytes
        "mov [rsp+0x00], rdi",
        "mov [rsp+0x08], rsi",
        "mov [rsp+0x10], rdx",
        "mov [rsp+0x18], rcx",
        "mov [rsp+0x20], r8",
        "mov [rsp+0x28], r9",
        "movaps [rsp+0x30], xmm0",
        "movaps [rsp+0x40], xmm1",
        "movaps [rsp+0x50], xmm2",
        "movaps [rsp+0x60], xmm3",
        "movaps [rsp+0x70], xmm4",
        "movaps [rsp+0x80], xmm5",
        "movaps [rsp+0x90], xmm6",
        "movaps [rsp+0xa0], xmm7",
        // Call objc_msg_lookup_nonnull(receiver=rdi, sel=rsi)
        // rdi and rsi are already set correctly.
        "call {lookup}",
        // rax = IMP
        "mov r11, rax",
        // Restore all argument-passing registers
        "mov rdi, [rsp+0x00]",
        "mov rsi, [rsp+0x08]",
        "mov rdx, [rsp+0x10]",
        "mov rcx, [rsp+0x18]",
        "mov r8,  [rsp+0x20]",
        "mov r9,  [rsp+0x28]",
        "movaps xmm0, [rsp+0x30]",
        "movaps xmm1, [rsp+0x40]",
        "movaps xmm2, [rsp+0x50]",
        "movaps xmm3, [rsp+0x60]",
        "movaps xmm4, [rsp+0x70]",
        "movaps xmm5, [rsp+0x80]",
        "movaps xmm6, [rsp+0x90]",
        "movaps xmm7, [rsp+0xa0]",
        "add rsp, 0xb8",
        // Tail-call the IMP
        "jmp r11",
        // Nil receiver: return 0
        "2:",
        "xor eax, eax",
        "xor edx, edx",        // struct returns may use rdx
        "pxor xmm0, xmm0",     // float return
        "pxor xmm1, xmm1",
        "ret",
        lookup = sym objc_msg_lookup_nonnull,
    );
}
