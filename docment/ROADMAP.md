# Cx Project Roadmap — Living Summary

Last updated: 2026-07-04

This file is a concise synthesis of the project's roadmap state. Detailed roadmaps live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.0 on submain)

---

## Frontend — v0.3.0 Released

All 9 hard blockers resolved. 292/292 matrix tests passing on main (v0.3.0). 8/8 examples passing.

**Status:** v0.3.0 released (tagged at 1654f5b, PR #326). No known soundness holes. Syntax frozen.

**Known limitations (documented, not blocking):**
- String arena grows monotonically (interpreter-only)
- No strref constructor syntax
- Expression statements still require semicolons

**Post-release hardening (on submain):**
- [x] Composite literal type-checking — struct field presence/type/unknown-field validation, array element type checking (8169d33)
- [x] Range-check hardening — generic type args, array elements, return values, branch tails (CR#1–4, on submain)
- [x] Arithmetic safety gates — INT_MIN/-1 overflow, div-by-zero trap, value narrowing, bounds-check array indexing (Gate-1a/1b/2a/2b, on submain)

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
  - [x] `when` block lowering (D1.2a refactor, D2.1 if-expressions, D2.2 tag-only enums — landed on submain)
  - [x] Unknown/TBool lowering (D2.x-A1, D2.x-A2 — landed on submain)
  - [x] len() constant folding on static strings/arrays (D2.3a — landed on submain)
  - [x] String literal lowering + cx_print_str (D2.3b — landed on submain)
  - [x] String concat folding + content equality (D2.3c — landed on submain)
  - [x] String interpolation (print-time) + f64 print (D2.3d — landed on submain; static string subset complete)
  - [x] Result<T> construct + print — packed-i128 rep, memory round-trip (D2.4a — landed on submain)
  - [x] `?`/Try operator on Result — unwrap Ok or early-return Err (D2.4b — landed on submain)
  - [ ] DotAccess in compound forms
- [ ] Phase 8 Round 2 — str/strref layout, Handle<T>, TBool calling convention
  - [x] Handle<T> construct — packed i64, scalar payloads only (D2.5a — landed on submain)
  - [x] Handle<T> read (.val) — widen to I128, Trap path for stale handles (D2.5b — landed on submain)
  - [ ] Handle<T> drop (.drop) — generational reuse, stale-handle empirical proof (D2.5c)
  - [ ] str/strref layout
  - [ ] TBool calling convention

### Landed (integrated to main via v0.3.0 merge)

- [x] Phase 13 — Cranelift lowering skeleton (CX-22)
- [x] JIT Host Boundary (CX-24: process ownership, exit codes, output capture; clean trap routing Gate-1b0)
- [ ] Phase 12 — Differential harness (parity classification CX-69, loop fixtures CX-68, determinism tests CX-55 merged; CX-228 adds t159–t177 parity fixtures; more in flight)
- [ ] Phase 9 — Runtime intrinsics boundary (assert/assert_eq lowered natively via CX-48; print/println for int/bool/str/f64 lowered via cx_printn/cx_print_bool/cx_print_str/cx_print_f64_inline; read/input still pending)
- [ ] Phase 14 — First executable Cranelift slice (CX-52 float comparison, CX-53 void return, CX-54 debug-trace gating merged; Gate-1a/1b/2a/2b safety items)
- [ ] Phase 15 — Cranelift JIT 0.1 target (CX-74 exit-code propagation merged; print arg widening 08fa2f9; literal-width narrowing complete across 5 operator sites; CX-57/58/60/63/64/66 instruction coverage in flight; 237 PASS / 60 SKIP / 0 PARITY_FAIL across 297 fixtures (submain); main at 292/292 matrix)

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

**2026-07-04:** D2.5a + D2.5b landed on submain. Handle.new packs {slot,gen} into i64 with a stateful HandleRegistry host-side; Handle.val reads via cx_handle_val with out-parameter validity, widens to I128, Trap on stale. Three new canary fixtures (val_positive, val_negative, val_bool) + one construct fixture. Matrix 297/0, parity 237/60/0. HandleDrop (D2.5c) next.

**2026-07-03:** PR #326 merged submain → main, tagged v0.3.0. 26 commits, 148 files, 3441 insertions. Closes a 28-day integration gap. Post-merge audit: Handle+array composition test added (6e9e41e), retracting Finding 2 (invalid arr[i] syntax in audit, correct syntax is arr:[i]).

**2026-06-27:** D2.4a + D2.4b landed on submain. Result<T> is a packed i128 (tag high, payload low) with memory-round-trip construction; `?` operator unpacks and early-returns Err. Parity 229/58/0 across 287. Submain 24 commits ahead of main. Cranelift `enable_llvm_abi_extensions` ISA flag now required (host_boundary.rs). New canary fixture `t_result_ok_negative` pins negative-payload tag integrity. Example files rewritten (uncommitted) with tutorial documentation.

**2026-05-18:** PR #268 merged `train/backend-determinism` → submain (host_boundary expansion, IR lowering fixes, 23 new parity fixtures including CX-228 t159–t177). CX-233 implements while-in loop source-to-IR lowering on `stokowski/CX-233` (branch-local, not yet merged) — WhileLoop parity moves to 8/0. Submain 171 commits ahead of main.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
