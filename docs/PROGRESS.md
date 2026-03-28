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

## Phase 3 ✅ — Complete

**Milestone:** Full object lifecycle; no leaks

### What was built

| File | Description |
|---|---|
| `src/retain_release.rs` | Side table (`DashMap<usize, SideTableEntry>`), retain/release, deallocation with `-dealloc` dispatch, and all five weak-reference functions. Weak logic is co-located here rather than split to `weak.rs` — the two concerns share `TABLE`, `WEAK_LOCKS`, `WeakLocation`, and the deallocation sequence too tightly to separate cleanly. |
| `src/autorelease.rs` | Thread-local pool stack (`RefCell<Vec<Vec<Id>>>`). Push returns the pre-push depth as an opaque token; pop drains all pools back to that depth in LIFO order. |

### C ABI exports (new in Phase 3)

```c
id   objc_retain(id obj);
void objc_release(id obj);
id   objc_autorelease(id obj);
void *objc_autoreleasePoolPush(void);
void  objc_autoreleasePoolPop(void *token);
id   objc_initWeak(id * _Nonnull location, id obj);
id   objc_storeWeak(id * _Nonnull location, id new_obj);
id   objc_loadWeakRetained(id * _Nonnull location);
id   objc_loadWeak(id * _Nonnull location);
void objc_destroyWeak(id * _Nonnull location);
```

### Tests passing (14/14)

All Phase 1–2 tests continue to pass, plus:

- `retain_release_count` — fresh object has implicit count of 1; retain/release adjust correctly
- `release_to_zero_calls_dealloc` — `-dealloc` is dispatched when count hits zero
- `autorelease_pool_releases_on_pop` — two autoreleased objects both released on pool pop
- `weak_reference_zeroed_on_dealloc` — weak slot is `nil` after the referent is deallocated

### Key implementation notes

- **`Id` as `Option<NonNull<ObjcObject>>`**: null-pointer niche makes this ABI-compatible with `*mut ObjcObject`; `None` is ObjC `nil`. All retain/release/weak functions propagate `None` with `?` or early return, eliminating explicit null checks.
- **Side table**: `DashMap<usize, SideTableEntry>` keyed by object address. Absent entry means implicit retain count of 1. Count is only written to the table when it rises above or falls back through 1.
- **Deallocation sequence**: set `deallocating = true` and extract `weak_locations` under the DashMap shard lock; release the shard lock; zero each weak slot under its stripe lock; call `-dealloc`; remove the entry. This ordering prevents concurrent `objc_retain` from reviving the object.
- **Weak location stripe locks**: `[Mutex<()>; 8]` indexed by *location* address (not object address), so the correct lock can be found without first reading the potentially-racy pointer stored at the location.
- **`WeakLocation(AtomicPtr<Id>)`**: stores the location address as `AtomicPtr<Id>` rather than `NonNull<Id>`. `AtomicPtr<T>: Send + Sync` unconditionally, eliminating the need for `unsafe impl Send/Sync` on `WeakLocation`. The pointer is written once at construction and loaded with `Relaxed` ordering in `lock()` (the DashMap shard lock that mediates insertion provides the necessary happens-before).
- **`ProxyGuard<T>`**: RAII type holding a `MutexGuard<'static, ()>` and a `Copy` value `T`. `Deref`/`DerefMut` expose the value; callers use `unsafe { guard.read() }` / `unsafe { guard.write(...) }` on the `NonNull<Id>` to make the unsafety visible at each call site. Concurrency reasoning (lock is held) is kept in plain comments; `SAFETY:` comments cover only pointer validity.
- **`weak.rs` not created**: the plan anticipated a separate file, but weak references share `TABLE`, `WEAK_LOCKS`, `WeakLocation`, and the deallocation sequence with retain/release. Co-locating avoids exposing all those internals as `pub(crate)`.

---

## Phase 4 ✅ — Complete

**Milestone:** Link and run a `.m` with no static classes

### What was built

