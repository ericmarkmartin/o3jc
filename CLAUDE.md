# o3jc — Claude Context

## Project

An Objective-C runtime implemented in Rust, targeting the GNUstep v2 ABI. Primary goal
is learning, with the long-term aim of executing real Clang-compiled ObjC binaries.

## Documentation

Always read these at the start of a session:

- `docs/PLAN.md` — architecture, locked-in design decisions, phased implementation plan
- `docs/PROGRESS.md` — what has been built, current phase status, key implementation notes

Check `docs/ideas/` for design explorations that were considered and parked. These often
contain useful context about tradeoffs relevant to upcoming phases.

## Unsafe Discipline

Minimize `unsafe` in internal code. Concentrate pointer-validity invariants at
construction boundaries using safe wrapper types (`ClassRef`, `method_list_iter`,
`sel_name_ptr`, etc.), so that traversal and business logic is safe Rust.

- **FFI boundaries** (`unsafe extern "C"` in `lib.rs`, loader entry points): unsafe is expected.
- **Internal read paths** (dispatch, cache flush, method search, selector comparison):
  should be safe functions. Use `ClassRef`, `method_list_iter`, and similar abstractions
  to push unsafe to the point where a raw pointer is first wrapped.
- **Internal mutation** (class construction, method addition, loader patching): unsafe is
  acceptable since these mutate `#[repr(C)]` structs through raw pointers, but keep it
  to the minimum necessary.
- When adding new internal code, prefer safe `fn` over `unsafe fn`. If a function body
  needs one unsafe operation, wrap that operation — don't mark the whole function unsafe.

## Source Layout

```
src/
  lib.rs            — C ABI surface + unit tests
  types.rs          — core #[repr(C)] structs and type aliases
  sel.rs            — selector intern table
  class_registry.rs — class allocation, registration, method addition
  msg_send.rs       — slow-path method dispatch
```
