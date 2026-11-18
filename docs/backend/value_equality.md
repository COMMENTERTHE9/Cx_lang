# Value equality

**Status:** decided by the owner on 2026-11-18 and implemented on both backends.
This document is the rule; `docs/known_issues.md` §55 is the history. Revised
the same day after an independent review (§70 there): several statements below
were wrong or incomplete in the first version, and are corrected. Revised again
after the review of those corrections (§72): how compiled code treats a payload
kind it cannot establish, and what "an arm of another kind" covers.

The decision: a string is equal to another string when their bytes are equal,
including a string inside a `Result`, `Ok` or `Err`. Equality of a `Result` is
recursive over its payload. An enum value is equal by its declared value — its
enum and its variant. This replaces the interpreter's earlier behaviour, which
compared a string inside a `Result` by where it was stored.

## 1. The rule

Two values are equal when they are the same kind of value and their content is
the same. Where a value is stored is never part of the answer.

| Kind | Equal when |
|---|---|
| integer (`t8` … `t128`) | the numbers are equal; width does not matter |
| `f64` | IEEE equality (`NaN` is not equal to itself) |
| `bool` | both `true` or both `false` |
| `char` | the same character |
| `str` | the same bytes |
| enum | the same enum **and** the same variant |
| handle | the same handle |
| `Result<T>` | the same variant (`Ok`/`Err`) and equal payloads, by this table |
| struct, array *inside a `Result`* | same shape and every part equal, by this table |

Two values of different kinds are not equal: the number `1` is not the string
`"1"`, `Color::Red` is not the number compiled code uses for its tag, a handle
is not the number compiled code keeps it as, and `Color::Red` is not
`Shape::Circle` whatever position each has in its enum. `Ok(x)` is never equal
to `Err(x)`. The same holds for a `when` arm: an arm of another kind than the
value being matched never matches. That covers every form of arm: a range
matches only a whole number that lies in it, `unknown` only a bool that is
unknown, and the whole number `1` is not the float `1.0` in either direction.

### A string has no identity

There is no operation in Cx that distinguishes two strings with the same bytes.
Consequently nothing allocates when a string is bound to a name, passed as an
argument (plain or `.copy`), returned, or stored in a `Result`: compiled code
passes the address of the string's descriptor and copies nothing. A loop that
binds and compares a string 20 million times runs in constant memory
(§5 below).

## 2. Where equality is observed

| Form | Accepts | Notes |
|---|---|---|
| `a == b`, `a != b` | two numbers (mixed widths and int/float are promoted to a common type by analysis), or two values of one type among `bool`, `char`, `str`, `strref`, enum; a struct whose type has an `Eq` phen | anything else is a semantic error, including two `Result`s |
| `when` arms | integers, floats, ranges of integers, strings (§53), enum variants, `unknown` | string arms compare bytes; an arm of another kind than the scrutinee never matches |
| `assert_eq(a, b)` | number/number, `bool`/`bool`, `f64`/`f64`, `char`/`char`, `str`/`str`, enum/enum, handle/handle, `Result`/`Result` | any other pair fails the assertion — see §4 |

`==` on two `Result`s is a semantic error, so `assert_eq` is the only place
`Result` equality can be seen.

## 3. Per-backend status

| `assert_eq` on | Interpreter | Cranelift JIT |
|---|---|---|
| integers of one width, `bool` | by value | by value |
| integers of two widths | by value | refused (operand types must match) |
| `str` | by bytes | by bytes (`cx_str_eq`) |
| enum, same enum | by variant | by variant (tag compare) |
| enum against another enum or a number | not equal | not equal, decided from the types — the tags are not compared |
| handle | the same handle | the same handle |
| handle against a number | not equal | not equal, decided from the types |
| `when` arm of another kind than the scrutinee (a number arm on an enum or a bool, a variant arm on a number or another enum, a range arm on anything but a whole number, `unknown` on anything but a bool, `1` on a float or `1.0` on a whole number) | never matches | never matches, decided from the types |
| `Result` with an integer payload | by value | by value |
| `Result` with a `str` payload, and any `Err` | by bytes | by bytes (`cx_str_eq` on the payload descriptors) |
| `Result` with an enum payload | by variant | by variant |
| `Result`s whose payload kinds differ | not equal | not equal |
| `Result` with a struct, array or nested `Result` payload | recursive, by this document | refused: compiled code does not carry these payloads (§60) |
| `Result` whose payload kind compiled code cannot establish | by content | refused — see below |
| `char`, `f64` | by value | refused (not lowered) |

