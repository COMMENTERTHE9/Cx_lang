# Cx Project Roadmap — Living Summary

Last updated: 2026-07-03

This file is a concise synthesis of the project's roadmap state. Detailed roadmaps live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.0 on submain)

---

## Frontend — v0.3.0 Released

All 9 hard blockers resolved. 292/292 matrix tests passing. 8/8 examples passing.

**Status:** v0.3.0 released (tagged at 1654f5b, PR #326). No known soundness holes. Syntax frozen.

**Known limitations (documented, not blocking):**
- String arena grows monotonically (interpreter-only)
- No strref constructor syntax
- Expression statements still require semicolons

**Post-release hardening (landed on main via v0.3.0):**
- [x] Composite literal type-checking — struct field presence/type/unknown-field validation, array element type checking (8169d33)
- [x] Range-check hardening — array elements in struct fields/call args (CR#1–2), return values at declared width (CR#3), branch tails in if/when (CR#4)
- [x] Integer-width facts refactoring — unified width table in frontend (D1.1)
- [x] Array element type carry-through in index reads (D1.1b)

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
- [x] Phase 10 — Loop lowering (while, for, break, continue, labeled break/continue)
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
  - [x] `when` block lowering — when stmt/expr pattern emission unified (D1.2a)
  - [x] If-expression lowering via branch-value merge (D2.1)
  - [x] Tag-only enum lowering — construct + match via variant_id (D2.2)
  - [x] Bool/TBool lowering — unknown construct, condition trap, Kleene logic (D2.x-A1/A2)
  - [x] Static string lowering — repr, literal construct, len fold, concat, equality, interpolation (D2.3a–d)
  - [x] Result<T> lowering — packed-i128 repr, construct, print, `?`/Try operator (D2.4a/b)
  - [x] Labeled break/continue — frontend parse + dual backend execution (f94c6a5, 0f56f1e)
  - [ ] DotAccess in compound forms
- [ ] Phase 8 Round 2 — str/strref layout, Handle<T>, TBool calling convention (static strings partially covered by D2.3)

### Landed (integrated to main via v0.3.0 merge)

- [x] Phase 13 — Cranelift lowering skeleton (CX-22)
- [x] JIT Host Boundary (CX-24: process ownership, exit codes, output capture, clean trap routing via Gate-1b0)
- [ ] Phase 12 — Differential harness (parity classification CX-69, loop fixtures CX-68, determinism tests CX-55 merged; CX-228 adds t159–t177 parity fixtures; D1.0 pins div-zero/INT_MIN/dual-bool divergences)
- [ ] Phase 9 — Runtime intrinsics boundary (assert/assert_eq lowered natively via CX-48; cx_print_str/cx_print_f64 added via D2.3b/D2.3d; read/input still pending)
- [ ] Phase 14 — First executable Cranelift slice (CX-52 float comparison, CX-53 void return, CX-54 debug-trace gating merged; Gate-1a INT_MIN/-1 guard, Gate-1b div/mod-by-zero trap, Gate-2a width narrowing, Gate-2b array bounds checking)
- [ ] Phase 15 — Cranelift JIT 0.1 target (233 PASS / 59 SKIP / 0 PARITY_FAIL across 292 fixtures; benchmark baseline added via perf-rider)

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

**2026-07-03:** PR #326 merged submain → main, tagged v0.3.0. 26 commits, 148 files changed, 3441 insertions, 478 deletions. Major additions: if-expressions, tag-only enums + when matching, three-state bool, static strings (len/concat/equality/interpolation), Result<T> + `?` operator, labeled break/continue, safety gates (INT_MIN/-1, div/mod-by-zero, array bounds, width narrowing), range-check hardening (CR#1–4), benchmark suite. Matrix: 292/292 on main. Post-merge: submain has 1 additional commit (6e9e41e — Handle+array composition test, retracted audit finding).

**2026-05-18:** PR #268 merged `train/backend-determinism` → submain (host_boundary expansion, IR lowering fixes, 23 new parity fixtures including CX-228 t159–t177). CX-233 implements while-in loop source-to-IR lowering on `stokowski/CX-233` (branch-local, not yet merged) — WhileLoop parity moves to 8/0. Submain 171 commits ahead of main.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
