# Cx values, receivers and operands — amendment

**Status: PROPOSED — under independent review. No code implements it yet.**

- Decided in principle by the owner on 2026-11-19: *value semantics by default,
  with mutation through explicitly defined mechanisms.*
- This document turns that decision into rules precise enough to implement and
  to test, resolves every open case of `docs/known_issues.md` §69, and lists
  every place where it changes what an existing program does.
- It amends `docs/backend/cx_eval_order.md` ("Method call", and its list "Where
  this is not the whole story") and clarifies known-issues §27 (Model B). It
  does not change any other LOCKED rule; §7 below checks each one.
- Where this document and an older text disagree, this document is the rule
  once it is approved, and the older text is corrected to match.

## 1. Words

- **Value.** What an expression evaluates to. A struct or array value is a
  copy: changing it changes nothing else.
- **Variable.** A named binding: a local, a parameter, or a module-level name.
- **Place.** A variable, or a field or element reached from one: `p`,
  `w.inner`, `arr:[0]`, `g.items:[i]`.
- **Receiver.** One of the names an `impl` block declares in its header.
  `ops: impl (a: V, b: V)` declares two. In `p.mix(q, 5)` the first is bound to
  `p` and the second to the leading argument `q`. A `phen` block declares one.
- **Aggregate.** A struct or an array.
- **Scalar.** A number, bool, char, string or enum value.

## 2. The rules

### V1 — Everything is a value, unless a rule below says otherwise

Binding a name, passing an argument, returning a result and evaluating an
operand each produce a value. For an aggregate that is a copy. This is Model B
(known-issues §27), unchanged.

The language has exactly three ways for a call to change a variable of its
caller. They are V2 (a receiver), V6 (`.copy`), and assignment to a
module-level variable by name. Nothing else writes back.

### V2 — A receiver bound to a named aggregate variable is that variable

In `x.m(y, …)`:

1. The arguments are evaluated left to right (`cx_eval_order.md`), then the
   method runs.
2. A receiver is **the variable itself** when both hold:
   - its declared type is an aggregate, and
   - what is bound to it is a variable named directly — `x`, `y`.

   The method then sees the variable as the arguments left it, and what the
   method writes through the receiver is written to the variable.
3. A receiver whose declared type is an aggregate, bound to **anything else** —
   a field or element (`w.inner`, `arr:[0]`), a call result, a literal — is
   bound to the **value** of that expression. It is a temporary. The method may
   change it; the change is visible nowhere.
4. A receiver whose declared type is a **scalar** is a value, read at its
   position in the argument list. The method may assign to it; the assignment
   is local and is **not** written back.

The receiver the method is called on (`x`) is always a variable named directly
and always a struct — a written method call cannot have an expression receiver,
and analysis rejects a method call on a non-struct — so for it only item 2
applies. Items 3 and 4 concern the further receivers of a multi-receiver
`impl`.

Why scalars are values: Model B's exemption is about aggregates ("Aggregates
are values … the method receiver is the one declared exception"); both
backends already read a scalar receiver at its position in the argument list
(known-issues §68 b, fixture `t_method_scalar_alias_at_argument_position`);
and the one thing they disagree about, the interpreter writing it back, is
pinned by no fixture and overwrites a `.copy` write-back made in the same
argument list (§69 b).

The rule that an argument bound to a receiver must have the receiver's declared
type exactly (known-issues §42) is unchanged.

### V3 — An operator's operands are values

`A op B`, where `op` is defined for the type by a `phen`:

1. `A` is evaluated, then `B` (LOCKED, unchanged).
2. The operator's method runs on **values**: its receiver is a copy of `A`'s
   value and its parameter a copy of `B`'s. Whatever the method does to either
   is visible in no variable and no place.
3. What evaluating `A` or `B` *itself* does through V2 or V6 — a `.copy`
   argument inside `B`, a method call on a named receiver inside `B` — happens
   as usual and is not undone.

So after `r = a + bumpv(a.copy)`, `r` is computed from `a` as it was before
`bumpv` ran, and `a` holds what `bumpv` wrote back.

A method called by name — `a.add(b)` — is a method call, and V2 applies to it:
the receiver is the variable. `a + b` and `a.add(b)` therefore differ in one
respect: only the second can change `a`.

### V4 — One variable is at most one receiver of a call

A call in which the same variable is bound to two receivers — `p.mix(p, 5)` —
is rejected by analysis. Because only a variable named directly is bound by
reference (V2 item 2), this is a comparison of names.

Passing the receiver's variable to an ordinary parameter (`p.keep(p)` where
the parameter is not a receiver), or as part of an expression that is bound by
value (V2 item 3), is allowed: those are copies.

This is the receiver counterpart of an existing rule: two `.copy` arguments may
not name the same variable (known-issues §29).

### V5 — While a method runs, its receiver variable is reached only through the receiver

From the moment the method starts until it returns, a variable that is bound
as a receiver (V2 item 2) must not be read or written by its own name — in the
method's body or in anything the method calls. Only a module-level variable
can be reached that way.

A program that does so has no defined result, and neither backend may give it
one silently:

