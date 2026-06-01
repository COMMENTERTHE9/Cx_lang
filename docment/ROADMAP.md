# Cx Project Roadmap — Living Summary

Last updated: 2026-06-01

This file is a concise synthesis of the project's roadmap state. Detailed roadmaps live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.0 on submain)

---

## Frontend — Release Candidate

All 9 hard blockers resolved. 117/117 matrix tests passing. 8/8 examples passing.

**Status:** 0.1 release candidate. No known soundness holes. Syntax frozen.

**Known limitations (documented, not blocking):**
- String arena grows monotonically (interpreter-only)
- No strref constructor syntax
- Expression statements still require semicolons

---

## Backend — Active Development

The backend pipeline converts verified SemanticProgram → IR → machine output (Cranelift JIT for 0.1).

### Done
- [x] Phase 0 — Foundation (semantic boundary)
- [x] Phase 1 — IR data model
- [x] Phase 2 — Straight-line lowering
- [x] Phase 3 — IR validation
- [x] Phase 4 — Function lowering
- [x] Phase 5 — if/else lowering
- [x] Phase 0.5 — Backend trait interface (&IrModule)
- [x] Phase 7 — IR pretty printer and diagnostics
- [x] Phase 6 — Function call lowering (direct calls, arity/type validation)
- [x] Phase 10 — Loop lowering (while, for, break, continue)
- [x] Phase 8 Round 1 — ABI (scalars, structs, arrays, enums, calling convention)

### Active
- [ ] Phase 11 — Surface area reduction
  - [x] Compound assign
  - [x] Unary expressions
  - [x] Struct literal lowering (CX-9)
  - [x] Struct field reads (CX-10)
  - [x] Struct field writes (CX-14)
  - [x] Void function calls (CX-13)
  - [x] Array type and literal lowering (CX-16)
  - [x] Array element access (CX-17)
  - [x] Array element writes (CX-20)
  - [x] Range structured error (CX-19)
  - [x] MethodCall structured error (CX-21)
  - [ ] Method call actual lowering
  - [ ] `when` block lowering or structured rejection
  - [ ] DotAccess in compound forms
- [ ] Phase 8 Round 2 — str/strref layout, Handle<T>, TBool calling convention

### Merged to submain (not yet on main)

- [x] Phase 13 — Cranelift lowering skeleton (CX-22)
- [x] JIT Host Boundary (CX-24: process ownership, exit codes, output capture)
- [ ] Phase 12 — Differential harness (parity classification CX-69, loop fixtures CX-68, determinism tests CX-55 merged; more fixtures in flight)
- [ ] Phase 9 — Runtime intrinsics boundary (assert/assert_eq lowered natively via CX-48; print/println/printn/read/input still pending)
- [ ] Phase 14 — First executable Cranelift slice (CX-52 float comparison, CX-53 void return, CX-54 debug-trace gating merged)
- [ ] Phase 15 — Cranelift JIT 0.1 target (CX-74 exit-code propagation merged; CX-57/58/60/63/64/66 instruction coverage in flight)

### Post-0.1
- [ ] Cranelift AOT (Phase 16)
- [ ] LLVM AOT (Phase 17)
- [ ] FFI and C boundary (Phase 18)

---

## Language Features — Post-0.1

- NullPoint<T>
- Generics v3 (type bounds)
- Generic structs
- Multi-struct impl blocks
- gene + phen trait system
- := type inference
- Stdlib (growable array, hash table, ring buffer)
- Full memory system (region invalidation, rc<T>, shared<T>)
- Full string model (strref escape, UTF-8, interop)
- I/O (read, input, string interpolation)
- GPU system

---

## Working Notes

**2026-06-01:** 4 tracker items closed on submain (#027 non-exhaustive `when` error, #026 `if <unknown>` runtime error, #019 enum annotations in function signatures, #038 string interpolation error on unresolvable `{...}`). Continues the tracker sprint from 2026-05-31 (9 items). Submain 23 non-merge commits ahead of main (92 files, 3145+/1894−). Matrix: 182/182 on main; submain gates report 207/0.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
