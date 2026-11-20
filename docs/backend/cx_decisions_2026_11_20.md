# Cx — the owner's decisions of 2026-11-20, as rules

**Status: PROPOSED — under independent review. Implemented by the commits
listed in §8 as they land.**

Six questions were put to the owner by the report on pull requests #499–#507.
This document turns each answer into a rule precise enough to implement and
test, says what changes for existing programs, and lists the implementation
steps. It amends `docs/backend/cx_value_and_alias_semantics.md` (rules V2 and
V5) and known-issues §42, §74, §75 and §77.

"Interpreter / compiled" below are results on the build before these changes
(`a277db2`), each run for this document.

## D1 — A further receiver that is not a struct or an array is a value

*Owner: "These types should behave as values by default. Passing one as an
additional method receiver must not unexpectedly modify the caller's variable.
Keep the existing, documented behavior for named struct and array receivers."*

1. A receiver of an `impl` whose declared type is a number, bool, char,
   string, enum, handle or `Result` is a **value**: the method gets the value
   the argument had at its position in the argument list, and nothing is ever
   written back to the caller's variable. (Rule V2 item 4, no longer held.)
2. Inside the method such a receiver is an ordinary local. The method **may
   assign it**, compound-assign it, and pass it as `.copy`; all of that stays
   inside the method. The temporary analysis error
   (`reject_value_receiver_write`) is removed.
3. Because it is a value, its argument is checked and converted **as an
   ordinary argument is**: a `t8` variable may be given for a `t64` receiver,
   a `Result` by the payload rule (D3). The exact-type requirement of §42
   remains for struct and array receivers, which are passed by reference.
4. Nothing changes for a struct or array receiver given a variable by name:
   it is that variable.

`.copy` is unchanged: a caller that wants a call to change its number passes
it `.copy` to a function. No new syntax.

| Program | Before | After |
|---|---|---|
| `q.grow(n, 1)` where `grow` does `k += 100` on its `t64` receiver; then `n` | analysis error (held) | 5 on both; `grow` returns 105 |
| `q.set(res)` where `set` assigns `Ok(99)` to its `Result` receiver; then `res` | analysis error (held) | `Ok(5)` on both |
| `q.grow(n8, 1)` with `n8: t8` and a `t64` receiver | analysis error (exact type) | accepted, converted |

## D2 — Inside a generic function a type parameter matches only itself

*Owner: "If a function declares a value as type T, the compiler must enforce
the actual type represented by T. … Implement the checks in semantic
analysis."*

Within the body of a generic function, for each of **its own** type
parameters `T`:

1. **`T` matches only `T`.** A value whose type is not `T` — a literal, a
   variable of a concrete type, a value of another type parameter `U`, a
   `Result` of something that is not `T` where `Result<T>` is declared — may
   not be used where `T` is declared: in a typed declaration, an assignment, a
   `return` or trailing expression, an argument for a `T` parameter, an `Ok`
   payload, a struct field. It is an analysis error in the body, whatever the
   function is called with. This extends to every position the rule §42
   already set for array element types.
2. **A `T` value is not a value of a concrete type**, with one exception that
   exists today (known-issues §43): where a number of a declared width is
   declared (`n: t64 = x`), a `T` value is converted like any number. That
   conversion is now **checked at each instantiation**: if the function is
   called with a `T` that is not a number, the call is an analysis error.
3. **A number literal as an element of an array literal whose other elements
   are `T`** stays legal (`[x, 1]` is a `[2: T]`, §42) and is also checked at
   each instantiation: a whole-number literal needs `T` to be a whole-number
   type the literal fits; a float literal needs `T` to be `f64`. Otherwise the
   call is an analysis error. (An integer literal beside an `f64` `T` already
   fails on the interpreter.)
4. An unknown value (`?`) and an `Err(..)` remain usable where `T` or
   `Result<T>` is declared, as everywhere.

