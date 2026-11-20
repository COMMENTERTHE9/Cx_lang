# Cx — the owner's decisions of 2026-11-20, as rules

**Status: IMPLEMENTED on the pull-request stack #504 / #505 / #507 (commits in
§8); not merged. Reviewed independently as a document on 2026-11-20; the
review's findings are folded in below and listed in §9.**

Six questions were put to the owner by the report on pull requests #499–#507.
This document turns each answer into a rule precise enough to implement and
test, says what changes for existing programs, and lists the implementation
steps. It amends `docs/backend/cx_value_and_alias_semantics.md` (rules V2 and
V5), `docs/backend/module_state.md`, and known-issues §42, §43, §74, §75,
§76f, §76h, §77 and §78; the generic rule is recorded as known-issues §81.

"Interpreter / compiled" below are results on the build before these changes
(`a277db2`), each run for this document.

The rules are numbered D1–D6 after the decisions. They are not the refusal
classes D1–D5 of `docs/backend/compiled_code_refusals.md`, which are older and
unrelated; where one of those is meant it is called a *register entry*.

## D1 — A further receiver that is not a struct or an array is a value

*Owner: "These types should behave as values by default. Passing one as an
additional method receiver must not unexpectedly modify the caller's variable.
Keep the existing, documented behavior for named struct and array receivers."*

1. A receiver of an `impl` whose declared type is a number, bool, char,
   string, enum, handle or `Result` is a **value**: the method gets the value
   the argument had at its position in the argument list, and nothing is ever
   written back to the caller's variable. (Rule V2 item 4, no longer held.)
2. Inside the method such a receiver is an ordinary local. The method **may
   assign it**, compound-assign it, pass it as `.copy` and give it to
   `read`/`input`; all of that stays inside the method. The temporary analysis
   error (`reject_value_receiver_write`) is removed.
