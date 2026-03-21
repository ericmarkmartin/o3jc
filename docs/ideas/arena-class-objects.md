# Idea: Arena-Allocated Class Objects with Index Handles

## The Idea

Instead of `Class = *mut ObjcClass` (a raw pointer), store all `ObjcClass` values in a
global arena (e.g. a `Vec` or slab allocator) and represent classes as typed index handles:

```rust
struct ClassId(u32);
```

`objc_allocateClassPair` would return a `ClassId`, `class_addMethod` would take one, and
the `isa` field in `ObjcObject` would store a `ClassId` rather than a raw pointer.

## Why It's Appealing

- No raw pointers leaking out of the registry — the arena owns all `ObjcClass` memory
- `ClassId` is `Send + Sync` trivially, no `SendClass` newtype needed
- Pre-registration discipline could be enforced via typestate on the handle
- Smaller `isa` field (u32 vs u64 pointer) saves 4 bytes per object instance

## Key Insight: Clang Doesn't Deref `isa`

In GNUstep v2 ABI, Clang-compiled code never directly reads the `isa` field. It calls:

```c
IMP imp = objc_msg_lookup(receiver, sel);
```

...passing the receiver opaquely. Only the runtime (us) dereferences `receiver->isa`
internally. This means `isa` could hold an arena index rather than a pointer, as long as
the runtime knows how to interpret it.

## Why We Punted

When loading Clang-compiled `.m` binaries, Clang emits *static* `ObjcClass` structs
(one per class defined in the translation unit) at linker-assigned addresses. The
`__objc_load` mechanism hands the runtime pointers to these statically-allocated structs.

Since we don't control the allocation of those class objects, they can't live in our
arena without copying them in and fixing up all `isa` pointers — a non-trivial
compatibility shim on every binary load.

The arena approach is viable for a pure-Rust object model where all classes are
registered at runtime (no Clang-emitted static structs). But since ABI compatibility
with Clang-compiled `.m` files is the project goal, the investment isn't justified.

## If Revisiting

- Would require changing `ObjcObject.isa` from `*mut ObjcClass` to `u32` — ABI-breaking
- Would need a shim in `__objc_load` to copy Clang-emitted static class structs into the
  arena and rewrite their `isa` fields
- Per-class lock on arena slots would make `unsafe impl Sync` genuinely sound
