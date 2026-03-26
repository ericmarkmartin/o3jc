# Idea: Arena-Allocated Selector Strings

## The Idea

Replace the per-selector `CString::into_raw()` (one scattered heap allocation per unique
selector) with a bump allocator that packs all selector strings into contiguous pages.

## Why It's a Natural Fit

Unlike the class arena idea, this requires no handle/index conversion. `SEL` is already
`*const c_char` — a pointer. The arena is purely an implementation detail: the pointer
still points to a valid null-terminated C string, still compares equal for the same name,
still lives for the process lifetime. The interning invariant is unchanged.

Current approach:

```rust
let cs = CString::new(name).unwrap();
cs.into_raw() as usize  // one Box<CString> leaked per unique selector
```

Arena approach: bump-allocate each string into a page, return a pointer into it.
Same result at the `SEL` level, better allocation profile.

## Benefits

- Fewer allocator round-trips (one page allocation per ~4KB of selector names vs. one
  allocation per selector)
- Better cache locality for selector string comparisons
- No per-string `Box` overhead

## Hard Constraint

The arena must **never move its memory** — existing `SEL` pointers would be invalidated
if the backing buffer reallocated. A `Vec`-based arena is therefore unsafe here.

The standard solution is a linked list of fixed-size pages (e.g. 4KB each): when the
current page fills, allocate a fresh page and chain it; never touch old pages.

## Prior Art

This is what real ObjC runtimes (including GNU libobjc) do for selector string storage.

## Why We Punted

The current approach is correct. The waste (scattered small allocations) is acceptable
for a learning project and selectors are registered infrequently relative to dispatch.
Revisit if allocation overhead becomes measurable.

---

## Variant: Arena Indices Instead of Pointers

A separate but related idea: rather than returning raw pointers from the arena,
return opaque indices (`struct SelectorName(u32)` or similar). The interning table
maps name strings to indices; selector equality compares indices.

**Gains:**
- `SelectorName` is trivially `Send + Sync` (it's just an integer) — no need for
  `unsafe impl` on `ObjcSelector` for the name field
- Smaller identity type (4 bytes vs 8 bytes for a pointer)

**Costs:**
- Every selector access requires an arena lookup (index → string pointer), adding
  indirection to `sel_getName` and any code that reads the name
- Compiled selectors from `__objc_selectors` have a raw `*const c_char` name field.
  The loader would intern the name, get an index, and write it into the pointer-sized
  field (a u32 fits in 8 bytes). But the rest of the runtime would need to treat the
  field as an index to look up rather than a pointer to dereference — this breaks the
  transparent reinterpret-cast from `CompiledSelector` to `ObjcSelector`, since the
  field's meaning changes from "pointer to C string" to "arena index"

### Union approach for the name field

A `#[repr(C)]` union makes the before/after fixup semantics explicit:

```rust
#[repr(C)]
union SelectorName {
    raw: *const c_char,     // before fixup (Clang-emitted pointer)
    id: SelectorNameId,     // after fixup (arena index, padded to pointer size)
}
```

`CompiledSelector` uses `SelectorName` for its name field. Before fixup the
loader reads `.raw` to get the Clang-emitted string pointer, interns it into
the arena, then writes `.id` back. After fixup the runtime only reads `.id`.
The union is pointer-sized either way, so `#[repr(C)]` layout is preserved.

`ObjcSelector.name` would be `SelectorNameId` (a `Copy + Send + Sync` newtype
around `u32` or `usize`), eliminating the need for `unsafe impl Send/Sync`.
The lifecycle transition from raw pointer to arena index is encoded in the
type system via the union rather than hidden behind a pointer reinterpretation.

**Verdict:** The `unsafe impl Sync` on `ObjcSelector` is a small, well-justified
cost. Arena indices would eliminate it but introduce indirection and ABI friction
that isn't worth it for the current design. Worth revisiting if the runtime ever
moves to a non-pointer SEL representation (which would be a major ABI break).