"Refused" is the JIT's exit 127: the program is not compiled, and the parity
harness reports SKIP. A refusal is never a different answer.

### How compiled code knows a payload's kind

A `Result` is one packed i128: the payload word in the low half, the tag in the
high half. The word is a number, an enum tag, or the address of a string
descriptor, and nothing in the word says which. Lowering therefore tracks the
payload kind of every `Result` value it produces (`LoweringCtx::result_types`),
checks it wherever a `Result` crosses a typed boundary (`ensure_result_value`),
and proves it over the finished function's SSA graph, including merges and loop
back-edges (`check_result_proofs`). The machinery is PR #476's. What it does
with the answer was changed after review (known-issues §70):

- A use whose payload kind is **proven to be something else** is refused,
  everywhere.
- A payload fits where the same kind or a **wider** whole number is declared,
  never a narrower one and never a whole number as a float. `?` converts the
  word to the declared type, so a narrower read would cut the number down.
- When the graph **cannot say** — a value loaded from memory, a call that
  builds what it returns — it matters whether anything else vouches for the
  value. If its own static type names a payload kind (a variable, parameter,
  field or function result declared `Result<t64>`), a use that existed before
  kinds were tracked (crossing a typed boundary, printing, `?`) goes by that
  type, as compiled code always did: analysis has checked it. Comparing two
  `Result`s by content is new, and dereferences a string payload, so there an
  unproven kind is refused.
- If its static type is `Result<unknown>` — an untyped variable that started
  as an `Err`, the result of a function generic in `T` that was given one —
  analysis accepts the value wherever a `Result` is declared, **whatever it
  holds** (§4 item 7). Nothing vouches for it, so where the graph cannot
  either, the use is refused. The first correction went by the declared type
  of the *use* here, and read `Ok(5)` as the address of a string
  (known-issues §72).
- So that this does not refuse ordinary programs, a function that can only
  return some of its own arguments unchanged, or an `Err`, is followed through
  the call: `second(Err("x"), Err("y"))` holds whatever its arguments hold.
  That covers a statement before the `return`, a branch between two arguments
  (written with `return` or as an `if`/`when` value), a function returning
  another such function's result, and recursion. A function that builds or
  fetches what it returns, reassigns a parameter, or carries one through a
  local is not followed.
- What is still refused for want of a proof, though the value would have been
  fine: an element of an untyped array of `Err`s — an error table — returned
  or assigned where a kind is declared, and the result of a forwarder of the
  kind just excluded (known-issues §72j). Values that pass through local
  memory are not followed.
- A constant payload — `Ok(5)`, `Ok(1 + 2)` — is recorded at the narrowest
  width that holds its value, so it can be read where a `Result<t8>` is
  declared and `Ok(300)` cannot. This holds where the `Ok(..)` is used
  directly or through an untyped variable. A constant that reaches the
  declaration through an `if`/`when` value or a generic constructor has the
  static type `Result<t64>` by then, and is refused like any other `t64`. A
  number with no declared width that is computed at run time is the whole
  word.

## 4. Not decided

These are recorded so they are decided on purpose. None of them is a case where
the two backends give different answers: in each, compiled code refuses the
program.

1. **`assert_eq` on two structs or two arrays at the top level.** The
   interpreter fails the assertion for every pair, equal or not
   (`assert_eq failed: P { x: 1, y: 2 } != P { x: 1, y: 2 }`). Inside a `Result`
   the same two structs compare equal. The open question is which rule the top
   level should follow: the structural one used inside a `Result`, or the type's
   own `Eq` phen as `==` does.