- the interpreter stops with a run-time error naming the variable;
- compiled code refuses the program.

(Today the interpreter works on a copy of the receiver and stores it back when
the method returns, so such a program reads a stale value and then has its
write overwritten; compiled code cannot lower a by-name access to a
module-level struct from a function at all and refuses, with a message that
reads like an internal error.)

Detecting this in analysis instead — so that it is an error before the program
runs — is a possible later refinement, not part of this amendment.

### V6 — `.copy` writes back when its call returns, wherever the call is written

`.copy` is copy-in, copy-out (unchanged): the callee works on its own value,
and when the call returns that value is written to the variable the argument
named. "When the call returns" means before evaluation continues, in every
position a call can be written:

- the condition of `if` and `while`, and the scrutinee of `when`;
- a branch of an `if` or `when` used as a value;
- the right operand of `&&` and `||`.

In the last two the call may not run; if it does not, nothing is written.

### V7 — `.copy.free` and `copy_into` are isolated

Unchanged, restated: neither writes back.

### V8 — `break` and `continue` belong to a loop of the same function

`break` and `continue` refer to the innermost enclosing loop in the same
function or method body. Outside such a loop they are rejected by analysis.
They never reach a loop of the caller.

## 3. Every open case of §69

Results are interpreter / compiled code, on the build before this amendment.

| §69 | Program | Today | Rule | After | Who changes |
|---|---|---|---|---|---|
| a | `r = a + bumpv(a.copy)`, then `a.x` | 1 / 101 | V3.3 | 101 | interpreter |
| a | `r = b + b.grow()`, then `b.x` | 1 / 1001 | V3.3 | 1001 | interpreter |
| a | `s = a + b` where `add` sets its receiver's `x` to 100, then `a.x` | 100 / 100 | V3.2 | 1 | **both** — see C1 |
| b | `q.grow(n, 1)` where `grow` adds 100 to its scalar receiver, then `n` | 105 / 5 | V2.4 | 5 | interpreter |
| b | `q.total(m, inc(m.copy))`, then `m` (`inc` adds 7) | 5 / 12 | V2.4, V6 | 12 | interpreter |
| c | `s = ps:[0] + b` where `add` sets its receiver's `x` to 100, then `ps:[0].x` | 1 / 100 | V3.2 | 1 | compiled code |
| c | the same with `w.inner + b`, then `w.inner.x` | 1 / 100 | V3.2 | 1 | compiled code |
| c | `p.mix(w.inner, 5)` where `mix` adds 1000 to its second receiver, then `w.inner.x` | 2 / 1002 | V2.3 | 2 | compiled code |
| c | `p.mix(arr:[0], 5)`, then `arr:[0].x` | 3 / 1003 | V2.3 | 3 | compiled code |
| c | `k.bump(h.items)` where `bump` adds 50 to element 1 of its array receiver, then `h.items:[1]` | 10 / 60 | V2.3 | 10 | compiled code |
| d | `p.mix(p, 5)` | 701, then 101 / 10706, then 106 | V4 | rejected by analysis | both |
| e | a method on module-level `top` whose body calls a function that changes `top` | 1001, then 111 / refused | V5 | run-time error / refused | interpreter; compiled code's message |
| f | `if inc(n.copy) > 0 { print(n) }` | 18 / 11 | V6 | 18 | compiled code |
| f | `while (inc(n.copy) > 0 && i < 3) { … }`, then `n` | 28 / 0 | V6 | 28 | compiled code |
| f | `x = if 1 > 0 { inc(n.copy) } else { 0 }`, then `n` | 18 / 11 | V6 | 18 | compiled code |
| f | `b = true && inc(n.copy) > 0`, then `n` | 25 / 11 | V6 | 25 | compiled code |
| g | `break` (or `continue`) in a function body outside a loop | breaks the caller's loop / refused | V8 | rejected by analysis | interpreter |

Unchanged and already agreeing, stated so the rules can be checked against
them: a named struct or array bound to a further receiver is the variable
(`p.mix(q, 5)`: 702, then `p.x` 6, `q.x` 102 on both; `k.bump(arr)`: 60 on
both); a call result or a struct literal bound to a further receiver is a
temporary (both); an operator's `rhs` is a copy (both).

## 4. What changes for existing programs

Each of these is a change in what a program that runs today does. None is made
silently: each gets its own commit, its own fixture, and an entry here.

**C1 — An operator no longer changes its left operand's variable. Both
backends; an existing fixture's expectation changes.**
`t_op_expr_receiver_mutation` was written as "the canary" for the operator
design and pins, as its first point, that *a named receiver still writes back*:
after `s = a + b`, where `add` sets its receiver's `x` to 100, `a.x` is 100.
Under V3 it is 1. The fixture's expected output changes from
`102 100 104 100 2 4` to `102 1 104 1 2 4` — lines 2 and 4. Its other two
points (a temporary writes back nowhere; operands are evaluated left to right)
are unchanged and still pinned.
The texts that say `a + b` *is* `a.add(b)` (`README.md`,
`docs/post_0_1/gene_phen_design.md`, which is marked "design locked") are
amended to say what V3 says: same method, value operands.
*This is the one change that contradicts a text marked locked and a fixture
written to protect it. It follows directly from the owner's decision that
operator operands "must not implicitly write back mutations to their source
variables", and is listed first so that it is approved knowingly.*