"At each instantiation" means: for every call that reaches the function with
concrete types — directly, or through other generic functions — the checks of
items 2 and 3 are made with `T` replaced by what it is bound to there. The
error is reported at the outermost call, and names the type and the line in
the generic body.

Not covered, and unchanged: a type parameter of a generic **struct** reaching
a function through a field read (`b.v` of a `Box<T>`). Struct type arguments
are not tracked through variables, so such a value is still not checked
against a declared type; compiled code refuses the cases it cannot prove
(known-issues §75). Closing that needs struct instantiations in the type
system and is a separate piece of work.

| Program | Before (interpreter / compiled) | After |
|---|---|---|
| `fnc: T <T> f(x: T) { return "not a T" }`, `f(5)` | prints `not a T` / refused | analysis error |
| `y: T = 5` in a generic body | 5 for every `T`, a bool included / refused | analysis error |
| `z: U = x` with `x: T` | runs / refused | analysis error |
| `pick(x, 5)` inside a generic body, `pick(a: T, b: T)` | runs / refused | analysis error |
| `fnc: T <T> launder(x: T) { return Ok(5) }` | `Ok(5)` under any declared type / refused | analysis error |
| `fnc: Result<T> <T> mk() { return Ok(5) }` | `Ok(5)` read as `Result<str>` / refused | analysis error |
| `n: t64 = x` with `x: T`, called with a string | run-time error after output / refused | analysis error at the call |
| `c: [2: T] = [x, 349]`, called with a `t8` | 349 / refused (93 on `submain`) | analysis error at the call |
| `c: [2: T] = [x, 34]`, called with a `t8` or `t64` | 34 / 34 | unchanged |
| `id(x)`, `[x, x]`, `Ok(x)`, `twice(x)` calling `id(id(x))` | agree | unchanged |

## D3 — A constant whole-number expression takes a declared `Result` payload type when it provably fits

*Owner: "if an integer constant expression is provably within the target
type's range, I want the same safe contextual conversion behavior that Cx
already permits for ordinary integer declarations. … If the compiler cannot
prove a conversion safe, produce a clear analysis error."*

1. Where a `Result<W>` with a whole-number payload type `W` is declared and
   the value written is `Ok(E)`, and `E` is a **constant whole-number
   expression**, analysis computes its value. If it fits `W`, the payload is
   that value at type `W`; if it does not, it is an analysis error naming the
   value and the type.
2. A constant whole-number expression is: a whole-number literal; unary minus
   of one; `+`, `-`, `*` of two; parentheses. Each intermediate value must fit
   a `t64`, so that both backends compute the same value. Division and
   remainder, variables, `const` names and calls are not constant expressions
   for this rule.
3. Everything else is unchanged: an `Ok` of a value whose type is already
   declared is not narrowed (`Result<t64>` is never a `Result<t8>`); a float
   is never a whole number or the reverse; `Err(..)` is a `Result` of every
   type.

Unlike an ordinary declaration — where `x: t8 = 150 + 150` is accepted and
wraps — a constant that does not fit a `Result` payload is an error: the owner
asked for a conversion only when its safety is established.

| Program | Before | After |
|---|---|---|
| `c: Result<t8> = Ok(1 + 2)` | analysis error | `Ok(3)` on both |
| `x: Result<t8> = if c { Ok(1) } else { Ok(1 + 2) }` | analysis error | `Ok(3)` or `Ok(1)` on both |
| `c: Result<t8> = Ok(100 + 100)` | analysis error (type) | analysis error: 200 does not fit `t8` |
| `c: Result<t8> = Ok(n)` with `n: t64` | analysis error | unchanged |

## D4 — A `when` used as a value needs an unguarded `_`

*Owner: "Keep the current rule."* No change. A `when` that produces a value
is exhaustive only if it has a `_` arm without a guard, or — the one proof
that is implemented — unguarded arms for `true`, `false` and `unknown` on a
bool. A `when` statement keeps the lenient rule. Recorded in known-issues §77.

## D5 — Both Windows CI jobs are permanent