2. **`assert_eq` on three-state bools.** The interpreter fails the assertion
   even for two values both known to be true (`assert_eq(u || true, true)` →
   `true != true`). `==` handles these: a known value compares by its truth, an
   unknown one makes the result unknown.
3. **`assert_eq` on an integer and a float.** `x == y` with `x: t64 = 1` and
   `y: f64 = 1.0` is true (analysis promotes); `assert_eq(x, y)` fails, because
   `assert_eq` promotes only an untyped literal — `assert_eq(1, 1.0)`, two
   literals, passes.
4. **A `Result`'s payload type is not checked on assignment.**
   `narrow: Result<t8> = large` with `large: Result<t64> = Ok(300)` is accepted
   by analysis, and the interpreter then prints `Ok(300)` and unwraps 300 from a
   `Result<t8>`; `f: Result<f64> = small` is accepted and prints the integer.
   Compiled code refuses both (§3). PR #476's fixture `t_result_numeric_alias`
   pinned the interpreter's output as expected; the revision keeps only the
   widening half (`t_result_numeric_widen`), which is sound. The same narrowing
   through a variable declared without a type — `lit = Ok(300)`, then
   `narrow: Result<t8> = lit` — was not refused until §72: it gave 300 on the
   interpreter and 44 in compiled code. It is refused now; `lit = Ok(5)` is not,
   because 5 fits.
5. **`assert_eq` on an unknown number.** `u: t64 = ?` then `assert_eq(u, u)`
   fails on the interpreter; compiled code refuses the unknown.
6. **Items 2 and 3 inside a `Result`.** `Ok(u || true)` against `Ok(true)`
   fails with `Ok(true) != Ok(true)`, and `Ok(1)` against `Ok(1.0)` with
   `Ok(1) != Ok(1)`: the payloads are compared by the same rule as at the top
   level, so they inherit its gaps. Compiled code refuses both payloads.
7. **A `Result<unknown>` is accepted wherever a `Result` is declared.** A
   variable declared without a type and first given an `Err` has the type
   `Result<unknown>`. It can then be given an `Ok` of any kind (through a
   `.copy` parameter, for one), and analysis accepts it as a `Result<str>`, a
   `Result<Color>` or a `Result<t8>` alike. The interpreter carries on with
   whatever is inside: a variable declared `Result<str>` prints `Ok(5)`.
   Compiled code refuses unless it can prove the kind. Item 4 is the same gap
   seen from the number side. The open question is whether analysis should
   reject such an assignment, or the declaration should be checked when the
   value arrives.
8. **`assert` on a value that is not a condition.** The interpreter passes
   `assert(x)` for `true` and for a non-zero number, and fails it for
   everything else — an enum, a handle, a `Result`, a string. Compiled code
   refuses the enum, the handle and the `Result` (it used to pass them when the
   word was not zero) and does not lower the rest. Whether these should be type
   errors instead is undecided.

## 5. Tests and measurements

Fixtures in `src/tests/verification_matrix/`, each run on both backends:

- `t_assert_eq_str_in_result` — every route that produces two strings with the
  same bytes in two places (two literals, two calls, a parameter, a `.copy`
  parameter, a forwarded `Result`, empty strings with other strings made in
  between, one `Err` read at two payload types): all equal.
- `t_assert_eq_result_ok_str_differs`, `t_assert_eq_result_err_str_differs`,
  `t_assert_eq_result_ok_vs_err`, `t_assert_eq_result_payload_kinds_differ` —
  the unequal cases: one byte differs at equal length, one message is a prefix
  of the other, same string in different variants, a number against a string.
- `t_assert_eq_enum`, `t_assert_eq_enum_variant_differs`,
  `t_assert_eq_enum_other_enum_differs`, `t_assert_eq_enum_not_a_number`,
  `t_assert_eq_result_enum_differs`.
- `t_assert_eq_char` — interpreter; the JIT does not lower `char`.
- `t_assert_eq_handle`, `t_assert_eq_handle_not_a_number`,
  `t_assert_eq_handle_is_itself` — the last runs to the end, so it pins that a
  handle equals itself on both backends; the first only pins how it ends.
