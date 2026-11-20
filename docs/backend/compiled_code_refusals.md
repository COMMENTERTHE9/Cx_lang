# Programs compiled code refuses — register

**Status: maintained record. Accepted temporarily by the owner on 2026-11-19.**

When the Cranelift JIT cannot compile a program so that it gives the
interpreter's answer, it refuses the program (exit 127) and says why. The
parity harness reports such a fixture as SKIP. A refusal is safe — it is never
a different answer — but it is **not parity and not finished work**: the
program is legal Cx, the interpreter runs it, and compiled code does not.

This register lists every refusal that the correctness work of November 2026
introduced (pull requests #499–#503 and what follows them), so that none is
mistaken for a pass and each has a stated way out. It does not list what
compiled code has never lowered (`char`, string interpolation, a top-level
struct read from a function, and so on); those are in
`docs/backend/cx_jit_parity_checklist.md` and `docs/known_issues.md`.

Each entry was run on `submain` (`d4b2c06`) and on the build that introduced
it, on both backends.

## 1. Legal programs that `submain` compiled and got right, and that are refused now

These are the regressions in what compiles. Each exists because the same code
path gave a wrong answer, or crashed, for a neighbouring program, and compiled
code could not tell the two apart.

| # | What | Smallest example | `submain` | Now | Refused because | Lifted by |
|---|---|---|---|---|---|---|
| R1 | A `Result` read at a narrower whole-number type, when the value fits | `d: Result<t64> = Ok(5)` then `e: Result<t8> = d`, `print(e)` | `Ok(5)` / `Ok(5)` | refused | The same lines with 300 give 300 on the interpreter and 44 in compiled code (known-issues §70 c) | The analysis rule of known-issues §75: the assignment becomes a type error on both backends, so it is no longer a legal program |
| R2 | A constant reaching a narrower `Result` through an `if`/`when` value or a generic constructor | `x: Result<t8> = if c { Ok(1 + 2) } else { Ok(1) }`; `n: Result<t8> = wrap(5)` | `Ok(3)` / `Ok(3)` | refused | Its static type is `Result<t64>` by then; see R1 (§72 j) | As R1 |
| R3 | An element of an untyped array of small `Ok` numbers read at a narrower type | `a = [Ok(5), Ok(7)]` then `n: Result<t8> = a:[0]` | `Ok(5)` / `Ok(5)` | refused | The element's width is not known once it is in memory (§72 j) | As R1 |
| R4 | An element of an untyped array of `Err`s used where a `Result` type is declared — an error table | `errs = [Err("a"), Err("b")]`, `return errs:[i]` from a function returning `Result<t64>` | `Err(b)` / `Err(b)` | refused | The element's type is `Result<unknown>`, which until §75 could hold an `Ok` of any kind; compiled code read one as a string (§72 a, §72 j) | Closing the last route by which an `Ok` gets under the type `Result<unknown>` (a generic body, known-issues §74); then that type means "an Err" and can be trusted |
| R5 | A generic function that carries its argument through a local, or reassigns its parameter, called with `Err(..)` and used where a `Result` type is declared | `fnc: T <T> dflt(a: T, b: T) { let r; r = a; if true { r = b }; return r }` then `t: Result<t64> = dflt(Err("x"), Err("y"))` | `Err(y)` / `Err(y)` | refused | As R4: nothing declares or proves what the call returns (§72 a, §72 j) | As R4 |
| R6 | `len` of a variable first assigned in both branches of an `if` | `let a`, `if c { a = [1, 2, 3] } else { a = [4, 5, 6] }`, `return len(a)` | `3` / `3` | refused | With the `else` removed `submain` printed 3 where the interpreter stops; compiled code does not track such a variable at all and cannot tell the two programs apart (§72 g) | Lowering that merges a variable first assigned inside both branches |
| R7 | A program that defines a function with the name of a runtime function | `fnc: bool cx_str_eq(a: str, b: str) { return true }` then a call to it | `true` / `true` | refused | The user's function was called wherever compiled code meant the runtime's: every string comparison, or every `%` on floats (§70 e, §72 d) | Giving user functions names the runtime's cannot collide with |
| R8 | A number as a `while` condition, when it is zero | `n: t64 = 0`, `while (n) { … }` | `0` / `0` | refused | For a non-zero number the two backends disagreed (§57 a) | A decision on whether a number is a condition |

## 2. Programs refused now that `submain` got wrong

Here the refusal replaced a defect. The interpreter runs each (or reports its
own error); compiled code used to answer differently or crash.

| # | What | `submain`, interpreter / compiled | Now | Entry | Lifted by |
|---|---|---|---|---|---|
| D1 | A `Result` whose payload is a struct or an array | `Ok(P { x: 1 })` / `Ok()`; an array payload was read out of a stack frame that had returned | refused | §60 | A decision on who owns the payload, and the `Result` calling convention |
| D2 | A `Result` whose payload kind nothing declares or proves, used at a declared kind | assertion fails / trap, later a crash reading a number as a string | refused | §72 a | As R4 |
| D3 | `assert` on a `Result`, an enum or a handle | assertion fails / passes for a non-zero word | refused | §70 f, §72 e | A decision on whether these are type errors |
| D4 | Unary minus on a `Result` | type mismatch / crash (134) | refused | §70 d | Nothing to lift: the interpreter rejects it too |

## 3. Fixtures that are compiled-code SKIPs because of the above

Eighteen fixtures added by this work, and one that existed before it, pin
the interpreter's answer for a program compiled code refuses. They are counted
as SKIP by the parity harness, never as PASS.

| Fixture | Entry |
|---|---|
| **`t_literal_hint_ok_payload`** — existed on `submain` and passed there; it passed by reading an array out of a dead stack frame | D1 |
| `t_result_struct_payload_print`, `t_result_aggregate_payload_is_a_snapshot`, `t_result_aggregate_equality`, `t_result_aggregate_snapshot`, `t_assert_eq_str_in_result_aggregate` | D1 |
| `t_result_of_unknown_kind_at_a_typed_use` | D2 |
| `t_assert_on_a_result`, `t_assert_on_an_enum`, `t_assert_on_a_handle` | D3 |
| `t_unary_minus_on_a_result` | D4 |
| `t_len_of_a_variable_assigned_in_both_branches` | R6 |
| `t_user_function_named_like_the_runtime`, `t_user_function_named_like_the_float_remainder` | R7 |
| `t_generic_local_result` | R5 |
| `t_generic_local_keeps_its_type` | — the interpreter stops with a type mismatch; refusing is the matching outcome |
| `t_print_struct_field_order`, `t_assert_eq_char`, `t_generic_local_char_handle_unknown` | — forms compiled code has never lowered (a struct print, `char`, a printed handle, an unknown bool argument); listed so the count adds up |

R1–R5 are pinned by unit tests in `src/ir/lower.rs`
(`divergence_shape_tests`) rather than by fixtures: a fixture can pin the
interpreter's answer, but only a unit test can assert that compiled code
refuses and why.

Totals at the tip of pull request #503: 714 fixtures, 647 PASS, 67 SKIP,
0 PARITY_FAIL. `submain` has 650 fixtures, 602 PASS, 48 SKIP. Of the 19
additional SKIPs, 18 are fixtures this work added and one is
`t_literal_hint_ok_payload`.

## 4. Keeping this current

- A change that makes compiled code refuse a program the interpreter runs adds
  a row here in the same commit, with the example run on the base and on the
  change.
- A row is removed only by a change that carries a fixture for that program
  passing on **both** backends.
- A row whose program becomes an analysis error (as R1–R3 do under §75) is
  moved out of section 1 with a note saying so: it is no longer a legal
  program, which is a language change, not a compiler capability.
- Pull request descriptions report SKIPs separately from PASSes and name the
  fixtures.
