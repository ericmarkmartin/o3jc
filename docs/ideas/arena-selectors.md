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
