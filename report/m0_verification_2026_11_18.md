# M0 verification — three lowering divergences, PR #476 review, next steps

- **Date:** 2026-11-18
- **Branch:** `backend/m0-lowering-divergences`, cut from `submain` @ `d4b2c06`
- **Environment:** Linux x86-64, rustc/cargo 1.97.0, debug builds
- **Follows:** `report/tsg_llvm_readiness_2026_11_18.md` (the audit) and the
  owner's eight decisions on it.

Nothing here touches `main` or a release. Nothing was merged into `submain`.

---

## 1. Outcome

| Item | Result |
|---|---|
| Clean branch from `submain`, audit report carried over | Done (§2) |
| PR #476 reviewed against the three audit failures | Fixes one of three; **not integrated** — a blocking regression and an unresolved semantic question (§3) |
| Field-assignment right-hand side evaluated twice | Fixed, 2 fixtures (§4.1) |
| String `when` arms never matching | Fixed, 1 fixture (§4.2) |
| `while` on an unknown condition | Fixed on both backends, 5 fixtures (§4.3). **Changes the interpreter; needs your confirmation** |
| `while` on a non-bool condition, found by review | Compiled code now refuses it, as `if` does (§4.4) |
| Default, JIT, parity, matrix | All pass; no fixture moved to SKIP; no expectation changed (§5) |
| Independent review of the fixes | No blocking defect (§5) |
| Silent differences still open | Three, all pre-existing, filed (§6) |
| M1.1 proposal | `docs/backend/static_data_design.md`, awaiting approval (§8) |

---

## 2. Branch and provenance

- `backend/m0-lowering-divergences` starts at `submain` `d4b2c06`.
- The audit report is commit `c49a12e`, cherry-picked from `0abfa9a` with the
  original commit recorded in its trailer. Its diff against `submain` is the one
  report file.
- The original commit `0abfa9a` is unchanged on `ccr-24d12998-ha2zap`, which
  still sits on `main`. Nothing else from that branch came across.

---

## 3. PR #476 — review

*"Preserve runtime string identity and owned Result snapshots"*,
`codex/runtime-string-identity` @ `2a68658`, one commit on `submain` `d4b2c06`.

### 3.1 What was checked

| Check | Result |
|---|---|
| Mergeable against `submain` | Clean; base is current |
| CI on the PR | Build, tests, matrix, stale-base all green |
| `cargo test` on its head, run here | 316 / 0 |
| `cargo test --features jit` | 498 / 0 |
| Parity | 615 PASS / 48 SKIP / 0 PARITY_FAIL (663 fixtures) — matches its description |
| Replay of the 694 pre-existing programs, both backends, base vs PR | No exit-code or stdout change; two refusal messages reworded |
| The three audit repros on its build | See below |
| Code read | All of `host_boundary.rs` and `lower.rs` changes |
| Independent review | §3.4 |

### 3.2 The three audit failures against its code

| Failure | `submain` JIT | PR #476 JIT | Interpreter |
|---|---|---|---|
| Field assignment evaluates the right-hand side twice | `20`, `2` | `10`, `1` — **fixed** | `10`, `1` |
| `when` on a string | `0 0 0` | `0 0 0` — not fixed | `1 2 0` |
| `while` on an unknown bool | `3` | `3` — not fixed | `0` |

The field-assignment fix is one deleted line, with a fixture.

### 3.3 Why it was not integrated

**Blocking: unbounded memory growth and a large slowdown on any string in a
loop.** The PR makes every string evaluation and every named string binding call
a host function that heap-allocates a descriptor and records it in a hash map.
Nothing is released until the program ends.

```cx
fnc: t64 work(n: t64) {
    hits: t64 = 0
    i: t64 = 0
    while (i < n) {
        s: str = "tick"
        if s == "tick" { hits += 1 }
        i += 1
    }
    hits
}
print(work(20000000))
```

| JIT build | Wall time | Peak memory |
|---|---|---|
| `submain` | 0.48 s | 15 MB |
| PR #476 | 57.6 s | 7.05 GB |

Both print the same answer, so no fixture or replay sees it. Memory grows in
proportion to the number of string evaluations: a loop twice as long would need
about 14 GB, on a machine with 15 GB.

**The reason for the allocation is an interpreter behaviour that looks like a
defect, not a rule.** The PR's stated aim is that `Result` equality in compiled
code should match the interpreter's, and the interpreter compares a string
inside a `Result` by its arena position:

