use crate::sel::sel_eq;
use crate::types::*;

/// Slow-path method lookup: walk the class hierarchy from `cls` upward.
///
/// Returns the first `IMP` found, or `None` if no class in the chain
/// implements `sel`.
pub fn class_lookup_method(cls: ClassRef, sel: SEL) -> Option<IMP> {
    cls.ancestors().find_map(|cls| {
        method_list_iter(cls.method_list())
            .flat_map(|list| list.entries.iter())
            .find(|entry| sel_eq(entry.sel, sel))
            .map(|entry| entry.imp)
    })
}

/// Core message lookup: given a receiver and selector, return the `IMP` to call.
///
/// **Fast path**: checks the per-class `MethodCache` first (one read-lock
/// acquisition + hash lookup). On a miss the slow path walks the class
/// hierarchy, then fills the cache before returning.
///
/// Returns `None` if `receiver` is null or the method is not found anywhere
/// in the class hierarchy. (Dynamic resolution and forwarding are Phase 7.)
///
/// # Safety
/// `receiver` must be null or point to a live `ObjcObject`.
pub unsafe fn objc_msg_lookup(receiver: Id, sel: SEL) -> Option<IMP> {
    let receiver = receiver?;
    // SAFETY: caller guarantees `receiver` points to a live ObjcObject.
    let cls = unsafe { ClassRef::from_raw(receiver.as_ref().isa) };

    // --- Fast path: check the per-class cache ---
    if let Some(cache) = cls.cache()
        && let Some(imp) = cache.lookup(sel)
    {
        return Some(imp);
    }

    // --- Slow path: walk the hierarchy ---
    let imp = class_lookup_method(cls, sel)?;

    // Fill the cache so the next call takes the fast path.
    if let Some(cache) = cls.cache() {
        cache.insert(sel, imp);
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
