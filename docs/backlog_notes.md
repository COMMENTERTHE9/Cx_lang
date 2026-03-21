# Cx Backlog Notes

## Language Features

- [ ] `when` blocks — tri-branch, needs true/false/unknown arms
- [ ] `if/else` — spec says forbidden until `when` is finalized
- [ ] Enums — tied to `when` since `when` is the natural pattern match for them
- [ ] Loops — `while`, `for`, `loop`
- [ ] Forward function declarations
- [ ] Fix nested function name leakage bug
- [ ] `is_known(x)` primitive — state check API, syntax locked
- [ ] `TBool` wired up — variant exists but never constructed
- [ ] Structs
- [ ] Arrays
- [ ] `NullPoint<T>`

## Runtime / Memory

- [ ] Phase 5b — `region_id` bulk handle invalidation on arena reset
- [ ] GPU/VRAM system

## Backend

- [ ] Cranelift JIT — senior dev's stream, starts after Phase 5 confirmed closed

## Tooling

- [ ] CLI visualizer — parked