| File | Description |
|---|---|
| `src/loader.rs` | `__objc_load` — GNUstep v2 module-load entry point. Clang places a `.objcv2_load_function` in `.init_array` for every `.m` file; it calls `__objc_load` with an `ObjcModuleInfo` struct whose fields are start/stop pointers into the ELF sections. Phase 4 stub validates the pointer and returns; section walking added in Phase 5. |
| `tests/gnustep_runtime.h` | Standalone minimal GNUstep ABI header (types + function declarations). Avoids depending on a system-installed libobjc2 while remaining ABI-compatible. |
| `tests/objc/*.m` | 13 ObjC fixture files — one per integration test case. Each is a standalone program that exercises the C ABI and prints results to stdout. |
| `tests/integration.rs` | Rust integration test harness: 13 `#[test]` functions. Each compiles its `.m` fixture on demand (via `clang`), runs it, and asserts stdout. Only fixtures matching `cargo test`'s filter are compiled. |
| `src/types.rs` (`ObjcPtr`) | `#[repr(transparent)]` newtype around `NonNull<ObjcObject>` with `Send + Sync`. Needed because `NonNull<T>` is unconditionally `!Send`, but `ShardedMutex<Id>` requires `Id: Send` for its `Sync` impl. `Id = Option<ObjcPtr>` preserves the null-pointer niche (ABI-compatible with `*mut ObjcObject`). |
| `.github/workflows/ci.yml` | CI installs clang and builds the cdylib before `cargo test`; release job uploads `libo3jc.so` + `o3jc.h` on tag push. |

### C ABI exports (new in Phase 4)

```c
void __objc_load(struct ObjcModuleInfo *info);   // GNUstep v2 module init hook
```

### Weak reference refactor (Phase 4)

Replaced hand-rolled `WEAK_LOCKS: [Mutex<()>; 8]` / `WeakLocation` / `ProxyGuard` (~70 lines) with `sharded_mutex` crate:

| Before | After |
|---|---|
| `WEAK_LOCKS: [Mutex<()>; 8]` — 8 stripe locks | `ShardedMutex<Id, WeakSlotTag>` — 127 stripe locks from global pool |
| `WeakLocation(AtomicPtr<Id>)` + `ProxyGuard<T>` — manual RAII guard, `unsafe` reads/writes | `ShardedMutexGuard` — `Deref`/`DerefMut` to `&mut Id`, plain `*guard = value` |
| `Mutex<()>` protects nothing; data access via raw pointers | Mutex genuinely guards the data it protects |
| `SmallVec<[WeakLocation; 0]>` with `AtomicPtr` for `Send + Sync` | `SmallVec<[WeakSlot; 0]>` — `WeakSlot` holds `&'static ShardedMutex<Id>` (auto `Send + Sync`) |

Side table keyed by `usize` (object address cast to inert integer — never dereferenced through the key, avoids `Send`/`Sync` wrapper on the key).

### Tests passing (27 total via `cargo test`)

14 Rust unit tests (phases 1–3) + 13 integration tests (`tests/objc/*.m`):
- `class_creation`, `class_invisible`, `class_add_method`
- `selector_same`, `selector_different`
- `msg_lookup_slow`, `imp_returns_self`, `cache_hit`
- `unknown_selector`, `null_receiver`
- `introspection`, `subclass_inherits`, `method_swizzle`

### Key implementation notes

- **`__objc_load` struct layout**: Clang emits `{ i64 version, i8**×16 }` (17 fields) as the `.objc_init` global in each `.m`. The Rust `ObjcModuleInfo` is `#[repr(C)]` with exactly those fields, named by section (`sel_start/stop`, `classes_start/stop`, etc.).
- **Null sentinels**: Clang always emits one null-initialised sentinel entry into each section (e.g. `{ i8*, i8* } zeroinitializer` for `__objc_selectors`). Phase 5's section walker must skip these.
- **Standalone test header**: System libobjc2 is not available in this environment. `tests/gnustep_runtime.h` declares only the subset of `<objc/runtime.h>` + `<objc/objc-arc.h>` used by the integration tests, keeping each test self-contained.
- **Rust integration harness**: `tests/integration.rs` compiles each `.m` fixture on demand via `std::process::Command` → `clang`. Only fixtures matching `cargo test`'s `--` filter are compiled, so `cargo test --test integration -- cache_hit` compiles and runs only `cache_hit.m`. Cargo's default thread pool parallelizes compilation of multiple fixtures.
- **cbindgen limitation**: cbindgen's `Option<NonNull<T>>` → pointer resolution is hardcoded by name (`"NonNull"` string match in `simplified_type`). It can't see through our `ObjcPtr` wrapper. `cbindgen.toml` manually defines `typedef struct objc_object *Id;` and excludes `ObjcPtr`/`Option_ObjcPtr`/`Id` from auto-generation.
- **`ObjcPtr` vs `NonNull`**: `NonNull<T>` is unconditionally `!Send + !Sync` regardless of `T`'s bounds. `ObjcPtr` adds `unsafe impl Send + Sync` justified by the runtime's lock-based serialization. This makes `Id: Send + Sync`, which propagates through `ShardedMutex<Id>`, `WeakSlot`, and `SideTableEntry` with no manual trait impls on any of those types.

