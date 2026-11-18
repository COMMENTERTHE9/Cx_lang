# Value equality

**Status:** decided by the owner on 2026-11-18 and implemented on both backends.
This document is the rule; `docs/known_issues.md` §55 is the history.

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
| `Result<T>` | the same variant (`Ok`/`Err`) and equal payloads, by this table |
| struct, array *inside a `Result`* | same shape and every part equal, by this table |

Two values of different kinds are not equal: the number `1` is not the string
`"1"`, `Color::Red` is not the number compiled code uses for its tag, and
`Color::Red` is not `Shape::Circle` whatever position each has in its enum.
`Ok(x)` is never equal to `Err(x)`.

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
| `when` literal arms | integers, strings (§53), enum variants | string arms compare bytes |
| `assert_eq(a, b)` | number/number, `bool`/`bool`, `f64`/`f64`, `char`/`char`, `str`/`str`, enum/enum, `Result`/`Result` | any other pair fails the assertion — see §4 |

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
| `Result` with an integer payload | by value | by value |
| `Result` with a `str` payload, and any `Err` | by bytes | by bytes (`cx_str_eq` on the payload descriptors) |
| `Result` with an enum payload | by variant | by variant |
| `Result`s whose payload kinds differ | not equal | not equal |
| `Result` with a struct, array or nested `Result` payload | recursive, by this document | refused: compiled code does not carry these payloads (§60) |
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
back-edges (`check_result_proofs`). A consumer that would have to interpret a
word of unproven kind — print it, compare it, unwrap it — is refused. This
machinery is PR #476's and is kept as written.

## 4. Not decided

These are recorded so they are decided on purpose. None of them is a case where
the two backends give different answers: in each, compiled code refuses.

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
3. **`assert_eq` on an integer and a float.** `1 == 1.0` is true (analysis
   promotes); `assert_eq(1, 1.0)` fails, because `assert_eq` promotes only an
   untyped literal.
4. **A `Result`'s payload type is not checked on assignment.**
   `narrow: Result<t8> = large` with `large: Result<t64> = Ok(300)` is accepted
   and prints `Ok(300)`; `f: Result<f64> = small` is accepted and prints the
   integer. Both backends agree. PR #476's fixture `t_result_numeric_alias`
   pinned this as expected output; the revision keeps only the widening half
   (`t_result_numeric_widen`), which is sound.

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
- `t_result_aggregate_equality`, `t_result_aggregate_snapshot` (PR #476) —
  interpreter; the JIT refuses the payload.
- `t_result_err_phi` (PR #476), `t_result_numeric_widen`.

Unit tests in `src/ir/lower.rs`, `divergence_shape_tests`:
`result_equality_compares_strings_by_content`,
`binding_and_passing_a_string_calls_no_runtime_allocator`,
`result_equality_on_an_aggregate_payload_is_refused`,
`an_enum_is_never_equal_to_a_number`.

Memory, debug build, Cranelift JIT, 20 million iterations:

| Program | Before | PR #476 | Now |
|---|---|---|---|
| bind a string and compare it with `==` | 0.45 s, 15 MB | 64.5 s, 7,054 MB | 0.42 s, 15 MB |
| build a `Result<str>` and an `Err`, `assert_eq` each against a literal | traps on the first comparison | — | 0.91 s, 15 MB |

## 6. What changed

In the interpreter, these four lines used to fail and now pass:

```cx
assert_eq(Ok("same"), Ok("same"))
assert_eq(parse(false), Err("bad"))     // parse returns Err("bad")
assert_eq(Color::Red, Color::Red)
assert_eq('x', 'x')
```

In compiled code, two `Result`s used to be compared as packed words, so two
strings were equal only when they came from the same literal in the source; they
are now compared by content. An enum compared against a number with the same
bit pattern as its tag used to pass and now fails.