- `t_when_arm_of_another_kind_never_matches`,
  `t_when_range_and_unknown_arms_of_another_kind`.
- `t_assert_eq_str_in_result_aggregate` — a string inside a struct, an array
  and a nested `Result` payload, each built from two separate literals;
  interpreter, the JIT refuses the payload. `t_result_aggregate_equality` and
  `t_result_aggregate_snapshot` (PR #476) are kept as well, but the first uses
  one string variable on both sides and would pass under the old rule.
- `t_result_err_phi` (PR #476), `t_result_numeric_widen`,
  `t_result_reaches_its_use_indirectly`,
  `t_result_forwarded_by_a_generic_function`,
  `t_result_small_constant_at_a_narrow_type`.
- `t_assert_on_a_result`, `t_assert_on_an_enum`, `t_assert_on_a_handle`,
  `t_unary_minus_on_a_result`, `t_user_function_named_like_the_runtime`,
  `t_user_function_named_like_the_float_remainder`,
  `t_result_of_unknown_kind_at_a_typed_use` — the interpreter's answer;
  compiled code refuses each, where it used to pass, crash, call the wrong
  function or read a number as a string.

What the unequal-case fixtures do and do not pin. An `expected_fail` fixture
asserts how the program ends, not what it printed first. Each of these has an
assertion on an equal pair before the unequal one, and on the build before
these changes it was the *first* that failed on the interpreter — which also
ends the program as expected. So they guard against two unequal values being
called equal, which is their job; that equal values are equal is pinned by the
`expected_output` fixtures, which do fail on the old build.

Unit tests in `src/ir/lower.rs`, `divergence_shape_tests`:
`result_equality_compares_strings_by_content`,
`binding_and_passing_a_string_calls_no_runtime_allocator`,
`result_equality_on_an_aggregate_payload_is_refused`,
`an_enum_is_never_equal_to_a_number`,
`a_result_is_not_read_at_a_narrower_integer_type`,
`a_result_no_declaration_vouches_for_is_refused_where_a_kind_is_declared`,
`a_function_that_returns_its_arguments_carries_their_kind`,
`a_constant_payload_is_as_wide_as_its_value`,
`a_when_arm_of_another_kind_is_not_compared`,
`assert_on_an_enum_or_a_handle_is_refused`.

Memory, debug build, Cranelift JIT, 20 million iterations:

| Program | Before | PR #476 | Now |
|---|---|---|---|
| bind a string and compare it with `==` | 0.45 s, 15 MB | 64.5 s, 7,054 MB | 0.42 s, 15 MB |
| build a `Result<str>` and an `Err`, `assert_eq` each against a literal | traps on the first comparison | — | 0.91 s, 15 MB |

## 6. What changed

In the interpreter, these used to fail and now pass:

```cx
assert_eq(Ok("same"), Ok("same"))
assert_eq(parse(false), Err("bad"))     // parse returns Err("bad")
assert_eq(Color::Red, Color::Red)
assert_eq('x', 'x')
assert_eq(h, h)                         // h a handle
assert_eq(Ok(Pair { label: "a", n: 1 }), Ok(Pair { label: "a", n: 1 }))
```

The last line stands for every `Result` whose payload is a struct, an array or
another `Result` with a string somewhere inside: such payloads were already
compared part by part, but a string part by where it was stored.

In compiled code, two `Result`s used to be compared as packed words, so two
strings were equal only when they came from the same literal in the source; they
are now compared by content. An enum or a handle compared against a number with
the same bit pattern used to pass and now fails. A `when` arm of another kind
than the scrutinee — a literal, a range or `unknown` — used to be compared by
its bits and now never matches; a whole-number arm on a float used to stop the
compiler and now never matches. A `Result` read at a narrower integer type than
it was made with used to compile and is refused. A `Result` whose kind nothing
declares and compiled code cannot prove used to be read as whatever the use
declared and is refused. `assert` on an enum or a handle used to pass for a
non-zero word and is refused.
