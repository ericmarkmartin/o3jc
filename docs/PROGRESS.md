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

## Phase 2 ✅ — Complete

**Milestone:** Hot dispatch path; method swizzling works

### What was built

| File | Description |
|---|---|
| `src/method_cache.rs` | `MethodCache`: dense `Vec<CacheEntry>` pre-allocated to 16 entries, flushed (not grown) when full. `parking_lot::RwLock<CacheInner>` for thread safety. `flush_class_cache_tree` walks `first_subclass` / `next_sibling` to propagate invalidation. |
| `src/types.rs` | `ObjcClass` flattened to single struct with all fields inline. `SEL` changed to `NonNull<ObjcSelector>` (zero-size opaque `#[repr(C)]` struct). `IMP` corrected to `unsafe extern "C" fn(Id, SEL, ...) -> Id`. `cache` field is `Option<NonNull<MethodCache>>`. |
| `src/class_registry.rs` | `objc_allocate_class_pair` allocates a `MethodCache` per class+metaclass and wires `first_subclass` / `next_sibling`. `class_add_method` prepends a new `MethodList` node (and flushes the cache tree) when called post-registration. New functions: `class_get_instance_method`, `class_replace_method`, `method_exchange_implementations`, `flush_all_caches`. |
| `src/msg_send.rs` | `objc_msg_lookup` checks cache first; fills on slow-path hit. |
| `src/lib.rs` | New C ABI exports; 3 additional tests (10 total). |
| `build.rs` + `cbindgen.toml` + `include/o3jc.h` | cbindgen generates a C header from exported Rust types for comparison against GNUstep's `runtime.h`. Exported struct names match GNUstep (`objc_object`, `objc_selector`, `objc_method`, etc.). `struct objc_class` is forward-declared opaque. |
| `third_party/libobjc2/` | Vendored GNUstep public headers for reference. |

### C ABI exports (new in Phase 2)

```c
Method   class_getInstanceMethod(Class cls, SEL sel);
IMP      method_getImplementation(Method m);
void     method_exchangeImplementations(Method m1, Method m2);
IMP      class_replaceMethod(Class cls, SEL sel, IMP imp, const char *types);
```

### Tests passing (10/10)

All Phase 1 tests continue to pass, plus:

- `cache_hit_after_first_dispatch` — second lookup returns same IMP (from cache)
- `method_swizzle_works` — after `method_exchangeImplementations`, sel_a dispatches to imp_b
- `post_registration_add_method` — `class_addMethod` after `objc_registerClassPair` works and is discoverable by `objc_msg_lookup`

### Key implementation notes

- **Cache design**: `Vec<CacheEntry>` pre-allocated with `with_capacity(16)`. On full, entries are cleared rather than grown — same strategy as Apple's runtime. No sentinels or hash table needed at this scale.
- **Cache field type**: `Option<NonNull<MethodCache>>` — null-pointer niche means same size as a raw pointer; pattern-matches cleanly at call sites.
- **Thread safety of cache**: `parking_lot::RwLock` guards the inner table. Read lock for lookups (fast path), write lock for insert/flush.
- **Post-registration method add**: prepends a new single-entry `MethodList` node to the chain rather than mutating the existing `Vec`. This keeps all previously-returned `*mut MethodEntry` pointers stable.
- **Swizzle cache flush**: `method_exchange_implementations` calls `flush_all_caches` (walks every registered class) because `MethodEntry` has no back-pointer to its owning class. Swizzling is rare so the global flush is acceptable.
- **`ObjcSelector` opaque type**: `SEL = NonNull<ObjcSelector>` where `ObjcSelector` is a zero-size `#[repr(C)]` struct, matching GNUstep's `const struct objc_selector *`. The SEL pointer value is actually the address of an interned `CString`; `sel_get_name` recovers it by casting back to `*const c_char`.
- **`ObjcClassAbi` split explored and collapsed**: We briefly split `ObjcClass` into an ABI-visible `ObjcClassAbi` prefix and a runtime-internal tail. Determined `struct objc_class` is fully opaque in the GNUstep public header — the compiler never accesses fields by name through it — so the split doesn't reflect a real boundary yet. Collapsed back to a single flat struct. The split will return when we implement loading of Clang-compiled binaries.

### GNUstep v2 static class layout (researched, not yet implemented)

When Clang compiles `@implementation`, it emits a 17-field class struct (from `CGObjCGNU.cpp`). Our current `ObjcClass` covers most fields but is missing and has deviations that must be fixed before loading Clang-compiled binaries:

**Missing fields:**

| Field # | Name | Type |
|---|---|---|
| 9 | `cxx_construct` | `IMP` |
| 10 | `cxx_destruct` | `IMP` |
| 14 | `extra_data` | `*reference_list` |
| 15 | `abi_version` | `long` |
| 16 | `properties` | `*objc_property_list` |

**Other deviations:**
- Clang emits `instance_size` as a **negative** value; the runtime patches it at load time.
- `cache` has no ABI slot — it must follow all 17 ABI fields, not sit in the middle.
- Method entry layout is wrong for v2: Clang emits `{IMP, *selector_struct, *types}` (IMP first); we have `{SEL, *const c_char, IMP}`.
- The selector in compiled method lists is a pointer to a `{name, types}` struct in `__objc_selectors`, not our interned opaque pointer. The runtime must process `__objc_selectors` at load time to intern and patch these.

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
