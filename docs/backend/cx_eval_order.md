# Cx Expression Evaluation Order — v0.1
Status: LOCKED for Cx 0.1

*(Extended 2026-08-24 with the compound-assignment section: single resolution of
the place, and the rule that a compound assignment does not see writes made by
its own operand.)*

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

## Compound Assignment: the place is resolved once

`target op= operand` resolves its place ONCE, reads through it, applies the
operator, and writes back through the same place. Every expression inside the
target and inside the index is therefore evaluated **exactly once**, even though
the statement both reads and writes.

```
a:[f()]:[g()] += 1        f() runs once, g() runs once
a:[f()]:[g()] = a:[f()]:[g()] + 1     f() runs TWICE, g() runs TWICE
```

The two lines are not equivalent, and the difference is observable: the compound
form prints each marker once, the expanded form prints each twice. Both backends
agree, and `t_place_compound_eval_once` asserts it.

This is structural rather than conventional. The JIT resolves one SSA pointer and
both the Load and the Store name that value; the interpreter resolves one
`(root, field, path)` and both the read and the write use it. Re-resolving would
require a second call to the resolver in either backend.

### A compound assignment does not see writes its own operand makes

The read happens **before** the operand is evaluated, and the write-back does not
re-read. If the operand mutates the place being assigned, that mutation is
overwritten:

```
a:[0] += f()      // where f() also writes to a:[0]
```

`a:[0]`'s old value is read, `f()` runs and its write to `a:[0]` lands, then
`old + f()` is stored over it. `f()`'s write is lost.

**This is the stated rule, not an accident** — it follows directly from
read-then-apply-then-write, and both backends emit that order. The JIT's IR for
`a:[0] += side()` shows it directly:

```
v14 = ptr_add v6 + v13     <- the place, resolved once
v15 = load i64 v14         <- read
v16 = call side() -> i64   <- operand, AFTER the read
v17 = add i64 v15, v16
store v14 v17              <- write back, no re-read
```

**Evidence status, stated precisely.** The behaviour above is *executed and
confirmed on the interpreter* (`a:[0] += side()` with `side()` writing `a:[0]`
prints 11, not 999). It is **not** confirmed by execution on the JIT, because
that program does not lower there at all: a function that writes a top-level
variable is an unsupported construct on the Cranelift backend today (exit 127).
So the JIT claim rests on the instruction order above, not on a run. When
top-level mutation from a function lowers, this deserves a parity fixture.

---

## Stability Guarantee

This ordering is a **language-level guarantee** for Cx 0.1. Optimisation passes
(if added post-0.1) must not reorder side-effecting expressions. The IR
instruction order produced by `lower_binary` and the argument-lowering loop
must be preserved through all backend stages.
