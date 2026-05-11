# CX JIT Parity Checklist — Phase 12

**Ticket:** CX-69  
**Roadmap:** Phase 12, Differential Backend Harness — "Per-feature parity checklist"

This document records the per-feature JIT parity classification scheme, the
current baseline from a real run, and the gate semantics used by the
`jit_parity_by_feature` test.

---

## 1. Feature Classification Scheme

Every fixture in `src/tests/verification_matrix/` maps to exactly one
`FeatureCategory` variant. The mapping is defined by `feature_of()` in
`src/diff_harness.rs`. The 16 categories cover the full 0.1 supported construct
set:

| Category       | Description                                  | Key fixtures |
|----------------|----------------------------------------------|--------------|
| Arithmetic     | Integer arithmetic, overflow, eval order     | t01, t89–t95, t103, t114_eval_order_binary_arith, t115_eval_order_compare, t116–t121 |
| VariableDecl   | Variable/const declarations, scope, type errors | t15, t56, t57, t101, t102, t122–t124 |
| IfElse         | Conditional branches                         | t44, t45, t46 (output-verified); t129, t130, t131 (exit-code-verified, CX-102/CX-111) |
| WhileLoop      | While loops and while-in construct           | t23, t34, t35, t105, t107, t108 (output-verified); t132, t133 (exit-code-verified, CX-102) |
| ForLoop        | For-in loops                                 | t48, t104 |
| InfiniteLoop   | `loop { ... break }` (infinite loop + break) | t25, t106, t134 |
| DirectCall     | Function definitions, calls, return semantics| t02–t08, t14, t29, t50, t113 |
| Struct         | Struct definitions, impl blocks, field access| t36, t39, t40, t43, t109, t110, t114_field_type_mismatch_reject, t115_strref_in_struct_reject, t125–t127 |
| Array          | Array literals and array-of-result           | t33, t112 |
| CompoundAssign | Compound assignment operators (+=, etc.)     | t26, t41, t128 |
| Unary          | Unary operators (negation, etc.)             | t96, t143, t144 |
| Cast           | Explicit type casts                          | t139, t140 |
| FloatOps       | f64 operations                               | t55, t135–t138 |
| BuiltinAssert  | `assert` and `assert_eq` builtins            | t77–t80 |
| LogicalOps     | Logical AND/OR short-circuit operators       | t141, t142 |
| Other          | Enums, generics, when-blocks, handles, macros, imports, Result/try, string interp, semicolons, copy semantics, and any fixture not matching a named category | t09–t22, t24, t27–t32, t37–t38, t42, t47, t49, t51–t54, t58–t76, t81–t88, t97–t100, t111 |

Fixtures not explicitly listed in `feature_of()` fall into `Other`.

---

## 2. PASS / SKIP / PARITY_FAIL Semantics

Each fixture is run through `Cx_0V --backend=cranelift <fixture>` as a
subprocess. The outcome is classified as follows:

### SKIP

A fixture is SKIP when the JIT backend could not compile or execute it due to
an unsupported construct. Two signals indicate SKIP:

1. **Exit code 127** (`JitExitCode::UNSUPPORTED_CONSTRUCT`): the binary
   propagated the unsupported-construct sentinel to the process exit code.
   This is the forward-compatible path once the binary is updated to call
   `std::process::exit(127)` on JIT codegen failure.

2. **Exit code 0 with non-empty stderr**: the IR lowering or JIT codegen step
   failed, printed an error message to stderr, and returned without running the
   Cx program. The process exits 0 because `CraneliftBackend::execute` returns
   `Err(msg)` which is logged via `eprintln!` in `main.rs` without setting an
   exit code. A non-empty stderr distinguishes this from a successful JIT run
   that produced no stdout output. Expected-fail (semantic-error) fixtures take
   a different path — they fail before reaching the JIT and exit non-zero via
   `std::process::exit(1)` — so they are not mistakenly classified as SKIP.

SKIP is not a failure. It records that the JIT has not yet gained coverage for
that construct and is expected during early Phase 12–14 development.

### PASS

A fixture is PASS when the JIT outcome matches the stored expectation:

- **Expected-fail** fixture: the subprocess exits non-zero (semantic analysis
  rejected the program, as expected). Both interpreter and JIT reject it.
- **Pass-any** fixture: the subprocess exits 0 with empty stderr (the JIT ran
  the program and it returned 0).
- **Pass-with-output** fixture: the subprocess exits 0 with empty stderr and
  stdout (after CRLF normalisation and trailing-whitespace trim) matches the
  stored `.expected_output` content.

### PARITY_FAIL

A fixture is PARITY_FAIL when the JIT outcome diverges from the stored
expectation and neither SKIP signal is set:

