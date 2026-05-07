# Cx Project Roadmap — Living Summary

Last updated: 2026-05-07

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

### Done (merged to submain, May 6–7)
- [x] Phase 13 — Cranelift lowering skeleton (CX-22: IrType mapping, module traversal, error types)
- [x] JIT Host Boundary (CX-24: process ownership, exit codes, output capture scaffold)
- [x] Phase 9 sub-packet 1 — Builtin audit and intrinsics boundary spec (CX-35)
- [x] Phase 14 sub-packets 1–2 — First JIT execution: ConstInt, Binary, Return, Alloca, Load, Store (CX-25, CX-26)
- [x] Phase 14 sub-packet 3 — Compare + Jump + Branch terminators (CX-27, CX-41)
- [x] Evaluation order spec — left-to-right verified and tested (CX-37)
- [x] IR validator loop-variable read-only enforcement (CX-40)
- [x] Phase 15 — No-panic guarantee for JIT on valid IR (CX-50)

### Active — In Human Review or Branch-Local
- [ ] Phase 12 — Differential harness
  - [x] Sub-packet 1 — fixture format, interpreter baseline (CX-23, merged)
  - [ ] Sub-packet 2 — JIT execution and comparison (CX-31, in review)
  - [ ] Sub-packet 3 — Full 0.1 construct set coverage (CX-34, in review)
  - [ ] Sub-packet 4 — JIT parity matrix and baseline (CX-51, branch-local)
- [ ] Phase 14 — First executable Cranelift slice
  - [x] Sub-packet 1 — ConstInt + arithmetic + Return (CX-25, merged)
  - [x] Sub-packet 2 — Alloca + Load + Store (CX-26, merged)
  - [x] Sub-packet 3 — Compare + Jump + Branch (CX-27+CX-41, merged)
  - [ ] Sub-packet 4 — Direct function calls (CX-30, in review)
- [ ] Phase 15 — Cranelift JIT 0.1 target
  - [x] No-panic guarantee on valid IR (CX-50, merged)
  - [ ] Sub-packet 1 — PtrOffset + PtrAdd (CX-32, in review)
  - [ ] Sub-packet 2 — SsaBind, ConstFloat, Cast (CX-33, in review)
  - [ ] Sub-packet 3 — Float arithmetic dispatch (CX-36, in review)
- [ ] Phase 9 — Runtime intrinsics boundary
  - [x] Sub-packet 1 — Builtin audit and boundary spec (CX-35, merged)
  - [ ] Sub-packet 2 — Runtime intrinsics dispatch (CX-38, in review)
  - [ ] Sub-packet 3 — Assert/assert_eq lowering via Trap (CX-48, branch-local)

### Next — 0.1 Path
- [ ] Merge 6 Human Review branches into submain
- [ ] Merge submain → main (gap 16+ commits)

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

**2026-05-07:** CX-50 merged to submain (no-panic guarantee, PR #101). CX-47/48/49/51 committed branch-local (loop back-edge fix, assert lowering, determinism tests, parity matrix). CX-44/45 rebased and verified 6 Human Review PRs. JIT tests: 197 on submain, 207 on branch tips. JIT parity: 23/120 pass, 97 skip, 0 fail. Matrix 117/117 stable.

**2026-05-06:** Massive backend sprint. CX-21–24 merged to submain. CX-25/26 (Phase 14 sub-packets 1–2), CX-35 (Phase 9.1), CX-37 (eval order), CX-40 (loop-var read-only), CX-41 (Jump/Branch fix) all merged to submain. 8 branches in Human Review. JIT tests grew from ~169 to ~191. Matrix 117/117 stable.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