---

## Phase 5 ✅ — Complete

**Milestone:** Link and run a `.m` with static classes (no framework deps, explicit root)

### What was built

| File | Description |
|---|---|
| `src/types.rs` | `ObjcSelector` is now a real `{ name, types }` struct matching GNUstep v2 ABI. `MethodEntry` reordered to `{ imp, sel, types }` (IMP first). `ObjcClass` restructured to exact 17-field ABI layout; cache stored in `dtable` via accessor methods. `class_flags` swapped: `CLASS_IS_METACLASS` = bit 0 (matches Clang), `CLASS_REGISTERED` = bit 16 (runtime-internal). |
| `src/sel.rs` | Split into `NAME_TABLE` (name string → interned `*const c_char`) and `SEL_TABLE` (name → leaked `ObjcSelector`). New `intern_selector_name` for loader use. New `sel_eq` for name-pointer comparison. |
| `src/loader.rs` | Full `__objc_load` implementation: compiled ABI types (`CompiledSelector`, `CompiledMethodList`, `CompiledMethodEntry`), `load_selectors` (interns names, patches pointers), `convert_method_list` (compiled → runtime format), `load_classes` (patches instance_size, converts method lists, bootstraps root metaclass ISA, initializes caches, registers classes). |
| `src/msg_send.rs` | `objc_msgSend`: x86_64 naked asm trampoline (saves all GPR + SSE arg registers, calls `objc_msg_lookup_nonnull`, restores, tail-calls IMP; nil receiver returns zero). `objc_msg_lookup_nonnull`: panics on miss. |
| `src/class_registry.rs` | `register_loaded_class` for loader use. `ObjcClass` field renames: `first_subclass` → `subclass_list`, `next_sibling` → `sibling_class`. All 17 ABI fields initialized in `objc_allocate_class_pair`. |
| `src/method_cache.rs` | Updated for field renames and `cache()` accessor. |
| `tests/objc/static_class.m` | Phase 5 test fixture: root class with `@implementation`, class method `+hello`, dispatched via `[TestRoot hello]`. |

### C ABI exports (new in Phase 5)

```c
IMP      objc_msg_lookup_nonnull(id receiver, SEL sel);  // non-nullable lookup
void     objc_msgSend(id receiver, SEL sel, ...);        // x86_64 asm trampoline
```

### Tests passing (28 total via `cargo test`)

14 Rust unit tests (phases 1–3) + 14 integration tests (13 from Phase 4 + 1 new):
- `static_class` — Clang-compiled root class loaded from `__objc_classes`, class method dispatched via `objc_msgSend`

### Key implementation notes

