# Inline Method Lists (skip convert_method_list)

## Current approach

Our `MethodList` uses `Vec<MethodEntry>` for the entries array. Clang emits
method lists as `{ next, count: i32, size: i64, entries[] }` with entries
inline (C flexible array member). These layouts are incompatible, so
`convert_method_list` in `loader.rs` copies every compiled method entry into
a heap-allocated `Vec` at load time.

libobjc2 doesn't do this — it uses the compiled method list in-place because
its runtime `objc_method_list` struct matches the compiled layout exactly.
The only in-place mutation is selector fixup.

## What would change

Replace `MethodList.entries: Vec<MethodEntry>` with a C-compatible layout:

```rust
#[repr(C)]
pub struct MethodList {
    pub next: *mut MethodList,
    pub count: i32,
    pub size: i64,
    // entries follow inline, accessed via pointer arithmetic
}
```

Access entries via `(self as *const u8).add(size_of::<MethodList>())` cast to
`*const MethodEntry`, with `count` as the bound.

## Tradeoffs

**Gains:**
- Eliminates per-class heap allocation + copy at load time
- Matches libobjc2's approach — compiled method lists used directly
- Slightly less memory (no Vec ptr/len/cap overhead per list)

**Costs:**
- Lose `Vec` ergonomics: no `.iter()`, `.push()`, bounds checking
- Every method list walk becomes raw pointer arithmetic with unsafe
- `class_add_method` (pre-registration) currently uses `Vec::push`; would
  need to allocate a new fixed-size list and copy, or use a different
  strategy for runtime-built lists vs compiled lists

**Why it was punted:** The copy is once per class at load time — not a
performance bottleneck. The Vec-based approach is safer and more idiomatic
Rust. Worth revisiting if load time becomes measurable or if the unsafe
pointer arithmetic can be cleanly abstracted.
