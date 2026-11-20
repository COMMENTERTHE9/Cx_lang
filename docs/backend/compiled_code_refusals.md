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
| R4 | An element of an untyped array of `Err`s used where a `Result` type is declared — an error table | `errs = [Err("a"), Err("b")]`, `return errs:[i]` from a function returning `Result<t64>` | `Err(b)` / `Err(b)` | refused | The element's type is `Result<unknown>`, which until §75 could hold an `Ok` of any kind; compiled code read one as a string (§72 a, §72 j) | Closing the last routes by which an `Ok` gets under the type `Result<unknown>`. A generic function's body was the main one and is closed (known-issues §81, 2026-11-20); those left are listed in §75 — a generic struct's field, an untyped `.copy` parameter, a `copy_into` field. Then that type means "an Err" and can be trusted |
| R5 | A generic function that carries its argument through a local, or reassigns its parameter, called with `Err(..)` and used where a `Result` type is declared | `fnc: T <T> dflt(a: T, b: T) { let r; r = a; if true { r = b }; return r }` then `t: Result<t64> = dflt(Err("x"), Err("y"))` | `Err(y)` / `Err(y)` | refused | As R4: nothing declares or proves what the call returns (§72 a, §72 j) | As R4 |
| R6 | `len` of a variable first assigned in both branches of an `if` | `let a`, `if c { a = [1, 2, 3] } else { a = [4, 5, 6] }`, `return len(a)` | `3` / `3` | refused | With the `else` removed `submain` printed 3 where the interpreter stops; compiled code does not track such a variable at all and cannot tell the two programs apart (§72 g) | Lowering that merges a variable first assigned inside both branches |
| R7 | A program that defines a function with the name of a runtime function | `fnc: bool cx_str_eq(a: str, b: str) { return true }` then a call to it | `true` / `true` | refused | The user's function was called wherever compiled code meant the runtime's: every string comparison, or every `%` on floats (§70 e, §72 d) | Giving user functions names the runtime's cannot collide with |
| R8 | A number as a `while` condition, when it is zero | `n: t64 = 0`, `while (n) { … }` | `0` / `0` | refused | For a non-zero number the two backends disagreed (§57 a) | A decision on whether a number is a condition |

### Moved out of this section: no longer legal programs

*(After the independent review of §75: for one shape this was not yet true.
`x: Result<t8> = if c { Ok(1) } else { Ok(1 + 2) }` — the constant in a
**later** branch — was still accepted by analysis and refused by compiled
code. Each branch of an `if`/`when` value is now checked like the first.)*

*(2026-11-20, by the owner's decision: a constant whole-number expression that
provably fits the declared payload type is converted to it, so `Ok(1 + 2)` at
a `Result<t8>` is legal again and runs on both backends — R2's commonest case
is neither refused nor an error. A constant that does not fit, and anything
that is not a constant, remains a type error.)*

R1–R3 were refusals of a `Result` read at a narrower whole-number type than it
was made with, when the value happened to fit: directly
(`e: Result<t8> = d` for a `d: Result<t64>` holding 5), through an `if`/`when`
value or a generic constructor (`Ok(1 + 2)`, `wrap(5)`), and from an untyped
array of small numbers. `submain` printed the same on both backends for each;
with a value that did not fit it printed 300 on the interpreter and 44 in
compiled code.

Since known-issues §75 these are **type errors in analysis**, on both
backends: a `Result`'s payload is never narrowed. That is a change to the
language, decided by the owner on 2026-11-19, not a capability compiled code
lacks, so they are no longer listed as refusals. Compiled code's own check
stays behind analysis as a second line. Fixtures:
`t_result_narrower_declaration_rejected`,
`t_result_constant_arithmetic_narrowed_rejected`,
`t_result_untyped_number_narrowed_rejected` and the other
`t_result_*_rejected` fixtures.

## 2. Programs refused now that `submain` got wrong

Here the refusal replaced a defect. The interpreter runs each (or reports its
own error); compiled code used to answer differently or crash.

| # | What | `submain`, interpreter / compiled | Now | Entry | Lifted by |
|---|---|---|---|---|---|
| D1 | A `Result` whose payload is a struct or an array | `Ok(P { x: 1 })` / `Ok()`; an array payload was read out of a stack frame that had returned | refused | §60 | A decision on who owns the payload, and the `Result` calling convention |
| D2 | A `Result` whose payload kind nothing declares or proves, used at a declared kind | assertion fails / trap, later a crash reading a number as a string | refused; since §75 the commonest route (a `.copy` write-back into an untyped `Err` variable) is a type error in analysis instead | §72 a, §75 | As R4 |
| D3 | `assert` on a `Result`, an enum or a handle | assertion fails / passes for a non-zero word | refused | §70 f, §72 e | A decision on whether these are type errors |
| D4 | Unary minus on a `Result` | type mismatch / crash (134) | **an analysis error on both backends since 2026-11-20** (known-issues §82 g) | §70 d | Lifted: unary minus on anything that is not a number is an analysis error |
| D5 | A number literal beside a `T` element in a generic function's array literal, when it does not fit the element type the function is called with | `[x, 349]` with a `t8`: 349 / 93; with a `bool`: 349 / `true` — exit 0 on both | **an analysis error on both backends since 2026-11-20** (known-issues §81); compiled code's refusal stays behind it | §78 | Lifted: the owner decided that a generic body is checked against its type parameters |

