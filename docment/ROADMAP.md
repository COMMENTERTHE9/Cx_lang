# Cx Project Roadmap — Living Summary

Last updated: 2026-05-10

This file is a concise synthesis of the project's roadmap state. Detailed roadmaps live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.2)

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
- [x] Phase 10 — Loop lowering (while, for, break, continue; loop-var read-only validator enforced CX-40)
- [x] Phase 8 Round 1 — ABI (scalars, structs, arrays, enums, calling convention)
- [x] Phase 9 sub-packet 1 — Audit + structured errors for all builtins (CX-35)
- [x] Phase 9 sub-packet 2 — print/printn/println/cx_print family lowered to runtime dispatch (CX-38/CX-77/CX-82/CX-84)
- [x] Phase 9 sub-packet 3 — assert/assert_eq lowered to abort-on-failure in IR and JIT (CX-48)
- [x] Phase 13 — Cranelift lowering skeleton (CX-22)
- [x] JIT Host Boundary — process ownership, exit-code extraction, output capture (CX-24)
- [x] Phase 14 — First executable Cranelift slice
  - [x] ConstInt + arithmetic + Return (CX-25)
  - [x] Alloca + Load + Store (CX-26)
  - [x] Compare + Jump + Branch (CX-27/CX-41)
  - [x] ConstFloat + fcmp float comparison (CX-52)
  - [x] Debug-trace gating (CX-54)
  - [x] Determinism tests (CX-55)
  - [x] Direct function calls JIT (CX-76)
  - [x] PtrOffset + PtrAdd JIT (CX-78)
  - [x] Runtime intrinsics dispatch — print family (CX-77/CX-82)

### Active
- [ ] Phase 11 — Surface area reduction (nearly complete)
  - [x] Compound assign
  - [x] Unary expressions
  - [x] Struct literal lowering (CX-9)
  - [x] Struct field reads via DotAccess (CX-10)
  - [x] Struct field writes via DotAccess (CX-14)
  - [x] Void function calls / IrType::Void (CX-53)
  - [x] Array type and literal lowering (CX-16)
  - [x] Array element access (CX-17)
  - [x] Array-of-structs tests (CX-18)
  - [x] Range structured error (CX-19)
  - [x] Array element writes (CX-20)
  - [x] MethodCall structured error (CX-21)
  - [x] Loop variable read-only invariant in validator (CX-40)
  - [ ] Method call actual lowering (structured error only)
  - [ ] `when` block lowering or structured rejection
- [ ] Phase 8 Round 2 — str/strref layout, Handle<T>, TBool calling convention
- [ ] Phase 12 — Differential harness
  - [x] Harness shell — interpreter baseline capture (CX-23)
  - [x] Per-feature parity classification, 15 categories (CX-69)
  - [x] Loop construct fixtures (CX-68)
  - [x] Exit-code-based fixtures — arithmetic/variable decl (CX-92)
  - [x] 120 fixtures, 0 PARITY_FAILs
  - [x] Determinism tests (CX-55)
  - [ ] Full construct set coverage expansion (CX-34 on feature branch)
  - [ ] CI gate on every PR
- [ ] Phase 15 — Cranelift JIT 0.1 target
  - [x] No-panic guarantee on valid IR (CX-50)
  - [x] Float comparison + ConstFloat (CX-52)
  - [x] Exit-code propagation verified (CX-74)
  - [x] PtrOffset + PtrAdd JIT (CX-78) — Phase 15 sub-packet 1
  - [x] Reserved intrinsic names rejected in validator (CX-85)
  - [x] Numeric literal cast lowering target-aware (CX-88/CX-89/CX-90)
  - [x] Exit-code-based parity fixtures (CX-92)
  - [ ] Cast instruction JIT coverage (CX-91 on feature branch)
  - [ ] DotAccess JIT parity fixtures (CX-94 on feature branch)
  - [ ] Full parity fixture coverage (CX-34 on feature branch)
  - [ ] Differential harness in CI

### Post-0.1
- [ ] Cranelift AOT (Phase 16)
- [ ] LLVM AOT (Phase 17)
- [ ] FFI and C boundary (Phase 18)

---

## Language Features — Post-0.1

- NullPoint<T>
- Generics v3 (type bounds)
- := type inference
- gene + phen trait system
- Stdlib (growable array, hash table, ring buffer)
- Full memory system (region invalidation, rc<T>, shared<T>)
- GPU system

---

## Working Notes

**2026-05-10 (CX-95):** Backend roadmap reconciled to v4.2. Phase 14 complete — arithmetic, branches, memory ops, direct calls, PtrOffset, print dispatch all execute in JIT. Phase 15 active — no-panic, float, exit-code, PtrOffset/PtrAdd, intrinsic validation, numeric casts all landed. Phase 11 nearly complete — `when` block rejection and method call actual lowering are the two remaining open items. Phase 9 sub-packet 2 done (print family via runtime dispatch). Phase 12 harness operational at 120 fixtures, 0 PARITY_FAILs. CX-91 (cast JIT), CX-93 (fmod libcall), CX-94 (DotAccess parity) in flight on feature branches. Submain 40+ commits ahead of main.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