```cx
assert_eq("same", "same")              // passes
assert_eq(Ok("same"), Ok("same"))      // fails: "assert_eq failed: Ok(same) != Ok(same)"
assert_eq(parse(false), Err("bad"))    // fails: "Err(bad) != Err(bad)"
assert_eq(Color::Red, Color::Red)      // fails: "Color::Red != Color::Red"
```

`assert_eq` has a hand-written comparison that handles numbers, bools, floats
and top-level strings (by content), passes `Result` payloads to the derived
`Value` equality (which compares a string's offset and length, not its text),
and returns false for everything else. The PR reproduces this in compiled code
with a simulated arena offset per string evaluation, and adds five
`expected_fail` fixtures that pin it — among them that two `Ok("")` values stop
being equal if `len("padding")` is evaluated between them.

I could find no document that states this as intended. Ordinary string equality
is by content everywhere else, the error text contradicts itself, and `==` on two
`Result` values is rejected by analysis, so `assert_eq` is the only place it can
be seen. It is recorded as `docs/known_issues.md` §55 for an owner ruling.

### 3.4 Independent review

A second reviewer was given the PR's diff and both binaries, without the
findings above, and asked to break it. It ran about 170 programs on both
backends with both builds. Its verdict: **blocking defects found.** I re-ran each
finding below before recording it.

| # | Severity | Finding | Reproduced |
|---|---|---|---|
| 1 | Blocking | Every string literal evaluation, string binding and string parameter allocates and is never freed during the run. It measured the same growth independently: 20 million bindings, 14 MB → 7 GB and 0.44 s → 56 s. Under a 1 GB memory limit the PR's JIT aborts where `submain`'s finishes | Yes |
| 2 | Should fix | `len(arr)` inside a function, where `arr` is a top-level array, ran on `submain`'s JIT and is refused by the PR's (exit 127). The PR makes `len` evaluate its operand, which then hits the module-storage refusal | Yes |
| 3 | Should fix | A new output difference: `?` inside a bare-expression `when` arm, in a function with a `.copy` parameter, writes back on the PR's JIT and not on the interpreter. The root cause is an interpreter bug (known-issues §56b), but before the PR the two agreed | Yes |
| 4 | Note | New clean refusals of programs `submain`'s JIT ran: `Ok(<generic struct>)`, `assert_eq` on two `Result` values of different array lengths, and user functions named `cx_str_copy`, `cx_value_copy` or `cx_str_identity_eq` | Not re-run |
| 5 | Note | Two of the PR's five `expected_fail` fixtures (`t_result_empty_identity_len`, `t_result_empty_identity_print`) already trap on `submain`, and the harness checks only the exit shape, so they cannot tell the intended trap from any other | Not re-run |

What held up under that review: no crash and no wrong output in the new `Result`
and identity logic itself; no unsoundness found in the new host functions; the
existing 650 fixtures unchanged; aggregate `Result` snapshots isolated correctly
across wrap, unwrap, loops and nested values; payload-kind checks could not be
tricked into reading a payload as the wrong type.

It also found four defects that exist on `submain` independently of the PR. They
are recorded as known-issues §56 and listed in §6 below.

### 3.5 What the PR gets right

Run on `submain`'s JIT, these PR fixtures fail or are refused; on the PR's JIT
they pass and match the interpreter:

| Fixture | `submain` JIT | What the PR fixes |
|---|---|---|
| `t_field_assignment_rhs_once` | trap | The double evaluation |
| `t_try_copy_writeback` | trap | An early `Err` return through `?` skipped `.copy` write-back |
| `t_result_aggregate_snapshot`, `t_result_aggregate_equality` | trap | An aggregate inside a `Result` pointed at a stack frame or at its source |
| `t_result_numeric_alias` | refused | `Result<t8>` assigned to `Result<t64>` / `Result<f64>` |

These are real defects and the fixes for them do not depend on string identity.

### 3.6 Recommendation: revise, do not replace

1. **Split the PR.** Keep the `?` write-back fix, the aggregate `Result`
   snapshots and the payload-kind checks. Drop the field-assignment line, which
   M0 now carries (same change, same fixture file, so the rebase is clean).
2. **Remove the string-identity emulation** — `cx_str_copy`,
   `cx_str_identity_eq`, the simulated offsets, and the five fixtures that pin
   identity — pending the ruling on §55.
3. **Bound the aggregate snapshots.** They use the same never-released storage.
   A `Result` holding an array built in a loop grows the same way. That needs a
   release point or a note that it waits for the memory model.
4. **Renumber its known-issues entry**; §52–§55 are taken by this branch.

If the ruling on §55 is "compare by content", the remaining string work becomes
small: the interpreter's `assert_eq` compares payloads by content, and compiled
code calls `cx_str_eq` on string payloads. No arena emulation is needed.

---

## 4. The three fixes

Each is one commit with its fixtures and its known-issues entry.

### 4.1 Field assignment — `7531675`, known-issues §52

- **Cause.** The `Assign` arm lowers the right-hand side before it looks at the
  target. The field-target arm lowered it again into a variable of the same name
  and stored that second value.
- **Fix.** Remove the second lowering. This is PR #476's change, taken as it is
  rather than rewritten.
- **Fixtures.** `t_field_assignment_rhs_once` (the PR's, byte for byte) and
  `t_field_assign_rhs_once_shapes`: scalar field, array field, struct-valued
  field, method-call right-hand side, each printing from inside the call.
- **Discriminates.** The unfixed JIT prints 19 lines for the second fixture; the
  interpreter and the fixed JIT print 15.

### 4.2 String `when` — `eed8a4d`, known-issues §53

- **Cause.** A string pattern was lowered to its own descriptor and compared to
  the scrutinee with the pointer comparison used for every other literal. Two
  different descriptors are never equal.
- **Reference behaviour, checked.** The interpreter resolves both strings and
  compares the text. That is also what `==` and `assert_eq` do for strings on
  both backends, so content comparison is the established rule and the
  interpreter is right.
- **Fix.** A string pattern lowers to `cx_str_eq`, in the statement and the
  expression form.
- **Fixture.** `t_when_str_content`: scrutinees from a parameter, a typed
  variable, a struct field, an array element and a function result; patterns
  include the empty string and a prefix pair.
- **Discriminates.** The unfixed JIT prints the catch-all value on all 14 lines,
  including the 10 where an arm should match.
- **Boundary, filed not fixed.** A string pattern against a non-string scrutinee
  is accepted by analysis and can never match on the interpreter. Compiled code
  now refuses it (exit 127). No fixture has that shape.

### 4.3 `while` on an unknown condition — `1b87fe6`, known-issues §54

- **Reference behaviour, checked — and found wrong.** The interpreter read
  unknown as false and skipped the loop. That is not the language's rule:
  - analysis rejects a loop condition typed as unknown, with the message
    *"Unknown value cannot be used as a loop condition -- control-critical
    context"*;
  - `if` raises a run-time error for a `bool` that holds the unknown state,
    because *"an unknown value cannot choose a branch"* (tracker #026, and the
    README's section on three-state bool).

  `while` had the compile-time half of that rule and not the run-time half.
- **Fix.** Both backends refuse. The interpreter raises a new
  `UnknownLoopCondition` error (exit 1, a diagnostic naming `while`). The JIT
  reuses the guard `if` already has, placed in the loop header so it runs on
  every evaluation (exit 126).
- **This changes interpreter behaviour, and it is a ruling that needs your
  confirmation.** A program that relied on an unknown `while` condition quietly
  ending the loop now stops with an error. No fixture, example or benchmark did.
  The independent reviewer's reading, which I accept: the principle existed, but
  the run-time rule for `while` did not — #026 was scoped to `if`, and the README
  stated the rule for `if` only (it now says `if` or `while`). The audit's own
  plan for this task said "no interpreter change"; your later instruction to
  check the interpreter independently is what this rests on. The reviewer would
  have made the same call on the merits.
- **Reach.** It also covers a comparison on an unknown number
  (`x: t64 = ?` … `while (x < 5)`), which is itself unknown. `if x < 5` already
  behaved this way.
- **Fixtures.** `t_while_unknown_condition`, `t_while_unknown_after_iterations`
  (both recorded as interpreter diagnostic, JIT trap — the same pair
  `t_if_unknown_condition` records), `t_while_definite_bool`,
  `t_while_three_state_definite`, `t_while_three_state_break_before_unknown`.
- **Discriminates.** On the second fixture the unfixed interpreter runs to
  completion and the unfixed JIT loops until killed. The last three are controls:
  they pass before and after, and exist to catch a guard that refuses too much.

### 4.4 A `while` condition that is not a bool — known-issues §57a

Found by the independent review at the site of 4.3, and not one of the three
audit failures.

- **Cause.** `while (n)` on an integer: the interpreter ends the loop for
  anything that is not true; compiled code branched on non-zero. Both exited 0.
- **Fix, compiled side only.** `while` now makes the type check `if` already
  makes, so compiled code refuses the shape (exit 127) as it always has for
  `if n { .. }`.
- **Left open.** The interpreter treats a number as false in `while` and as
  true-if-non-zero in `if`, and analysis accepts both. Deciding what a number
  means as a condition is a language question, so nothing was changed there.
- **Test.** A unit test for the refusal. No fixture: the interpreter runs the
  program and compiled code refuses it, which would add a SKIP.

---

## 5. Regression results

All run on Linux on the branch head.

| Gate | `submain` `d4b2c06` | M0 head | Change |
|---|---|---|---|
| `cargo test` | 314 / 0 | 320 / 0 | +6 new unit tests |
| `cargo test --features jit` | 493 / 0 | 499 / 0 | +6 new unit tests |
| Fixtures | 650 | 658 | +8 new |
| Parity | 602 PASS / 48 SKIP / 0 FAIL | 610 PASS / 48 SKIP / 0 FAIL | +8 PASS, SKIP unchanged |
| `run_matrix.sh` | not re-run | 658 / 658 | |

- **Replay.** All 694 pre-existing fixtures, examples and benchmarks, on both
  backends, with the pre-fix and post-fix binaries: 1,388 runs, zero differences
  in exit code, stdout or stderr, zero timeouts. Run twice: after the three
  fixes, and again on the final build.
- **No expectation was edited.** The diff adds files under
  `src/tests/verification_matrix/` and changes none.
- **No fixture moved to SKIP.** The 48 are the same 48.
- **The original repro programs** from the audit now agree on both backends.
- **Wider sweep.** All 202 probe programs written during the audit and this
  phase, interpreter against JIT on the fixed build: none exits 0 on both
  backends with different output. Three still differ by exit code, all already
  recorded: the `cx_trap` name collision from the audit, and the two `assert_eq`
  cases in known-issues §55. That sweep is not a proof of absence: the two
  independent reviews afterwards found four more silent differences that none of
  these programs exercised. One is fixed (§4.4); three are open (§6).
- **Six unit tests pin the IR** (`divergence_shape_tests` in `src/ir/lower.rs`):
  one call per field-assignment right-hand side, one `cx_str_eq` per string arm,
  the refusal at the string-pattern boundary, the unknown guard in the loop
  header, no guard for a definite condition, and the refusal of a non-bool
  `while` condition. Run against the parent of the fixes, the first four of the
  original five fail and the negative control passes.

**Each commit passes on its own.** The JIT gate was run at every commit, not
only at the head:

| Commit | Tests | Parity |
|---|---|---|
| `7531675` field assignment | 493 / 0 | 604 PASS / 48 SKIP / 0 FAIL (652 fixtures) |
| `eed8a4d` string `when` | 493 / 0 | 605 PASS / 48 SKIP / 0 FAIL (653) |
| `1b87fe6` unknown `while` | 493 / 0 | 609 PASS / 48 SKIP / 0 FAIL (657) |
| `63c4544` IR-shape unit tests | 498 / 0 | 609 PASS / 48 SKIP / 0 FAIL (657) |
| Head: non-bool `while` refusal, eighth fixture | 499 / 0 | 610 PASS / 48 SKIP / 0 FAIL (658) |

**Clippy.** 111 warnings on both feature sets, before and after (clippy
0.1.97). No new lint.

**Benchmarks.** The eight programs in `bench/` on the JIT, pre-fix and post-fix
binaries interleaved in one session: none is slower. `bench_struct_loop` drops
from 17.7 ms to 10.8 ms (median of 22 runs), because its field assignments no
longer compute their right-hand side twice. `bench_fib_rec` lowers to
byte-identical IR. These programs finish in about 10 ms, so they can show a
regression of this size but not a small one.

### Independent review of the M0 commits

A second reviewer was given the three fix commits and both binaries and asked to
break them and to judge the `while` ruling on its merits. Its verdict: **no
blocking defect; each fix does what its commit claims and could not be broken.**

What it ran, all on both backends with the pre-fix and post-fix binaries:

| Check | Result |
|---|---|
| Replay of all 706 tracked `.cx` files | Only the five discriminating new fixtures changed; no hang, crash or disagreement on the fixed build |
| IR dump of all 706 files, before and after | 24 differ: 20 field-assignment programs (the duplicate lowering removed, nothing else), `t_when_str_content`, and the three three-state `while` fixtures. No definite-bool `while` changed |
| 900 generated programs mixing the three constructs | No disagreement on the fixed build; the interpreter changed only on the 159 that reach an unknown `while` condition |
| About 95 hand-written programs | No disagreement on the fixed build |
| The seven fixtures | All expected outputs correct; five discriminate, two are appropriate controls |

What it raised, and what was done:

| # | Its finding | Action |
|---|---|---|
| 1 | The interpreter change is a new ruling, disclosed but beyond the audit plan's stated scope; it would make the same call | Flagged for your confirmation (§4.3); README aligned |
| 2 | A non-bool `while` condition still differs silently | Compiled side fixed (§4.4); interpreter side filed, §57a |
| 3 | A string pattern on an integer scrutinee went from running to refused on the JIT | Intended and stated in the commit; bool, enum and float scrutinees were already refused. Kept |
| 4 | `--backend=validate` now fails on `t_when_str_content` | The validator does not know `cx_str_eq`; same gap as string `==`. Input to M1.2 |
| 5 | `t_while_unknown_after_iterations` does not assert "runs twice, then refused" | Added `t_while_three_state_break_before_unknown`, which pins the first half |
| 6 | Two more pre-existing silent differences, unrelated to these commits | Reproduced and filed, §57b and §57c (§6 below) |

It did not run `cargo test` or the parity harness; those results are mine.

My own adversarial pass, run on both backends with the pre-fix and post-fix
binaries, found nothing on the fixed build:

- a string scrutinee that is a call with a side effect is evaluated once;
- patterns with spaces, a tab escape, accented and CJK text, a 70-character
  string, and case differences all match by content;
- string `when` inside a method, inside `for` and inside `while`;
- a `while` condition with a side effect is evaluated the same number of times
  as on the interpreter;
- an unknown condition in an inner loop of a nested pair is refused at the same
  point on both backends, after identical output;
- conditions built from `!`, `&&` and `||` over unknown and definite values;
- a field assigned from a method that mutates the same struct, and
  `p.y = next() + next()`, which the unfixed JIT evaluated four times.

---

## 6. Found during this work, not fixed

| Finding | Where recorded |
|---|---|
| A compound field assignment whose right-hand side changes the same struct through `.copy`: interpreter 102, JIT 101, both exit 0. **Silent** | Known-issues §56a |
| A `.copy` write-back made while evaluating an `if`, `while` or `when` condition is lost in compiled code: interpreter `1 1`, JIT `0 0`. **Silent** | Known-issues §57b |
| A guarded catch-all `when` arm followed by another arm drops the later arms in compiled code: interpreter `0 5`, JIT `5`. **Silent** | Known-issues §57c |
| The interpreter treats a number as false in a `while` condition and as true-if-non-zero in an `if` condition; analysis accepts both | Known-issues §57a |
| The interpreter leaks a function scope when `?` returns early from a bare-expression `when` arm | Known-issues §56b |
| Printing a `Result` that holds a struct crashes compiled code (segfault or abort) | Known-issues §56c |
| The interpreter prints a struct's fields in a different order on different runs | Known-issues §56d |
| `assert_eq` compares strings in a `Result` by storage position and rejects two equal enums; the JIT differs | Known-issues §55 — needs a ruling |
| A string `when` pattern against a non-string scrutinee is accepted by analysis | Known-issues §53, boundary |
| A `bool` assigned an unknown value inside a loop cannot be compiled, so that shape of §54 is covered on the interpreter only | Known-issues §54 |
| The standalone IR validator rejects the IR of three-state `&&` / `||` ("Binary rhs type mismatch: operand has Bool, result declares I8"), which the JIT runs correctly. Same on `submain` before these commits; a third case, with `cx_str_eq` and `cx_handle_*`, of the validator disagreeing with lowering | Here; input to M1.2 |
| `interpreter_baseline_all` and the parity test report "SKIP — binary not found" and pass or abort when the binary is not where they expect, rather than failing clearly | Here; relevant to CI trust |
| The remaining M0 items from the audit: reserving runtime symbol names (`cx_trap` can be replaced by a user function), rejecting an unknown `--backend`, and the document corrections | Audit §8.2 — not in the scope restated for this phase; small separate packets |

---

## 7. Proposals asked for in the decisions

### 7.1 Where the native-executable milestone sits in the version roadmap (Decision 7)

`docment/ROADMAP.md` is not edited. This is a proposal; each row that differs
from the roadmap says so.

| Version | Roadmap today | Proposed | Change |
|---|---|---|---|
| 0.4 | Stdlib v1, Cranelift AOT / Ricey v0, LLVM AOT, bootstrapping begins/completes, math layer; remaining lowering blockers; array design gate | **First native executables.** M0, M1, M2: Cranelift AOT producing standalone executables for every JIT-passing fixture on Windows and Linux. Plus the lowering blockers the roadmap already lists as open, and multidimensional arrays, which have landed | **Narrowed.** Leaves 0.4: LLVM AOT, Ricey v0, stdlib v1 beyond what the runtime library provides, bootstrapping, the math layer |
| 0.5 | Multidimensional arrays landed or completing | **Optimized native builds.** M3 and M4: LLVM AOT at parity, then release mode. Place expressions (E2). The math-layer design gate, then E3 | **Re-scoped.** Arrays move up because they are already done; 0.5 takes LLVM and the math layer |
| Before 1.0, unnumbered | — | Memory and collections (E4), FFI (E5), stdlib v1, Ricey v0, bootstrapping begins | **No version numbers invented.** Each gets one when it is scoped, which is the roadmap's own rule |
| 1.0 | First stable release | Unchanged | — |
| 1.0+ … 2029+ | Graphics, renderer, physics, audio, networking / NOD, TSG playable | Unchanged, subject to the reconciliation in §7.3 | — |

Two points to decide:

1. Whether LLVM AOT may ship inside 0.4 marked experimental if it reaches parity
   before 0.4 is tagged, or always waits for 0.5.
2. "Bootstrapping begins/completes" is one line in the roadmap. I propose
   "begins" stays before 1.0 and "completes" has no date until E4 and E5 exist.

### 7.2 Supported LLVM / Clang versions (Decision 5)

**Proposed: Clang 18 minimum, tested through 20.**

| Platform | What is available | How I know |
|---|---|---|
| This Linux container | Clang 18.1.3 | Ran it; built and ran a hand-translated program in the audit |
| GitHub Actions `ubuntu-24.04` | Clang 16.0.6, 17.0.6, 18.1.3 | Runner image manifest, read today |
| GitHub Actions `windows-2022`, `windows-2025` | LLVM 20.1.8, plus Visual Studio 2022 17.14 with its Clang component | Runner image manifests, read today |
| The owner's Windows machine | **Not verified** | No access |

- 18 is the floor because it is what Ubuntu 24.04 ships, and every release since
  17 uses only opaque pointers, which is what the emitter will write.
- 20 is the ceiling tested because it is what the Windows runners ship.
- The IR subset needed (integer and float arithmetic, `getelementptr` on bytes,
  phi, calls, constant data) has been stable across those versions. **I have not
  run LLVM 19 or 20**, so that is an expectation, and the first LLVM task
  includes a CI job on both runner families that compiles a fixed program.
- The emitter records the version it found in its error message when the
  toolchain is missing or too old.

### 7.3 TSG requirements (Decision 8)

Recorded as owner-provided context, not yet reconciled:

- TSG is *That Space Game*, a space MMO. Earlier planning covers fleet command,
  NPC systems, reputation, physics, graphics, audio, and networking through the
  NOD Protocol.
- Historical engine work exists: a Rust virtual file system, a C ABI, graphics
  tooling. Treated as possibly relevant, not as current requirements.
- The older Cx master roadmap includes Vulkan/DirectX 12, math types,
  rendering, physics, audio and networking.

The engine-capability track (E1–E7 in the audit) stays **unlocked**. Nothing in
it has been turned into a commitment, and no graphics API has been chosen.

To reconcile, I need the material itself. The account can see a private
repository named `TSG-TeamComms` (last pushed 2025-06) and several other private
repositories whose contents I have not looked at. I have attached none of them.
Tell me which repositories or exports hold the current TSG design, the older Cx
master roadmap and the engine work, and I will compare them against the audit's
gap list and return a reconciled requirement set with conflicts called out.

---

## 8. Next: M1.1

`docs/backend/static_data_design.md` is the implementation-ready design for
relocatable static data in the IR. It is not implemented. It lists the IR
changes, the single lowering function affected, the validation and parity
requirements that must hold, the predicted fallout, and why behaviour does not
change. It asks two decisions: approval to implement, and whether a minimal
JIT-path check may land with it ahead of the full validator work.
