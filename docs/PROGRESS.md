# o3jc: Implementation Progress

## Phase 1 ✅ — Complete

**Milestone:** Define a class, add methods, dispatch works (no cache)

### What was built

| File | Description |
|---|---|
| `src/types.rs` | Core `#[repr(C)]` types: `ObjcObject`, `ObjcClass`, `MethodEntry`, `MethodList`, type aliases (`SEL`, `IMP`, `Id`, `Class`), `class_flags` |
| `src/sel.rs` | Selector intern table: `LazyLock<DashMap<Box<str>, usize>>`, pointer equality guaranteed |
| `src/class_registry.rs` | `objc_allocate_class_pair`, `objc_register_class_pair`, `objc_get_class_str`, `class_add_method` |
| `src/msg_send.rs` | `class_lookup_method` (hierarchy walk via `std::iter::successors`), `objc_msg_lookup` (returns `Option<IMP>`) |
| `src/lib.rs` | Module declarations, `#[unsafe(no_mangle)]` C ABI surface, 7 unit tests |

### C ABI exports

```c
SEL      sel_registerName(const char *name);
char    *sel_getName(SEL sel);
Class    objc_allocateClassPair(Class superclass, const char *name, size_t extra);
void     objc_registerClassPair(Class cls);
Class    objc_getClass(const char *name);
bool     class_addMethod(Class cls, SEL sel, IMP imp, const char *types);
IMP      objc_msg_lookup(id receiver, SEL sel);   // GNUstep-style; nullable
```

### Tests passing (7/7)

- `selector_interning_same_pointer` — same name → same pointer
- `selector_interning_different_pointers` — different names → different pointers
- `class_allocate_and_find` — class invisible before registration, findable after
- `direct_method_dispatch` — IMP found and called; duplicate `addMethod` returns false
- `inherited_method_dispatch` — method found via superclass walk
- `null_receiver_returns_null_imp` — null receiver → `None`
- `child_overrides_parent_method` — child IMP wins; parent IMP not called

### Key implementation notes

- **Rust edition 2024**: `#[no_mangle]` must be written `#[unsafe(no_mangle)]`; explicit `unsafe {}` blocks required inside `unsafe fn`; all `unsafe` blocks have `// SAFETY:` justifications
- **Nullable IMP**: `objc_msg_lookup` returns `Option<IMP>` — Rust's niche optimization makes `Option<fn()>` layout-compatible with a nullable function pointer
- **Selector table**: `LazyLock<DashMap<Box<str>, usize>>` — `usize` stores the pointer address to satisfy `DashMap`'s `Send` bound; each selector string is leaked via `CString::into_raw` for `'static` stability
- **Class registry**: `LazyLock<RwLock<HashMap<Box<str>, SendClass>>>` — `SendClass(*mut ObjcClass)` newtype with `unsafe impl Send + Sync` allows the raw pointer to live in the map; the `RwLock` provides the actual synchronization
- **Nullable pointer fields**: `ObjcClass.isa`, `ObjcClass.super_class`, and `MethodList.next` are `Option<NonNull<_>>` — FFI-safe due to Rust's guaranteed null-pointer optimization, and cleaner than raw pointer null checks
- **`ObjcObject.isa`**: `NonNull<ObjcClass>` (non-optional) — valid objects always have a non-null isa
- **`ObjcClass.method_list`**: `Option<NonNull<MethodList>>` — `None` when no methods have been added; lazily initialized by `class_add_method` via `get_or_insert_with(MethodList::new)`
- **Method dispatch**: `search_method_lists` and `class_lookup_method` use `std::iter::successors` to walk linked lists, with `find_map`/`flat_map` for clean short-circuiting
- **Method lists**: `Vec<MethodEntry>` for now (not a C inline array); ABI-compatible layout comes in Phase 2
- **Pre-registration discipline**: `class_add_method` must only be called before `objc_registerClassPair`; nothing enforces this in the type system, but violating it is a data race on `method_list`

### `Cargo.toml` dependencies

```toml
dashmap = "6"
parking_lot = "0.12"
libc = "0.2"
```

---

## Phase 2 — Not started

**Scope:** Method cache + `class_addMethod` (runtime) + `method_exchangeImplementations`

**Planned files:** `src/method_cache.rs`

**Key work:**
- Per-class power-of-two bucket array: hash = `(sel as usize >> 2) & mask`
- Fill cache on slow-path hit; invalidate on swizzle / category attach
- Cache invalidation propagates through `first_subclass` / `next_sibling` tree (needs those pointers added to `ObjcClass`)
- `method_exchangeImplementations`: atomic IMP swap + cache flush
- Replace `Vec<MethodEntry>` in `MethodList` with a count + inline array to match the GNUstep v2 ABI layout (`struct objc_method_list` uses a flexible array member); add `#[repr(C)]` to `MethodList` at the same time

---

## Phase 3 — Not started

**Scope:** Retain/Release + Autorelease Pools + Weak References

**Planned files:** `src/retain_release.rs`, `src/autorelease.rs`, `src/weak.rs`

---

## Phase 4 — Not started

**Scope:** Dynamic Method Resolution + Message Forwarding

**Planned files:** `src/dynamic_resolution.rs`, `src/forwarding.rs`

---

## Phases 5–8 — Not started

See PLAN.md for full scope.
