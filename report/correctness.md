# Pillar 2 — Correctness: Where the Language Quietly Misbehaves

**Audit base:** `submain` @ `22015c420ad40483d0531d1109fa9837d44deca3`.
**Date:** 2026-05-24.
**Method:** systematic differential probing — every probe run on both the interpreter (default backend, the **shipped** path) and the Cranelift JIT (`--backend=cranelift`), comparing stdout + exit code. A JIT `exit 127` is a lowering SKIP (pending implementation, not a bug). A *disagreement* is: both paths terminate with different output, **or** different non-127 exit codes. Per the Pillar-2 framing, disagreement-hunting focused on constructs both backends can run end to end; SKIP-classified fixtures were not chased.
**Repros:** every finding has a minimal program in `report/pillar2_repros/`. They live there (not in `src/tests/verification_matrix/`) deliberately — adding currently-failing fixtures to the authoritative matrix would break the green 189/0 baseline mid-audit. Each should be **promoted to a verification_matrix fixture when its bug is fixed** (correct `.expected_output` / `.expected_fail` noted per finding).

---

## Severity summary

| ID | Finding | Where | Severity |
|---|---|---|---|
| A1 | `f64→t8`/`t16` cast **panics** Cranelift codegen | JIT | High (crash) |
| A2 | Array OOB **read** returns garbage, no bounds check | JIT | High (soundness) |
| A3 | Array OOB **write** → **SIGSEGV** | JIT | High (soundness/crash) |
| A4 | Negative array index returns garbage | JIT | High (soundness) |
| A5 | `INT_MIN / -1` → **SIGFPE** | JIT | High (crash) |
| A6 | Runtime integer **div-by-zero** → **SIGILL** | JIT | High (crash) |
| A7 | Deep recursion **aborts** (SIGABRT), not catchable | Interp | Medium (robustness) |
| B1 | Struct literal **missing field** silently accepted | Interp | High (type soundness) |
| B2 | Struct **field type mismatch** silently accepted | Interp | High (type soundness) |
| B3 | Struct literal **extra/unknown field** silently ignored | Both | Medium (laxness) |
| B4 | Array literal **element types unchecked** | Interp | High (type soundness) |
| C1 | `if <unknown>` silently takes `else` | Interp | Medium (design) |
| C2 | Non-exhaustive `when` silently no-ops | Interp | Medium (design) |
| C3 | Out-of-range literal assignment silently wraps | Interp | Low (design) |
| C4 | Float `/0.0` raises error instead of IEEE `inf` | Interp | Low (design) |
| C5 | `b == b` on unknown yields `?`, not reflexive `true` | Interp | Low (design) |
| C6 | Nested-block shadowing rejected as "already declared" | Interp | Low (design) |
| D1–D4 | Misleading error messages (see §Error messages) | Interp | Medium |

> **Key framing:** Category A is JIT-only — the **shipped default interpreter is memory-safe and traps these cleanly**. Category A is therefore *not* an emergency in shipped behavior, but it is a wall of high-severity bugs the JIT must fix before it can be the default. Category B is the more urgent class: these are **type-soundness holes in the shipped interpreter** — programs that are wrong but run silently.

---

## Category A — Interpreter/JIT disagreements (all JIT-side)

A consistent root theme: **the JIT omits the safety checks the interpreter performs**, and emits raw machine instructions that trap or read/write out of bounds.

