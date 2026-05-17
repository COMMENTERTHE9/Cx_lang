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
| ForLoop        | For-in loops                                 | t48, t104 (output-verified); t149, t150 (exit-code-verified, CX-124) |
| InfiniteLoop   | `loop { ... break }` (infinite loop + break) | t25, t106, t134 |
| DirectCall     | Function definitions, calls, return semantics| t02–t08, t14, t29, t50, t113 |
| Struct         | Struct definitions, impl blocks, field access| t36, t39, t40, t43, t109, t110, t114_field_type_mismatch_reject, t115_strref_in_struct_reject, t125–t127 |
| Array          | Array literals and array-of-result           | t33, t112 (output-verified); t146_array_read_exit, t147_array_write_exit, t148_array_in_func_exit (exit-code-verified, CX-121) |
| CompoundAssign | Compound assignment operators (+=, etc.)     | t26, t41, t128, t151, t152, t153 (mixed output/exit-code fixtures; CX-119, CX-187) |
| Unary          | Unary operators (negation, etc.)             | t96 |
| Cast           | Explicit type casts                          | t139, t140 |
| FloatOps       | f64 operations                               | t55, t135–t138 |
| BuiltinAssert  | `assert` and `assert_eq` builtins            | t77–t80 |
| LogicalOps     | Logical AND/OR short-circuit operators       | t141, t142 |
| Other          | Enums, generics, when-blocks, handles, macros, imports, Result/try, string interp, semicolons, copy semantics, and any fixture not matching a named category | t09–t22, t24, t27–t32, t37–t38, t42, t47, t49, t51–t54, t58–t76, t81–t88, t97–t100, t111; exit-code-verified: t143–t145 (CX-113); integration: t154_integration_multifn (CX-182) |

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

Run on branch `train/backend-while-in` (submain as of CX-226 merge window, 2026-05-17).
Includes all prior baselines through CX-196, plus CX-226 while-in JIT lowering
(`lower_while_in_single` in IR lowering + `Op::Mul` cursor-deref unary support),
which moves t34 (while-in exclusive) and t35 (while-in-then) from SKIP to PASS.

```text
Feature                PASS   SKIP  PARITY_FAIL
------------------------------------------------
Arithmetic                8      9            0
VariableDecl              5      3            0
IfElse                    4      2            0
WhileLoop                 7      1            0
ForLoop                   4      0            0
InfiniteLoop              2      1            0
DirectCall                7      4            0
Struct                    6      5            0
Array                     3      2            0
CompoundAssign            4      2            0
Unary                     0      1            0
Cast                      0      2            0
FloatOps                  0      5            0
BuiltinAssert             2      2            0
LogicalOps                2      0            0
Other                    17     48            0
------------------------------------------------
Total: 158 fixtures, 0 PARITY_FAILs
```

### Interpretation

**PASS fixtures** are those where parity is confirmed today:

- **Expected-fail fixtures** in any category exit non-zero (semantic error),
  matching the expectation. Both interpreter and JIT correctly reject them.
- **Exit-code-verified fixtures** use `assert_eq` instead of `print`, so
  their correctness is verified by exit code 0.
- **Output-verified fixtures** whose `print` calls were previously SKIP are
  now PASS following CX-136 print/println dispatch to `cx_printn` for i64
  arguments.
- A small number of **pass-any fixtures** where the JIT happened to compile
  and execute successfully (no stderr error, exit 0) also appear as PASS.

**SKIP fixtures** are those where IR lowering or JIT codegen has not yet been
implemented for the construct used. The primary remaining gaps are constructs
not yet JIT-lowerable (when-blocks, float ops, casts, unary, enums, generics,
handles, and other Phase 14+ constructs). As subsequent phases land, SKIP
counts will continue to decrease and PASS counts will increase.

**PARITY_FAIL = 0** across all 16 categories. The gate holds.

**IfElse parity coverage (CX-102/CX-111/CX-136):** t129 mirrors t44 (basic if/else),
t130 mirrors t45 (if/else in function), t131 mirrors t46 (negated conditions,
including bool-variable negation added in CX-111). The 4 PASS reflect the
3 exit-code-verified fixtures plus one print-based original now passing via
CX-136 print dispatch; the 2 SKIP are the remaining print-based originals.

**WhileLoop parity coverage (CX-102/CX-136/CX-226):** t132 covers basic while loops and
top-level while at file scope; t133 covers while in a function. CX-226 added
`lower_while_in_single` (5-block CFG with counter param, array load/store, and
`Op::Mul` cursor-deref unary lowering), which moves t34 (while-in exclusive) and
t35 (while-in-then with `then in` chain) from SKIP to PASS. The 7 PASS reflect
the 2 exit-code-verified fixtures plus 5 print-based originals; the 1 remaining
SKIP is t23 (`let i;` untyped variable — Numeric type not yet JIT-lowerable).

**WhenBlock parity coverage (CX-113):** t143 mirrors t19 (numeric pattern), t144
mirrors t20 (TBool three-way), t145 mirrors t21 (range pattern). All 3 are SKIP
in JIT (when-block lowering not yet implemented; exits 127). Once when-block
lowering lands, these fixtures will transition from SKIP to PASS without changes.

**CompoundAssign parity (CX-119/CX-187):** t151 tests all five compound-assign
operators (+=, -=, *=, /=, %=) on a typed t64 plain variable, verified by exit
code (CX-119). t152 tests all five operators on a struct field (DotAccess target),
extending t128 which only covered `-=`. t153 tests all five operators on an array
element (Index target), enabled by CX-187 parser support for `arr:[i] op= value`
(added `AssignTarget::Index` to the AST and a new `index_compound_assign` parser
rule). The 4 PASS reflect t128 (struct field -=), t151 (plain variable), t152
(struct field all operators), and t153 (array element all operators); the 2 SKIP
are t26 and t41 (print-based originals that remain SKIP).

**ForLoop parity coverage (CX-124/CX-136):** t149 mirrors t48 (top-level for loop at file scope),
covering exclusive range (`0..5`), empty range (`3..3`, 0 iterations), and inclusive range
(`1..=4`). t150 mirrors t104 (for loop inside a function), covering a sum-accumulator pattern
with a mutable variable carried through the loop. The 4 PASS cover all four ForLoop fixtures:
t149 and t150 (exit-code-verified) plus t48 and t104 (print-based originals now passing via
CX-136 print dispatch). ForLoop has 0 SKIP — full parity coverage across this category.

**Array parity coverage (CX-121):** t146_array_read_exit, t147_array_write_exit, and
t148_array_in_func_exit are exit-code-verified fixtures that exercise stack array allocation,
element read/write, and array passing through function calls. The 3 PASS reflect these three
fixtures passing via the CX-121 ArrayAlloca JIT emit (`IrInst::ArrayAlloca` lowered to
Cranelift via `compute_array_layout`). The 2 SKIP are t33 and t112 (print-based originals
that remain SKIP).

**Integration fixture (CX-182, rebased CX-196):** t154_integration_multifn is a
PassWithOutput fixture exercising two user-defined functions (`sum_up_to` using a while loop,
`abs_diff` using an if/else), arithmetic operations, and three `print` calls verified against
`.expected_output`. It crosses DirectCall + WhileLoop + IfElse + Arithmetic category
boundaries and falls into `Other` via the wildcard. The fixture is PASS via CX-136 print
dispatch; its original number (t152) was taken by the CX-187 CompoundAssign DotAccess exit
fixture so it was renumbered to t154 during the CX-196 rebase.

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