*Owner: "KEEP BOTH WINDOWS CI JOBS PERMANENTLY."* The jobs "Windows Frontend
Build + Matrix" and "Windows Backend Build + Tests" in
`.github/workflows/ci.yml` are part of the required gate for every compiler
change, on the same footing as the Linux jobs. They are not to be removed,
skipped or made non-blocking to get a build through.

## D6 — A receiver's variable reached by name while its method runs is an analysis error

*Owner: "Prefer detecting invalid overlapping mutable access during semantic
analysis. … If a case cannot be determined statically, use a sound runtime
check where supported or refuse compilation."*

Rule V5 said the interpreter stops at run time and compiled code refuses. It
becomes a static rule:

1. For every function and method, analysis computes the set of variables
   declared **outside every function** (module-level variables, and locals of
   top-level blocks) that its body names — reads, assigns, takes a field or an
   element of, passes as `.copy`, calls a method on, or interpolates into a
   string — or that anything it can call names. Calls are followed by name:
   a call to a function, a method call on a value of a known struct type, an
   operator on one. A method call on a value whose type is a type parameter
   is taken to reach every method of that name.
2. A method call `x.m(..)` in which `x`, or a struct or array variable given
   by name for a further receiver, is such a variable **and is in the set of
   `m`** is an analysis error: the method, or something it calls, reaches the
   variable it is running on.
3. The rule is deliberately conservative: it rejects a program in which the
   access is on a path that is never taken. It does not depend on whether the
   access is a read or a write.
4. The interpreter keeps its run-time check and compiled code keeps its
   refusal, as a second line. A program that analysis accepts cannot reach
   either through this route: a variable local to a function cannot be named
   by a method called on it.

A receiver that has the name of its variable (`impl (top: V)` called as
`top.run()`) names the receiver, not the variable, and is unaffected.

| Program | Before (interpreter / compiled) | After |
|---|---|---|
| a method on `top` whose body calls a function that changes `top` | run-time error / refused | analysis error |
| the same with the call on a path not taken | runs / refused | analysis error |
| `impl (top: V)` called as `top.run()`, using `top` inside | agree | unchanged |
| a method on `top` that calls a function reading another variable | agree | unchanged |

## 7. What changes for existing programs

- **D1:** nothing that ran before changes result. Programs rejected since
  §76f (a method that writes a non-aggregate receiver) are accepted, with the
  result compiled code always gave.
- **D2:** programs that ran on the interpreter and were refused by compiled
  code become analysis errors on both. The one shape that ran on both and is
  now an error is a number literal that does not fit beside a `T` element —
  where the two backends disagreed. A generic function that is never called
  with a type that violates items 2 and 3 is unaffected.
- **D3:** accepts programs that the Result rule (§75) had started to reject;
  no program that is accepted today changes.
- **D4, D5:** none.
- **D6:** a program whose method reaches its receiver's variable only on a
  path that is not taken ran on the interpreter (and was refused by compiled
  code); it is now an analysis error.

Every tracked `.cx` file is replayed on both backends before and after each
step, and any tracked program whose outcome changes is listed in the step's
commit.

## 8. Implementation — one bounded change per decision

| Step | Decision | Change | Branch |
|---|---|---|---|
| 1 | D5 | The workflow file and the refusal register say the Windows jobs are permanent | `backend/ci-windows-gates` |
| 2 | D3 | `Ok(E)` with a constant `E` at a declared whole-number payload | `backend/result-type-analysis` |
| 3 | D1 | Remove the held error; non-aggregate receivers are checked as arguments; fixtures on both backends | `backend/value-alias-semantics` |
| 4 | D6 | The static rule, as a pass over the analyzed program | `backend/value-alias-semantics` |
| 5 | D2 | `T` matches only `T`; the two conversions checked per instantiation | `backend/value-alias-semantics` |

Each step carries fixtures for both backends, unit tests for the reason an
error is given, the full gates on Linux, and CI on Linux and Windows. D4 needs
no code.