- **A1 — `f64→t8`/`f64→t16` cast panics codegen.** `report/pillar2_repros/A1_*`. Interp saturates (`300.9→127` for t8, `300` for t16, via Rust `as`); JIT panics inside cranelift-codegen (`assertion failed: dst_size.is_one_of(...)`, exit 2). `f64→t32`/`t64` lower fine — only the 8/16-bit narrowing is unsupported. *Fix:* lower narrow float→int as `fcvt_to_sint`→i32 then `ireduce`. Promote as a parity fixture once both saturate identically.
- **A2 — OOB array read → garbage.** `A2_*`. `a:[5]` on a 3-element array: interp errors (exit 1); JIT prints uninitialized heap (`exit 0`). Information-disclosure-class soundness hole.
- **A3 — OOB array write → SIGSEGV.** `A3_*`. `a:[5] = 99`: interp errors; JIT writes past the allocation → segfault (exit 139). Memory-corruption-class hole.
- **A4 — negative index → garbage.** `A4_*`. `a:[-1]`: interp errors; JIT prints garbage (exit 0).
- **A5 — `INT_MIN / -1` → SIGFPE (exit 136).** `A5_*`. Interp guards via `if b == -1 { a.wrapping_neg() }` (`runtime.rs:516`) → `INT_MIN`; JIT emits a bare `sdiv` which traps on the one overflowing case. (`INT_MIN % -1` is fine on both — interp returns 0 at `runtime.rs:536`, JIT agrees.)
- **A6 — runtime div-by-zero → SIGILL (exit 132).** `A6_*`. With a zero *variable* divisor, interp raises a clean `DivByZero`; JIT's unguarded `sdiv` traps. (A literal `/0` is caught earlier; the gap is the runtime value path.)
- **A7 — deep recursion aborts the interpreter.** `A7_*`. `rec(100000)`: interp overflows its 64 MB thread stack and **SIGABRTs** (exit 134, "has overflowed its stack") — uncatchable, because Cx calls use native Rust recursion (see Pillar 1 §3 / `main.rs:76-88`). The JIT runs it fine (exit 0 → 100000). So the interpreter has a far lower, hard-aborting recursion ceiling. *Direction:* either an explicit Cx-level depth guard that returns a catchable error before the host stack overflows, or (longer term) an explicit interpreter stack.

## Category B — Interpreter type-soundness holes (shipped default — highest priority)

Semantic analysis does **not** type-check the contents of composite literals. These programs are statically wrong but run to completion with no diagnostic.

- **B1 — missing struct field accepted.** `B1_*`. `P { a: 1 }` for `struct P { a, b }` → no error; later `p.b` access yields the misleading "p.b not declared" message (D2). *Expected:* `.expected_fail` (semantic "missing field 'b'"). The JIT *does* catch this at lowering (exit 127), so the check exists conceptually — it's just absent from the semantic phase.
- **B2 — wrong field type accepted.** `B2_*`. `b: t64` initialized with `"hello"` → prints `hello`. No type check on struct-literal field values. *Expected:* `.expected_fail`.
- **B3 — extra field ignored (both backends).** `B3_*`. `P { a: 1, c: 99 }` where `P` has only `a` → silently drops `c`, prints `1`. *Expected:* `.expected_fail` ("no field 'c'").
- **B4 — array element types unchecked.** `B4_*`. `[1, "two", 3]` for `[3: t64]` → prints `two`. Note array **length** *is* checked (`[1,2]` for `[3:t64]` correctly errors), so this is specifically missing element-type validation. *Expected:* `.expected_fail`.

These four share one root cause and should be fixed together: **add type-and-completeness validation of struct/array literals in `src/frontend/semantic.rs`** (struct: every declared field present, no unknown fields, each value assignable to its field type; array: each element assignable to the element type). This is the single highest-value correctness work surfaced by Pillar 2.

## Category C — Design footguns (intended? — surface for a decision)

These are internally consistent and may be deliberate, but each can silently hide a bug. Listed for an explicit 0.2 ruling rather than as defects.

