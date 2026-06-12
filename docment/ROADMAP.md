# Cx Project Roadmap — Living Summary

Last updated: 2026-06-12

This file is a concise synthesis of the project's roadmap state. Detailed roadmaps live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.0 on submain)

---

## Frontend — v0.1.0 Released

All 9 hard blockers resolved. 182/182 matrix tests passing. 8/8 examples passing.

**Status:** v0.1.0 released (tagged at 9fc0d24). No known soundness holes. Syntax frozen.

**Known limitations (documented, not blocking):**
- String arena grows monotonically (interpreter-only)
- No strref constructor syntax
- Expression statements still require semicolons

**Post-release hardening (on submain):**
- [x] Composite literal type-checking — struct field presence/type/unknown-field validation, array element type checking (8169d33)
- [x] CR#1–4: range-check literals through generic type args, array elements in struct fields/call args, return values, if/when branch tails; unified width checking through single semantic-level function (b8e92fb..cbba042)
- [x] D1.1b: carry element type through array index read-expressions — live interpreter bug fix, sub-64-bit array arithmetic now wraps correctly (aadbd55)
- [x] D1.1: unify integer-width facts into `frontend/int_facts.rs` — single source of truth replacing 3 hand-synced copies (a5c930d)

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
  - [x] Method call actual lowering (0ab7e9b — synthesis-and-recurse via Call arm)
  - [ ] `when` block lowering or structured rejection
  - [ ] DotAccess in compound forms
- [ ] Phase 8 Round 2 — str/strref layout, Handle<T>, TBool calling convention

### Landed (integrated to main via v0.1.0 merge)

- [x] Phase 13 — Cranelift lowering skeleton (CX-22)
- [x] JIT Host Boundary (CX-24: process ownership, exit codes, output capture)
- [ ] Phase 12 — Differential harness (parity classification CX-69, loop fixtures CX-68, determinism tests CX-55 merged; CX-228 adds t159–t177 parity fixtures; D1.0 pins div-zero/INT_MIN/dual-bool JIT divergences bb3823a; more in flight)
- [ ] Phase 9 — Runtime intrinsics boundary (assert/assert_eq lowered natively via CX-48; print/println/printn/read/input still pending)
- [ ] Phase 14 — First executable Cranelift slice (CX-52 float comparison, CX-53 void return, CX-54 debug-trace gating merged)
- [ ] Phase 15 — Cranelift JIT 0.1 target (CX-74 exit-code propagation merged; print arg widening 08fa2f9; literal-width narrowing complete across 5 operator sites; CX-57/58/60/63/64/66 instruction coverage in flight; 171 PASS / 101 SKIP / 0 PARITY_FAIL across 272 fixtures on submain)

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

**2026-06-12:** Quiet day — no commits on any branch since June 11 01:24 UTC. Submain 8 commits ahead of main (CR#1–4 + D1.0/perf-rider/D1.1b/D1.1). Matrix 230/0 on main, 272/0 on submain. Daily-log backlog: branches June 5–10 unmerged.

**2026-06-10:** D1 audit arc opened on submain. D1.0 pins 5 JIT divergence fixtures (div-zero, INT_MIN, dual-bool). Performance baseline established (bench/ directory, 8 programs, JIT ~9-13x faster). D1.1b fixes live array-index element type bug. D1.1 extracts integer-width facts into `frontend/int_facts.rs`. Matrix 272/0 on submain; main unchanged at 230/0. Submain 8 commits ahead of main.

**2026-05-18:** PR #268 merged `train/backend-determinism` → submain (host_boundary expansion, IR lowering fixes, 23 new parity fixtures including CX-228 t159–t177). CX-233 implements while-in loop source-to-IR lowering on `stokowski/CX-233` (branch-local, not yet merged) — WhileLoop parity moves to 8/0. Submain 171 commits ahead of main.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
