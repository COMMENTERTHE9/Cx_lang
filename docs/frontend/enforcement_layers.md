# Enforcement Layers — Locked Principle

**Status: Locked.** Same standing as the method-ownership principle in
`docs/post_0_1/gene_phen_design.md`. Changing it is a design decision, not an
implementation choice.

---

## The Principle

> **If a fact is known to semantic analysis, semantic analysis rejects it. Any
> surviving backend check is defense-in-depth only.**

Two corollaries, both load-bearing:

1. **A backend check is never the only check.** If the interpreter or the JIT is
   the only layer that refuses a program, the other backend will eventually
   accept it — and the two will disagree on a real program.
2. **A check only speaks where the fact is actually known.** When analysis has
   not resolved a type (`Unknown`, a type parameter, a `copy_into` container),
   it stays silent rather than guessing. Silence there is the cost of the rule,
   not a loophole in it.

---

## Why This Is Locked

Cx has two execution backends with different failure modes. The interpreter
carries a dynamic value model; the JIT carries a static one. When analysis is
silent about something it could have decided, the two improvise *independently*
and their improvisations differ:

- the interpreter tends to invent something plausible — a struct field that was
  never declared, an enum variant that does not exist — and run to completion
  with a wrong answer and exit 0;
- the JIT tends to hit an internal invariant and bail out with exit 127, which
  the parity harness counts as a clean SKIP.

That asymmetry is why this class of bug surfaces as *interpreter runs wrong /
JIT skips* rather than as a parity failure, and why the parity harness — the
project's strictest gate — cannot see it. **A layer-enforcement hole is
invisible to the invariant that would otherwise catch it.** That is what makes
this a principle rather than a preference.

---

## Where It Came From

Six holes of this exact shape were found in one working session, each by
accident: `f64` comparison, enum equality, the width checks, the ordering
allowlist, const immutability, and const container mutation. A deliberate audit
(`docs/known_issues.md` §14) then found four more at once — and the four turned
out to be a single omission wearing four hats: **a fact validated on the
construction path and never on the access path.**

A struct's field list, an enum's variant list, and a receiver's type are all
carried in the semantic type. Analysis had every one of them in hand. In each
case the check that consumes that knowledge was written once, where a value is
*constructed* (struct literal, enum declaration, assignment lvalue), and never
where one is *accessed*.

The sharpest evidence that this was architectural rather than incidental: the
rule "loop variables are read-only" was implemented in three places, each of
which documented itself as backup for another. Analysis pattern-matched two
statement shapes; the interpreter's `RuntimeError::ReadOnlyLoopVar` was
`#[allow(dead_code)]` with a comment deferring to the IR layer; the IR
validator's `LoopVariableReassignment` check did not fire on the program. The
comment in `types.rs` was a written record of the misattribution.

---

## How To Apply It

### Writing a new check

Put it in `src/frontend/semantic.rs`, at the **single choke point where the fact
and the use meet** — not one arm per syntactic form. The per-form shape is the
recurring failure: three separate fixes each shipped with a form missed
(equality had the ordering allowlist, ordering did not; plain assignment had the
const guard, index and field assignment did not; the loop-counter scan matched
two statement shapes and one level of nesting defeated it).

The proven structure, from the const fix and reused for C1–C4:

- **one shared helper** holding the lookup or predicate, and
- **one call at the entry point**, before the target match — so every form
  routes through it by construction rather than by anyone remembering to patch
  each arm.

The test that the structure worked is not that the cases you wrote pass. It is
that the check fires on a form you wrote **no** case for.

### Deciding what to do with an existing backend check

Decide explicitly; do not default either way. Both answers have been correct:

- **Keep it** when a path can reach the backend write without passing through
  the analysis choke point. The const guard in `src/runtime/scope.rs` was kept
  for exactly this reason — method write-back and string-interpolation targets
  call `set_var` directly and never touch `Stmt::Assign`.
- **Remove it** when it is strictly subsumed and its presence would diverge. The
  JIT's const-assignment guard was removed once analysis rejected first.

Either way, state the reason in the commit. A backend check with no recorded
rationale is how the loop-counter rule ended up owned by nobody.

### Diagnostics

The analysis-time message replaces a backend message that users have seen. Make
it name the actual thing: `unknown field 'zzz' on struct 'P'`, not a variable
diagnostic about `p.zzz`. Backend messages phrased as internal invariant
violations (`lowering invariant violation: ...`) must never be a user's first
report of an ordinary mistake in their program.

---

## Known Limits Of The Current Implementation

Recorded so they are not mistaken for the principle being satisfied:

- **Positions.** Several analysis errors report position `0`, which renders as
  line 1. `Expr::DotAccess`, `Expr::Val` and `AstValue::EnumVariant` carry no
  source position in the AST, so field-read and value-position enum errors point
  at the wrong line. See `docs/known_issues.md` §17.
- **Statically decidable cases still left to runtime.** A constant array index,
  and division by a literal or const zero, are both decidable from information
  analysis already has. Both backends currently reject them, but differently —
  a clean diagnostic on the interpreter, a hard trap on the JIT. See
  `docs/known_issues.md` §15.
- **String interpolation** is validated entirely at runtime, including cases
  that are pure string-literal-plus-scope facts. See `docs/known_issues.md` §16.
- **The parity harness cannot see the difference.** `TestExpectation::Fail` is
  satisfied by any non-zero exit, so a fixture passes whether the JIT emitted a
  diagnostic or trapped. Strengthening it is the standing counterpart to this
  principle.