- **C1 — `if <unknown>` → `else`.** `C1_*`. An unknown (`?`) condition silently takes the false branch. The language has `when … unknown =>` precisely for three-valued dispatch; `if` collapsing unknown→false may be surprising. *Option:* require `when` for unknown-typed conditions, or warn.
- **C2 — non-exhaustive `when` no-ops.** `C2_*`. No matching arm and no `_` → the `when` does nothing, no error/warning. *Option:* exhaustiveness check or a mandatory `_`.
- **C3 — out-of-range literal wraps silently.** `x: t8 = 250` → `-6`, `return 300` for a `t8` fn → `44`, with no diagnostic. This is the *intended* model (fixture `t89` asserts it) but emitting no warning for a literal that visibly can't fit is a footgun. (The JIT instead rejects `return 300` — backend strictness mismatch.)
- **C4 — float `/0.0` errors.** `1.0/0.0` raises `DivByZero` rather than yielding IEEE `+inf` (`runtime.rs:519`). Defensible for a safety-oriented engine language, but it means `inf` is unreachable via division. Worth an explicit decision.
- **C5 — `b == b` on unknown → `?`.** Equality of an unknown with itself is unknown, not reflexively true. Consistent with three-valued logic; flagged only because it can surprise.
- **C6 — nested-block shadowing rejected.** `x` redeclared inside an `if` block errors "variable already declared in this scope" — the if-block is treated as the same scope. If no-shadowing is intended, the message wording ("in this scope") is misleading for a nested block.

## Error messages to rewrite (proposed text)

- **D1 — array index out of bounds** (`runtime.rs:391, 739, 875`). Currently the bounds error is constructed as `RuntimeError::UndefinedVar { name: format!("index {} out of bounds for '{}'", …) }`, which renders as:
  > `variable 'index 5 out of bounds for 'a'' has not been declared — declare it with 'index 5 out of bounds for 'a': TYPE = value' before use`
  Nonsensical "declare it" advice. **Proposed:** add `RuntimeError::IndexOutOfBounds { pos, name, index, len }` with diagnostic:
  > `index 5 is out of bounds for array 'a' of length 3 (valid indices 0..2)`
  (Moderate change: new enum variant + diagnostic arm + `BackendError` mapping; proposed, not done, because it ripples beyond a one-liner.)
- **D2 — access of an unset struct field** (consequence of B1). Currently:
  > `variable 'p.b' has not been declared — declare it with 'p.b: TYPE = value' before use`
  **Proposed (once B1 is fixed this becomes a compile error; until then):**
  > `field 'b' of 'p' was never initialized`
- **D3 — integer literal exceeds i128** (`src/frontend/lexer.rs`). `report/pillar2_repros/D3_*`. Currently:
  > `unrecognized token "…728" — this character is not valid in Cx`
  It is a valid token, merely out of range. **Proposed:**
  > `integer literal exceeds the range of t128 (max 170141183460469231731687303715884105727)`
- **D4 — cascade after a failed declaration.** A failed typed declaration (e.g. array length mismatch) leaves the variable unregistered, so subsequent uses emit a spurious `use of undeclared variable 'a'`. Low priority; suppress downstream "undeclared" errors for names whose declaration already errored.

## What was checked and found correct

Worth recording so the next pass doesn't re-tread: division/mod by zero (literal) — caught; `INT_MIN % -1` — 0 on both; `t8/t16/t64` overflow wrapping — consistent across backends and per-expression by inferred type (A-class casts excepted); `-0.0 == 0.0` → true; `t64`/`t128` literal boundaries; empty/unicode/escaped strings (interp); use-before-assignment — caught at semantic; nested-scope handle survival and **stale-handle access — cleanly trapped, exit 1** (the handle/arena model has prior dedicated coverage in `examples/audit_memory/AUDIT_REPORT.md` and held up in spot checks); array **length** validation.

## Recommendations for 0.2

1. **Fix Category B together** — composite-literal type/completeness validation in the semantic phase. Highest-value: closes shipped-interpreter soundness holes. (4 fixtures ready to promote.)
2. **JIT safety checks (Category A)** — bounds checks (A2–A4), guarded `sdiv`/`srem` for div-by-zero and `INT_MIN/-1` (A5, A6), and narrow float→int lowering (A1). Backend-owned; coordinate with backend dev. These gate the JIT becoming default.
3. **Rule on Category C** — pick intended semantics for `if unknown`, `when` exhaustiveness, and out-of-range-literal warnings; document them.
4. **Rewrite D1–D4** — the index-OOB message (D1) is the worst offender and is hit by any real array-using program.
5. **Recursion ceiling (A7)** — add a catchable depth guard so the interpreter degrades gracefully instead of SIGABRT.