## 3. Fixtures that are compiled-code SKIPs because of the above

Eighteen fixtures added by this work, and one that existed before it, pin
the interpreter's answer for a program compiled code refuses. They are counted
as SKIP by the parity harness, never as PASS. One further fixture is counted
as SKIP for a reason that is not a refusal, and is listed last.

| Fixture | Entry |
|---|---|
| **`t_literal_hint_ok_payload`** — existed on `submain` and passed there; it passed by reading an array out of a dead stack frame | D1 |
| `t_result_struct_payload_print`, `t_result_aggregate_payload_is_a_snapshot`, `t_result_aggregate_equality`, `t_result_aggregate_snapshot`, `t_assert_eq_str_in_result_aggregate` | D1 |
| `t_assert_on_a_result`, `t_assert_on_an_enum`, `t_assert_on_a_handle` | D3 |
| `t_len_of_a_variable_assigned_in_both_branches` | R6 |
| `t_user_function_named_like_the_runtime`, `t_user_function_named_like_the_float_remainder` | R7 |
| `t_generic_local_result` | R5 |
| `t_generic_local_keeps_its_type` | — the interpreter stops with a type mismatch; refusing is the matching outcome (since §81 the value reaches the local through a generic struct's field, which compiled code does not lower) |
| `t_print_struct_field_order`, `t_assert_eq_char`, `t_generic_local_char_handle_unknown`, `t_generic_local_raw_unknown`, `t_result_value_branch_with_err` | — forms compiled code has never lowered (a struct print, `char`, a printed handle, an unknown bool argument, a bare `?` passed for a type parameter, an `if`/`when` value whose branches are a `Result<t8>` and an Err); listed so the count adds up |
| `t_result_copy_free_of_err_only_variable` | — **not a refusal.** Both backends run it and agree. The harness counts a run that writes to stderr as SKIP, and `.copy.free` prints its deprecation warning there |

R4 and R5 are pinned by unit tests in `src/ir/lower.rs`
(`divergence_shape_tests`) rather than by fixtures: a fixture can pin the
interpreter's answer, but only a unit test can assert that compiled code
refuses and why.

Totals at the top of the stack (`backend/value-alias-semantics`): 806
fixtures, 749 PASS, 57 SKIP, 0 PARITY_FAIL. `submain` has 650 fixtures, 602
PASS, 48 SKIP. The 57 are `submain`'s 48, less 11 that are now analysis
errors on both backends (below), plus the 18 fixtures this work added to pin
the interpreter's answer where compiled code refuses, plus
`t_literal_hint_ok_payload`, plus the `.copy.free` fixture above.

Fixtures that were SKIPs and are now analysis errors on both backends, by the
owner's decisions of 2026-11-20: `t_receiver_variable_reached_by_name` (a
method that can reach its receiver's variable by name);
`t_result_of_unknown_kind_at_a_typed_use` and
`t_generic_array_literal_element_that_does_not_fit` (register entries D2 and
D5 — a generic body is checked against its type parameters, §81); and, of the
fixtures `submain` already had as SKIPs, `t_print_interpolation_type_conflict`,
`t_import_mono_enum_conversion` and the nine `t_module_enum_generic_*_reject`
fixtures, for the same reason. And `t_unary_minus_on_a_result` (register entry
D4): unary minus on anything that is not a number is an analysis error
(known-issues §82 g).

**Nothing in the value-and-alias amendment (known-issues §76) is a new
refusal.** Its rules are analysis errors on both backends, fixes to compiled
code, or — for V5 — a run-time error on the interpreter for a program compiled
code already refused. Two refusals were *lifted* along the way without being
listed here, because they were never introduced by this work: a `.copy`
write-back in a value branch (compiled code now carries it, §76g) and a
guarded catch-all followed by another arm (§77).

*(The independent review of the implementation found that this was not true
of the first version: the value merge of §76g refused a function's trailing
`if`/`else` in which a three-state bool is definite on one path, which had
compiled before. That is fixed (§79b) — the program runs again, and so does
the statement form, which `submain` refused.)*

## 4. Keeping this current

- A change that makes compiled code refuse a program the interpreter runs adds
  a row here in the same commit, with the example run on the base and on the
  change.
- A row is removed only by a change that carries a fixture for that program
  passing on **both** backends.
- A row whose program becomes an analysis error (as R1–R3 did under §75) is
  moved out of section 1 with a note saying so: it is no longer a legal
  program, which is a language change, not a compiler capability.
- Pull request descriptions report SKIPs separately from PASSes and name the
  fixtures.
- The gates are run on Linux **and Windows**. Both Windows CI jobs are
  permanent by the owner's decision of 2026-11-20; a refusal or a SKIP is
  never introduced to make one of them pass.
