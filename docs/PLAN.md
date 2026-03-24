# o3jc: Objective-C Runtime in Rust — High-Level Plan

## Context

Build a GNUstep v2 ABI-compatible Objective-C runtime in Rust. Primary purpose is learning, with the long-term goal of executing real Clang-compiled ObjC binaries/libraries. GNUstep v2 ABI (as used by libobjc2) is the best target: well-documented, Linux-native, tractable, and compatible with Clang's ObjC codegen without being locked to Apple's platform.

---

## Locked-In Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| ABI target | GNUstep v2 | Well-documented, Clang-compatible, Linux-native |
| `id` representation | `NonNull<ObjcObject>` raw ptr | ObjC is a memory manager; safety invariants are runtime-owned |
| ISA encoding | Plain `*mut ObjcClass` (no bit packing) | Start simple; add non-pointer ISA later as optimization |
| Global table threading | `RwLock<HashMap>` for class registry, `DashMap` for selectors | Class reg written rarely; selector table is hot |
| `objc_msgSend` first pass | Pure Rust `extern "C"` | Correct semantics first; assembly fast-path later |

---

## Major Components

### Layer 1: Type System
- `ObjcObject { isa: *mut ObjcClass }` — base object struct, `#[repr(C)]`, `isa` at offset 0 always
- `ObjcClass` extends `ObjcObject` — class objects are objects too (`isa` → metaclass)
- Type aliases: `id = NonNull<ObjcObject>`, `SEL = *const c_char` (interned), `IMP = unsafe extern "C" fn()`, `Class = *mut ObjcClass`
- GNUstep v2 ABI requires exact field order and `#[repr(C)]` throughout

### Layer 2: Selector Intern Table
- Global map: string → unique stable pointer (the SEL)
- Uniqueness guarantee: `sel_a == sel_b` iff same method name (pointer equality after interning)
- DashMap for thread-safe concurrent access; `Box::leak` / bump alloc for `'static` string storage

### Layer 3: Class/Metaclass Object Model
- Every class has a paired metaclass; allocated together
- ISA chain: `instance.isa → Class`, `Class.isa → Metaclass`, `Metaclass.isa → RootMetaclass` (self-referential)
- `superclass` chains: metaclass supers mirror class supers, root metaclass super → root class
- `class_rw_t` / `class_ro_t` split: read-only base data (compiled), read-write live data (category additions, caches)

### Layer 4: Class Registry
- Global `RwLock<HashMap<str, *mut ObjcClass>>`
- `objc_allocateClassPair` → allocates class+metaclass pair, returns pointer
- `objc_registerClassPair` → inserts into live registry, freezes ivar layout
- Subclass tree (`first_subclass`/`next_sibling` pointers) for cache invalidation traversal

### Layer 5: Method Tables
- `MethodEntry { sel: SEL, types: *const c_char, imp: IMP }` + `MethodList` (inline array + flags)
- Type encoding strings (e.g., `"v24@0:8"`) describe return type, params, and frame layout
- Lists are sorted by SEL address → binary search possible
- `class_rw_t.methods` is a list-of-lists (base + categories prepended, index 0 = highest priority)

### Layer 6: Method Dispatch (`objc_msgSend`)
- **Fast path**: check per-class method cache (hash table of `(SEL, IMP)` buckets), tail-call IMP
- **Slow path**: walk `cls → superclass → ... → root → nil`, binary search each method list
- On hit: fill cache, call IMP; on miss after full walk: invoke dynamic resolution
- `objc_msgSendSuper`: same but starts hierarchy walk at explicit super_class
- The method cache: power-of-two bucket array, hash = `(sel >> 2) & mask`, rehash at 75% full
- Cache invalidation propagates through the `first_subclass` tree

### Layer 7: Dynamic Method Resolution + Forwarding
- **Stage 0 (Resolution)**: call `+resolveInstanceMethod:` / `+resolveClassMethod:` on miss; re-run lookup
- **Stage 1 (Fast forward)**: call `-forwardingTargetForSelector:` → redirect to another receiver
- **Stage 2 (Full forward)**: call `-methodSignatureForSelector:` → build `NSMethodSignature`
- **Stage 3**: call `-forwardInvocation:` with captured `NSInvocation`; extract return value
- Final fallback: `-doesNotRecognizeSelector:` → raises exception

### Layer 8: Memory Management
- **Retain/release**: side table per-object (global array of 8 `SideTable` buckets, keyed by `obj_ptr % 8`)
- Each `SideTable`: `parking_lot::Mutex` + `HashMap<usize, usize>` (ptr → retain count)
- **Autorelease pools**: thread-local page stack
- **Weak references**: `WeakTable` inside `SideTable`; on dealloc, zero all registered weak pointer locations

### Layer 9: Categories
- `category_t` descriptor: name, target class, instance methods, class methods, protocols, properties
- `attachCategories`: prepend method lists to `class_rw_t.methods`; flush caches

### Layer 10: Protocols
- `ProtocolEntry` struct (is itself an ObjC object)
- Conformance check: walk class hierarchy + protocol inheritance graph (DFS, cycle-safe)

### Layer 11: Introspection API (`<objc/runtime.h>`)
- All `class_*`, `method_*`, `ivar_*`, `object_*`, `sel_*` functions with `#[unsafe(no_mangle)] unsafe extern "C"`
- Method swizzling (`method_exchangeImplementations`) atomically swaps IMPs + flushes caches

### Layer 12: Exception Handling
- `objc_exception_throw` → `__cxa_throw` (Itanium ABI interop)
- *First pass*: implement as `abort()` — real EH in a later pass

