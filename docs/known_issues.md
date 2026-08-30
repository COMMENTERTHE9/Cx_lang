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

---

## 6. `if`/`while`/`loop`/`while-in` bodies not scoped in semantic analysis

**Status: FIXED — commit `603aa61`.**

`semantic.rs`'s `Stmt::IfElse`/`Stmt::While`/`Stmt::Loop`/`Stmt::WhileIn` didn't
push/pop a scope around their bodies, unlike sibling `Stmt::Block`/`Stmt::For`/
`Stmt::When` in the same file, and unlike the interpreter (`runtime/exec.rs`),
which already scoped all four correctly and symmetrically. Result: two
sibling branches or loop bodies each declaring an unrelated same-named local
were rejected outright with `SEMANTIC ERROR: variable already declared in
this scope` — valid, ordinary programs, on both backends.

**Fix:** added `self.push_scope()`/`self.pop_scope()` pairs around each body,
mirroring the existing `Stmt::Block` pattern (push once, analyze via the
existing `?`/`.collect()` chain, pop once on success) rather than
`analyze_for`'s more complex explicit-pop-on-every-error-path pattern, which
doesn't apply here since these four constructs have no equivalent
early-return-from-within-the-body logic. `IfElse` gets three independent
scopes (then-body, each `else if` body, else-body); `WhileIn` gets one for
the main body plus one independent scope per `then_chains` chain.

Verified via a discriminating shadowing canary for all four constructs — not
just "two unrelated same-named locals stop colliding" (which wouldn't rule
out a fix that simply disabled the collision check entirely, a worse
regression than the bug it fixes): an outer-scope variable plus an inner
same-named local proves the inner one shadows correctly (reads inside the
body see the inner value), does not leak (a read after the body sees the
outer value, unchanged), and does not corrupt the outer binding. Four new
regression fixtures added (`t179_if_scope.cx` through
`t182_whilein_scope.cx`), each covering both the sibling-blocks case and the
shadowing case.

JIT parity moved **267/55/0 → 270/56/0** across 326 fixtures (322 existing +
4 new): +3 PASS for `if`/`while`/`loop`, +1 SKIP for `while-in` — confirmed,
not assumed, to be a pre-existing, unrelated Cranelift lowering gap: an
existing, scope-collision-free fixture (`t34_while_in.cx`) hits the identical
`unsupported semantic construct during lowering: WhileIn` error regardless of
this fix, via a stash-based before/after diff. `WhileIn` simply isn't lowered
to Cranelift IR yet, independent of scoping.

---

## 7. Silent integer truncation at method-call args, plain reassignment, array-index assignment

**Status: FIXED — commit `2d9a70b`.**

`check_semantic_num_fits` (the width-range check) claimed in its own doc
comment to be the single entry point for every relevant site, but was not
actually called at three of them: method-call args (`semantic.rs`
~1738-1760), plain reassignment (`Stmt::Assign`'s `Expr::Ident` arm,
~429-464), and array-index assignment (`Stmt::Assign`'s `Expr::Index` arm,
~505-524) — each has a sibling site in the same file (struct-field
assignment, typed declaration, free-function call args) that calls it
correctly. Result: `x: t8 = 5; x = 300; print(x)` printed `44` (300 mod 256)
with zero error, on the interpreter.

**Fix:** added the missing `check_semantic_num_fits(...)` call at each of
the three sites, mirroring the working sibling sites' exact call shape and
ordering (check num-fits, then check type-compat, then insert cast). Verified
via a discriminating in-range canary at all three sites — proving legitimate
in-range values are not overcorrected into rejection — plus 6 new regression
fixtures (a reject/accept pair per site).

**Correction to this finding's own premise, found during verification, not
assumed:** Cranelift was never silently truncating the way the interpreter
was. It already refused these cases via a lowering-time check
(`unsupported semantic construct during lowering: integer literal 300 does
not fit in I8`, exit 127/SKIP) — a pre-existing, independent backstop,
confirmed via a stash-based before/after diff. The bug's live impact was
interpreter-only; the fix's actual improvement on Cranelift is replacing that
ad-hoc, late, SKIP-coded lowering rejection with a clean, early
`SemanticError`, produced identically on both backends before either
backend's execution begins.

JIT parity moved **270/56/0 → 276/56/0** across 332 fixtures (326 existing +
6 new): all six new fixtures land as full PASS on both backends (no new
SKIP), since both backends now reject at the semantic layer before ever
diverging.

---

## 8. `<`/`>`/`<=`/`>=` silently accepted on non-numeric operands

**Status: FIXED — commit `3ad89b7`.**

The ordering-comparison semantic branch (`Op::Lt | Op::Gt | Op::LtEq |
Op::GtEq`, `semantic.rs` ~2273-2299) had no type-allowlist, unlike its
sibling `EqEq`/`NotEq` branch immediately above it. Result: `Color::Red <
Color::Blue` was correctly rejected by the interpreter (`RUNTIME ERROR:
operator 'Lt' cannot be applied to enum variant and enum variant`) but
silently accepted by the JIT, computing a nonsensical `true`, exit 0.
Reproduced identically for `bool` operands and for all four operators
(`<`/`>`/`<=`/`>=`), not just `<`.

**Fix:** added an `else` arm to the branch, mirroring the equality branch's
shape (`Unknown` pass-through → numeric fast path → `sem_err!` otherwise).
Confirmed via a stash-based before/after diff that the equality branch
itself is completely untouched by this change.

**Scope generalization, found and folded in during the fix, not originally
in the audit's finding:** the audit named `Bool`/`Enum` specifically, but
`runtime/ops.rs` only ever supported ordering on `Num`/`Float` operands —
`Char` reproduces the identical bug shape (interpreter: a late
`RuntimeError`; JIT behavior not independently confirmed for `Char`, but
presumed the same class). Rather than special-case exactly the two named
types, the fix rejects any non-numeric operand pair, matching what the
runtime actually supports and closing the whole class in one change instead
of leaving a near-identical gap unaddressed for `Char`/`Str`.

Verified via a discriminating canary: numeric ordering (including `f64`)
unaffected on both backends; equality comparison on `bool`/`enum` unaffected,
confirmed byte-identical pre/post-fix via stash diff. Four new regression
fixtures added (reject-on-enum, reject-on-bool, accept-on-numeric,
accept-equality-on-bool).

JIT parity moved **276/56/0 → 280/56/0** across 336 fixtures (332 existing +
4 new): all four land as full PASS on both backends (no new SKIP) — a pure
semantic-time catch that fires identically before either backend executes,
matching finding #7's shape rather than finding #6's.

---

## 9. Enum `==`/`!=` crashes on the interpreter

**Status: FIXED — commit `7a4fd5f`.**

Semantic analysis correctly allows equality comparison on same-typed `Enum`
operands (the `EqEq`/`NotEq` branch's type-allowlist includes
`SemanticType::Enum(_)`), but `runtime/ops.rs`'s `Op::EqEq`/`Op::NotEq` match
had no arm for two enum-variant operands — it fell through to the generic
`(l, r) => Err(RuntimeError::BadOperands)` catch-all. E.g. `a: Color =
Color::Red; b: Color = Color::Blue; print(a == b)` crashed at runtime on the
interpreter with `RUNTIME ERROR: operator 'EqEq' cannot be applied to enum
variant and enum variant`. Cranelift already handled this correctly (computes
and prints the right `bool`) — for once, the JIT was the reference to copy
from, not the interpreter.

Found incidentally during audit finding 3.3's (#8 above) equality-branch
regression verification. Confirmed pre-existing and completely unrelated to
that fix via a stash-based before/after diff: byte-identical error, both
before and after that fix, on both backends.

**Same shape as finding #1 above** (the `f64`-comparison bug): the type
system (semantic analysis) promises support that a downstream layer — here,
the interpreter's runtime dispatch table; there, the same — doesn't actually
implement. Third instance of this exact pattern found this project.

---

## 10. Silent struct-return corruption on the JIT — all function kinds

**Status: FIXED — same commit as this entry (caller-allocated return slot).**

Any function returning a struct by value — free function, impl method, or
phen method, all through the shared `lower_semantic_function` path — returned
the `Ptr` of a callee-frame alloca. That frame is dead the moment the call
returns, so the caller read garbage: probed empirically on the baseline
binary, `make(13, 24)` printed `13` then `32758` on cranelift (interp:
`13`/`24`), **exit 0, no error** — the worst failure class this project
tracks, silent wrong output with matching exit codes.

**The coverage blind spot, named explicitly:** this was live for every
function kind and never surfaced as a PARITY_FAIL because **not one fixture
in the entire corpus returned a struct from any function** — the parity
harness can only compare shapes the corpus exercises. Found only when
slice 5's ABI check (the bare-I128 lesson: enumerate what crosses a boundary,
probe it, don't assume) probed a method-struct-return before reusing the
path. Free functions were the worst exposure: methods briefly had slice 5's
guard forcing clean SKIPs, but a free function returning a struct silently
corrupted with no guard at all.

**Fix:** caller-allocated return slot, one convention in the shared path for
all three producer kinds. `FunctionSignature` carries the returned struct's
identity; struct-returning functions receive a hidden trailing `$ret_slot:
Ptr` param; every return site field-copies the result through the slot
(same Load/PtrOffset/Store idiom as struct-literal lowering) and returns the
slot pointer — no pointer into a dead frame ever escapes. Call sites alloca
the slot in the caller's frame and pass its address. Nothing wider than a
machine word crosses any boundary; nothing new crosses the Cranelift/host
boundary at all. Slice 5's phen struct-return guard is lifted; ARRAY returns
remain guarded (same dangling-frame shape, slot convention not yet built for
array layouts — tracked follow-up).

Regression fixtures: `t_struct_return_free` / `t_struct_return_impl` (the
exact 13/32758 shapes, now asserting 13/24), double-call contamination
canaries `t_struct_return_double_free` / `t_struct_return_double_method`
(two calls, asymmetric inputs, both results independently correct — no slot
reuse contamination), plus `t_gene_phen_call_self_runtime` converting from
guarded SKIP to genuine PASS. A `v.method().x` nested-receiver case is not
expressible in today's grammar (`DotAccess` containers are identifiers, not
expressions) — noted, not forced.

**Fix:** the interpreter's `Value::EnumVariant(String, String)`
representation (`enum_name`, `variant_name` — confirmed by reading
`runtime/eval.rs:178-179` and `runtime/exec.rs:548`'s existing pattern-match
comparison, not assumed) has no numeric tag at all, unlike the JIT's IR
representation, where an enum erases to a bare `IrType::I8` tag
(`src/ir/lower.rs:4563`) and equality falls through Cranelift's generic
scalar `Compare` lowering (`src/ir/lower.rs:2574-2612`) once `TBool`/`str`/
composite `Ptr` cases are ruled out — i.e. Cranelift's "reference" logic here
is just an ordinary integer-tag compare, confirmed by reading the actual
lowering path rather than assumed from the pattern-matching arc's tag-only
framing. Since the interpreter's `Value` has no tag field to compare, the fix
adds one `(Value::EnumVariant(e1, v1), Value::EnumVariant(e2, v2))` arm to
each of `Op::EqEq` and `Op::NotEq`, comparing both fields
(`e1 == e2 && v1 == v2`), mirroring the exact style `exec.rs:548`'s
pattern-match arm already uses for the identical comparison. `NotEq` needed
its own explicit arm — confirmed, not assumed: `EqEq` and `NotEq` are two
fully independent `match` blocks in `ops.rs`, each with its own complete set
of per-type-pair arms, not a shared helper with negation, so nothing "falls
out for free."

