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

## Source Layout

```
src/
  lib.rs            — C ABI surface + unit tests
  types.rs          — core #[repr(C)] structs and type aliases
  sel.rs            — selector intern table
  class_registry.rs — class allocation, registration, method addition
  msg_send.rs       — slow-path method dispatch
```
