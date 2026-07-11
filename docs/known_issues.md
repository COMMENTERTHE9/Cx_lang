# Cx Known Issues

This is a working log of known gaps and reference/JIT divergences found during
ongoing work, not a roadmap — see `docment/ROADMAP.md` for planned feature
sequencing. Entries are added as findings surface and updated (not deleted)
when fixed, so the history of what broke and why stays visible.

---

## 1. `f64` comparison (`<`, `>`, `<=`, `>=`) rejected by the interpreter

**Status: FIXED — commit `c5e8e22`.**

`src/runtime/ops.rs:173-208`'s comparison dispatch for `Lt`/`Gt`/`LtEq`/`GtEq`
had exactly one match arm per operator — `(Value::Num(a), Value::Num(b))`,
integers only — with no arm for `(Value::Float(a), Value::Float(b))`, falling
through to `RuntimeError::BadOperands`. Semantic analysis already accepted
float operands (`is_numeric` includes `F64`), so this was purely a
runtime-dispatch gap in the interpreter, not a semantic-analysis rejection.

**This was the reverse of the usual pattern.** Every other interpreter/JIT
divergence found this session had the JIT lagging the interpreter (JIT
missing something the interpreter already did correctly). Here the JIT was
already correct and the interpreter — normally treated as the reference
implementation — was the one that was wrong. Keep that context in mind if
anyone looks back at this entry later confused by the direction.

Fix: added the missing `(Value::Float, Value::Float)` arms, mirroring the
existing integer arms exactly. A mixed `Num`/`Float` comparison (e.g.
`5 > 3.14`) is not a separate case — semantic analysis's
`common_numeric_type`/`insert_cast_if_needed` already promotes the int
literal to `F64` via an inserted `Cast` node before it reaches the runtime,
so it flows through the same `(Float, Float)` arm.

---

## 2. Bare builtin call in trailing function/method-body position doesn't JIT-lower

**Status: FIXED — commit `b6310fe`.**

A bare `print(...)` call as a function or method body's *sole or last*
statement, with no trailing semicolon, used to fail to lower on the JIT:

```
unresolved semantic artifact reached lowering: function 'print'
```
Exit code 127. The interpreter always handled this correctly — this was a
JIT-only gap.

**Fix:** the parser's `func_body` combinator (`src/frontend/parser.rs:1207-1259`)
no longer promotes a trailing bare call to `print`/`println`/`printn`/
`assert`/`assert_eq` into the function's `ret_expr`. It's left as a normal
body statement instead, where the existing `lower_stmt` builtin dispatch
(`src/ir/lower.rs:864-880`, unchanged) already handles it correctly. No
interpreter changes and no lowering changes were needed — both were already
correct once the call stopped reaching them through the wrong path.

**Isolation confirmed, not assumed:** `t16`'s implicit-return-type check and
`t03`/`t160`/`t24`'s explicit-return-plus-trailing-expression check
(`src/ir/lower.rs:670-714`) were re-run before and after the fix and produce
**byte-for-byte identical** stdout and exit codes in both cases — confirmed
by direct `diff`, not by re-reading the (unmodified) code alone. Both
remain their own distinct, still-open bugs.

**Fixture-count result — the real, diff-verified number, not the original
estimate:** the investigation's static fixture search found 7 files (8
call sites) with this shape and predicted all would convert from JIT-SKIP
to JIT-PASS. The actual result, obtained by running the full fixture corpus
through the JIT before and after the fix and diffing the two SKIP sets
directly (not by re-checking the originally-named fixtures one at a time):

- **5 fixtures genuinely converted from SKIP to PASS**: `t29_forward_decl`,
  `t31_strref_forward_combined`, `t67_macro_outer_test`,
  `t68_macro_outer_deprecated`, and `t_array_elem_arg_in_range` — the last
  of these was **not** in the original investigation's list at all; its
  static search missed it because its function body is condensed onto a
  single line (`fnc: f(a: [3: t8]) { print(a:[2]) }`), a shape the
  line-based search wasn't tuned for. Found only by actually running the
  fixture corpus, not by re-reading source.