3. Because it is a value, its argument is checked and converted **as an
   ordinary argument is**: a `t8` variable may be given for a `t64` receiver
   (and the method then works at the receiver's width: 120 `+= 100` is 220), a
   `Result` by the payload rule (D3). The exact-type requirement of §42
   remains for struct and array receivers, which are passed by reference.
4. Nothing changes for a struct or array receiver given a variable by name:
   it is that variable.

`.copy` is unchanged: a caller that wants a call to change its number passes
it `.copy` to a function. No new syntax.

| Program | Before | After |
|---|---|---|
| `q.grow(n, 1)` where `grow` does `k += 100` on its `t64` receiver; then `n` | analysis error (held) | 5 on both; `grow` returns 105 |
| the same where the caller's variable is itself called `k` | analysis error (held) | 5 on both |
| `q.set(res)` where `set` assigns `Ok(99)` to its `Result` receiver; then `res` | analysis error (held) | `Ok(5)` on both |
| `q.grow(n8, 1)` with `n8: t8 = 120` and a `t64` receiver | analysis error (exact type) | 220 on both; `n8` is 120 |
| a `[2: t8]` given for a `[2: t64]` receiver | analysis error (exact type) | unchanged |

## D2 — Inside a generic function a type parameter is a type of its own

*Owner: "If a function declares a value as type T, the compiler must enforce
the actual type represented by T. … Implement the checks in semantic
analysis. … Preserve legitimate generic operations and type inference."*

Within the body of a generic function, for each of **its own** type
parameters `T`:

1. **Where `T` is declared, only a `T` fits.** A value whose type is not `T`
   — a literal, a variable of a concrete type, a value of another type
   parameter `U`, a `Result` or an array of something that is not `T` where
   `Result<T>` or `[N: T]` is declared, the result of a function that returns
   nothing — may not be used where `T` is declared: in a typed declaration,
   an assignment, a `return` or trailing expression, an `Ok` payload, a
   struct field, an argument for a parameter whose type the call has made
   `T`, a gene method's `Self` on a `T` value, and **each** branch of an `if`
   or `when` used as a value (the value is typed by its first branch, so the
   others are checked on their own). It is an analysis error in the body,
   whatever the function is called with — including a call that gives `T`
   exactly the type of that value. A function that makes a `T` out of
   something else is not generic in `T`.
2. **A value whose type is, or holds, `T` may be used where another type is
   declared** — `n: t64 = x`, `s: str = x`, `phase = x` for an enum variable,
   a `Result<T>` returned as a `Result<t64>`. That is right only for some
   `T`, so it is **checked for every call**: with `T` replaced by what it is
   there, the value must fit by the rule for concrete types —
   - the same type (a string, a bool, an enum, a struct, an array);
   - any number where a number is declared, which is converted as it is
     today (known-issues §43);
   - a `Result` by the payload rule of §75, which converts nothing: a
     `Result<T>` is a `Result<t8>` only in a call whose `T` is a t8.

   Otherwise the call is an analysis error. An array of `T` is never an array
   of another element type, in any call — the existing rule of §42.
3. **A number literal as an element of an array literal whose other elements
   are `T`** stays legal (`[x, 1]` is a `[2: T]`, §42) and is checked for
   every call: a whole-number literal needs `T` to be a whole-number type the
   literal fits; a float literal needs `T` to be `f64`. Otherwise the call is
   an analysis error. That includes a whole number beside an `f64` `T`, which
   printed the same on both backends but was an integer on the interpreter
   and could not then be bound to an `f64`.
4. Unchanged, here as everywhere: an `Err(..)` is a `Result` of every type,
   `Result<T>` included; a value whose type analysis does not know — the
   unknown value `?`, the result of an untyped expression — is not checked;
   a call that binds `T` to `?`, or leaves it unbound, is checked for nothing.

"Every call" is defined by what the program says, not by what runs: every
call written outside a generic function, and every generic function reached
from it through calls, each with the types it is reached with. The error is
reported at that outermost call, once per thing the body does, and names the
function, the types and what the body does with them. A function that calls
itself with an ever larger type (`g([x])` inside `g`) is followed to a
nesting depth of 24; compiled code refuses such a program at 64
instantiations.

Not covered, and unchanged:

- A type parameter of a generic **struct**. A variable's type is `Box`,
  without its type argument, so the type of `b.v` is not known and is matched
  with everything — a concrete type and a function's own `T` alike, inside a
  function or at top level. This is a silent difference today for a number
  narrowed on the way (300 / 44; known-issues §82 a, found by this document's
  review and present on `submain`). Closing it needs struct type arguments in
  the type system, which is a separate piece of work.
- A `T` value where a type is needed but none is declared: an `if` condition,
  the operand of `!`, an array index. Analysis does not check these for a
  concrete type either (§57 a, §82 b).

| Program | Before (interpreter / compiled) | After |
|---|---|---|
| `fnc: T <T> f(x: T) { return "not a T" }`, `print(f(5))` | prints `not a T` / refused | analysis error |
| `y: T = 5` in a generic body, called with a string | 5 / refused | analysis error |
| the same, called only with a t64 | 5 / 5 | **analysis error** |
| `z: U = x` with `x: T`, called with two t64 | 5 / 5 | **analysis error** |
| `pick(x, 5)` inside a generic body, `pick(a: T, b: T)`, called with a t8 | 5 / refused | analysis error |
| `fnc: T <T> launder(x: T) { return Ok(5) }` | `Ok(5)` under any declared type / refused | analysis error |
| `fnc: Result<T> <T> mk(x: T) { return Ok(5) }` read as a `Result<str>` | `Ok(5)` / refused | analysis error |
| `return if c { x } else { 300 }`, called with a t8 | 300 / refused | analysis error |
| `n: t64 = x` with `x: T`, called with a number | agree | unchanged |
| the same, called with a string | prints, then a run-time error / refused | analysis error at the call |
| `s: str = x`, called with a string; `phase = x`, called with that enum | agree | unchanged |
| the same, called with a number | a run-time error for the string, the number printed as an enum / refused | analysis error at the call |
| `fnc: Result<t8> <T> conv(r: Result<T>) { return r }`, called with a `Result<t64>` | `Ok(300)` / refused | analysis error at the call |
| `c: [2: T] = [x, 349]`, called with a `t8` | 349 / refused (93 on `submain`) | analysis error at the call |
| `c: [2: T] = [x, 34]`, called with a `t8` or `t64` | 34 / 34 | unchanged |
| the same, called with an `f64`, the element printed | 34 / 34 | **analysis error at the call** |
| the same, the element bound to an `f64` | type mismatch / 34 | analysis error at the call |
| `id(x)`, `[x, x]`, `Ok(x)`, `twice(x)` calling `id(id(x))` | agree | unchanged |

The rows in bold ran the same on both backends and are now rejected.

## D3 — A constant whole-number expression takes a declared `Result` payload type when it provably fits

*Owner: "An already-typed Result<t64> must not silently narrow into
Result<t8>. However, if an integer constant expression is provably within the
target type's range, I want the same safe contextual conversion behavior that
Cx already permits for ordinary integer declarations. … Preserve polymorphic
Err values. If the compiler cannot prove a conversion safe, produce a clear
analysis error."*

1. Where a `Result<W>` with a whole-number payload type `W` is declared and
   the value written is `Ok(E)`, and `E` is a **constant whole-number
   expression**, analysis computes its value. If it fits `W`, the payload is
   that value at type `W`; if it does not, it is an analysis error naming the
   value and the type.
2. A constant whole-number expression is defined by its form: a whole-number
   literal; or `-E`, `E + E`, `E - E`, `E * E`, `E / E`, `E % E` or `(E)`,
   where each `E` is itself one. `/` and `%` round toward zero, as both
   backends do. Variables, `const` names and calls are not part of one.
3. Analysis follows the expression only where the two backends cannot differ:
   every intermediate value must fit a `t64`, and a divisor must not be zero.
   An expression that steps outside that — `9223372036854775807 + 1`,
   `6 / 0` — **is not a constant for this rule** and is treated as any other
   expression of its type, exactly as before: at a `Result<t64>` it is
   accepted and evaluated at run time (the first wraps, the second stops, on
   both backends); at a narrower payload type it is the type error of item 4.
4. Everything else is unchanged: an `Ok` of a value whose type is already
   declared is not narrowed (`Result<t64>` is never a `Result<t8>`), and
   `Err(..)` is a `Result` of every type.
5. "Declared" includes a type settled by the call: in
   `unwrap_or(Ok(1 + 2), a8)`, where another argument makes the type parameter
   a t8, the constant is a t8.

Three deliberate differences from an ordinary declaration, which the owner's
words ("provably", "cannot prove … analysis error") call for:

- `x: t8 = 150 + 150` is accepted and wraps to 44; `Ok(150 + 150)` at a
  `Result<t8>` is an error. A payload is converted only when its safety is
  established.
- `x: t8 = 9223372036854775807 + 1 - 9223372036854775807` is 1 on both
  backends, by wrapping; as a `Result<t8>` payload it is an error, because
  analysis does not follow a step outside a t64.
- At a declared `Result<f64>`, only a literal is converted (`Ok(1)` is 1.0,
  as §75 already had it); a constant expression is not (`Ok(1 + 2)` is an
  error). The decision is about whole-number payload types.

An array payload's elements follow the array rule, not this one:
`r: Result<[2: t8]> = Ok([150 + 150, 3])` holds 44 and 3, as
`xs: [2: t8] = [150 + 150, 3]` does.

| Program | Before | After |
|---|---|---|
| `c: Result<t8> = Ok(1 + 2)` | analysis error | `Ok(3)` on both |
| `x: Result<t8> = if c { Ok(1) } else { Ok(1 + 2) }` | analysis error | `Ok(3)` or `Ok(1)` on both |
| `c: Result<t8> = Ok(-7 / 2)`, `Ok(-1000 % 300)` | analysis error | `Ok(-3)`, `Ok(-100)` on both |
| `c: Result<t8> = Ok(100 + 100)` | analysis error (type) | analysis error: 200 does not fit `t8` |
| `c: Result<t8> = Ok(n)` with `n: t64` | analysis error | unchanged |
| `c: Result<t64> = Ok(9223372036854775807 + 1)` | wraps, on both | unchanged |

## D4 — A `when` used as a value needs an unguarded `_`

*Owner: "Keep the current rule. A when expression that produces a value must
include an unguarded _ fallback unless the compiler has a sound, implemented
way to prove that every possible case is covered."*

The rule is unchanged: a `when` that produces a value is exhaustive only if
it has a `_` arm without a guard, or — the one proof that is implemented —
unguarded arms for `true`, `false` and `unknown` **on a bool**. A `when`
statement keeps the lenient rule. Recorded in known-issues §77.

One correction to the implementation was needed for the rule to be what it
says. The three-state proof did not look at the type of the value matched, so
it was not sound: `x: t64 = when n { true => 1, false => 2, unknown => 3 }`
with a t64 `n` was accepted, no arm matches a number, the interpreter yielded
0 and compiled code trapped. The proof now counts only when the value's
static type is `bool`. No exhaustiveness system is added.

| Program | Before (interpreter / compiled) | After |
|---|---|---|
| `when b { true => 1, false => 2, unknown => 3 }` as a value, `b: bool` | agree | unchanged |
| the same on a t64, a string, an enum, a `Result`, a generic `T` | 0, and carries on / trap | analysis error |
| the same as a statement | nothing runs, on both | unchanged |

## D5 — Both Windows CI jobs are permanent

*Owner: "KEEP BOTH WINDOWS CI JOBS PERMANENTLY. … Do not remove or weaken the
Windows checks to make builds pass."* The jobs "Windows Frontend Build +
Matrix" and "Windows Backend Build + Tests" in `.github/workflows/ci.yml` run
on every push and pull request, on the same footing as the Linux jobs: neither
has a skip condition or `continue-on-error`. They are not to be removed,
skipped or made non-blocking to get a build through. (Whether a failing job
*blocks a merge* is a branch-protection setting of the repository, which this
file does not control.)

## D6 — A receiver's variable reached by name while its method runs is an analysis error

*Owner: "Prefer detecting invalid overlapping mutable access during semantic
analysis. If the compiler can determine that a method will access the same
mutable receiver through conflicting aliases, reject the program before
execution. If a case cannot be determined statically, use a sound runtime
check where supported or refuse compilation until it can be implemented
safely."*

Rule V5 said the interpreter stops at run time and compiled code refuses. It
becomes a static rule, with those two as the fallback:

1. For every function and method, analysis computes the set of variables
   declared **outside every function** (module-level variables, and locals of
   top-level blocks) that its body names — reads, assigns, takes a field or an
   element of, passes as `.copy`, calls a method on, or interpolates into a
   string — or that anything it can call names. Variables are told apart by
   their binding, not their spelling: a block's own `top`, or a library's, is
   not the entry file's `top`.
2. Calls are followed by name — a call to a function, wherever it is defined,
   a method call on a value of a known struct type, an operator on one. A
   method call on a value whose type is a type parameter is the method of the
   struct each call gives that parameter, so a generic function is followed
   once for each such struct it is reached with; where a call does not say
   (a parameter it leaves unbound, or binds to a generic struct's field), it
   is every method of that name.
3. A method call `x.m(..)` in which `x`, or a struct or array variable given
   by name for a further receiver, is such a variable **and is in the set of
   `m`** is an analysis error: the method, or something it calls, reaches the
   variable it is running on.
4. The rule is deliberately conservative in two ways, both already true of
   the run-time check it replaces as the first line. It rejects an access on
   a path that is **never taken**. And it rejects a **read** as much as a
   write: the interpreter runs a method on a copy of its receiver and stores
   it back at the end, so a read by name sees the value from before the call,
   which compiled code would not. The way out is the same for both: reach the
   value through the receiver, or pass what the method needs as an argument.
5. The interpreter keeps its run-time check and compiled code keeps its
   refusal, as a second line. A program that analysis accepts cannot reach
   either through this route: a variable local to a function cannot be named
   by a method called on it, and what the rule cannot determine statically it
   counts as reached. (The second line is not relied on: the review of the
   implementation found a method reaching its receiver's array through `len`
   that compiled code ran while the interpreter stopped.)

A receiver that has the name of its variable (`impl (top: V)` called as
`top.run()`) names the receiver, not the variable, and is unaffected.

One over-rejection is known and accepted: a `"{top}"` in a string that the
method passes to a function with a parameter named `top` is expanded there, as
that parameter; analysis reads the name where it is written.

| Program | Before (interpreter / compiled) | After |
|---|---|---|
| a method on `top` whose body calls a function that changes `top` | run-time error / refused | analysis error |
| the same, only reading `top` | run-time error / refused | analysis error |
| the same with the call on a path not taken | runs / refused | analysis error |
| `impl (top: V)` called as `top.run()`, using `top` inside | agree | unchanged |
| a method on `top` that calls a function reading another variable | agree for a number; for a struct, runs / refused | unchanged |
| a method on `top` that calls `call(s)`, `fnc <T: G> call(t: T) { t.touch() }`, where `V`'s `touch` names `top` | run-time error / refused | analysis error |
| the same where only another struct's `touch` names `top` | runs / refused | unchanged |
| a block-local `top` shadowing the module's, with a method that changes the module's | runs / refused | unchanged |

## 7. What changes for existing programs

- **D1:** nothing that ran before changes result. Newly accepted: a method
  that writes a non-aggregate receiver (rejected since §76f), with the result
  compiled code always gave; and an argument of a convertible type for such a
  receiver (a `t8` for a `t64`), which the exact-type rule rejected.
- **D2:** programs that ran on the interpreter with a value of the wrong type
  and were refused by compiled code become analysis errors on both. Two kinds
  of program that **ran the same on both backends** are now rejected: a
  concrete value where `T` is declared, in a function whose every call gives
  `T` that very type; and a whole-number literal beside `T` elements called
  with an `f64`, when the element was only printed. A generic function that
  only moves its `T` values around is unaffected. Tracked fixtures whose
  outcome changed are listed in known-issues §81.
- **D3:** accepts programs that the Result rule (§75) had started to reject;
  no program that is accepted today changes.
- **D4:** a `when` used as a value with only `true`/`false`/`unknown` arms on
  something that is not a bool is now an analysis error. It yielded 0 on the
  interpreter and trapped in compiled code. No tracked program does this.
- **D5:** none.
- **D6:** a program whose method reaches its receiver's variable only on a
  path that is not taken, or only to read it, ran on the interpreter (and was
  refused by compiled code); it is now an analysis error. One tracked program
  changes its kind of failure: the fixture written for the run-time check.

Every tracked `.cx` file was replayed on both backends before and after each
step; the tracked programs whose outcome changed are named in the step's
commit.

## 8. Implementation — one bounded change per decision

| Step | Decision | Change | Branch | Commit |
|---|---|---|---|---|
| 1 | D5 | The workflow file and the refusal register say the Windows jobs are permanent | `backend/ci-windows-gates` (#504) | `c434001` |
| 2 | D3 | `Ok(E)` with a constant `E` at a declared whole-number payload | `backend/result-type-analysis` (#505) | `f86ce93` |
| 2b | D3 | `/` and `%` in a constant expression | `backend/result-type-analysis` (#505) | `3f8fd30` |
| 3 | D1 | Remove the held error; non-aggregate receivers are checked as arguments | `backend/value-alias-semantics` (#507) | `7098921` |
| 4 | D6 | The static rule, as a pass over the analyzed program | `backend/value-alias-semantics` (#507) | `899d969` |
| 4b | D6 | A method call on a type parameter is resolved by what each call gives it | `backend/value-alias-semantics` (#507) | `6ef02cf` |
| 5 | D2 | A generic body is checked against its own type parameters | `backend/value-alias-semantics` (#507) | `a7cefe7` |
| 6 | D4 | The three-state proof counts only for a bool | `backend/value-alias-semantics` (#507) | `a1aeb13` |

Each step carries fixtures for both backends, unit tests for the reason an
error is given, and the full gates on Linux; CI runs them on Linux and
Windows.

## 9. What the independent review of this document changed

The document was reviewed at its first version (`e88785e`) by a reviewer who
ran every "before" cell and probed each rule (about a hundred programs). Its
verdict was *blocking* on two points, both now resolved:

- **D2 rejected generic code that works.** The first version let a `T` value
  be used at a concrete type only where a number is declared. A string, a
  bool, a struct, an enum or a `Result` passed through a `T` to its own type
  runs identically on both backends, a tracked fixture does it
  (`t_module_enum_state`) and `module_state.md` documents it as supported.
  Rule 2 now checks every such use per call, for every type. The review also
  found the uses the first version did not name — a later branch of an
  `if`/`when` value, a gene method's `Self`, a function that returns nothing,
  recursion — and each is covered and tested. It asked for "every call" to be
  defined; it is.
- **D4 was not "no code".** The proof the rule relies on did not check the
  type it is a proof about (step 6).

Other findings folded in: the over-rejection in D6's type-parameter clause
(step 4b) and its wording (bindings, reads, untaken paths); the unstated
narrowings of D3 (division added in step 2b; the t64 limit and the `f64` case
now stated); the D2 × D3 case of a `Result<T>` at a narrower payload (per
call, no conversion); the whole-number-beside-`f64` case, which is a change to
a passing fixture and is now said to be; D5's "required"; the texts each rule
amends. The review's corrections to "before" cells are in the tables above.

Found by the review and **not** changed, because they are present on
`submain` and outside the six decisions (known-issues §82): a generic
struct's field value used at a narrower declared type is a silent difference
(300 / 44); and `!n` on a number and a string index are accepted by analysis.
