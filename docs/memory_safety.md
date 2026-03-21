# Memory Safety in Cx

Cx uses explicit ownership, scoped cleanup, arenas, and handles instead of a tracing GC or borrow checker.

## Core Rules
- Owned values live in explicit runtime storage.
- Arena-backed scope cleanup is deterministic.
- Handles use slot + generation pairs, so stale access can be detected instead of silently aliasing freed state.
- `unknown` is a first-class state in the type/runtime model, so uncertain data can be propagated or blocked in control-critical paths.
- `StrRef` is boundary-limited in the semantic layer and is not allowed to escape through returns, variable storage, or handles.

## Current Runtime Model
- Strings live in a runtime-owned byte arena.
- `Value::Str` stores offsets into that arena.
- Scoped runtime frames track cleanup and bleed-back behavior separately.
- Handles are validated through the runtime handle registry.

## Current Limits
- This is still an interpreter-first implementation.
- Some surface syntax and future memory forms are scaffolded but not fully wired yet.
- The source of truth for real behavior is still the verification matrix under `src/tests/verification_matrix/`.
