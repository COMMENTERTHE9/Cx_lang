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
