# Cx Expression Evaluation Order — v0.1
Status: LOCKED for Cx 0.1

---

## Summary

All expressions in Cx are evaluated **strictly left-to-right**. This rule applies
at every level of nesting. Both the tree-walk interpreter and the IR lowering
implement this order identically, so observable behaviour (including side effects
such as function-call output) is the same on all execution paths.

This document is the authoritative specification for 0.1. It closes the hard
blocker listed in `docs/backend/cx_backend_roadmap_v3_1.md` (line 89):

> Evaluation order for supported expressions is documented and stable —
> assignment side effects match semantic layer behavior exactly

---

## Rule: Left-to-Right Evaluation

For any expression of the form `A ⊕ B` (where `⊕` is any binary operator),
all side effects of evaluating `A` complete before evaluation of `B` begins.

This holds transitively: in `(A ⊕ B) ⊕ C`, the evaluation order is `A`, `B`,
`C` — the parenthesised sub-expression is evaluated in full (left operand of the
outer operator) before `C` (right operand of the outer operator) is evaluated.

The same rule applies to function call argument lists: in `f(A, B, C)`, `A` is
evaluated first, then `B`, then `C`.

---

## Covered Expression Forms

| Form | Evaluation order | Notes |
|------|-----------------|-------|
| `A + B`, `A - B`, `A * B`, `A / B`, `A % B` | A then B | arithmetic |
| `A == B`, `A != B`, `A < B`, `A <= B`, `A > B`, `A >= B` | A then B | comparison |
| `f(A, B, …)` | A then B then … | argument list, left-to-right |
| `(A ⊕ B) ⊕ C` | A then B then C | nested, outermost rule applied recursively |
| `A && B`, `A || B` | A then B (B skipped on short-circuit) | short-circuit logical; lowered via `lower_logical()` in `src/ir/lower.rs` (decision/rhs/sc/merge CFG), fixtures t141/t142 |
| `when X { ... }` (statement and expression) | X then arms left-to-right | chained Compare/Branch CFG via `lower_when_stmt` / `lower_when_expr` in `src/ir/lower.rs` (Option A, landed bed71c1); supports Literal/Range/Bool/Catchall arms + TBool unknown wire-match; fixtures t143/t144/t145 PASS |

**Not covered in 0.1** (unsupported in IR lowering, structured error returned):

- `EnumVariant` arms in `when` — rejected with structured error pending enum lowering

---

## Implementation Evidence

### Interpreter — `src/runtime/runtime.rs`

`eval_semantic_expr`, `SemanticExprKind::Binary` arm (line 684):

```rust
// lhs evaluated first
let l = self.eval_semantic_expr(lhs)?;
// rhs evaluated second — all lhs side effects have completed
let r = self.eval_semantic_expr(rhs)?;
```

`call_semantic_func` (line 1389): arguments resolved via `params.iter().zip(args.iter())`,
which iterates both slices in index order (left-to-right).

### IR Lowering — `src/ir/lower.rs`

`lower_binary` (line 1727):

```rust
// Left operand lowered first — all instructions emitted into active block
let lhs = lower_expr(lhs, ctx, active)?;
// Right operand lowered second
let rhs = lower_expr(rhs, ctx, active)?;
```

Because `lower_expr` emits instructions into `active` (an `ActiveBlock`) as it
recurses, the lhs instruction sequence is appended to the block before the rhs
instruction sequence. This is a structural guarantee: the IR instruction order
is determined by the order in which `lower_expr` calls emit instructions, not
by the order the `IrInst::Binary` struct names its fields.

Call argument lowering (line 1465): `args.iter().enumerate()` iterates left-to-right,
so each argument expression is lowered in declaration order before the next.

---

## Test Coverage

The following verification matrix fixtures confirm the left-to-right guarantee
through observable side effects (a function prints before returning):

| Fixture | What it tests |
|---------|--------------|
| `t114_eval_order_binary_arith.cx` | `f() + g()` — f's print precedes g's print |
| `t115_eval_order_compare.cx` | `f() > g()` — comparison operand order |
| `t116_eval_order_nested.cx` | `(f() + g()) + h()` — nested, three-operand order |

Expected output files (`.cx.expected_output`) provide the ground truth for the
differential harness.

---

## Stability Guarantee

This ordering is a **language-level guarantee** for Cx 0.1. Optimisation passes
(if added post-0.1) must not reorder side-effecting expressions. The IR
instruction order produced by `lower_binary` and the argument-lowering loop
must be preserved through all backend stages.
