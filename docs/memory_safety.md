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


## Copy target identity

A `.copy` argument snapshots its resolved lexical binding at argument evaluation
and writes the parameter's final value back to that same binding on scope exit.
The target can belong to a module or an outer block; the immediate caller frame
and a same-named variable are not substitutes for its identity. Runtime copy
metadata retains both the owning frame and BindingId. `.copy.free` remains an
isolated, deprecated value copy and resolves the same lexical argument identity.
Regression fixtures cover an outer block, nested copy write-back, module state
and a caller's shadowing parameter.

A `.copy` argument requires a writable target: const bindings and readonly loop
counters are rejected during semantic analysis. The isolated `.copy.free` form
does not grant write access and remains accepted for those values.