- Expected-fail fixture that the JIT accepted (exit 0, empty stderr)
- Pass-any fixture that the JIT rejected (exit non-zero, empty stderr)
- Pass-with-output fixture where stdout does not match (either wrong output or
  JIT crashed after having already written partial output to stdout)

A non-zero PARITY_FAIL count in any category causes `jit_parity_by_feature`
to fail. PARITY_FAIL represents a real divergence between the JIT and the
expected program behavior and must be investigated and fixed.

---

## 3. Current Per-Feature Baseline

Captured from:

```bash
cargo build --features jit && cargo test --features jit jit_parity_by_feature -- --nocapture
```

Run on branch `stokowski/CX-116` (submain as of CX-116 merge window, 2026-05-11).
Includes exit-code-verified fixtures added in CX-102 (t129–t134), CX-105/CX-107 LogicalOps
fixtures (t141–t142), the CX-111 bool-variable negation extension to t131, and CX-116
Unary parity fixtures (t143–t144).

```text
Feature                PASS   SKIP  PARITY_FAIL
------------------------------------------------
Arithmetic                6     11            0
VariableDecl              5      3            0
IfElse                    3      3            0
WhileLoop                 2      6            0
ForLoop                   0      2            0
InfiniteLoop              1      2            0
DirectCall                5      6            0
Struct                    5      6            0
Array                     0      2            0
CompoundAssign            1      2            0
Unary                     2      1            0
Cast                      0      2            0
FloatOps                  0      5            0
BuiltinAssert             2      2            0
LogicalOps                2      0            0
Other                    13     48            0
------------------------------------------------
Total: 148 fixtures, 0 PARITY_FAILs
```

### Interpretation

**PASS fixtures** are those where parity is confirmed today:

- **Expected-fail fixtures** in any category exit non-zero (semantic error),
  matching the expectation. Both interpreter and JIT correctly reject them.
- **Exit-code-verified fixtures** (t117–t144) use `assert_eq` instead of
  `print`, so their correctness is verified by exit code 0. These pass
  even though the `print` builtin is not yet JIT-lowerable (Phase 9 pending).
- A small number of **pass-any fixtures** where the JIT happened to compile
  and execute successfully (no stderr error, exit 0) also appear as PASS.

**SKIP fixtures** are those where IR lowering or JIT codegen has not yet been
implemented for the construct used. The primary gap remains that the `print`
builtin is not lowerable to IR (Phase 9 pending), affecting all output-verified
fixtures. As Phase 9, Phase 14, and subsequent phases land, SKIP counts will
decrease and PASS counts will increase.

**PARITY_FAIL = 0** across all 16 categories. The gate holds.

**IfElse parity coverage (CX-102/CX-111):** t129 mirrors t44 (basic if/else),
t130 mirrors t45 (if/else in function), t131 mirrors t46 (negated conditions,
including bool-variable negation added in CX-111). The 3 PASS reflect the
exit-code-verified set; the 3 SKIP are the print-based originals.

**WhileLoop parity coverage (CX-102):** t132 covers basic while loops and
top-level while at file scope; t133 covers while in a function. The 2 PASS
reflect the exit-code-verified set; the 6 SKIP include print-based originals
and while-in/while-in-then constructs (not yet JIT-lowerable).

**Unary parity coverage (CX-116):** t143 covers integer arithmetic negation
(`-x`) verified by summing `−x + x = 0`; t144 covers boolean NOT (`!x`)
storing the result in a bool variable. The 2 PASS reflect exit-code-verified
fixtures; the 1 SKIP is t96 (print-based, awaiting Phase 9).

---

## 4. Gate Criteria

`jit_parity_by_feature` (in `src/diff_harness.rs`) enforces:

- **Zero PARITY_FAILs** across all feature categories.
- Any non-zero PARITY_FAIL count causes the test to fail with a diagnostic
  table showing which categories diverged.

SKIP counts are informational only — they do not cause the test to fail.

Run the gate with:

```bash
cargo build --features jit && cargo test --features jit jit_parity_by_feature --nocapture
```

Or as part of the full suite:

```bash
cargo build --features jit && cargo test --features jit
```

---

## 5. Updating This Document

When new Phase 14+ work lands and JIT coverage expands:

1. Run `cargo test --features jit jit_parity_by_feature --nocapture` to
   capture the new baseline.
2. Update the table in Section 3 with the new counts.
3. Update the interpretation note to reflect which new categories have moved
   from SKIP to PASS.

When new fixtures are added to the verification matrix, update the
classification table in Section 1 and the `feature_of()` function in
`src/diff_harness.rs` to keep every fixture mapped to exactly one category.