Verified via a discriminating canary proving genuine discrimination, not
just "didn't crash": same-variant operands compare equal in both directions
(`==` → `true`, `!=` → `false`); different-variant operands compare unequal
in both directions (`==` → `false`, `!=` → `true`) — matching Cranelift's
output exactly, on all four combinations, both before (JIT only) and after
(both backends) the fix. One new permanent regression fixture added,
`t_enum_equality.cx`, covering both cases and both operators in a single
program (no error path here to force splitting across files, unlike prior
scoping/truncation fixes).

JIT parity moved **280/56/0 → 281/56/0** across 337 fixtures (336 existing +
1 new): a full PASS on both backends, no new SKIP — a pure interpreter-side
runtime fix with no lowering-side change at all.

---

## 11. `exit()` builtin does not lower on the JIT

**Status: FIXED — commit `3d7a2cd`.**

`exit(code)` runs correctly on the interpreter — it raises the Exit control-flow
signal and the process exits with the given code — but has no lowering path.
A call reaches the IR layer as an unresolved name:

```
unresolved semantic artifact reached lowering: function 'exit'
```

Exit code 127 (the JIT's SKIP code), so it fails cleanly rather than silently.
Verified directly: `print(1); exit(3)` → interpreter prints `1` and exits `3`;
cranelift prints the error above and exits `127`.

**7 SKIPs** — the second-largest single cause in the 61-fixture SKIP set, behind
generic functions (8). Worth calling out for what it means in practice: on the
JIT this is the difference between a program *running* and a program *running
with the right exit code*, which is exactly what a build script or a test
harness keys on.

The README listed `exit` among the working builtins with no backend caveat
(corrected in the 0.3.2 accuracy pass — the "not yet lowered" list named it;
that entry is now stale in the other direction and should drop on the next
docs pass).

**Fix:** a `cx_exit(code: i32) -> !` host callback mirroring `cx_trap`'s shape
(symbol registration, Cranelift signature declaration, IR-validator intrinsic
entry), plus a `lower_stmt` intercept keyed on `BuiltinKind::Exit` like the
other statement-level builtins. `exit()` with no argument lowers as `exit(0)`,
matching the interpreter's `None => 0`. Nothing wider than a machine word
crosses the boundary — the code is an `i32` on both sides
(`RuntimeError::Exit(i32)`), so the `i128`-class hazard does not apply.

**stdout is flushed before `process::exit`** on the host side, mirroring the
interpreter's own exit path. Without it, `print(...); exit(N)` would agree on
the exit code and silently lose piped output — a failure a code-only fixture
would never catch. Verified with piped stdout on both backends.

**A second, distinct bug surfaced during verification** — six fixtures
converted immediately but `t_exit_in_function` did not, still reporting
`unresolved semantic artifact reached lowering: function 'exit'`. Cause: the
parser's `is_statement_level_builtin_call` guard (added for known-issues #2)
lists `print`/`println`/`printn`/`assert`/`assert_eq` but **not** `exit`, so a
function whose body ends in a bare `exit(...)` had that call promoted into
`ret_expr` and routed through `lower_expr`, which has no builtin interception.
`exit` is void and statement-level exactly like the others; adding it to the
guard fixed it. Same bug class as #2, one builtin missed when that list was
written.

Exit-code fidelity verified against the interpreter at 0, 1, 3, 42, 125, 126,
127, 200, 255, and across the wrap boundary (256 → 0, 300 → 44) — identical on
both backends at every value, including negative codes (both 127 on Windows).
JIT parity moved **319/61/0 → 326/54/0** across 380 fixtures: exactly the seven
`exit` fixtures converted, nothing else moved.

---

## 12. Top-level `const` declarations do not lower on the JIT

**Status: FIXED — commit `6c37339`** (with one narrowing and one finding, both
below).

`const NAME: T = value` at top level is a documented language feature that works
on the interpreter and has no JIT path:

```
unsupported semantic construct during lowering: ConstDecl
```

Verified directly: `const MAX: t32 = 100; print(MAX)` → interpreter prints `100`,
exit 0; cranelift emits the error above and SKIPs.

**3 SKIPs.** Structured error, exit 127 — clean refusal, not corruption. Listed
in the README's "not yet lowered" section as of the 0.3.2 accuracy pass.

**What a Cx const actually is** (established before building, since the fix's
shape depended on it): the grammar requires a type annotation and accepts an
arbitrary expression on the right — not literals only. `const` is
**top-level-only**; inside a function body it is a parse error. Semantic
analysis gives it an ordinary `BindingId` via `declare()`, and the interpreter
evaluates the RHS once and stores it like any typed binding
(`runtime/exec.rs`'s ConstDecl arm → `set_var_typed`). A const is therefore an
immutable *binding*, not a substituted literal.

**Fix:** the `ConstDecl` lowering arm mirrors `TypedAssign`'s general path —
lower the RHS, bind the SSA value to the const's binding — rather than
introducing a module-level global the IR has no concept of. One addition
`TypedAssign` did not need: a width `Cast` when the value's type differs from
the declared type. The semantic ConstDecl arm, unlike TypedAssign's, never
calls `insert_cast_if_needed`, so `const A: t32 = 5` reaches lowering with an
I64 literal against an I32 target. Narrowing in the lowering arm (the same
idiom struct-field stores use) keeps this a lowering-only change — the
semantic tree the interpreter consumes is untouched.

Verified byte-identical on both backends for `t32`/`t64`/`f64`/`bool`/`str`
consts, and for consts used in arithmetic, comparisons, loop bodies, and as
function arguments. Discriminating canary: two distinct same-type consts
(`7` and `900`) in the same expression, checked in both operand orders
(`893` / `-893`) so a substitution mixup could not produce a symmetric
false pass.

### Narrowing: assignment to a const is refused, not lowered — RESOLVED in `fa95c12`

*(The narrowing below described the state at `6c37339`. The underlying hole is
now closed at the semantic layer — see "Const immutability moved to analysis
time" at the end of this entry. The JIT guard has been removed and
`t57_const_reassign_reject` is a genuine PASS on both backends.)*

`t57_const_reassign_reject` did **not** convert, deliberately. Lowering the
whole construct made the JIT print `200` for a program the interpreter rejects
— a genuine `PARITY_FAIL`, observed before the guard was added.

**Root cause, and it is a real finding:** const immutability is enforced only
by the **interpreter at runtime** (`invalid assignment target — only variables
and container fields (t.x) can be assigned to`). Semantic analysis accepts
`MAX_HP = 200` without complaint; there is no analysis-time const-immutability
check anywhere. Any backend that does not happen to reproduce the interpreter's
runtime check will therefore silently accept the assignment.

The guard: lowering tracks `ConstDecl` bindings and refuses an `Assign`
targeting one, with a structured `UnsupportedSemanticConstruct` (exit 127,
clean SKIP). That is honest — the JIT declines to compile what it cannot match
— but it is a workaround, not the fix.

**The proper fix is an analysis-time rejection**, so both backends refuse
identically for the same reason at the same phase. That changes
interpreter-observable behavior (the error moves from runtime to semantic
analysis, with different text), so it is filed here for a separate dispatch
rather than smuggled into a lowering-only change.

### Also not lowered: a const referenced inside a function body

Top-level consts bind into synthetic main's SSA scope; a function lowers as a
separate `IrFunction` that cannot see that binding, so
`const K: t32 = 10` + `fnc f() { v + K }` fails with
`binding 'K' referenced before any SSA value was assigned` — exit 127, a clean
SKIP, never a wrong value. Supporting it needs real module-level globals in the
IR, which is a larger change than this slice. Passing a const *as an argument*
from a top-level call site works and is verified.

JIT parity moved **326/54/0 → 328/52/0** across 380 fixtures: `t56_const_basic`
and `t173_const_decl_exit` converted; `t57_const_reassign_reject` remains a
SKIP by the narrowing above; nothing else moved.

### Const immutability moved to analysis time — FIXED in `fa95c12`

The filed finding above is closed. `reject_const_assignment` in `semantic.rs`
rejects any assignment rooted at a const binding, at analysis time, so both
backends refuse identically before either runs — the same reasoning that put
the width checks and the ordering-comparison allowlist in the semantic layer
rather than in a backend.

**Enumerating the assignment forms first turned up a worse bug than the one
being fixed.** The interpreter's runtime guard (`scope.rs`, `set_var` /
`set_var_by_id`) checks the const table *by name*, so it only ever covered
name-based writes. Writing *through* a const was unguarded **on both
backends**:

```
const A: [3: t32] = [1, 2, 3]    const S: P = P { x: 1 }
A:[0] = 9                        S.x = 9
print(A:[0])   // printed 9      print(S.x)   // printed 9
```

Silent const mutation, exit 0, no error — not a JIT divergence but a live
language hole. Enumerating every form before fixing the obvious one is exactly
what the width-check bug (#7, three missed sites) taught; it paid for itself
here.

All six forms now reject identically on both backends:

| form | result |
|---|---|
| `K = 2` | `cannot assign to 'K' — it is declared const` |
| `K += 2` / `K -= 2` | same |
| `A:[0] = 9` | `cannot assign to 'A' — it is declared const` |
| `A:[0] += 9` | same |
| `S.x = 9` | `cannot assign to 'S' — it is declared const` |

The check sits at the single entry point of `Stmt::Assign` and
`Stmt::CompoundAssign`, before either arm's target match, with a shared
`assign_target_base_name` helper resolving `x` / `x.f` / `x:[i]` to their root
name. That placement is deliberate: a per-arm check is what allowed three forms
to be missed in the first place, so the guard covers every form by construction
rather than by remembering to patch each one.

**Anti-overcorrection verified**: ordinary (non-const) bindings still mutate
normally in every affected form — plain, compound, index, index-compound,
field, field-compound — byte-identical on both backends
(`t_const_mutation_still_works`). Const *reads* are unaffected.

**Interpreter runtime check: kept.** It is now redundant for programs that pass
analysis, but it is three lines, it costs one `is_empty()` on the write path,
and it is the last line of defense if a future construct reaches a runtime
write without going through `Stmt::Assign`/`Stmt::CompoundAssign` analysis
(method write-back and string-interp targets both call `set_var` directly).
Removing it would trade real defense-in-depth for no measurable gain.

The JIT's const-assignment lowering guard from `6c37339` is **removed** — it
existed only because the hole existed. Confirmed no divergence returns:
analysis now rejects before either backend runs.

Parity moved **328/52/0 → 333/51/0** across 384 fixtures (380 + 4 new):
`t57_const_reassign_reject` converted SKIP → genuine PASS, the 4 new fixtures
pass, nothing else moved.

---

## 13. Library-only fixtures produce compile/link SKIPs — harness artifact, not a language gap

**Status: NOT A BUG — recorded so it stops being re-investigated.**

Four fixtures in the verification matrix are *library* files: they contain only
`pub fnc` definitions and no `main` or top-level statements, and exist to be
imported by a sibling fixture that does the actual asserting:

- `t64_import_pub_only_lib.cx`
- `t74_import_basic_lib.cx`
- `t_prelude_multi_import_lib_a.cx`
- `t_prelude_multi_import_lib_b.cx`

`run_matrix.sh` and any SKIP-set enumeration glob `t*.cx`, so these are also run
*standalone*, where a module with no entry point produces:

```
--- cx jit: compile/link failed — IR dump ---
```

exit 127. That is a SKIP in the totals, but it represents **no lost language
coverage** — the feature they exist to test is exercised through their importing
fixture, which passes.

This has twice appeared as phantom "new SKIPs" in release SKIP-set diffs (once
during the slice-5 JIT arc, once at the 0.3.2 release) and both times cost a
round of stash-rebuild-retest to confirm it was baseline-identical. The shape to
recognize: **a `*_lib.cx` name plus a `compile/link failed` first line means
harness artifact, not regression.** A future fixture-organization pass could move
library files to a non-`t*` prefix so the glob stops picking them up.

---

## 14. Access-path enforcement holes (audit C1–C4) — FIXED in `2de51aa`

**Status: FIXED.** The locked principle this produced lives in
`docs/frontend/enforcement_layers.md`.

A deliberate layer-enforcement audit asked one bounded question: for every error
the interpreter can raise at runtime, does semantic analysis catch it first?
Four holes came back, and they were one omission wearing four hats — **a fact
validated on the construction path and never on the access path.** A struct's
field list, an enum's variant list and every receiver's type are all carried in
the semantic type; analysis had each of them in hand and used them only where a
value is *constructed*.

### What each hole did

| | program | interpreter | JIT |
|---|---|---|---|
| **C1** | `a.zzz = 5; print(a.zzz)` on `struct P { x: t32 }` | **exit 0, printed `5`** — invented the field at runtime | exit 127 → counted a clean SKIP |
| **C1** | `print(a.zzz)` | exit 1, `variable 'a.zzz' has not been declared — declare it with 'a.zzz: TYPE = value'` | exit 127 |
| **C2** | `for i in 0..3 { if i >= 0 { i = 99 } print(i) }` | **exit 0, `99 99 99`** | **exit 0, `99`** |
| **C3** | `enum L { Red, Green }` + `c: L = L::Blue` | **exit 0** — phantom variant, matched no real arm, compared equal to itself | exit 127 |
| **C4** | `x: t64 = 5; print(x:[0])` | exit 1, but reported as an *assignment target* error for a read | exit 127, `lowering invariant violation` |

C1's write form and C3 are silent wrong answers. C1's write also contradicts
Cx's stable-layout guarantee: a typo'd field name grew the struct at runtime.

**C2 was a live PARITY_FAIL with no fixture.** By the harness's
`PassWithOutput` rule, `99 99 99` against `99` is a divergence — it escaped only
because no fixture in the matrix exercised loop-counter mutation. The rule was
implemented in three places, each documented as backup for another: analysis
matched two statement shapes, the interpreter's `RuntimeError::ReadOnlyLoopVar`
was `#[allow(dead_code)]` with a comment deferring to the IR layer, and the IR
validator's `LoopVariableReassignment` did not fire (`--backend=validate`
printed "IR validation passed" on the divergent program).

### The fix — choke points, not arms

Each hole closes at the single point where the fact and the access meet. The
per-form shape is what let three earlier fixes each miss a form, so none of
these adds a match arm per syntactic form:

- **C1 + C4 (fields)** — `Analyzer::resolve_field_access`, replacing three
  copies of the same lookup (rvalue read, `Stmt::Assign` lvalue,
  `Stmt::CompoundAssign` lvalue) that each ended in `.unwrap_or(Unknown)`.
- **C2** — `readonly_bindings`, checked beside `reject_const_assignment` at the
  same two assignment choke points, replacing the flat body scan. Keyed by
  `BindingId`, **not by name**: `for i { if c { i: t64 = 5; i = 6 } }` is legal
  today and a name-keyed set would reject the write to the shadow.
- **C3** — `resolve_enum_variant`, one resolver for value position, `when`
  literal and range patterns, enum-variant arms, and `as v` arms.
- **C4 (index)** — the rvalue arm of `Expr::Index`. Both assignment lvalue paths
  already rejected a non-array target; only the read form fell through.

`type_has_no_fields` and `type_is_not_indexable` are deliberately conservative:
`Unknown`, `TypeParam`, `Container`, `Handle` and `Result` all answer `false`,
because a check that only speaks where the fact is known must stay silent where
it is not. Verified against generic struct fields (`Box<T>.v` still resolves)
and `copy_into` containers.

### Coverage-by-construction proof

The check was demonstrated firing on nine forms for which **no case was
written**: a loop-counter write inside a `when` arm inside an `if`; inside a
`loop` inside an `if`; the index-write and field-write forms against a counter;
and field accesses in a `when` arm body, a call argument, an array-literal
element, a `return` expression, and a `for` inside an `if`. All nine reject
identically on both backends.

### Residual — C2 has a bypass path, currently unreachable

Following the const precedent, the bypass question was asked rather than
assumed. The interpreter has by-name write paths that never touch
`Stmt::Assign`/`Stmt::CompoundAssign`: `read(var)` and `input(p, var)`
(`src/runtime/call.rs:164,191`) and multi-alias method write-back
(`src/runtime/call.rs:371-379`). All three call `set_var` on a caller-scope name
directly.

**They cannot currently reach a loop counter — but only because of a type
check, not an immutability check.** `analyze_for` hard-declares the counter
`Type::T64`, and every bypass path carries a `str` or struct payload, so the
runtime refuses with `type mismatch — expected 't128' but got 'str'`. That is a
coincidental barrier in a different layer for a different reason — precisely the
shape this audit exists to eliminate.

**Disposition:** the IR validator's `LoopVariableReassignment` check is **kept**
as JIT-side defense-in-depth (different representation, own unit tests). No new
interpreter-side guard was added: it would require the runtime to track which
bindings are loop counters and to exempt the loop's own per-iteration write —
a runtime feature, not the retention of an existing check. The hole is latent
and unreachable by any source program today; it becomes reachable if loop
counters ever gain a non-`t64` type (typed counters, or `for x in array`).
**Anyone making that change must add the runtime guard in the same commit.**

### Delta

Matrix 384 → 396 PASS / 0 FAIL (12 new fixtures). `cargo test` 246/0,
`--features jit` 421/0. Parity 333/51/0 → **345 PASS / 51 SKIP / 0 PARITY_FAIL**
across 396. Clippy 111 → 110 warnings — the flat scan's redundant guard, removed
with it; no new warnings.

**The SKIP count did not move, and that is the expected result.** C1/C3/C4 exit
127 on the JIT, so it would be natural to expect SKIPs converting to PASSes —
but no *shipped* fixture exercised those holes; the 127-exits were all in audit
probes. The observable proof is in the new fixtures: all 12 land as parity
**PASS**, none as SKIP, because both backends now reject at analysis time before
lowering is reached.

---

## 15. Statically decidable, but decided at runtime and differently per backend (audit C5, C6)

**Status: OPEN — low severity, both backends do reject.**

Two facts analysis already holds are left to runtime, and the two backends then
reject through different mechanisms:

```
A: [3: t32] = [1, 2, 3]
print(A:[5])       // interp: exit 1, "array index 5 out of bounds for array of
                   //         length 3"
                   // jit:    exit 126, "cx: runtime trap"
print(10 / 0)      // same split
const Z: t64 = 0
print(10 / Z)      // same split — decidable since `fa95c12` tracks const names
```

An array's length is in its type, and a literal or const-zero divisor is known
at analysis time. Also covers `A:[-1]` and the out-of-bounds *write* form.

**The harness half — RESOLVED in `1f92b39`.** This class used to be invisible to
the parity gate: `TestExpectation::Fail` was `outcome.exit_code != 0`, so a hard
trap satisfied a fixture whose interpreter emits a clean line-numbered
diagnostic. `t_div_by_zero.cx` and `t_oob_negative_index.cx` both PASSed parity
with the JIT exiting 126.

The survey that preceded the fix changed what the fix should be. Across all 137
`.expected_fail` fixtures the interpreter's error *kind* predicts the pair
exactly, with no exceptions:

| interpreter raises | JIT exits | n |
|---|---|---|
| PARSE (9), RESOLVE (4), SEMANTIC (99) | 1 | 112 |
| **RUNTIME** | 126 | 18 |
| **RUNTIME** | 127 (SKIP) | 2 |

The 112 agree because parse/resolve/semantic errors are raised *before* backend
dispatch — there, the two backends are the same code path. Every fixture that
reaches a **runtime** error is (1, 126). That is a documented contract, not
drift: `host_boundary.rs` picked 126 so "expected-fail fixtures see a clean
rejection", and states outright that "exact-message parity with the interpreter
is not required (the parity harness only checks for a non-zero exit on
expected-fail fixtures)". The harness's weakness was written into the backend as
a design assumption.

So a rule "interpreter diagnoses ⇒ JIT must diagnose" would not have flagged 18
divergences — it would have flagged the JIT's entire designed runtime-error
channel. The harness now **records** the pair instead, via a `#!` directive in
`.expected_fail`:

```text
#! interp=diagnostic jit=trap
```

asserted in both directions and on both backends. A `jit=diagnostic` fixture
that starts trapping is flagged; a `jit=trap` one that stops is flagged; the
interpreter's shape is asserted too, where the sidecar previously said nothing
about it at all. The 18 are enumerable rather than invisible —
`grep -l 'jit=trap' src/tests/verification_matrix/*.expected_fail`, or the
`differing_rejection_shapes_are_enumerable` test, which prints the list.

**What remains open, and is explicitly NOT scoped here.** Two things:

1. *The language half of C5/C6* — a constant array index and a literal or
   const-zero divisor are decidable from what analysis already holds. Fix shape:
   constant-fold and reject in analysis; the runtime checks stay for computed
   values, which are genuinely runtime-only.
2. *The underlying asymmetry* — the interpreter has fifteen distinct runtime
   diagnostics, the JIT has one `cx_trap`. Giving the JIT real per-error
   diagnostics would collapse the 18 to (1, 1) naturally and let a strict
   same-shape rule land for free. That is a backend feature and a roadmap
   candidate, not a harness change; recorded here so the annotation is not
   mistaken for the end state.

**`.expected_exit` — also fixed in `1f92b39`, found unasked during the survey.**
The sidecar was honoured by `run_matrix.sh` only; `collect_matrix_tests` had no
branch for it, so the five `exit()` fixtures were classified from their
`.expected_fail` companion and their designed codes (3, 4, 5, 7, 9) went
unasserted on both backends — any non-zero satisfied them. It is now a
first-class `ExitCode` expectation at the top of the sidecar priority order,
matching `run_matrix.sh`. The `.expected_fail` companions are now redundant for
classification and were deliberately **kept**: removing them changes five
sidecars for no behavioural gain, and `run_matrix.sh` reads `.expected_exit`
first as well.

---

## 16. String interpolation is runtime-checked, not compile-checked (audit C7)

**Status: OPEN. The inaccurate README claim has been corrected.**

Three `RuntimeError` variants — `BadInterpolation`, `TemplateInvalidPlaceholder`,
`TemplateInvalidFormat` — all fire at runtime from `expand_template`
(`src/runtime/runtime.rs:294,300`) and `src/runtime/print.rs:49`, on data that is
a string *literal* plus scope. All are statically decidable:

```
print("{nope}")     // interp: runtime error;  jit: exit 127 (SKIP)
print("{f(2)}")     // interp: runtime error;  jit: exit 127
s: str = "v {1+2}"  // interp: runtime error;  jit: exit 127
s: str = "v {x:%}"  // interp: runtime error;  jit: exit 127
```

The README described the non-identifier case as "a compile-checked error rather
than silent literal output". That was inaccurate — it is runtime-checked, and
the JIT skips all four rather than agreeing. Corrected.

Fix shape: move the validation into analysis, where the interpolation segments
and the scope are both available. This is the cleanest of the open items — the
check is a pure function of a literal and the symbol table.

---

## 17. Diagnostics: hardcoded `pos: 0`, and `%` reporting `/`

**Status: OPEN — cosmetic, but they misdirect.**

**Position 0.** Fifteen runtime-error construction sites in `src/runtime/`
(`exec.rs:72,76,102,106,123,184,188,378,382,393,422,464,493`, and others) pass
`pos: 0`, which renders as **line 1** regardless of where the error occurred.
Analysis has the same problem in two places, and there it is structural rather
than an oversight: `Expr::DotAccess`, `Expr::Val` and `AstValue::EnumVariant`
carry **no source position in the AST**, so the C1 field-read error and the C3
value-position variant error both report line 1. Fixing those two requires
adding positions to those AST nodes — worth doing, out of scope for a check.

**Wrong operator in the divide-by-zero message.** `print(10 % 0)` reports
*"division by zero — the right-hand side of `'/'` evaluated to 0"*. The `Mod`
arm in `src/runtime/ops.rs` reuses the `Div` diagnostic verbatim.

---

## 18. Generic struct instantiations now lower — FIXED in `5dcd548`

**Status: FIXED for structs. Generic FUNCTIONS remain open — see §19.**

Both halves of the generics SKIP cluster shared one root cause: `lower_type`
([lower.rs](../src/ir/lower.rs)) has no IR type for `SemanticType::TypeParam`.
They reported differently only because three call sites handle that same `Err`
differently — the FuncDef emission loop propagates it with `?`, while
`build_struct_table` and `build_signature_table` swallow it with `continue`. The
struct half therefore surfaced far from its cause, as
`unresolved semantic artifact reached lowering: struct type 'Pair'`.

Analysis had already computed the substitution — it needs it to type- and
range-check the fields — and then threw it away. The fix retains it:
`SemanticExprKind::StructInstance` carries `type_args: Vec<SemanticType>`, and
mangling to a table key (`Pair$t8`) happens only at the lowering boundary,
following the `PhenDef` / `mangle_method` precedent rather than baking a mangled
name into the semantic layer.

**The part that was not obvious.** Retaining the arguments at the literal is
only half of it. A binding's type *erases* the instantiation — `p: Pair =
Pair<t8> { .. }` and `q: Pair = Pair<t64> { .. }` are both `Struct("Pair")` —
so a field access cannot pick a layout from the type alone, and laying both out
under the bare name would hand them ONE shared layout. `struct_instance_keys`
maps a binding to the layout its storage actually has: populated at the
construct site, where the instantiation is known; consulted at the access site,
where it is not. A binding the map does not cover falls back to the bare name,
which for a generic struct is never in the table — so it SKIPs cleanly instead
of reading someone else's layout.

Two consequences of that same erasure are handled explicitly:

- the semantic field type at an access site is a placeholder (`T` resolves
  against an empty type-parameter scope), so `resolve_field_ptr`'s IR-type
  cross-check is skipped **for instantiations only** — it still applies in full
  to plain structs, which is where it catches real mismatches;
- `print`'s f64 branch keys on the semantic type and so missed `v.x` on a
  `Vec2<f64>`; it now also routes on the lowered type. Anything whose semantic
  type was already F64 took the earlier branch, so no existing behaviour moved.

**Canary.** Layout sharing across instantiations would be silent corruption of
the same family as the struct-return-slot bug, so it is pinned by four fixtures
chosen to be discriminating rather than symmetric: differing widths
(`1000000` does not fit `t8`), differing kinds (`f64` against `t32`), reversed
declaration order (so neither first-wins nor last-wins passes), and differing
field **offsets** (`t8`: 0,1 — `t64`: 0,8, so a shared offset table reads the
second field from the wrong address).

**Delta.** SKIP 51 → 46, exactly the five named fixtures, zero additions.
Corpus 396 → 401 (five new canary/control fixtures), 0 FAIL. `cargo test`
250/0, `--features jit` 425/0, parity **355 PASS / 46 SKIP / 0 PARITY_FAIL**,
clippy 110/110.

**Known coverage limit.** A function *returning* a generic struct
(`fnc: Pair mk(..)`) still SKIPs: the return-slot path keys on the bare name
from the signature, where no instantiation is recorded. It is a clean SKIP, not
a wrong answer, and no corpus fixture exercises it. Field *writes*, generic
structs as parameters, and `impl` blocks on generic structs are all still
rejected by analysis, so they are not reachable regardless.

---

## 19. Generic functions need an instantiation-collection pass that does not exist

**Status: FIXED in `959a980` — see §23, which supersedes this entry. Kept for the
design record: the three prerequisites named below are what the fix implements.**

~~**Status: OPEN — 8 fixtures.**~~ `t37_generics_multi`, `t38_generics_array`,
`t42_generics_identity_chain`, `t52_generics_multi_param`,
`t53_generics_two_same`, `t_gene_bound_dispatch`, `t_gene_bound_forward`,
`t_gene_bound_multi`.

The struct half needed no collection pass because the concrete types are present
*at the literal*. A generic function learns its types only at call sites,
possibly several, and **nothing records them**: `analyze_call` builds a local
`type_param_map`, uses it for bound checking, argument substitution and the
return type, then constructs `SemanticExprKind::Call { callee, function, args }`
without it. The map dies with the call.

The interpreter does not monomorphize at all — it executes one generic body over
dynamic values, and for gene-bound method calls dispatches on the runtime
value's struct name, ignoring the recorded `struct_name` (which is the type
parameter's own name, `"T"`).

Gene/phen slice 5's machinery transfers only in part. Its **emission** half —
given a concrete `SemanticFunction` under a mangled name, emit it through the
ordinary function path — is exactly what a specialization needs, and is already
proven. Its **collection** half does not exist, because `phen Compute (a: Adder)`
names the concrete type at the declaration and never had to search for it.

Three prerequisites, none present:

1. **A transitive instantiation worklist.** Generic-calling-generic is reachable
   today (`fnc: T <T> wrap(x: T) { id(x) }` runs on the interpreter), so
   specializing `wrap@i64` creates a demand for `id@i64`.
2. **Suppression of eager template emission.** [lower.rs](../src/ir/lower.rs)'s
   FuncDef arm pushes every function unconditionally with `?`, so a generic
   template declared and *never called* still fails lowering. Verified.
3. **Return-type substitution before the signature is built.** `ret_struct_of`
   matches only `Some(Struct(_))`; a generic returning `T` gives `TypeParam` and
   falls to `None`, so a specialization returning a struct would silently miss
   its hidden `$ret_slot` parameter — the same silent-corruption shape the
   caller-allocated-slot fix closed.

Scale is not the risk: across all thirteen cluster fixtures the maximum is **two**
distinct instantiations of any one template. Termination is — see §20.

---

## 20. Interpreter stack overflow on type-growing recursive generics

**Status: FIXED in `878ba7c` — see §24, which supersedes this entry and covers
plain unbounded recursion too (this was never generics-specific). The
exit-code collision with the harness's SKIP sentinel is closed.**

~~**Status: OPEN — a crash, not a diagnostic.**~~

Self-recursion at the same `T` is fine (`rec(7, 3)` → `7`, and it would
monomorphize to a single self-calling specialization). But a generic function
that recurses at a *larger* type is expressible, and invoking it kills the
interpreter:

```cx
struct Box<T> { v: T }
fnc: T <T> grow(x: T) { grow(Box { v: x }) }
print(grow(1))
```

```
thread 'cx-interpreter' (28196) has overflowed its stack
[exit 127]
```

No diagnostic; the process dies. Two things make it worse than an ordinary
crash: the type sequence `T`, `Box<T>`, `Box<Box<T>>`, … is infinite and
analysis does not detect it, and the exit code **127 collides with the parity
harness's SKIP sentinel** (`JIT_SKIP_EXIT_CODE`), so a crashing program is
shaped like an unsupported construct.

Declared-and-uncalled, both backends accept it, so this is reachable only by
actually invoking such a function. It also sets the termination requirement for
§19: monomorphization needs a depth or distinct-instantiation cap that produces
a real diagnostic.

---

## 21. `build_signature_table` still swallows `lower_type` errors; `run_matrix.sh` drops `--features jit`

**Status: OPEN — both filed rather than fixed, with reasons.**

**The swallow.** `build_struct_table` and `build_signature_table` both discard a
`lower_type` failure with `continue`, converting a precise type error into a
missing-artifact error reported at a distant site — the anti-pattern
`docs/frontend/enforcement_layers.md` warns about, and the reason the
generic-struct cluster's message was confusing.

For `build_struct_table` this is now benign, and that was measured rather than
assumed: an instrumented build that logged every **non-generic** struct dropped
by the `continue` was run across all 401 fixtures and reported **none**. The
only case it swallows is a generic template, which genuinely has no layout.
`build_signature_table` still swallows for generic *functions*, which is the §19
path; propagating there is a decision about that slice, not this one.

**The harness tax.** `src/tests/run_matrix.sh` invokes `cargo run --quiet` with
no `--features jit`, so running the matrix silently replaces the JIT binary and
any parity number derived afterwards is wrong (this cost a re-derivation during
the generics audit). **Filed, not fixed**: it is two occurrences rather than one
line, and it changes which binary the project's primary corpus gate exercises —
validating that properly means a full matrix re-run and comparison, which is its
own small change rather than a footnote to a lowering slice.

---

## 22. Field access through a gene bound was unsound — FIXED in `e105e56`

**Status: FIXED.** Cross-references §17 (the diagnostic it used to produce) and
§19 (the monomorphizer, which now inherits a sound rule).

A gene declares `fnc` signatures and nothing else (design doc, Locked Rules), so
a bound promises **methods**. Reaching through a type parameter to a *field* was
accepted anyway:

```cx
fnc: t32 <T: Compute> reach(t: T) { t.base }
```

That compiled, ran correctly for whichever concrete type the author had in mind,
and failed at **runtime** for any other type satisfying the same bound. The
failure was §17's field-as-variable diagnostic, on line 1:

```
5
RUNTIME ERROR (line 1): variable 't.base' has not been declared —
declare it with 't.base: TYPE = value' before use
```

So the first call printed its answer and the second one produced a message about
a variable that was never written, pointing at the wrong line, for a field that
the bound never promised. Analysis holds both the bound and the gene's contract;
this is the C1 family and `docs/frontend/enforcement_layers.md` decides it.

**Sequencing.** Fixed *before* the monomorphizer rather than after, so that pass
is not built over an unsound access rule. A monomorphizing backend would
otherwise have had to either reject at specialization time — discovering the
rule late — or emit a specialization that is broken for some instantiations.

### The rule

One arm in `resolve_field_access`, the choke point the C1–C4 slice established,
so field read / write / compound-write are covered by construction rather than
one arm each. Two messages, because the two cases genuinely differ:

| receiver | message |
|---|---|
| `T` with a bound | `cannot access field 'base' on 't' — 'T' is bound by gene 'G', and a gene declares methods, not fields; call a method from the gene's contract instead` |
| `T` unbounded | `cannot access field 'base' on 't' — 'T' is an unbounded type parameter, so nothing is known about its fields` |

`type_is_not_indexable` gains the **bare** type parameter for the same reason —
`t:[0]` previously reached the interpreter and failed there with the C4-family
wrong message (`invalid assignment target`, for a read). An
`Array(_, TypeParam)` parameter is still an array and stays indexable, which is
pinned by a fixture.

### A prerequisite inconsistency, fixed on the way

The rule could not fire until a real discrepancy was corrected. The three
`DotAccess` sites resolved the receiver's type with an **empty** type-parameter
scope (`semantic_type_from_decl(t, &[])`), so a `t: T` parameter surfaced as an
unregistered struct named `"T"` rather than `TypeParam("T")` — while the
`MethodCall` path already used the enclosing function's real scope. The same
receiver therefore typed differently depending on which access form was used.
All three now use the enclosing scope; outside a generic function the list is
empty, so nothing else moves.

This is worth remembering as its own shape: a check that cannot fire because an
upstream resolution quietly produces the wrong type is indistinguishable from a
check that is missing.

### Access-form verdicts

| form | before | after |
|---|---|---|
| field read `t.base` | accepted, ran | rejected at analysis |
| field write `t.base = 9` | accepted, ran | rejected |
| compound `t.base += 1` | accepted, ran | rejected |
| index `t:[0]` | accepted at analysis, wrong runtime message | rejected |
| **method call `t.go()`** | **works** | **unchanged — this is the feature** |
| index on `[3: T]` | works | unchanged |
| lvalue `t:[0] = 1` | parse error | unchanged |
| compound `t:[0] += 1` | already rejected | unchanged |

### Nothing relied on it

Checked before building rather than discovered by breakage: the prelude has
**zero** generic functions, and all eleven fixtures declaring a generic function
use method calls only on the type parameter. Every `t_gene_bound_*` and
`t_gene_phen_*` fixture behaves exactly as before (`15 50`, `17`, `31`, and the
reject fixtures with their own messages).

### Delta

Corpus 401 → 406 (five new fixtures), 0 FAIL. `cargo test` 250/0,
`--features jit` 425/0, parity 355/46/0 → **359 PASS / 47 SKIP / 0
PARITY_FAIL**, clippy 110/110.

**Zero pre-existing fixtures changed SKIP status** — this rejects programs
earlier, it does not lower anything new. The single SKIP addition is
`t_bound_method_still_works`, the new anti-overcorrection fixture: it exercises
generic *functions*, which do not lower yet for the pre-existing §19 reason.

---

## 23. Generic functions monomorphize — FIXED in `959a980` (supersedes §19)

**Status: FIXED, for the plain and the bounded case alike.** §19 listed three
prerequisites and named the bounded case as a separate slice; all of it landed
together, for a reason worth recording below.

One specialized `SemanticFunction` per (template, concrete type arguments),
emitted through the ordinary function-lowering path and called directly by a
mangled name — the shape gene/phen slice 5 already ships for phens. The
difference §19 identified holds: a phen names its concrete type at the
*declaration*, while a generic function learns its types only at call sites.

### What made it tractable

`analyze_call` already derives the substitution — it needs it to check bounds,
substitute argument types and compute the return type — and used to discard it.
`Call` now records it as `type_args`.

The record is **not always concrete**, and that is the useful part. A call inside
a generic body records the enclosing function's parameters *symbolically*:
`id(x)` inside `wrap<T>` records `[TypeParam("T")]`. Specializing `wrap` under
`{T -> I64}` turns that into `[I64]` by ordinary substitution. So propagation is
**map composition**, not re-derivation — no second inference engine outside the
analyser, which is what the alternative would have required.

One narrowing detail: an unsuffixed literal argument is still `Numeric` when it
binds a type parameter, and a specialization's signature needs a concrete type.
It is pinned to the default integer **in the recorded vector only**, never in
`type_param_map` — narrowing the map would change what analysis accepts
(`x: t8 = identity(100)` would stop type-checking).

### The walker

New, over `SemanticStmt` (23 variants) and `SemanticExprKind` (20), with **no
catch-all arm**, so a future variant is a compile error in `monomorphize.rs`
rather than a silently unvisited subtree. `map_stmt_types` was not reusable on
three counts: it walks the AST not the semantic tree, maps AST `Type` not
`SemanticType`, and never descends into expressions at all — which is where
calls live. `collect_binding_names_stmt` was a partial precedent for the
statement half only (7 of 23 variants, 0 expression variants).

Four name-carrying fields are rewritten alongside the types: `Call.callee`,
`MethodCall.struct_name`, `StructInstance.type_name`/`type_args`, and
`DotAccess.struct_name`.

### Why the bounded case came along

§19 scoped `t_gene_bound_*` as a separate slice. It converted with this one,
because the walker must rewrite `MethodCall.struct_name` to be exhaustive — and
that substitution *was* the whole of the bounded case's job, exactly as the
design predicted: it fixes both the callee (`mangle_method` then produces
`Adder$apply`, already in the signature table) and the receiver argument's
synthesized type, since the desugaring at `lower.rs` uses `struct_name` twice.

`t_gene_bound_dispatch` prints **15 | 50** on both backends — the discriminating
result its own comment names, where a monomorphization leak would print 15/15 or
50/50. So this is real per-instantiation dispatch, not a collapse.

### Termination, and what the cap actually catches

A per-template cap of 64 distinct instantiations, with a diagnostic naming the
template and the first few instantiation types so the growth is visible. A
per-template count is the natural guard because the dedup map already tracks
exactly that; a depth cap has no meaning in a worklist, which has no stack.

**§20's `grow` program does not explode the worklist.** Worth recording, because
it is the opposite of what the design assumed:

```cx
struct Box<T> { v: T }
fnc: T <T> grow(x: T) { grow(Box { v: x }) }
```

`Box { v: x }` types as `Struct("Box")` — struct type arguments are erased from
`SemanticType` — so `Box<Box<T>>` and `Box<T>` are the *same* key and the
worklist converges after two instantiations. It terminates via the struct-return
slot lookup instead, a clean SKIP in 0s.

The cap does fire on array-growing recursion, where the types are genuinely
distinct because `Array` carries its element type:

```
fnc: T <T> g(x: T) { g([x]) }

unsupported semantic construct during lowering: generic function 'g' exceeded 64
distinct instantiations — this usually means a recursive call instantiates it at
an ever-larger type. First instantiations were: g$t64, g$[1: t64],
g$[1: [1: t64]], g$[1: [1: [1: t64]]]
```

Exit 127, one second, no hang and no panic.

### The cap has no matrix fixture yet, deliberately

The obvious fixture is the array-growing program above. It cannot be added,
because the **interpreter** still crashes on it (§20) — exiting `0xC00000FD`,
a stack overflow, not a diagnostic. The rejection-shape harness (§15) caught
this: an `.expected_fail` marker defaults to `interp=diagnostic`, and the
harness correctly refused to accept a crash as a rejection.

The available workaround would be to add a third `RejectionShape` meaning
"crashes", which is recording a bug as an expectation — precisely what
`docs/frontend/enforcement_layers.md` says not to do ("do not widen the
assertion until every current fixture passes"). **The cap fixture lands with the
§20 fix**, and until then the cap's proof lives in this entry and in the commit.

That the harness caught a fixture about to assert a crash was a diagnosis is the
clearest evidence so far that the §15 work was worth doing.

### Delta

SKIP 47 → 39, **converting exactly eight**: `t37_generics_multi`,
`t38_generics_array`, `t42_generics_identity_chain`, `t52_generics_multi_param`,
`t53_generics_two_same`, `t_gene_bound_dispatch`, `t_gene_bound_forward`,
`t_gene_bound_multi`. Zero additions.

Corpus 406 → 410 (four new fixtures), 0 FAIL. `cargo test` 250/0,
`--features jit` 425/0, parity 359/47/0 → **371 PASS / 39 SKIP / 0
PARITY_FAIL**, clippy 110/110.

### The user-visible limit

The instantiation cap is a language-visible limit, not an implementation detail
— a program can hit it. It is documented in the README's JIT-limitations section
alongside the other lowering boundaries, since that is where a user looks when
the JIT refuses a program the interpreter runs.

---

## 24. Interpreter call-depth guard — §20 FIXED in `878ba7c`

**Status: FIXED for the interpreter. The JIT still crashes on the same shape —
see the end of this entry; that guard is its own work.**

A Cx call is a native recursion inside the interpreter
(`run_semantic_stmt` → `eval_semantic_expr` → `call_semantic_*`), so unbounded
Cx recursion was unbounded native recursion and the thread's stack went with it:

```
thread 'cx-interpreter' (22916) has overflowed its stack
[exit 127]
```

No message, no line, no function name. **And exit 127 is the parity harness's
SKIP sentinel**, so an interpreter death was shaped exactly like an unsupported
construct — that collision was half the danger, and it is now closed: the guard
reports on the ordinary diagnostic path at **exit 1**.

This was never generics-specific. Both shapes crashed identically, and both now
diagnose:

```
fnc: t64 boom(n: t64) { boom(n + 1) }                     // plain recursion
fnc: T <T> grow(x: T) { grow(Box { v: x }) }              // type-growing

RUNTIME ERROR (line 1): call depth limit reached in 'boom' — 257 nested calls,
limit is 256. This is almost always unbounded recursion: check that the
recursive call has a base case it can actually reach
```

### The limit, and why 256

Bounded by measurement on both sides rather than picked:

| | value | how established |
|---|---|---|
| deepest legitimate recursion in the corpus | **15** | `fib(15)`, `t113_recursive_fib` |
| interpreter crash point | **~492** | bisected: `down(490)` survives, `down(495)` overflows — debug build, 64 MB stack |
| chosen limit | **256** | ~17× above real code, ~2× below the crash |

The 2× headroom below the crash matters: a heavier native frame in some other
build cannot overshoot the guard into a real overflow. The same shape as the
monomorphizer's 64-instantiation cap against a corpus maximum of 2.

Boundary behaviour is exact: `down(255)` runs (256 frames, at the limit),
`down(256)` is refused (257 frames). The limit counts frames, so `down(n)` uses
`n + 1`.

### Cost

One increment, one compare, one decrement per Cx call — placed on the call path
only, nothing on the statement or expression path where the interpreter actually
spends its time. Measured on `fib(24)` (~150k calls), three runs each: minimum
**1177 ms with** the guard against **2202 ms without**. The guard is not a
regression; run-to-run variance (roughly 2×) dwarfs anything it could cost, and
structurally three integer ops sit on a path that already does a `HashMap`
lookup, a `Vec` of resolved parameters, a scope push, and a `type_of_value` per
argument.

Both call entry points carry it — `call_semantic_func` and
`call_semantic_method` — placed around each path's own `push_function_scope`,
*after* the early rejections, so a failed lookup cannot leak the counter.

### What it unblocked

`t_mono_instantiation_cap` — the fixture §23 could not write. The interpreter
used to die on it at exit 127, and the rejection-shape harness (§15) correctly
refused to accept a crash as a diagnosis. It now lands: the interpreter
diagnoses (call depth) and the JIT SKIPs (instantiation cap). That is the
concrete proof this fix unblocked something real.

### The JIT needs its own guard — reported, not fixed here

Unbounded recursion in JIT-compiled code still dies, because compiled code
genuinely recurses until the native stack is gone:

```
fnc: t64 boom(n: t64) { boom(n + 1) }
--backend=cranelift → thread has overflowed its stack, exit 0xC00000FD
```

The raw Windows code is `-1073741571`, **not** 127 — so it does not even
masquerade as a SKIP; the harness sees a crash and parity-fails. That is why
`t_interp_recursion_guard`, the natural fixture for the interpreter fix, could
not be added: the JIT crashes on the same program. The interpreter-side proof
lives in this entry until the JIT guard lands, at which point that fixture
becomes writable — the same pattern this fix just resolved one level down.

A JIT-side guard is a different mechanism (a stack-depth check in compiled code
or a recursion counter threaded through the runtime intrinsics), not an
extension of this one.

### Delta

Corpus 410 → 412 (`t_mono_instantiation_cap`, `t_interp_deep_recursion_ok`),
0 FAIL. `cargo test` 250/0, `--features jit` 425/0, parity 371/39/0 →
**372 PASS / 40 SKIP / 0 PARITY_FAIL**, clippy 110/110.

SKIP 39 → 40: one addition, `t_mono_instantiation_cap`, which SKIPs on the JIT
by design (the monomorphizer's cap refuses to lower it). Zero removals, zero
pre-existing fixtures moved.

The limit is documented in the README under "Limits that apply to every run",
separately from the JIT-lowering list — it is an interpreter limit, so it binds
every ordinary run, not only JIT-compiled ones.

---

## 25. JIT call-depth guard — the third crash-instead-of-rejection instance, FIXED in `664850d`

**Status: FIXED. The pattern is closed** — this was the last backend that
crashed where the other rejected.

Compiled code recurses natively, so unbounded Cx recursion took the process's
stack: `0xC00000FD` on Windows, no diagnostic. **Worse than the interpreter's
old behaviour**, which at least exited 127 and so masqueraded as a SKIP — the
raw crash code is `-1073741571`, which is nothing the harness recognises, so it
parity-failed outright.

Three instances of the same shape are now resolved, and they were found in
sequence, each unblocking the next:

1. §15 — the harness could not tell a diagnosis from a crash at all.
2. §24 — the interpreter crashed on unbounded recursion.
3. §25 — the JIT crashed on the same program.

The evidence that it is closed is `t_interp_recursion_guard`, a fixture that
**could not be written** while either backend crashed: the rejection-shape
harness correctly refused to call a crash a rejection. It now lands, annotated
`interp=diagnostic jit=diagnostic`, as a parity PASS.

### Mechanism, and why not the cheaper one

A **frame counter**, incremented by a host callback at each Cx function's entry
and decremented before each return — the `cx_trap` / `cx_handle_new` precedent,
with a process-lifetime `AtomicUsize` for the same reason `HANDLES` uses a
static (`jit_builder.symbol()` registers raw function pointers, so a closure
over state is not an option).

**A stack probe is disqualified, not merely worse.** Comparing SP against a
limit is far cheaper — two or three instructions, no call — but it terminates on
*bytes consumed*, which varies per function. It could never agree with the
interpreter's 256-**frame** limit, and approximate agreement between backends is
a divergence. Matching semantics is what costs a counter.

Exit **1**, not the 126 trap path, deliberately: both backends then reject in
the same *shape*, so the fixture states agreement rather than papering over a
difference with `jit=trap`.

### Boundary agreement — exact, and it needed a fix

The first version counted `main` and refused at 255 where the interpreter
refused at 256. A one-frame disagreement between backends is a divergence, not a
rounding difference. `main` is now excluded, matching the interpreter's guard,
which lives in `call_semantic_func`/`call_semantic_method` and therefore counts
nested *user* calls — top-level code is not inside one.

| depth | 254 | 255 | 256 | 257 |
|---|---|---|---|---|
| interpreter exit | 0 | 0 | 1 | 1 |
| JIT exit | 0 | 0 | 1 | 1 |

The limit is shared, not duplicated: the backend reads
`crate::runtime::runtime::MAX_CALL_DEPTH`.

### Refined to cycle-only in `ab62a6d`

The guard shipped uniformly, taxing every compiled call two host callbacks
(~11 ns, +31% on call-saturated code in a debug build). It is now emitted **only
for functions that can participate in a call cycle** — a function that cannot
reach itself cannot recurse, so it needs no counter.

Soundness rests on the call graph being complete, and it is: Cx has **no
indirect call of any kind**. `IrInst::Call.callee` is a `String` and never a
`ValueId`, the backend emits no `call_indirect`, and neither `Type` nor
`SemanticType` has a function or callable variant — a function cannot be stored
in a variable, a struct field, an array element, or a `Handle` payload. Every
edge is a compile-time-known name: direct calls, methods and operator genes
(desugared to `mangle_method`), phens (monomorphized at their declaration), and
generic functions (monomorphized to `name$type`). Builtins are `cx_*` leaves and
never call back into Cx, so an edge to one cannot close a cycle.

The graph is read off the IR **after** lowering rather than off the semantic
tree, so it sees the calls actually emitted instead of re-deriving the mangling
`lower.rs` performs — the shape that produced the C1-C4 family of bugs. Tarjan's
SCC, written iteratively rather than recursively: this compiler must not
overflow its own stack while adding a guard against stack overflow. A function
is guarded iff it is in an SCC of size > 1 or carries a self-loop.

**Mutual recursion is the case a self-loop-only implementation misses**, and a
missed cycle is an unguarded crash — strictly worse than the tax it replaces.
`t_mutual_recursion_guard` covers it, and a unit test additionally pins
length-3 cycles, a non-recursive *caller* of a recursive function staying
unguarded, and host intrinsics not counting as edges.

### Measured cost — recorded so it is findable, not folklore

Minima of 3-5 runs. `fib(30)` is ~2.7M recursive calls; the non-recursive
benchmark is 6M calls through two non-recursive functions in a loop.

| | debug: uniform | debug: cycle-only | release: none | release: uniform | release: cycle-only |
|---|---|---|---|---|---|
| `fib(30)` (recursive) | 141 ms | 139 ms | 98 ms | 104 ms | 112 ms |
| non-recursive, 6M calls | 200 ms | **110 ms** | 103 ms | 128 ms | **106 ms** |

Non-recursive code now matches unguarded — the whole point. Recursive code is
unchanged, which is correct: it is the only code that can recurse.

**Release compresses the per-call cost about fourfold** — roughly 3.7 ns against
14.5 ns in debug — because the callback body optimises well. The debug figure
that motivated this work overstates the shipped cost by a factor of four.

### The inline counter stays deferred, and the release numbers are why

An inline counter (Cranelift `declare_data` / `declare_data_in_func` /
`create_global_value`, all present in 0.115.1) would replace the two host calls
with a load/add/store and a compare, cutting the per-call cost to roughly a
tenth. Two measurements now argue against spending that codegen work:

1. Cycle-only already removes the cost entirely from non-recursive code, which
   is most code.
2. In release the residual on recursive code is ~3.7 ns/call — on `fib(30)`,
   112 ms against 98 ms unguarded, and the run-to-run spread on that benchmark
   (98-133 ms) is wider than the gap being optimised.

Revisit if a real recursive workload shows the guard in a profile. The numbers
above are the evidence base for that decision rather than an intuition about it.

### Delta

Guard (`664850d`): corpus 412 -> 413 (`t_interp_recursion_guard`), parity
372/40/0 -> 373/40/0. Cycle-only (`ab62a6d`): corpus 413 -> 414
(`t_mutual_recursion_guard`), parity 373/40/0 -> **374 PASS / 40 SKIP / 0
PARITY_FAIL across 414**, `--features jit` 425 -> 426. Both: 0 FAIL,
`cargo test` 250/0, clippy 110/110, SKIP set unchanged throughout — the new
fixtures land as PASSes, not SKIPs, which is the whole point.

The call-depth limit is documented in the README under "Limits that apply to
every run", which is now literally accurate: it binds both backends identically.

---

## 26. Array returns corrupted silently on two of three paths — FIXED in `65736b5`

**Status: FIXED. This was not a lowering gap — it was a live silent-corruption
bug on shipped code paths**, and it is recorded that way because the v0.3.3
release notes and the README both described it as merely "not yet lowered".

A function returning an array alloca'd the array in its own frame and returned a
pointer to it. The frame died on return and the caller read through a dangling
pointer. Both backends exited **0**; the JIT simply produced wrong values.

```
fnc: [3: t64] make() { [11, 22, 33] }     interp  11 22 33
                                          jit     11 77306400 140695085549632

fnc: [3: t8]  small() { [1, 2, 3] }       interp  1 2 3
                                          jit     1 -120 -104
```

Two calls shared the dead frame, so the first call's result came back holding
the second's:

```
p = mk(10); q = mk(100)                   interp  10 11 12 · 100 101 102
                                          jit     100 <garbage> · 100 <garbage>
```

### Why it survived: the guard covered one path of three

An array-return guard existed, but both of its sites sat inside
`SemanticStmt::PhenDef` arms. So:

| path | guarded | behaviour before the fix |
|---|---|---|
| phen method | yes | clean SKIP |
| **impl method** | **no** | **exit 0, corrupt** |
| **free function** | **no** | **exit 0, corrupt** |

Guarding one path of three was worse than guarding none: it made the gap look
handled. The audit that found this was scoped to "lift the phen guard"; probing
the other two paths rather than assuming them is what widened it.

### Why no test caught it: length-1 blindness

The corpus's only array-returning fixture, `t38_generics_array`, returns a
**length-1** array — and length 1 is exactly the case that cannot detect a
dangling-frame read. Element 0 sat at the top of the dead frame and survived;
everything after it was clobbered:

```
[1: t64]   interp 42      jit 42          identical
[2: t64]   interp 42 99   jit 42 -2       diverges
```

So `t38` passed parity for the entire life of the bug while every longer array
was corrupt. A fixture can be green and blind at the same time; length was the
variable nobody varied. `t_array_return_generic_len3` is its companion at a
detecting length — `t38` itself is left alone.

### The fix

The caller-allocated slot convention — introduced for struct returns — now
covers arrays. Three changes, all extending proven machinery:

- `ret_struct_of` (which answered "is this a named struct?") becomes
  `returns_via_slot` (which answers "does this need a slot?"). An array has no
  `struct_table` entry to be named by, so the gate could not key on a name.
- `ret_slot_plan` computes size, alignment and the (offset, type) parts once,
  and **both ends read it** — the caller's `Alloca` and the callee's copy — so
  the two cannot disagree about size or stride. Struct parts are fields at their
  layout offsets; array parts are elements at `i * stride`.
- `compute_array_layout` already supplied stride, total size and alignment.
  Nothing about the convention needed inventing for arrays; it was a
  layout-lookup gap, exactly as the recon predicted.

The phen guard is lifted only now that all three paths are sound.

### Verification

Six fixtures were written **before** the fix and run failing first: five
PARITY_FAILs and one SKIP, with the corrupted values captured. All six now pass,
both backends byte-identical.

A length sweep at 1, 2 and 3 elements across `t8` and `t64` confirms the
length-1 blindness is genuinely gone rather than the fix accidentally covering
only short arrays — every element correct at every length and width.

### Delta

Corpus 414 → 420, 0 FAIL. `cargo test` 250/0, `--features jit` 426/0, parity
374/40/0 → **380 PASS / 40 SKIP / 0 PARITY_FAIL across 420**, clippy 110/110.
SKIP set unchanged — the six new fixtures land as PASSes, and no pre-existing
fixture moved.

---

## 27. Aggregate value semantics — Model B, FIXED on both halves

*(Two commits. The binding half was fixed by `copy_if_memory_resident` in
`src/ir/lower.rs`; the parameter half by the `is_receiver` flag on
`SemanticParam` plus the entry copy in `lower_semantic_function`. Both cited by
name rather than by hash, because each fix and its write-up are one commit.)*

**Status: FIXED.** Aggregates are values on both backends — every bind, every
parameter, every return. The method receiver is the one declared exception.

A live silent divergence on shipped flat-array paths. `r: [3: t64] = a` bound
`r` to the same storage as `a` on the JIT, while the interpreter copied the
value. Both backends exited 0 with different answers:

```
a: [3: t64] = [11, 22, 33]
r: [3: t64] = a
r:[1] = 99
print(a:[1])          interp: 22      jit: 99
```

**Copy is the language's semantics.** Cx has explicit `.copy` / `.copy.free` /
`copy_into` vocabulary, which presupposes that plain assignment is not aliasing;
silent aliasing in a GC-free language is exactly what that vocabulary exists to
make visible. The JIT was the bug.

### Why nothing caught it

**No fixture assigned one array variable to another and then mutated.** Searched
before fixing: zero. Every array fixture in the corpus either initialises from a
literal or reads elements, and neither shape can observe aliasing. The same
blindness as the length-1 array-return fixture — the corpus tested the feature
without testing the property.

### Mechanism

An array or struct lowers to `IrType::Ptr`, so a bare `SsaBind` copies the
POINTER. `copy_if_memory_resident` allocates fresh storage and copies through it
before the bind, at the single choke point every binding form funnels through.

The copy plan is `ret_slot_plan` — already used by the caller-allocated return
slot. One plan, several consumers, so the caller's `Alloca`, the callee's return
copy and this bind copy cannot disagree about size or stride.

A source that already owns fresh storage — an array literal or a struct literal
— is exempt: nothing else holds a pointer to it, so there is nothing to alias,
and `a: [3: t64] = [1, 2, 3]` is the commonest array assignment there is. That
tests a PROPERTY of the source, not a list of statement forms; every binding
form still routes through the one function.

**Structs were fixed by the same change, and that was not planned.** They alias
for the identical reason and `ret_slot_plan` already covered them, so writing
the array-only version would have meant deliberately narrowing natural code.
Verified: `b = a; b.x = 99` gave `11 99` / `99 99` before and `11 99` on both
after — and no struct case was written for.

### Cost

No measurable cost. 200k array binds of 8 `t64` elements in a loop: minimum
465 ms with the copy against 428 ms without, inside the run-to-run noise band.
Array literals and struct literals — the common forms — are exempt and emit
nothing extra, which a unit test asserting exactly three `Store`s for a
three-element literal caught and pinned.

### The second half: parameter passing — Model B

The bind fix left parameters divergent, and the divergence was deliberate at the
time because it collided with method receivers:

```
fnc: t64 f(r: [3: t64]) { r:[0] += 88   return r:[0] }
a: [3: t64] = [11, 22, 33]
print(f(a))  print(a:[0])      interp: 99 11      jit: 99 99
```

An impl or phen method's receiver is an ordinary parameter, so copying
parameters copies the receiver, and JIT method mutation worked *only* because
the receiver aliased the caller's storage. The two backends were reaching the
same observable result by opposite mechanisms: the interpreter binds every
parameter by value and then writes receiver mutations back explicitly
(`call_semantic_method`), while the JIT never writes anything back — the
receiver simply *is* an argument.

**The ruling: Model B.** Aggregates are values. Every bind, parameter and return
copies. **The method receiver is the one declared exception**, passed by
reference as a stated language rule. The interpreter was already Model B; this
makes the JIT agree, which is what "the interpreter is the reference" has to
mean if it means anything.

### The exemption is a property of the parameter, not of its position

`SemanticParam` carries `is_receiver`. It is set in the two places receivers are
built — the impl-block alias capture and the phen-def receiver capture — and
lowering copies every aggregate parameter that does not carry it.

**"Skip argument 0" would have been wrong, not merely fragile.** An impl block
may declare several aliases: `impl (p: Player, w: World)` makes both `p` and `w`
receivers, and `t177_multi_alias_impl_exit` has shipped depending on it since
0.2. A positional rule gets that fixture wrong on its second alias.

The field has **no default**, so it is a compile error for any new
receiver-producing path to stay silent about which it is. That is the same
discipline as the monomorphize walker's missing catch-all arm, and the direct
answer to the array-return guard that covered `PhenDef` only and looked handled.

### What proves the flag covers paths nobody wrote a case for

Four parameter-passing forms with no specific handling, each DISAGREE before and
AGREE after:

| Form | before (interp / jit) | after |
|---|---|---|
| phen-dispatched operator, `rhs` param mutated | `10` / `999` | `10` |
| monomorphized generic fn, struct param | `11` / `777` | `11` |
| impl method's non-receiver param | `22` / `555` | `22` |
| aggregate forwarded through two frames | `11 22` / `111 29` | `11 22` |

The impl-method row is the sharpest: in one call the receiver is mutated
(by reference) and the non-receiver parameter is not (by value). The exemption
is behaving per-parameter, which is the whole design.

### Cost of the entry copy

Measured on the parameter path specifically, because the bind benchmark's
"inside the noise band" says nothing about a hot call. Release build, cranelift,
a function whose entire body is one compound assignment, 256-byte aggregate
parameter (`[32: t64]`), minimum of seven runs, per-call cost isolated by
differencing 200k against 2M calls:

```
base    2 ns/call        model B   7 ns/call
```

~5 ns per call for 256 bytes — about 36 GB/s, which is what an L1-resident copy
should cost. Over 1.8M calls that is 9 ms. It is measurable, unlike the bind
copy, and it scales with aggregate size; it is not material against call
overhead on any realistic path.

### Fixtures

Seven, written before the change and failing on the JIT when written — six of
seven, verbatim, `interp` / `jit`:

```
t_param_array_is_copy       111 11 22 33     / 111 111 222 333
t_param_struct_is_copy      99 11 22 33      / 99 99 27 33
t_param_multi_aggregate     58 1 2 3 7 8 9   / 58 51 2 3 7 8 9
t_param_array_copy_t8       44 10 20 30 40   / 44 11 22 33 44
t_param_mutate_in_branch    111 11 22 33     / 111 111 22 33
t_param_mutate_in_loop      51 11 22 33      / 51 51 22 37
```

The seventh, `t_param_nested_aggregate`, passed on both before and after. It is
there to pin what exists rather than to fail: the aggregate copy is **shallow**
over row pointers — `ret_slot_plan` for `[2: [3: t64]]` yields two `Ptr` parts,
and the IR shows `alloca size 16`, not 48 — so `b = a` shares both rows. No
program can observe that today, because every route to an inner element goes
through a bind and every bind copies. **Multidimensional arrays are where this
stops being true**: making inner elements directly writable makes the sharing
visible. If nested arrays become contiguous, `ret_slot_plan` yields six `i64`
parts and the copy becomes deep through the existing authority; if they stay
independently-owned rows, a deep-copy walker is needed — a second authority for
aggregate size and stride, which is what the one-plan rule exists to prevent.
Contiguity and copy depth are one decision. See the 0.5 design gate.

A unit test, `aggregate_param_copy_is_governed_by_the_receiver_flag`, pins the
mechanism directly: same type, same position, only the flag differs, and only
the flagged one skips the copy. A positional rule fails it.

### Note: only compound assignment can mutate an array parameter

`r:[0] = 99` inside a function body is a parse error, a separate pre-existing
limitation. It is why the fixtures below use `+=`.

### `.copy.free` now has no distinct meaning — reported, not acted on

Read off `call.rs` and confirmed by running each form: all four parameter kinds
bind by value in the interpreter, and the only difference between them is
whether the value is written back on return.

| form | binding | write-back | probe (callee sets 99, caller had 11) |
|---|---|---|---|
| plain `n: t64` | by value | no | `99 11` |
| `.copy` | by value | **yes** | `99 99` |
| `.copy.free` | by value | no | `99 11` |
| `copy_into(x, y)` | bundles named outers into a container | no | — |

`.copy` is copy-in/copy-out — the inverse of what the name suggests — and
remains the language's only explicit mutation-out channel.

**`.copy.free` is a deprecation candidate.** It is semantically identical to
ordinary passing: same by-value binding, no write-back, and — now that plain
passing copies on the JIT too — no distinct meaning in the value model. Nothing
a program can write distinguishes `f(x.copy.free)` from `f(x)`.

**It is not removed, and nothing here changes its behaviour.** The ruling comes
when blocker #3 closes — `.copy` / `.copy.free` / `copy_into` do not lower on
the JIT at all today (exit 127), so the deprecation question cannot be settled
against a half-implemented feature. Recorded now so the finding is not
rediscovered, and so the decision is made deliberately rather than by whichever
implementation lands first.

### Delta

**Bind half:** corpus 424 → 429, parity 384/40/0 → 389/40/0.

**Parameter half (Model B):** corpus 429 → 436, 0 FAIL. `cargo test` 250 → 251,
`--features jit` 426 → 427, parity **389/40/0 → 396 PASS / 40 SKIP / 0
PARITY_FAIL across 436**, matrix 436/436. SKIP set unchanged — no fixture
started or stopped skipping.

Every pre-existing fixture is byte-identical on both backends: 857 of 858
captured outputs match exactly, and the single difference is the IR-dump text
inside `t_bound_method_still_works`, a fixture that already exits 127 and still
exits 127 — the dump now shows the entry copy.

Clippy is unchanged by this work: 111/111 before and after (`cargo clippy`
without `--all-targets`; 119/113 with it, also unchanged).

The 110/110 recorded at the 0.3.3 tag was chased afterwards and there is no
drift: building the tag in a worktree reports **111**, with a lint multiset
byte-identical to this HEAD's — same lints, same counts. No code change
introduced a lint; the recorded number was simply not reproducible under
clippy 0.1.96. `docment/ROADMAP.md` now records 111/111 for the tag.

Three unit tests asserted instruction shapes over a whole function while meaning
to assert what a single expression lowers to; the entry copy exposed that. They
now read `body_insts`, which skips the copy preamble by its shape. Their
positive assertions still hold, so the helper is not swallowing the body.

---

## 28. Multidimensional arrays — Model A, contiguous — LANDED

*(Implemented by the commit that introduces this entry: the recursive
`ret_slot_plan` and `elem_stride` in `src/ir/lower.rs`, the folded index rules in
`src/frontend/parser.rs`, and the index path in `src/runtime/scope.rs`. Cited by
name rather than by hash because the work and this entry are one commit.)*

**Status: LANDED.** A nested array is one owned, contiguous, row-major value.
`[R: [C: T]]` types, `a:[i]:[j]` indexing and `[[1,2],[3,4]]` literals all work
on both backends, and the three aggregate-aliasing divergences are closed.

### What changed

`ret_slot_plan` is recursive. `Array(3, Array(4, I32))` is 48 bytes with twelve
`(offset, I32)` parts rather than three pointers. That one change is what makes a
nested copy deep, and it is why nothing needed a separate deep-copy walker: the
bind copy, the parameter entry copy, the return slot and aggregate stores all
already read that plan.

A slot is storage, not a reference. An aggregate array element, an aggregate
struct field and an aggregate parameter are laid out inline, so reading one
yields its ADDRESS and writing one COPIES INTO it.

### The three divergences, closed

Each copied on the interpreter and aliased on the JIT, at v0.3.3 and since:

```
a:[0] = n   then n:[0] = 5     was interp 77 / jit 5      now 77 both
h.arr = n   then n:[0] = 5     was interp 77 / jit 5      now 77 both
arr:[0] = x then x.v = 99      was interp 11 / jit 99     now 11 both
```

They are closed structurally, not patched: under contiguous storage a slot has
no pointer to overwrite, so the alias cannot be formed.

### One authority, pinned by a test that fails

`elem_stride` is the stride. `ret_slot_plan` lays parts out with it and
`resolve_array_element_ptr` does index arithmetic with it.
`index_stride_and_copy_plan_agree` asserts that element `i`'s parts land inside
element `i`'s stride window, over scalars, nested arrays and structs.

Deliberately broken — indexing made to derive its own stride, the pre-Model-A
behaviour where every aggregate is a Ptr at 8 bytes — it fails and says why:

```
Array(3, Array(4, I32)): part at offset 8 belongs to element 0, whose stride
window is [0, 8) — indexing and the copy plan disagree about where element 0
begins
```

A comment cannot fail. That is the difference between recording the constraint
and enforcing it.

### Contiguity and non-materialisation, from the IR

`a: [2: [3: t64]] = [[11,22,33],[44,55,66]]` emits **one** allocation and six
stores — no row allocations:

```
v0 = alloca size 48 align 8
store v0 v1              ptr_offset v0 + 8       ptr_offset v0 + 16
ptr_offset v0 + 24       ptr_offset v0 + 32      ptr_offset v0 + 40
```

Row-major, last index varying fastest. `a:[1]:[2]` then emits **zero** further
allocations:

```
v18 = const i64 24    v20 = ptr_add v12 + v19     <- row address, arithmetic only
v26 = const i64 8     v28 = ptr_add v20 + v27     <- element address
v29 = load i64 v28
```

Both dimensions bounds-checked. The intermediate row never materialises, which
was the question the storage model turned on: had the first index produced a
value, binds copy, and contiguous storage would have broken silently.

### What fell out without a case being written

- A 2-D parameter's entry copy is `alloca size 48` plus six scalar stores — deep,
  through the same recursive plan.
- `struct Grid { cells: [2: [3: t64]], tag: t64 }` lays out at **56** bytes:
  48 inline plus 8. The array is in the struct, not pointed to by it.
- 2-D returns travel the caller-allocated slot unchanged.
- Arrays of structs holding arrays work.

### Syntax

The index rule went from `.or_not()` to `.repeated()` with a left fold, and
`index_assign` took the same fold. `a:[i]:[j]` and `(a:[i]):[j]` produce the
identical tree — the parenthesised form is ordinary grouping, not an alias to
maintain. Types and literals needed nothing: `[N: T]` was already recursive and
literal shape validation already recursed.

### One assignment-target grammar — the write-position gaps, closed

Two of the three gaps recorded here were never separate implementations. There
was one `index_assign` production, and the function body's statement list simply
**did not include it** — it listed `index_compound_assign` and `assign` but not
`index_assign`. That single omission is why `r:[0] = 9` and `a:[i]:[j] = 9` were
both parse errors inside a body while working at top level. The write-through-a-
field gap had the same character: `index_assign` rooted at a bare identifier,
while the READER already accepted `g.cells:[0]:[1]` through the ordinary
expression grammar.

Both are now one production:

```
target := ident ( '.' ident )? ( ':' '[' expr ']' )*
```

`x = v`, `x.f = v`, `x:[i] = v`, `x.f:[i] = v` and `x:[i]:[j] = v` are the same
rule, folding left into exactly the tree the reader builds for the same text.
`assign` and `index_assign` collapsed into it, and the function body folds that
one rule rather than a subset of it. Deleting the duplicate took clippy from 111
to **110** — one fewer `clone` on a Copy combinator.

Nothing downstream needed widening: the semantic layer's `Expr::Index`
assignment arm already called `analyze_expr` on the target, and the JIT's
`resolve_array_element_ptr` already lowered any Ptr-producing expression. Only
the interpreter needed work, and only because it walks by name rather than by
address — its root descent now starts at a variable *or* one of its fields,
sharing the index walk instead of duplicating it.

### Compound assignment — closed

`AssignTarget` is **deleted**. `Stmt::CompoundAssign` carries `target: Expr`, the
same place representation `Assign` uses, and both statements parse through the
same target grammar — they differ only in `op=` versus `=`. `a:[i]:[j] += 1`,
`g.cells:[0]:[1] += 5` and rank-3 compounds all work on both backends.

`AssignTarget` was a parser-to-semantic hop introduced 2026-03-15 in "Structs
Phase 1+2", when the writable set was a variable, a named container's field, and
a named array's element. It never reached either backend: the semantic layer
already converted it to `SemanticLValue`, which is why the tell was that its
index arm had to RECONSTRUCT `Expr::Ident(arr_name)` to analyse it — a name was
all it had kept.

One `Expr` → `SemanticLValue` conversion, `resolve_place`, now serves both
statements' index targets, so a target shape that resolves for one resolves for
the other at any depth.

**The JIT needed zero changes** — it already matched `SemanticLValue::Index` with
a generic target, and all five fixtures passed on Cranelift the moment the parser
produced the wider tree.

**The interpreter did need a change, and the prior investigation was wrong to say
it would not.** It does speak `SemanticLValue`, but its compound arm still
pattern-matched the target for a `VarRef` root and took a single index — the same
narrowing the `Assign` arm carried before the previous slice widened it. The fix
is the walk that already existed: `resolve_index_place` now resolves the root and
index path once, and `Assign` and `CompoundAssign` share it.

### Single evaluation

A compound assignment resolves its place once, so `a:[f()]:[g()] += 1` calls
`f()` and `g()` exactly once each. `t_place_compound_eval_once` prints a marker
from each and asserts one of each; the hand-expanded
`a:[f()]:[g()] = a:[f()]:[g()] + 5` prints two of each, on both backends, which
is what makes the canary discriminating rather than merely green.

Structural, not conventional: the JIT holds one SSA pointer that both the Load
and the Store name, and the interpreter holds one `(root, field, path)` that both
the read and the write use. See `docs/backend/cx_eval_order.md` for the rule,
including the consequence that a compound assignment does not see writes made by
its own operand.

### Delta

**Model A:** corpus 436 → 444, matrix 444/444, parity 396/40/0 → 404/40/0,
clippy 111/111.

**Assignment-target unification:** corpus 444 → 448, matrix 448/448.
`cargo test` 252/0, `--features jit` 428/0, parity **404/40/0 → 408 PASS /
40 SKIP / 0 PARITY_FAIL across 448**. SKIP set unchanged. Clippy 111 → **110**.

**CompoundAssign widening:** corpus 448 → 453, matrix 453/453. `cargo test`
252/0, `--features jit` 428/0, parity **408/40/0 → 413 PASS / 40 SKIP /
0 PARITY_FAIL across 453**. SKIP set unchanged. Clippy **110/110** — the new
`resolve_index_place` costs one lint and the deleted duplicate compound
production returns one.

Every pre-existing fixture is byte-identical on both backends across both
changes.

---

## 29. `.copy` semantics settled — blocker #3 remains PARTIAL

*(Implemented by the commit that introduces this entry: `check_copy_arg_contract`
and the `is_method` parameter on `analyze_function` in `src/frontend/semantic.rs`.
Cited by name because the work and this entry are one commit.)*

**Status: semantics settled, lowering NOT done. The `.copy` blocker is PARTIAL
and converts ZERO fixtures.** SKIP is unchanged at 40. Scalar `.copy` still does
not lower and waits on ABI option (a); `copy_into` still does not lower.

### What was wrong

Four things, all of them language questions rather than lowering gaps.

**Repeated `.copy` targets were nondeterministic.** `f(x.copy, x.copy, x.copy)`
setting the three parameters to 10, 20 and 30 bled all three back to `x` in
`HashMap` order. Ten runs of one program, before:

```
10 30 30 20 10 20 30 30 10 30
```

A program whose output varies run to run cannot be a parity reference, which is
what the interpreter is required to be.

**`copy_into`'s bundle contract was enforced at runtime.** Declared
`copy_into(a, b)`, called `copy_into(x)`, produced
`RUNTIME ERROR: variable 't.a' has not been declared` — although both name lists
are static. Same layer violation as the C1–C4 access-path holes.

**`.copy` on a method parameter was accepted and silently inert.**
`call_semantic_method` never registered a bleed-back, so the modifier did
nothing. The divergence, before: free function `f(x.copy)` gave `15` with the
caller at `15`; method `z.take(y.copy)` gave `8` with the caller still at `7`.

**Array `.copy` failed with a message about assignment targets.** Both
`f(r.copy)` and `f(r.copy: [3: t64])` gave
`SEMANTIC ERROR: index assignment target must be an array` — a diagnostic about
assignment, on a parameter declaration.

### The rulings, implemented

**Repeated `.copy` targets are rejected** at analysis time. The ambiguous program
is now unwritable rather than ordered. Ten runs now give one error, identically.

**`copy_into` bundles are checked at analysis time**, in both directions: a name
the parameter declares that the call does not bundle, and a name the call
bundles that the parameter does not declare.

**Option B — `.copy` is rejected on methods**, at both the argument and the
declaration. A method already has a declared mutation channel: its receiver.
No fixture or example anywhere passed a copy kind to a method, so this cost
nothing.

**Array `.copy` — the binding was fixed, not the arrays rejected.** The parser
already parsed `n.copy: T` and then discarded the type; `ParamKind::Copy` had no
slot for it. It now carries `Option<Type>` and the declared type reaches the
binding, so `f(r.copy: [3: t64])` works and writes back: `101 / 101 / 2`.

Rejecting arrays was the alternative and was not chosen, for two reasons. The
locked ABI ruling for Slice 2 is uniform by-address `.copy` for **every** type,
so a rejection would have to be reversed. And discarding a type the user wrote
is the actual defect — the array failure was a symptom.

An untyped `.copy` parameter still cannot be indexed, which is correct: it has no
type. The message now says which type it does have
(`— it has type numeric literal`) instead of asserting something about assignment
targets.

### The choke points

Two, each covering every form rather than one arm per call shape.

`check_copy_arg_contract` is called once per call site — four sites build
copy-kind arguments — and carries all three argument rules. `analyze_function`
gained a required `is_method: bool` and rejects copy kinds on method parameter
declarations there; every impl method, phen method and free function reaches it,
and the flag has no default, so a new method-producing path must say which it is
or it does not compile.

Proven on forms with no case written: repeated `.copy` split across three
non-adjacent arguments, a repeated target through a different call path,
`copy_into` with the names reordered (legal — the bundle is keyed by name, not
position, so `copy_into(b, a)` against a declared `(a, b)` is fine and gives 12),
and `copy_into` bundling a duplicate name.

### `freed` removed

A per-frame `HashSet<String>`, read by the bleed-back filter and by a
`ScopeEvent::Free` printer, and **never inserted into anywhere in the tree**. It
was the state of `free_variable`, a scope-level reclamation operation introduced
`3110ffe` (2026-03-02) and removed by `5247b32` (2026-03-06) — the commit that
added `Handle<T>` with generational indices. The state outlived its operation by
five months.

The `ScopeEvent::Free` printer went with it, and that was not merely dead: since
the set was always empty, the debug trace printed `= freed` for **every live
variable** at every scope close.

### `.copy.free` deprecated — accept and warn

Behaviour unchanged; the parameter kind still works. The warning records the real
history: its `free` suffix names `free_variable`, removed 2026-03-06. **It has
been a no-op since March, not since Model B** — Model B only removed the last
accidental difference, which was plain passing aliasing on the JIT. Removal waits
until `.copy` itself settles so the family changes once.

Only two fixtures use it, neither with an output sidecar, so the notice moves no
gate.

### What this does NOT do

No ABI change, no lowering. `t49_copy_contract` (scalar `t64` `.copy`) and
`t54_post_split_verify` (`copy_into`) both still SKIP. The four `.copy`-named
fixtures `t10`–`t13` remain blocked on nested FuncDef regardless.

### Delta

Corpus 453, unchanged — **no fixtures added, none converted**. `cargo test`
252/0, `--features jit` 428/0, parity **413 PASS / 40 SKIP / 0 PARITY_FAIL across
453**, matrix 453/453, clippy 110/110. **SKIP set unchanged at 40.**

Every fixture's **stdout and exit code is byte-identical** across all 906 runs.
The only stdout+stderr differences are the deprecation notice on
`t11_copy_free_isolated` and `t24_full_system_regression`, the two fixtures that
use `.copy.free`.
