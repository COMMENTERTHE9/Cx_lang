# Cx values, receivers and operands — amendment

**Status: REVIEWED. Corrected after its independent review (2026-11-20); the
review's findings and what was done about each are in §9. Implemented by the
commits listed in §6. One rule — V2 item 4 — changes what an owner-decided
text says and is held behind an analysis error until the owner confirms it
(§4, C2).**

- Decided in principle by the owner on 2026-11-19: *value semantics by default,
  with mutation through explicitly defined mechanisms.*
- This document turns that decision into rules precise enough to implement and
  to test, resolves every open case of `docs/known_issues.md` §69 and §57b, and
  lists every place where it changes what an existing program does.
- It amends `docs/backend/cx_eval_order.md` ("Method call", and its list "Where
  this is not the whole story") and known-issues §27 (Model B) and §42. §7
  checks each existing rule and names the texts that are reworded.
- Where this document and an older text disagree, this document is the rule,
  and the older text is corrected to match.

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
- **Non-aggregate.** Everything else: a number, bool, char, string, enum value,
  handle, or `Result` (whatever its payload).
- **Named directly.** An argument that is just a variable's name: `q`, or `(q)`
  — parentheses do not make an expression.
- **Refuses.** Compiled code declines to compile the program, says why, and
  exits 127. The parity harness counts that as SKIP, never as PASS, and every
  refusal of a legal program is recorded in
  `docs/backend/compiled_code_refusals.md`.

## 2. The rules

### V1 — Everything is a value, unless a rule below says otherwise

Binding a name, passing an argument, returning a result and evaluating an
operand each produce a value. For an aggregate that is a copy. This is Model B
(known-issues §27).

A call can change a variable that is not its own in these ways and no others:

1. through a receiver (V2);
2. through a `.copy` argument (V6);
3. by assigning, by name, to a variable of an enclosing scope it can see — a
   module-level variable, or a local of the function a nested function is
   declared in;
4. the built-ins `read(x)` and `input(p, x)`, which assign the variable they
   are given.

### V2 — A receiver bound to a named aggregate variable is that variable

In `x.m(y, …)`:

1. The arguments are evaluated left to right (`cx_eval_order.md`), then the
   method runs.
2. A receiver is **the variable itself** when both hold:
   - its declared type is an aggregate, and
   - what is bound to it is a variable named directly.

   The method then sees the variable as the arguments left it, and whatever the
   method writes to the receiver — a field, an element, or an assignment to the
   receiver as a whole (`s = V { x: 9 }`) — is written to the variable.
3. A further receiver whose declared type is an aggregate, bound to **anything
   else** — a field or element (`w.inner`, `arr:[0]`), a call result, a literal
   — is bound to the **value** of that expression. It is a temporary. The
   method may change it; the change is visible nowhere.
4. A receiver whose declared type is a **non-aggregate** is a value, read at
   its position in the argument list, and is **not** written back.
   *Held for confirmation:* an owner-decided text describes such a receiver as
   written back (§4, C2). Until the owner confirms this item, analysis rejects a
   method that writes a non-aggregate receiver — an assignment to it, a
   compound assignment, passing it as `.copy`, or giving it to `read`/`input`
   — so that no program's result depends on which way the question is answered.

The receiver a written method call is made on (`x`) is a variable named
directly: the grammar has no expression receiver. So for it only item 2
applies; items 3 and 4 concern the further receivers of a multi-receiver
`impl`. If expression receivers (`arr:[0].m()`) are added later, what they bind
is not decided here.

(A method call on something that is not a struct fails loudly on both backends
— in analysis for most shapes, at run time or as a refusal for a few generic
ones. None is silent, and none is changed here.)

The rule that an argument bound to a receiver must have the receiver's declared
type exactly (known-issues §42) is kept for every receiver.

### V3 — An operator's operands are values

Applies to every operator a `phen` defines for the type of its left (or only)
operand: `+ - * / %` (`Add`, `Sub`, `Mul`, `Div`, `Mod`), `==` and `!=` (`Eq`),
`< <= > >=` (`Ord`), and unary `-` (`Neg`).

1. `A` is evaluated, then `B` (LOCKED, unchanged).
2. The operator's method runs on **values**: its receiver is the value `A`
   had when it was evaluated, and its parameter is the value of `B`. Whatever
   the method does to either is visible in no variable and no place.
3. What evaluating `A` or `B` *itself* does through V2 or V6 — a `.copy`
   argument inside `B`, a method call on a named receiver inside `B` — happens
   as usual and is not undone.

So after `r = a + bumpv(a.copy)`, `r` is computed from `a` as it was before
`bumpv` ran, and `a` holds what `bumpv` wrote back.

A method called by name — `a.add(b)` — is a method call, and V2 applies to it:
the receiver is the variable. `a + b` and `a.add(b)` run the same method and
differ in one respect: only the second can change `a`.

### V4 — One variable is at most one receiver of a call

A call in which the same variable is bound, under V2 item 2, to two receivers
is rejected by analysis: `p.mix(p, 5)`, `p.mix((p), 5)`, and `p.mix3(q, q)`
where both leading arguments are receivers. Because only a variable named
directly is bound by reference, this is a comparison of names.

Not rejected, because nothing is bound twice by reference: the receiver's
variable passed to an ordinary parameter (`p.keep(p)`); a field or element of
it bound to a further receiver (`w.absorb(w.inner)`, a temporary under V2 item
3); the same variable given for two non-aggregate receivers (values under V2
item 4).

This is the receiver counterpart of an existing rule: two `.copy` arguments may
not name the same variable (known-issues §29).

### V5 — While a method runs, its receiver's variable is reached only through the receiver

From the moment a method starts until it returns, a variable bound to one of
its receivers under V2 item 2 must not be reached through any reference that
resolves to **the variable's own binding** — in the method's body or in
anything the method calls. The receiver, whatever it is called, is the
receiver: an `impl (top: V)` whose receiver has the same name as the
module-level variable it is called on shadows that variable, and using it is
the ordinary case.

Reaching the variable means any of: reading or assigning it, a field or an
element of it; passing it as `.copy`; binding it as a receiver again
(`top.inc()` inside `top.run()`); naming it in a string interpolation.

Today only a module-level variable can be reached that way, because of three
facts: an `impl` block cannot be declared inside a function; a nested function
cannot be called before its declaration; and a name captured by a nested
function resolves to the nearest activation of the function that declares it.

A program that does so has no defined result, and neither backend may give it
one silently:

- the interpreter stops with a run-time error naming the variable, at the
  moment it is reached (**dynamic**: a program that never takes that path
  runs);
- compiled code refuses the program (**static**: it refuses a function that
  names a module-level aggregate at all, whether or not the path is taken).

The two differ for a program whose offending path is not taken: the
interpreter runs it and compiled code refuses it. That is a refusal, recorded
as one, not a different answer. When compiled code learns to reach a
module-level aggregate from a function, it must keep refusing — or check —
every method call on a module-level variable whose method, or anything it can
call, names that variable.

Detecting this in analysis, so that it is an error before the program runs on
either backend, is the better end state and is not part of this amendment (§8).

### V6 — A `.copy` write-back happens when its call returns, wherever the call is written

`.copy` is copy-in, copy-out (unchanged): the callee works on its own value,
and when the call returns that value is written to the variable the argument
named. "When the call returns" means before evaluation continues, in **every**
position a call can be written. There is no position in which the write is
dropped or deferred. In particular — these are examples, not the list:

- the condition of `if`, `else if` and `while`; the scrutinee of `when`; a
  `when` arm's guard;
- either operand of `&&` and `||`;
- a branch of an `if` or `when` used as a value;
- an argument of a call or method call written in any of those.

In a position that is evaluated conditionally — a guard, the right operand of
`&&`/`||`, a value branch — the call may not run; if it does not, nothing is
written.

The same holds for any other change a value branch makes to a variable (an
assignment in a branch of a function's trailing `if`/`else`): it is visible
after the branch.

### V7 — `.copy.free` and `copy_into` are isolated

Unchanged, restated: neither writes back.

### V8 — `break` and `continue` belong to a loop of the same function

- Unlabeled: the innermost enclosing loop **in the same function or method
  body**.
- Labeled (`break 'L`): the enclosing loop with that label in the same
  function. A label of a loop in an enclosing function is not visible in a
  nested function, and may be reused there.
- Outside such a loop — in a function body, or at the top level of a file —
  `break` and `continue` are rejected by analysis. They never reach a loop of
  the caller.

## 3. Every open case

Results are interpreter / compiled code, on the build before this amendment
(`543dc9d`), each run for this document.

| Case | Program | Before | Rule | After | Who changes |
|---|---|---|---|---|---|
| §69a | `r = a + bumpv(a.copy)`, then `a.x` | 1 / 101 | V3.3 | 101 | interpreter |
| §69a | `r = b + b.grow()`, then `b.x` | 1 / 1001 | V3.3 | 1001 | interpreter |
| §69a | `s = a + b` where `add` sets its receiver's `x` to 100, then `a.x` | 100 / 100 | V3.2 | 1 | **both** — C1 |
| new | `n = -a` where `neg` adds 100 to its receiver, then `a != a` where `eq` adds 1000; `a.x` after each | 101 1101 / 101 1101 | V3.2 | 1 1 | **both** — C1 |
| §69b | `q.grow(n, 1)` where `grow` adds 100 to its number receiver, then `n` | 105 / 5 | V2.4 | rejected by analysis (held, C2) | both |
| §69b | `q.total(m, inc(m.copy))`, then `m` (`inc` adds 7; `total` does not write `m`) | 5 / 12 | V2.4, V6 | 12 | interpreter |
| new | `q.set(res)` where `set` assigns `Ok(99)` to its `Result` receiver, then `res` | Ok(99) / Ok(5) | V2.4 | rejected by analysis (held, C2) | both |
| §69c | `s = ps:[0] + b` where `add` sets its receiver's `x` to 100, then `ps:[0].x` | 1 / 100 | V3.2 | 1 | compiled code |
| §69c | the same with `w.inner + b`, then `w.inner.x` | 1 / 100 | V3.2 | 1 | compiled code |
| §69c | `p.mix(w.inner, 5)` where `mix` adds 1000 to its second receiver, then `w.inner.x` | 2 / 1002 | V2.3 | 2 | compiled code |
| §69c | `p.mix(arr:[0], 5)`, then `arr:[0].x` | 3 / 1003 | V2.3 | 3 | compiled code |
| §69c | `k.bump(h.items)` where `bump` adds 50 to element 1 of its array receiver, then `h.items:[1]` | 10 / 60 | V2.3 | 10 | compiled code |
| new | `v.reset()` where `reset` does `s = V { x: 9 }` on its receiver, then `v.x` | 9 / 1 | V2.2 | 9 | compiled code |
| new | an array further receiver assigned whole (`b = [7, 8, 9]`), then the array | 8 / 10 | V2.2 | 8 | compiled code |
| §69d | `p.mix(p, 5)` | 701, then 101 / 10706, then 106 | V4 | rejected by analysis | both |
| new | `p.mix3(q, q)`, two further receivers | 12102, then 102 / 112112, then 112 | V4 | rejected by analysis | both |
| §69e | a method on module-level `top` whose body calls a function that changes `top` | 1001, then 111 / refused | V5 | run-time error / refused | interpreter |
| §57b, §69f | `if inc(n.copy) > 0 { print(n) }` | 18 / 11 | V6 | 18 | compiled code |
| §69f | `while (inc(n.copy) > 0 && i < 3) { … }`, then `n` | 28 / 0 | V6 | 28 | compiled code |
| §69f | `x = if 1 > 0 { inc(n.copy) } else { 0 }`, then `n` | 18 / 11 | V6 | 18 | compiled code |
| §69f | `b = true && inc(n.copy) > 0`, then `n` | 25 / 11 | V6 | 25 | compiled code |
| new | a `when` guard `1 if inc(n.copy) > 0 => print(n)`, then `b = inc(n.copy) > 0 && n > 7`, then `n` | 7 true 14 / 0 false 0 | V6 | 7 true 14 | compiled code |
| new | a `.copy` parameter assigned in a branch of a function's trailing `if`/`else`, then the caller's variable | 11 / 1 | V6 | 11 | compiled code |
| §69g | `break` (or `continue`) in a function body outside a loop | breaks the caller's loop / refused | V8 | rejected by analysis | interpreter |
| new | `break 'outer` in a function nested in the loop labeled `'outer` | leaves the loop / refused | V8 | rejected by analysis | interpreter |
| new | `break` at the top level outside a loop | run-time error after earlier output / refused | V8 | rejected by analysis | both |

Unchanged and already agreeing, stated so the rules can be checked against
them: a named struct or array bound to a further receiver is the variable
(`p.mix(q, 5)`: 702, then `p.x` 6, `q.x` 102 on both; `k.bump(arr)`: 60 on
both); a call result or a struct literal bound to a further receiver is a
temporary (both); an operator's right operand is a copy (both); every
aggregate `.copy` write-back, in every position (both).

## 4. What changes for existing programs

Each of these is a change in what a program that runs today does. None is made
silently: each has its own commit, its own fixtures, and an entry here.
"Tracked programs" are the 929 `.cx` files in the repository; the review ran
every one on both backends and scanned them for each shape.

**C1 — An operator no longer changes a variable through its operands. Both
backends; an existing fixture's expectation changes.**
`t_op_expr_receiver_mutation` was written as "the canary" for the operator
design and pins, as its first point, that *a named receiver still writes back*:
after `s = a + b`, where `add` sets its receiver's `x` to 100, `a.x` is 100.
Under V3 it is 1. The fixture's expected output changes from
`102 100 104 100 2 4` to `102 1 104 1 2 4` — lines 2 and 4. Its other two
points (a temporary writes back nowhere; operands are evaluated left to right)
are unchanged and still pinned. The same holds for every other dispatched
operator: a `neg`, `eq` or ordering method that changes its receiver no longer
changes the operand's variable.
The texts that say `a + b` *is* `a.add(b)` (`README.md`,
`docs/post_0_1/gene_phen_design.md`, which is marked "design locked", and the
comment on the dispatch in `src/frontend/semantic.rs`) are amended to say what
V3 says: same method, value operands.
*This is the one change that contradicts a text marked locked and a fixture
written to protect it. It follows directly from the owner's decision that
operator operands "must not implicitly write back mutations to their source
variables". No other tracked program is affected.*

**C1b — After an operator whose right operand changes the left operand's
variable, the variable keeps that change on the interpreter.** `a.x` after
`r = a + bumpv(a.copy)` is 101, not 1; `b.x` after `b + b.grow()` is 1001, not
1. The interpreter used to store the operand it had read back into the
variable, undoing what the right operand did. Compiled code already gives 101
and 1001. Two tracked fixtures execute this shape without reading the variable
afterwards; no tracked output changes.

**C2 — A non-aggregate receiver is a value. HELD: needs the owner's
confirmation.** Known-issues §42, decided by the owner on 2026-10-24, says an
impl's extra receivers are passed by address, "numbers included: nothing
converted on the way in could be written back"; the analyzer's comments and its
diagnostic ("an impl's receivers are passed by reference") say the same; and
the interpreter does write a number, bool, string, enum, handle or `Result`
receiver back (`n` after `q.grow(n, 1)` is 105). Compiled code never has
(`n` is 5), `cx_eval_order.md` lists the case as undecided, and a fixture pins
that both backends read such a receiver at its position in the argument list.
The owner's decision of 2026-11-19 — "further method arguments and aliases
should follow value semantics unless the language explicitly grants write-back"
— supports the value reading, and V2 item 4 adopts it; but because §42's own
words can be read as that explicit grant, it is not adopted silently:

- What is implemented now changes no program's result: a method that writes a
  non-aggregate receiver is an **analysis error** on both backends, and a
  method that does not write one behaves identically under either reading.
  The interpreter no longer writes an *unwritten* receiver back, which stops it
  overwriting a `.copy` write-back made in the same argument list (`m` after
  `q.total(m, inc(m.copy))` is 12 on both, as compiled code already had it).
- If the owner confirms the value reading: the analysis error can be dropped
  (the assignment becomes local to the method), and §42's stated reason and the
  diagnostic are reworded.
- If the owner wants write-back instead: the analysis error is replaced by an
  implementation of the write-back in compiled code, and V2 item 4 is reversed.

No tracked method writes a non-aggregate receiver.

**C3 — In compiled code, a field or element bound to a further receiver, or
used as an operator's operand, is a temporary.** The interpreter already
treats it so. No tracked program passes a field, element or expression for an
aggregate further receiver.

**C4 — In compiled code, an assignment to a receiver as a whole reaches the
variable.** `s = V { x: 9 }` inside a method used to rebind the name inside
the method only, and later field writes went to the new storage. The
interpreter already writes the variable. No tracked method assigns its
receiver whole.

**C5 — `p.mix(p, 5)`, and one variable for two further receivers, is an
analysis error.** Both backends ran it, with different results. No tracked
program does it.

**C6 — The interpreter stops when a variable is reached by its own binding
while it is a method's receiver.** It used to carry on with a stale copy and
then overwrite what had been written. No tracked program does it.

**C7 — `break` or `continue` outside a loop of the same function is an
analysis error.** In a function body the interpreter used to break the
caller's loop; a labeled one in a nested function used to leave the enclosing
function's loop; at the top level it was a run-time error after whatever the
program had already printed. Compiled code refused all three. All 17 tracked
files that use `break`/`continue` do so inside a loop of the same function.

**C8 — Compiled code keeps every `.copy` write-back and every assignment made
in a condition, a guard, an operand of `&&`/`||`, or a value branch.** A fix to
compiled code only; the interpreter is already right. Nothing is refused in
its place: the one tracked program with a scalar `.copy` in a value branch,
`t_enum_boundary_valid`, keeps passing.

## 5. What does not change

- `x.m(args)` on a named receiver: the arguments first, then the method on the
  variable as they left it (`k.add(k.next())`; confirmed by the owner).
- A named struct or array bound to a further receiver is the variable.
- `.copy`: copy-in, copy-out; rejected on method calls; two `.copy` arguments
  may not name one variable.
- Aggregates passed to ordinary parameters are copies.
- The order of evaluation of everything.
- `a += b` on a type with a user `+`, and an operator on a generic `T`, are
  analysis errors today and stay so.

## 6. Implementation — one bounded change per rule

Each is a separate commit with fixtures that fail before it, run on both
backends, with the full gates before and after.

| Step | Rule | Change |
|---|---|---|
| 1 | V8 | Analysis rejects `break`/`continue` outside a loop of the same function; loop labels stop at a function boundary |
| 2 | V4 | Analysis rejects one variable bound to two aggregate receivers |
| 3 | V3 | Operators, all of them: the left operand always travels as a value. The interpreter evaluates it at its position and writes nothing back; compiled code gives the method a copy of it unless it is already a fresh value or the method cannot write its receiver and the right operand makes no call. C1's fixture change is in this commit |
| 4 | V2.3 | Compiled code binds a field, element or other non-name expression given for a further aggregate receiver by value |
| 5 | V2.2 | Compiled code stores an assignment to a receiver as a whole into the receiver's storage |
| 6 | V2.4 | Analysis rejects a method that writes a non-aggregate receiver (held, C2); the interpreter writes back aggregate receivers only |
| 7 | V6 | Compiled code: branches start from the bindings the condition, scrutinee or guard left; `&&`/`||`, `if` and `when` values merge the variables their paths changed |
| 8 | V5 | The interpreter reports reaching a variable in use as a receiver; compiled code's refusal says what it refuses |

"Cannot write its receiver" in step 3 is decided syntactically, with no
analysis of what a callee does. A method can write its receiver if its body or
trailing expression contains any of: an assignment rooted at the receiver
(whole, field or element); the receiver passed as `.copy`; a method call on
it; it passed to a method as an argument that is not a number, bool, string or
enum; or a nested function declaration. When in doubt the operand is copied —
that costs a copy and nothing else.

## 7. Checked against the existing rules

| Rule | Source | Effect of this amendment |
|---|---|---|
| Strict left-to-right evaluation; `A + B`: A then B | `cx_eval_order.md` (LOCKED) | None. V3 keeps it and adds what happens to the operands afterwards |
| Index read: the array, then the index | `cx_eval_order.md` (LOCKED) | None. Its sentence that a further alias "is passed by reference" is narrowed to a struct or array variable named directly |
| Method call: arguments, then the method on its receiver | `cx_eval_order.md` (LOCKED), §65 | Kept (V2.1, V2.2). The bullets under "Where this is not the whole story" are resolved by V3, V2.4, V2.3 (further receivers) and V3.2 (operator operands), V4, V5 in that order, and the list is replaced by a pointer to this document |
| Compound assignment resolves its place once | `cx_eval_order.md` (LOCKED) | None |
| Model B: aggregates are values; the receiver is the one exception | known-issues §27 (ruling) | Kept. V2 states which bindings the exception covers. §27's heading calls the exemption "a property of the parameter, not of its position"; after V2.3 and V3 it also depends on what is bound — a variable named directly — and on the call being a method call rather than an operator. Noted there |
| Every name in an `impl` header is a receiver | §27, fixture `t177_multi_alias_impl_exit` | Kept for aggregates bound to named variables (V2.2) |
| An extra receiver's argument must have its declared type exactly, "numbers included: nothing converted on the way in could be written back" | §42 (owner-decided) | **The rule is kept. Its stated reason is contradicted by V2.4 for non-aggregate receivers** — change C2, held for the owner. The analyzer's comments and diagnostic that say the same are reworded when C2 is confirmed |
| `.copy` is copy-in/copy-out, by address, written back at every exit | §30 (decision), `copy_shapes.md` | None; V6 states when. `copy_shapes.md`'s "Method receiver parameters retain their established mutation behavior" now points here |
| `.copy` is rejected on method calls; repeated `.copy` targets are rejected | §29 (rulings) | None; V4 is modelled on the second |
| Operators are implemented as genes; `a + b` dispatches to the type's method | `gene_phen_design.md` ("design locked"), `README.md` | The dispatch is kept. "`a + b` *is* `a.add(b)`" and the statement that a named left operand is written back are replaced by V3 — change C1 |
| An unknown `while`/`if` condition is refused | §54 (owner ruling) | None |
| A `.copy` write-back made in a condition is lost in compiled code | §57b (open) | Closed by V6 |

## 8. Not decided here

- **V2 item 4** — whether a non-aggregate receiver is a value (adopted, held)
  or is written back (§42's wording). The owner's call; see C2.
- Whether analysis should detect V5 statically, so that it is an error on both
  backends before the program runs.
- What an expression receiver binds, if the grammar ever gets one.
- The cost of V3 in compiled code when an operator's method can write its
  receiver, or its right operand makes a call: a copy of the left operand per
  use. An operator whose method only reads its receiver, applied to operands
  that make no call, costs nothing extra. That holds while compiled code
  refuses to reach a module-level aggregate from a function; when it stops
  refusing, an operator on a module-level variable needs the copy as well.
- The "smaller notes" of §69 are not addressed: a `return` nested in a
  function's trailing `if`/`else` value runs on the interpreter and is refused
  by compiled code with a misleading message; the cost of an index read from a
  large array field.
- §57c (a guarded catch-all followed by another arm) is a separate defect of
  `when` lowering and is not a question of values and aliases.

## 9. The independent review, and what was done

The first draft (`f40aec1`) was reviewed against the owner's decision, the
locked rules, the code, and all 929 tracked programs. Verdict: *ready after the
listed corrections*. Each finding was re-run before being accepted.

| # | Finding | Done |
|---|---|---|
| 1 (blocking) | The draft said scalar receivers "were never covered" by the existing rulings and that §42 was unaffected. False: §42, the analyzer and a fixture comment describe every extra receiver, numbers included, as written back | §7 rows corrected; C2 states the conflict and holds the rule behind an analysis error for the owner |
| 2 | A whole assignment to a receiver is lost in compiled code (9 / 1) | V2.2 names it; §3 rows; C4; step 5 |
| 3 | `Result` and handle receivers were unclassified, and diverge today (Ok(99) / Ok(5)) | "Non-aggregate" defined in §1; V2.4 covers them; §3 row |
| 4 | V3 covered only `A op B` | V3 lists every dispatched operator, unary `-` and `!=` included; §3 row |
| 5 | "Whenever the method can write its receiver" was undefined | Defined under §6 |
| 6 | V5 condemned a receiver with the same name as its variable; did not list what counts; did not say it is dynamic on one backend and static on the other | V5 rewritten |
| 7 | V6's list was incomplete (guards, the left operand of `&&`, `else if`), and "keep or refuse" is not a rule | V6 is stated for every position; nothing is refused; C8 |
| 8 | V8 ignored labels and nested functions; top-level `break` | V8 rewritten; C7 |
| 9 | V1's "exactly three ways" was false | V1 lists four |
| 10 | V4 did not say which receivers, two further receivers, or `(p)` | V4 rewritten |
| 11 | No entry for the interpreter change after `a + f(a.copy)` | C1b |
| notes | Texts needing rewording; "refuses" undefined; `a += b`; the §69 smaller notes | §1, §5, §7, §8 |