**C2 — The interpreter no longer writes a scalar receiver back.** `n` after
`q.grow(n, 1)` is 5, not 105. Compiled code already gives 5. No fixture pins
either answer.

**C3 — In compiled code, a field or element bound to a further receiver, or
used as an operator's left operand, is a temporary.** The interpreter already
treats it so. No fixture pins compiled code's current answer.

**C4 — `p.mix(p, 5)` is an analysis error.** Both backends ran it, with
different results. No fixture uses it.

**C5 — The interpreter stops when a variable is reached by name while it is a
method's receiver.** It used to carry on with a stale copy. No fixture is known
to do this; the corpus replay for the implementing commit is the check.

**C6 — `break` or `continue` outside a loop is an analysis error.** The
interpreter used to break the caller's loop. Compiled code already refused.

**C7 — Compiled code keeps a scalar `.copy` write-back made in a condition, a
value branch, or the right of `&&`/`||`.** A fix to compiled code only; the
interpreter is already right. If keeping the write-back in a conditionally
evaluated position (a value branch, the right of `&&`/`||`) cannot be done
soundly within a bounded change, compiled code refuses such a program instead
of dropping the write, and the refusal is recorded as a limitation.

## 5. What does not change

- `x.m(args)` on a named receiver: the arguments first, then the method on the
  variable as they left it (`k.add(k.next())`; confirmed by the owner).
- A named struct or array bound to a further receiver is the variable.
- `.copy`: copy-in, copy-out; rejected on method calls; two `.copy` arguments
  may not name one variable.
- Aggregates passed to ordinary parameters are copies.
- The order of evaluation of everything.

## 6. Implementation plan — one bounded change per rule

Each is a separate commit with a fixture that fails before it, run on both
backends, with the full gates, the corpus replay and the reviewers' generators
(methods/`.copy`/indexing; operators) before and after.

| Step | Rule | Change |
|---|---|---|
| 1 | V8 | Analysis rejects `break`/`continue` outside a loop of the same function |
| 2 | V4 | Analysis rejects one variable bound to two receivers |
| 3 | V3 | Operators: interpreter stops writing the left operand back; compiled code gives the method a copy of the left operand whenever the method can write its receiver. C1's fixture change is in this commit |
| 4 | V2.4 | Interpreter stops writing a scalar receiver back |
| 5 | V2.3 | Compiled code binds a field, element or other non-name expression given for a further receiver by value |
| 6 | V6 | Compiled code keeps `.copy` write-backs made in conditions and scrutinees; in value branches and the right of `&&`/`||` it keeps them or refuses |
| 7 | V5 | Interpreter reports by-name access to a variable in use as a receiver; compiled code's refusal says what it refuses |

## 7. Checked against the existing locked rules

| Rule | Source | Effect of this amendment |
|---|---|---|
| Strict left-to-right evaluation; `A + B`: A then B | `cx_eval_order.md` (LOCKED) | None. V3 keeps it and adds what happens to the operands afterwards |
| Index read: the array, then the index | `cx_eval_order.md` (LOCKED) | None |
| Method call: arguments, then the method on its receiver | `cx_eval_order.md` (LOCKED), §65 | Kept (V2.1, V2.2). The five bullets under "Where this is not the whole story" are resolved by V3, V2.4, V2.3, V4, V5 in that order, and the list is removed |
| Compound assignment resolves its place once | `cx_eval_order.md` (LOCKED) | None |
| Model B: aggregates are values; the receiver is the one exception | known-issues §27 (ruling) | Kept. V2 states which bindings the exception covers: a named aggregate variable bound to a receiver. Scalars were never covered by its wording |
| Every name in an `impl` header is a receiver | §27, fixture `t177_multi_alias_impl_exit` | Kept for aggregates bound to named variables (V2.2) |
| A receiver's argument must have its declared type exactly | §42 | None |
| `.copy` is copy-in/copy-out, by address, written back at every exit | §30 (decision), `copy_shapes.md` | None; V6 states when |
| `.copy` is rejected on method calls; repeated `.copy` targets are rejected | §29 (rulings) | None; V4 is modelled on the second |
| Operators are implemented as genes; `a + b` dispatches to the type's method | `gene_phen_design.md` ("design locked") | The dispatch is kept. The statement that a named left operand is written back is replaced by V3 — change C1 |
| An unknown `while`/`if` condition is refused | §54 (owner ruling) | None |

## 8. Not decided here

- Whether analysis should detect V5 statically.
- Whether a scalar should ever be bindable by reference to a method. Under
  this amendment the only way for a call to change a caller's scalar is `.copy`
  to a function, or assignment to a module-level variable by name.
- The cost of V3 in compiled code when an operator's method writes its
  receiver: a copy of the left operand per use. An operator that does not write
  its receiver costs nothing extra.