- **2 of the originally-named fixtures were never actually SKIP**:
  `t71_macro_unknown_outer_reject` and `t73_macro_reactive_reserved` are
  `.expected_fail` fixtures that reject at the macro-processing step in
  semantic analysis, before a function's `ret_expr` is ever considered —
  both backends already converged on the same correct rejection before this
  fix, so there was nothing for this fix to change for them. Confirmed
  unaffected (identical error text and exit code, before and after).
- **1 originally-named fixture correctly did not convert**:
  `t50_nested_func_no_leak` remains SKIP, but for a completely separate,
  pre-existing, already-documented JIT limitation — nested function
  definitions aren't lowered at all (`unsupported semantic construct during
  lowering: nested FuncDef`), independent of this bug. Confirmed identical
  before and after via a real before/after diff (used `git stash` to build
  the exact pre-fix binary and compare). `t50` is now cleanly isolated as
  blocked *only* by the nested-function-lowering limitation — a natural
  proof-fixture for whenever that limitation gets addressed.

JIT parity moved from **261/60/0** to **267/55/0** across 322 fixtures
(321 existing + 1 new permanent regression fixture,
`t_print_trailing_position.cx`, added for the original reproducer shape).
261 + 5 conversions + 1 new passing fixture = 267; 60 − 5 = 55 — the totals
reconcile exactly.

**Not method-specific** — confirmed via a plain free-function reproducer
(`fnc: show(x: t32) { print(x) }`) with an identical failure and identical
error text/exit code. The original framing ("print inside a method body")
mis-attributed the cause to methods; methods just happen to be a natural
place to write this shape (a one-line `fnc: show_health() { print(p.health) }`).

**Mechanism:** the parser's `func_body` combinator
(`src/frontend/parser.rs:1207-1259`) purely-syntactically promotes the body's
last statement into `ret_expr` whenever it's a bare `ExprStmt` with no
trailing semicolon — with zero understanding of what the expression is. The
resulting `ret_expr` is lowered via the general expression path,
`lower_expr` (`src/ir/lower.rs:686-687`), not the statement path,
`lower_stmt`, which is where `print`/`println`/`printn`/`assert`/`assert_eq`
get their special-case builtin interception (documented at
`src/ir/lower.rs:94-95`). `lower_expr`'s `Call` handling
(`src/ir/lower.rs:1969-1984`) carries an explicit comment acknowledging this
exact scenario was assumed not to happen: *"assert/assert_eq, print,
println, and printn are handled at statement level and should not reach
`lower_expr` in well-formed programs."* `is_cx_builtin`
(`src/ir/lower.rs:96-104`) only recognizes builtins whose JIT status is
`GatedUnsupported` (genuinely-unimplemented ones like `read`/`input`) —
`print`'s status is `Lowered`, so it isn't caught there either, and falls
through to a raw `signature_table` miss, producing the observed
`UnresolvedSemanticArtifact`.

Only `print` was empirically tested; `println`/`printn`/`assert`/`assert_eq`
likely share the same failure by the same code path (same
`lower_stmt`-interception list) but this has not been individually
confirmed for each.

**Risk framing:** low risk of a silent false-parity-fail. This fails with
the JIT's own SKIP exit code (127), so if someone later writes a fixture
with this exact shape, `jit_parity_by_feature` will correctly bucket it as
SKIP, not a misleading PARITY_FAIL — unlike finding #1 above (which, before
the fix, would have shown as a genuine PARITY_FAIL had a fixture existed,
since the JIT succeeded while the interpreter errored).

**Relation to #3 below:** sits in the same code region (both concern a
function's trailing-expression / `ret_expr` handling) but is **not a
duplicate** — confirmed by direct mechanism/error-text comparison, not
name-similarity. This bug raised `LoweringError::UnresolvedSemanticArtifact`
at `lower.rs:686-687` (the `lower_expr` call itself failing); #3 raises
`LoweringError::InternalInvariantViolation` one step further down, at
`lower.rs:677-684`. In the specific reproducer tested here (a void function,
no `return` statement, `print(...)` as the only body statement), this bug
fired first and **preempted** #3's check from ever being reached.

*Correction after actually fixing and testing it, not just predicting:*
before the fix, this section speculated that fixing this bug would make the
reproducer fall through to the *separate* `t16` implicit-return-type gap
instead. That didn't happen. The fix (parser-level: don't promote the
builtin call into `ret_expr` at all) means `ret_expr` stays `None` for this
reproducer, not just "successfully lowered" — so neither this check nor
`t16`'s (which specifically requires `ret_expr.is_some()`) ever fires. The
prediction was reasonable at the time but wrong; the actual fix sidesteps
the whole region rather than moving the failure point within it. Confirmed
by running the reproducer post-fix: clean output, exit 0, on both backends.

---

## 3. `t03`/`t160`/`t24` — explicit return + trailing expression

**Status: OPEN, deferred** (found during the `t16`-cluster scoping audit;
deferred there as open-ended, not yet sized).

A function with **both** an explicit `return` statement and a separate,
now-dead trailing expression statement after it triggers:

```
LoweringError::InternalInvariantViolation {
    detail: "function '{name}' has both explicit return terminator and trailing return expression"
}
```
at `src/ir/lower.rs:677-684`.

Affects `t03_explicit_return.cx`, `t160_direct_call_explicit_return_exit.cx`,
and (as a compound case, masking a second, separate bug — the `t16`
implicit-return-type gap) `t24_full_system_regression.cx`.

**Distinct from #2 above** — different error variant
(`InternalInvariantViolation` vs. `UnresolvedSemanticArtifact`), different
trigger (requires an explicit `return` statement present in the same
function; #2's reproducer has none) — confirmed by direct comparison of
error text and mechanism, not by name-similarity.

Not yet sized. The `t16`-cluster audit found the related implicit-return-type
gap touches broad, shared function-signature infrastructure with unclear
full extent, and recommended deferring the whole cluster rather than
attempting a quick cleanup. This entry (`t03`/`t160`/`t24` specifically) was
not separately re-scoped beyond that.

---

## 4. `print(enum)` diverges between interpreter and JIT

**Status: OPEN — sized, not attempted.** Confirmed real sizing, not a
dispatch-arm fix.

Printing a bare enum-typed value produces different output on each backend:

- Interpreter: the variant name, e.g. `Color::Green`
  (`src/runtime/runtime.rs:152`: `Value::EnumVariant(e, v) => format!("{}::{}", e, v)`).
- JIT: the raw tag value, e.g. `1` — because `SemanticType::Enum(_)` erases
  to the tag's IR type, `IrType::I8`, at lowering
  (`src/ir/lower.rs:4563`), and printing an `I8` just prints the integer
  (`route_print_arg`, see #5 below for its full dispatch).

Found incidentally while designing the pattern-matching `as v`-binding
discriminating-canary fixture: `print(v)` on an enum-typed binding was
briefly considered as the test's payload, then dropped in favor of a
nested-`when`-based canary specifically to avoid this exact divergence
contaminating an unrelated feature's regression test.

No existing fixture in the verification matrix exercises `print` on a bare
enum value in a way that's checked for interp/JIT parity, so this gap has
not yet surfaced as a `PARITY_FAIL` in `jit_parity_by_feature` — but would,
if one were added.

**Real sizing, confirmed by reading the lowering path before attempting
anything:** `EnumDef` lowering (`src/ir/lower.rs:511,1100`) emits **zero
IR** — no static data, no table, nothing. There is no tag→name lookup
structure anywhere in the JIT pipeline; by the time a value reaches
`route_print_arg` it's already erased to a bare `IrType::I8`, structurally
indistinguishable from a plain `t8`, with no way to know it came from an
enum at all, let alone which enum or what its variant names are. Fixing
this is not a dispatch-arm addition — it requires designing and building
new static infrastructure: a per-enum tag→name string table, emitted at
`EnumDef`-lowering time (from the semantic layer's `EnumId`/variant-name
info, which does still exist at that point), referenced at the print call
site via a new lookup mechanism. This is real design work, not a quick fix.

This is now the **more-precisely-understood** of the two "not attempted"
fixes from tonight's pass (see #5) — sized correctly and deliberately not
attempted, rather than attempted and found broken. It remains the more
dangerous of the two bugs in this file: a silent divergence with matching
exit codes on both sides, not a clean refusal.

---

## 5. Bare `I128` printing not lowered on JIT

**Status: OPEN — attempted, built cleanly, JIT reproducer segfaulted.** The
previous "fully scoped, low risk, ready to build" framing in this entry was
wrong — removed below.

`route_print_arg` (`src/ir/lower.rs:4646-4667`) dispatches on the print
argument's `IrType`: `I64` direct, `I8`/`I16`/`I32` via a widening `Cast`,
`Bool`/`TBool` via `cx_print_bool` — and a catch-all `_ => Ok(None)` that
rejects everything else uniformly (`F64`, `I128`, `Ptr`, `Str`, composites).
`I128` is a **plain omission**, not a deliberate architectural exclusion —
nothing else in the surrounding code treats `I128` specially or defers it on
purpose.

Affects any bare `i128`-typed print, including reading a `Handle<T>`'s value
when `T` is `t128`.

**What was actually tried:** a new `IrType::I128` arm in `route_print_arg`,
a new `cx_print_i128(n: i128)` host callback mirroring `cx_printn`'s exact
shape (`extern "C" fn` + JIT symbol registration + Cranelift signature
declaration), plus the matching IR-validator registration. Built with zero
compile errors. The JIT reproducer (`x: t128 = 42; print(x)`) then
**segfaulted (exit 139)** — not value-dependent: reproduced identically with
a trivial value and with `i128::MAX`. Reverted in full
(`git checkout --` on all three touched files); confirmed the working tree
returned to a clean diff and the original behavior (a structured
`UnsupportedSemanticConstruct` error, exit 127 — not a crash) still holds at
HEAD.

**Root cause (diagnosed, not fixed):** passing a raw `i128` by value into a
Rust `extern "C"` host callback from Cranelift-JIT-compiled code is a
boundary this codebase has never actually exercised before, despite
appearances. `Result<T>`'s `i128` (D2.4a) is returned *from* Cranelift code
via the packed representation — it never crosses into a Rust host function
as an `i128` argument. Every existing Handle callback (`cx_handle_new`,
`cx_handle_val`, `cx_handle_drop`) passes `i64`. So this fix was the first
real attempt to pass a native Cranelift `I128` value as an argument to an
`extern "C" fn(i128)`, and it doesn't work. The existing
`enable_llvm_abi_extensions` flag (already enabled, `host_boundary.rs:566`)
is documented as covering the packed-i128 `Result<T>` rep for *internal*
Cranelift-to-Cranelift value passing — a different boundary from calling
into an external Rust host symbol. Most likely explanation: a Windows x64
calling-convention mismatch — the Microsoft x64 ABI conventionally passes
wide (>8-byte) scalars by reference, not in a register pair, and Cranelift's
native `I128` type may not marshal to that convention when emitting a call
to an external symbol.

**Suggested fix direction for whoever picks this up next — untested, a
direction, not a solution:** pass the `i128` by pointer instead of by value,
mirroring how `str` descriptors already cross this exact host-boundary
successfully today (`cx_print_str`, `src/backend/cranelift/host_boundary.rs:301-310`,
a leaked `&'static` descriptor passed as an address). This would need the
JIT to spill the `I128` SSA value to a stack slot and pass its address, then
have the callback dereference it — a materially different (and larger)
shape than "one new match arm plus one host callback." Not attempted; needs
its own sizing pass before a second attempt.