- **SEL representation change**: `ObjcSelector` is now `{ name: *const c_char, types: *const c_char }` matching GNUstep v2 ABI. The intern table maps name strings to stable `*const c_char` pointers. Two SELs are equal iff `(*sel_a).name == (*sel_b).name` (interned name pointer equality). `sel_eq()` is used everywhere instead of direct pointer comparison on the SEL itself.
- **Cache in dtable**: Compiled classes are exactly 17 fields. Rather than adding an 18th field and reallocating every compiled class, the method cache pointer is stored in the `dtable` field (ABI field #9, always null from Clang). `ObjcClass::cache()` / `set_cache()` accessors abstract the cast. This also matches libobjc2's approach where dtable IS the dispatch accelerator.
- **Compiled classes used in-place**: No reallocation needed. The loader patches the compiled class struct directly: negates `instance_size` if negative, converts method lists, initializes cache in `dtable`, sets root metaclass ISA self-loop, and registers.
- **Method list conversion**: Compiled method lists (`{ next, count: i32, size: i64, entries[] }`) have inline C arrays. The loader converts them to heap-allocated `MethodList { next, entries: Vec<MethodEntry> }` at load time, interning selectors from the already-fixedup `CompiledSelector` structs.
- **Selector fixup**: `load_selectors` walks `__objc_selectors`, interns each name via `intern_selector_name`, and overwrites the `name` field with the interned pointer. Method list conversion then casts `*mut CompiledSelector` to `SEL` directly since `CompiledSelector` and `ObjcSelector` have identical layout.
- **Root metaclass bootstrap**: Clang emits root metaclass with `isa = null`. The loader sets `metaclass.isa = Some(metaclass)` (self-loop) and `metaclass.super_class = Some(root_class)` for proper dispatch chain.
- **`objc_msgSend` trampoline**: x86_64 naked asm function. Saves 6 GPRs (rdi–r9) + 8 SSE registers (xmm0–xmm7) = 176 bytes + 8 alignment = 184 bytes on stack. Calls `objc_msg_lookup_nonnull`, restores registers, and tail-calls the IMP via `jmp r11`. Nil receiver path returns zero in rax/rdx/xmm0/xmm1.
- **Flag bit fix**: `CLASS_IS_METACLASS` moved to bit 0 (matching Clang's `info = 1` for metaclass). `CLASS_REGISTERED` moved to bit 16 to avoid conflict with any GNUstep ABI flags.

---

## Phase 6 ✅ — Complete

**Milestone:** Static class hierarchies — inherited method dispatch through compiled superclass chain

### What was built

| File | Description |
|---|---|
| `src/loader.rs` | Full metaclass chain fixup in `load_classes`: root metaclass gets ISA self-loop + super → root class; non-root metaclass gets ISA → root metaclass + super → superclass's metaclass. Subclass list threading for cache invalidation (class and metaclass). |
| `tests/objc/static_hierarchy.m` | Phase 6 test fixture: two-class hierarchy (`Root` + `Child : Root`), tests inherited class method dispatch, own method dispatch, and direct dispatch on root. |
| `tests/integration.rs` | `static_hierarchy` integration test. |

### Tests passing (29 total via `cargo test`)

14 Rust unit tests (phases 1–3) + 15 integration tests (14 from phases 4–5 + 1 new):
- `static_hierarchy` — Clang-compiled class hierarchy loaded; `[Child rootMethod]` dispatches inherited class method through metaclass superclass chain

### Key implementation notes

- **Clang emits ALL metaclasses with `isa = null` and `super_class = null`**: The GNUstep v2 ABI (at least with Clang 14) does not resolve metaclass chain pointers at link time. The runtime must fix up the entire metaclass chain at load time. This was the main discovery — the Phase 5 code only handled root metaclasses, silently setting every metaclass's ISA to a self-loop.
- **Metaclass fixup logic**: For root classes (`super_class == None`): metaclass ISA = self (self-loop), metaclass super = root class. For non-root classes: metaclass super = superclass's metaclass (`super_class.isa`), metaclass ISA = root metaclass (found by walking the superclass chain to the root and reading its `isa`).
- **Order-independent**: The fixup works regardless of class processing order in `__objc_classes`. Class `super_class` pointers are linker-resolved (always valid), so the superclass chain can be walked even if the superclass hasn't been patched yet. The root class's `isa` (pointing to the root metaclass struct) is also linker-set.
- **Subclass list threading**: Both classes and metaclasses are wired into their respective superclass's `subclass_list`/`sibling_class` linked list, enabling `flush_class_cache_tree` to propagate cache invalidation through compiled hierarchies. Root metaclass → root class wiring is skipped (that relationship is for method fallback, not hierarchy).
- **No changes to dispatch code**: `class_lookup_method` in `msg_send.rs` already walks `super_class` via `std::iter::successors`. Once the metaclass chain is correctly fixed up, inherited method dispatch works without any dispatch-side changes.

---

## Phases 7–13 — Not started

See PLAN.md for full scope.