### Layer 13: Blocks Runtime (optional)
- `Block_layout` struct: `isa`, flags, `invoke` fn ptr, descriptor, captured vars
- Three classes: `_NSConcreteStackBlock`, `_NSConcreteGlobalBlock`, `_NSConcreteMallocBlock`

---

## Phased Implementation

Phases are now organized around binary-linking milestones — each phase targets the
most minimal possible Clang-compiled `.m` file, then expands. Phases 1–3 are
complete (pure-Rust runtime foundation); phases 4 onward target real binaries.

### Completed

| Phase | Milestone | Key work |
|---|---|---|
| **1** ✅ | Rust-only: define a class, add methods, dispatch works | Types, selector intern table, class registry, slow-path `objc_msg_lookup` |
| **2** ✅ | Rust-only: hot dispatch path; method swizzling | Method cache, `class_addMethod` post-registration, `method_exchangeImplementations` |
| **3** ✅ | Rust-only: full object lifecycle; no leaks | Retain/release side table, autorelease pools, weak references |

### Upcoming (binary-linking milestones)

| Phase | Milestone | Key work |
|---|---|---|
| **4** | Link and run a `.m` with **no static classes** | Build `.so`; verify symbol surface against GNUstep ABI; test harness (Makefile + minimal `.m` using only runtime APIs dynamically) |
| **5** | Link and run a `.m` with **static classes** (no framework deps, explicit root) | Fix `ObjcClass` layout (17 fields, `cache` after ABI fields); fix `MethodEntry` layout (`IMP` first); ELF section loader (`__objc_classes`, `__objc_selectors`, `__objc_init_func`); selector fixup at load time; root class bootstrap; `instance_size` sign patch |
| **6** | Static class **hierarchies** | Superclass chain fixup from compiled data; method resolution through compiled hierarchy; proper metaclass ISA chain |
| **7** | **Dynamic resolution + forwarding** | `+resolveInstanceMethod:`, `-forwardingTargetForSelector:`, full forwarding pipeline, `doesNotRecognizeSelector:` |
| **8** | **Categories** | `__objc_cats` ELF section loader; `attachCategories` at load time; cache invalidation |
| **9** | **Protocols** | Protocol objects from `__objc_prots`; conformance checking; `@protocol` expressions |
| **10** | **ARC return-value optimizations** | `objc_retainAutoreleasedReturnValue`, `objc_autoreleaseReturnValue`, `objc_retainAutorelease`; thread-local magic for the fast path |
| **11** | **Introspection API + Associated Objects** | Complete `<objc/runtime.h>` surface; `objc_setAssociatedObject` / `objc_getAssociatedObject` |
| **12** | **Exception handling** | `objc_exception_throw` → `__cxa_throw`; Itanium personality function; `@try/@catch` works end-to-end |
| **13** | **Blocks runtime** | `Block_layout`; `_Block_copy` / `_Block_release`; stack → heap promotion |

---

## Module Structure

```
src/
  lib.rs                  — crate root, re-exports, #[unsafe(no_mangle)] C API
  types.rs                — ObjcObject, ObjcClass, SEL, IMP, id, Class (all #[repr(C)])
  sel.rs                  — selector intern table
  class_registry.rs       — global class table, allocate/register pair
  method_cache.rs         — cache_t: bucket table, fill, flush, invalidation
  msg_send.rs             — objc_msg_lookup: cache fast path + slow path
  retain_release.rs       — side table, retain, release, dealloc, weak references
  autorelease.rs          — AutoreleasePool page stack, push/pop
  loader.rs               — ELF section walker (__objc_classes, __objc_selectors, etc.)
  bootstrap.rs            — root class initialization
  dynamic_resolution.rs   — resolveInstanceMethod/resolveClassMethod
  forwarding.rs           — 3-stage forwarding pipeline
  category.rs             — category_t, attachCategories
  protocol.rs             — protocol_t, global table, conformance checking
  associated_objects.rs   — AssocTable, set/get/remove, cleanup on dealloc
  introspection.rs        — all public C API
  exceptions.rs           — objc_exception_throw, personality function
  blocks.rs               — Block_layout, _Block_copy, _Block_release
```

---

## Key External Dependencies

| Crate | Purpose |
|---|---|
| `parking_lot` | Fast Mutex/RwLock for side tables |
| `dashmap` | Concurrent HashMap for selector intern table |
| `libc` | `malloc`/`free` for C-ABI-compatible allocation |
| `std::sync::OnceLock` | Global singletons (root classes, selector table) |

---

## GNUstep v2 ABI Notes

- Struct layouts: `libobjc2/objc/runtime.h`, `objc-runtime-new.h`
- Class flag bits, `class_rw_t` layout, and method list flags must match exactly for Clang-compiled code to link
- The `objc2` crate on crates.io targets this ABI and is a useful reference implementation

## Verification

Each phase from 4 onward is verified by a Clang-compiled `.m` file. Compile with:

```
clang -fobjc-runtime=gnustep-2.0 -fobjc-arc foo.m -L. -lo3jc -o foo
```

The test `.m` for each phase is the simplest possible file that exercises that
phase's new capability. Earlier phases continue to pass as later ones are added.

---

## Design Explorations

Ideas that were considered and parked — useful context for future decisions.

| Document | Summary |
|---|---|
| [Arena-Allocated Class Objects](ideas/arena-class-objects.md) | Use an index handle instead of raw `*mut ObjcClass`; sound for pure-Rust but breaks Clang binary loading |
| [Arena-Allocated Selector Strings](ideas/arena-selectors.md) | Pack selector strings into bump-allocated pages instead of one heap allocation per selector; purely an implementation detail since `SEL` stays a pointer |
