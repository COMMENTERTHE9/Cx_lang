# Correctness follow-up, PR #476 revision and M1.1 — status

- Date: 2026-11-18
- Asked for by: the owner's third set of decisions ("finish the independent M0
  review and resolve its findings; carry out the bounded correctness follow-up
  and prepare the PR revisions; then proceed with M1.1; return with the test
  results, performance and memory measurements, remaining blockers, and the
  exact status of PR #476")
- Nothing has been merged into `submain`. `main`, `submain`, the releases and
  the PR's own branch are untouched. No pull request was opened and no comment
  was posted.

## 1. In one page

**What is done.** The bounded follow-up (four fixes), the revision of PR #476
(content equality, payload checks, `len`), and M1.1 (relocatable string data)
are implemented, each on its own branch, each commit passing the build and
test gates.

**What it took.** Every piece was reviewed independently — seven reviews in
all — and every review found something. Three rounds of fixes introduced
regressions of their own, which the next review caught: a store through a
nested index that silently went nowhere, a user-defined `+` that read its
operands in the wrong order, and a `Result` payload read on trust that crashed
compiled code. All are fixed and pinned by fixtures. That history is the main
reason this report asks the owner to read §5 and §7 before integrating
anything.

**Where the reviews ended.** The last two found nothing blocking. The seventh
attacked the riskiest change with about 30,000 generated programs and could
not produce a wrong answer, a crash or an exit mismatch; what it found was
that the change *refuses* more programs than it had said (§7.4), and two
overstated sentences. One change was made after it — about
twenty lines, `823b791` — and was sent back to the same reviewer; §7.1 says
where that stands.

**What needs the owner.** Six behaviour changes to the interpreter (§5), one
cluster of open semantic questions about receivers, aliases and operator
operands (§7.2), two gaps in what analysis checks (§7.3), one interpreter bug
found and not fixed (§7.2), and the disposition of PR #476 (§8).

**One thing I did out of order.** I began M1.1 before the follow-up was really
closed: the review of its fixes was still to come and found regressions. I
parked M1.1, fixed those, and resumed. M1.1's commits sit on top of the
corrected follow-up and were reviewed in that position, but the instruction was
to finish the follow-up first.

## 2. Branches

Each is stacked on the one before. All are pushed.

| Branch | Tip | Commits above `submain` | Content |
|---|---|---|---|
| `backend/m0-lowering-divergences` | `5498b41` | 10 | The audit report; M0 (three divergences, plus a non-bool `while`); the M0 report; the M1.1 design |
| `backend/m0-followup-correctness` | `5bc7782` | 23 | The four follow-up fixes; three fixes from their review; three from the review of those; one from the review after that |
| `backend/result-content-equality` | `823b791` | 37 | The PR #476 revision: payload checks, `len`, content equality; two commits of fixes from its review; five from the review of those; one from the review after that |
| `backend/m1.1-static-data` | `046aac2`, then this report | 49 + this report | M1.1 in four commits; four commits from its reviews; the TSG/roadmap reconciliation; the provisional version changes; this report |

Untouched: `main` @ `24b6820`, `submain` @ `d4b2c06`,
`codex/runtime-string-identity` @ `2a68658` (PR #476).

## 3. Reviews and what they found

Each review was done by a separate reviewer given the code and the binaries,
not the conclusions.

| # | Subject | Verdict | Findings, and what was done |
|---|---|---|---|
| 1 | M0 fixes (three divergences) | no blocking | The `while` change is a ruling (owner confirmed it since); a number as a `while` condition diverged (compiled code now refuses); two older silent divergences recorded (§57b, §57c, still open) |
| 2 | Follow-up fixes (`.copy` storage, early `?`, aggregate `Result`, struct print) | all do what they claim; one regression | An index read through a changed array (fixed, §66); a method's receiver changed by its argument — the interpreter was losing updates (fixed, §65); `?` in a trailing expression escaped its function on the interpreter (fixed, §64) |
| 3 | The three fixes of review 2 | **one blocking** | A store through a nested index was lost — introduced by the §66 fix (fixed, §68a); scalar aliases re-read (fixed, §68b); a missed alias case (fixed, §68c); a user-defined operator read its left operand after its right — a regression against a LOCKED rule (fixed, §68d); five older disagreements about receivers and aliases recorded (§69, open) |
| 4 | PR #476 revision | **first commit blocking** | The payload checks refused ordinary programs — 358 of 3000 fuzzed programs (fixed, §70a); `len` of an element still refused (fixed, §70b); a `Result` read at a narrower integer type miscompiled (now refused, §70c); unary minus on a `Result` crashed (refused, §70d); a user function named `cx_str_eq` replaced the runtime's (names reserved, §70e); handles, cross-kind `when` arms and `assert` on a `Result` disagreed (fixed, §70f); documentation errors (corrected, §70g) |
| 5 | The fixes of reviews 3 and 4 | **one blocking** | §70a's fix read a payload on trust and crashed compiled code (fixed by proof, §72a); the operator fix missed field, element and `if`/`when` operands (fixed, §68 addendum); narrowing through an untyped literal (now refused, §72b); range and `unknown` arms still compared bits (fixed, §72c); `__cx_fmod` not reserved (fixed, §72d); `assert` on an enum or handle passed (refused, §72e); a fixture that pinned nothing (replaced, §72f); `len` of a branch-assigned variable newly refused (kept, argued in §72g); an interpreter bug with generic locals (recorded, §72i) |
| 6 | M1.1 | no blocking | The reported slowdown is the compiler binary's layout, not M1.1 (docs corrected, §73a); taking the address of an empty data object crashed (fixed, §73b); the corpus test passed with nothing to run (fixed, §73c); two claims had no default-run test (added, §73d); the address-constant rule is narrower than described (descriptions corrected, §73e); seven inaccurate comments (corrected, §73f) |
| 7 | The fixes of reviews 5 and 6 | no blocking | Could not break the payload proofs (0 wrong answers in about 30,000 generated programs and 900 written ones). The fix of §72a refuses more than it said: a forwarder written as an `if`/`when` value (fixed, §72j), and three shapes that pass a value through local memory (recorded, §7.4). Two overstated statements (corrected). Two test notes (addressed). Re-ran review 5's measurements and got the same numbers |

## 4. Test results

Gates at each branch tip: `cargo build --features jit`, `cargo test`,
`cargo test --features jit`, and the JIT parity harness.

| Tip | `cargo test` | `--features jit` | Parity (PASS / SKIP / PARITY_FAIL) | Fixtures |
|---|---|---|---|---|
| `submain` `d4b2c06` | 314 | 493 | 602 / 48 / 0 | 650 |
| `backend/m0-lowering-divergences` `5498b41` | 320 | 499 | 610 / 48 / 0 | 658 |
| `backend/m0-followup-correctness` `5bc7782` | 324 | 503 | 622 / 52 / 0 | 674 |
| `backend/result-content-equality` `823b791` | 343 | 523 | 645 / 64 / 0 | 709 |
| `backend/m1.1-static-data` `046aac2` | 372 (+1 ignored) | 563 (+1 ignored) | 645 / 64 / 0 | 709 |

Clippy with `--features jit`: 112 warnings at every tip from the follow-up
branch on (111 at the M0 tip; the extra one was not isolated).

**No existing fixture's expectation was changed, weakened, deleted or
skipped.** 59 fixtures were added. Fifteen of them are
compiled-code SKIPs: compiled code refuses the program and the fixture pins
the interpreter's answer.

**One existing fixture changed status and needs acknowledging:**
`t_literal_hint_ok_payload` went from PASS to SKIP on the JIT with §60. It
passed by reading an array out of a stack frame that had already returned; the
second review confirmed that with one extra call in between the old build
prints the wrong numbers.

**Corpus replay** (every program under `src/tests/verification_matrix`,
`examples` and `bench`, both backends, exit code and output compared):

| Change | Programs | Differences |
|---|---|---|
| M0 | 694 | none |
| follow-up branch, tip against its start | 694 | none |
| PR #476 revision, tip against its start | 694 | none in exit code or output; two refusal messages reworded |
| the fixes of reviews 5 and 7, before and after | 694 | none |
| M1.1, with and without, on the final base | 743 | none, stderr included |
| M1.1's empty-object fix, before and after | 753 | none, stderr included |

**Fuzzing by the reviewers' own generators**, on the final builds:

| Generator | Programs | Disagreements | Refused that the base ran |
|---|---|---|---|
| methods, `.copy`, indexing (reviews 2–3) | 7,000 | 0 (the first run found the lost store in 67) | 0 |
| user-defined operators, named operands (review 5) | 2,000 | 0 (1 before the last fix) | — |
| user-defined operators, field and element operands | 2,000 | 0 (195 before the last fix) | — |
| `Result`s, top-level and in functions (review 4) | 12,000 | 0 | 0 (358 of 3,000 at the first revision) |
| payload-kind confusion: untyped `Err` variables filled through `.copy`, generic helpers, arrays, struct fields (review 7) | 12,000 | 0 | 485 — see §7.4 |
| generic and typed forwarder bodies: reassignment, loops, recursion (review 7) | 8,000 | 0 | 502 — see §7.4 |

**The reviewers' probe programs**, interpreter against compiled code:

| Set | Build | Crashes | Exit mismatches | Silent differences | Agreeing |
|---|---|---|---|---|---|
| Review 5's 1,709 | `submain` | 13 | 81 | 196 | 757 |
| | before review 5's fixes | 5 | 46 | 105 | 1,165 |
| | now | 0 | 1 | 9 | 1,377 |
| Review 4's 418 | base of the revision | 3 | 20 | 17 | 161 |
| | now | 0 | 0 | 0 | 193 |

The ten that remain in the first set are all present on `submain`: the
interpreter bug of §72i (one) and the open operand and alias questions of §69
(nine). They are listed in §7.2. The same figures hold on the tip of the
M1.1 branch, and review 7 re-ran the first set independently and got them
exactly.

## 5. Changes to what the interpreter does — each needs the owner's eye

The interpreter is the reference. These change it. Each is argued in the
known-issues entry named; each is a separate commit and can be reverted alone.

| # | Change | Was | Now | Entry |
|---|---|---|---|---|
| 1 | `while` on an unknown condition | loop skipped silently | run-time error, as `if` already gave — **confirmed by the owner** | §54 |
| 2 | `assert_eq` on strings inside a `Result`, on enums, chars, handles | failed for equal values | by content — **decided by the owner** (chars and handles are my extension of the same rule) | §55, §70f |
| 3 | A struct's printed field order | hash order, different each run | declaration order | §61 |
| 4 | `?` in a trailing expression | returned from the caller too, or "compiler bug" error | returns from its own function | §64 |
| 5 | A method whose argument changes its receiver (`k.add(k.next())`) | the change was lost: `next()` returned 1, then 2, and `k.n` stayed 0 | the method runs on the receiver as the arguments left it | §65 |
| 6 | Early `?` from a `when` arm | leaked the function's scope into the caller | leaves as a return does | §59 |

Item 5 is the one to look at. It changes the result of an ordinary idiom, in
the direction compiled code always had. No tracked program depended on the old
result.

Nothing in the last two rounds changed the interpreter.

## 6. Measurements

Cranelift JIT. Debug builds unless stated. Final = tip of
`backend/m1.1-static-data`.

### 6.1 Memory

| Program | `submain` | PR #476 as submitted | Final |
|---|---|---|---|
| Bind a string and compare it, 20 million times | 14 MB | **7,054 MB** | 15 MB |
| Build a `Result<str>` and an `Err`, `assert_eq` each against a literal, 20 million times | traps on the first comparison | — | 15 MB |
| Pass, return, `.copy`, wrap in `Ok`, forward, compare — 20 million times (review 4) | traps (15 MB without the comparison) | killed at 12,760 MB after 119 s | 15 MB |
| Lower one 60-literal program 4000 times in one process — growth | 39,060 KB | — | 84 KB (M1.1) |

Peak memory does not grow with the iteration count: 15 MB from 200 thousand to
200 million iterations (review 6).

### 6.2 Time

| Program | `submain` | PR #476 as submitted | Final |
|---|---|---|---|
| Bind a string and compare it, 20 million | 0.50 s | 64.5 s | 0.48 s |
| `Result` equality loop, 20 million | traps | — | 0.93 s |
| Compare an empty string with an empty string, 20 million | 2.1 s | — | 0.45 s (§73b) |
| The eight `bench/` programs | 3–16 ms each | — | unchanged |
| Index read where the index calls a method on the same variable, 8-element array, 20 million | 0.17 s | — | 0.11 s |
| The same on a 3000-element array, 20,000 passes (review 3) | 0.35 s | — | 1.8 s — the cost §66 states |

### 6.3 M1.1's performance criterion — met; the earlier report was wrong

The design required no program to be more than 10% slower. My first
measurement, on optimized builds, showed a loop that only binds and compares a
string 7.5% to 10.9% slower over six variants, and I reported the cause as
unidentified and the question as the owner's. Review 6 found the cause, and I
reproduced it: the runtime's string comparison is 36 bytes of code that fit
inside one cache line in the build before M1.1 and cross a line boundary in the
build after, because unrelated code moved. In a single binary that can do
either, with that function's position held fixed, the old address constants
and M1.1's data objects measure the same (17.55 against 17.57 ns over six
variants; 8.61 against 8.79 in my re-run over three). Crossing the boundary
costs about 9.5% either way.

So M1.1 costs 0 to 2%, and there is nothing to decide. The useful lesson is
separate: a string comparison in the JIT moves by a tenth with the layout of
the compiler binary, so any commit can shift such a benchmark, and debug-build
timings cannot resolve differences of that size.

## 7. Remaining blockers and open items

### 7.1 Before anything is integrated

Nothing known in the code is blocking. What integration is waiting on:

1. **The last change has not completed review.** `823b791` teaches the
   payload proof to follow a forwarding function written as an `if`/`when`
   value. It was checked with the seventh reviewer's own generators (20,000
   programs, no disagreement between the backends, before or after), the
   corpus replay, the 2,127 earlier probes and 6,000 programs of the earlier
   generator, all unchanged; and it was sent back to that reviewer for a
   focused soundness check. **That check had not reported when this was
   written.** Until it has, treat the tip of
   `backend/result-content-equality` as one commit ahead of what has been
   independently reviewed; `24ed07a`, one commit below, is fully reviewed.
   The test-only commit `046aac2` on the M1.1 branch is in the same position.
2. **The owner's confirmation of §5**, item 5 in particular.
3. **The owner's acceptance of the refusals in §7.4**, or a decision to close
   the gap in analysis (§7.3) instead.
4. **No pull request exists.** The four branches are stacked and would be
   integrated lowest first.
5. **Platforms.** Everything here was built and run on Linux x86-64. GitHub's
   CI (also Linux) ran on every push and passed on `5bc7782`, `823b791`,
   `10059aa` and the merges between them; its run on `046aac2` was still in
   progress when this was written. Windows has not been exercised by any of
   this work.

### 7.2 Known disagreements between the backends that are still open

Both backends exit 0 and print different things, or one passes where the other
fails. All are present on `submain`; none is hidden by a skipped test.

| Entry | What |
|---|---|
| §57b, §69f | A scalar `.copy` write-back made inside a condition, an `if`/`when` value branch, or the right of `&&`/`||` is lost in compiled code |
| §57c | A guarded catch-all `when` arm followed by another arm drops the later arms in compiled code |
| §57a | A number as a `while` condition: compiled code refuses; the interpreter treats it as false, and as true-if-non-zero in an `if` |
| §69a | What an operator's left operand's variable holds afterwards, when the right operand changed it (the *result* agrees, for every form of operand, since the §68 addendum) |
| §69b | A scalar further alias: the interpreter writes it back, compiled code never does |
| §69c | An expression receiver or alias that is a place is a temporary on the interpreter and the place itself in compiled code |
| §69d | The same variable passed as two aliases |
| §69e, §69g | Interpreter only: a method body works on a copy of its receiver; `break` outside a loop breaks the caller's loop |
| §67 | Struct print order across two modules declaring one name; two places still printing in hash order |
| §72i | **Interpreter bug, found in the last round, not fixed:** a local declared with a type parameter — `y: T = x` in any generic function — fails on the interpreter with a type mismatch; compiled code runs it. Not silent (the interpreter stops), but the reference rejects a correct program |

§69a–d are one question: are an operator's operands, and a method's further
aliases, values or references? It should be answered once.

### 7.3 Not decided, compiled code refuses (no divergence)

`docs/backend/value_equality.md` §4:

- `assert_eq` on top-level structs and arrays, on three-state bools, on an
  integer against a float, on an unknown number.
- **Two gaps in what analysis checks.** A `Result` may be assigned to a
  `Result` of a narrower payload type (`Result<t64>` to `Result<t8>`), and a
  value typed `Result<unknown>` — an untyped variable that started as an `Err`
  — is accepted wherever any `Result` is declared, whatever it holds. The
  interpreter carries on with whatever is inside (a variable declared
  `Result<str>` prints `Ok(5)`). Compiled code refuses unless it can prove the
  payload fits. Both gaps produced miscompiles until they were refused; closing
  them in analysis would be the real fix and is a language decision.
- `assert` on a value that is not a condition (an enum, a handle, a `Result`):
  the interpreter fails it at run time; compiled code refuses.

### 7.4 What compiled code newly refuses that `submain` ran

Stated so that it is a decision and not a surprise. In each case `submain`
compiled the program and the answer happened to agree.

| What | Why refused | Entry |
|---|---|---|
| A `Result` read at a narrower integer type, when the value fits (15 of review 4's probes, which only print it) | The same code truncates when the value does not fit | §70c |
| `len` of a variable first assigned in both branches of an `if` | Compiled code cannot tell it from the one-branch program, where `submain` printed a length the interpreter does not | §72g |
| A generic function that reassigns its parameter, or carries it through a local, and returns it — called with `Err(..)` and used where a `Result` kind is declared | Nothing declares or proves what the value holds | §72a, §72j |
| An element of an untyped array of `Err`s — an error table — returned from a typed function or assigned to a typed variable | The same: the value passes through memory the proof does not follow. **This is the one a real program is likely to contain** | §72j |
| An element of an untyped array of small `Ok` numbers read at a narrower type; a constant reaching a narrower `Result` through an `if`/`when` value or a generic constructor | The width is known only for a constant used directly | §72j, §70c |
| `assert` on an enum variant or handle whose word happened to be zero | Agreed by coincidence | §72e |
| A program that defines a function named like a runtime function | It was called in the runtime's place | §70e, §72d |

The second, third and fourth rows all come from one decision: a value typed
`Result<unknown>` is trusted only where compiled code can prove what it
holds. Review 7's generators, which are built around exactly those shapes,
put a number on it: 4 to 6% of their programs that `submain` ran are refused.
The earlier generator, built around ordinary `Result` use, finds none in
12,000. The alternative to refusing is what the fifth review found — a number
read as a string. The real remedy is in analysis (§7.3), not in more proof.

### 7.5 Deferred by decision

A `Result` whose payload is a struct or an array is refused by compiled code
(§60). Supporting it needs a decision about who owns the payload and about the
`Result` calling convention.

## 8. PR #476 — exact status

- **Open, not merged.** Head `2a68658` on `codex/runtime-string-identity`, base
  `submain` @ `d4b2c06`, one commit, 33 files, GitHub reports it mergeable.
  Nothing was pushed to its branch and no comment was posted on it.
- **Not suitable to merge as it is**: it allocates on every string evaluation
  and never frees (7 GB and a 140× slowdown on a 20-million-iteration loop); it
  pins, with five fixtures, the rule the owner has since replaced (equal
  strings in a `Result` must compare unequal); it refuses `len(arr)` of a
  top-level array; and its payload checks refuse ordinary programs.
- **Superseded by `backend/result-content-equality`**, which takes its payload
  checks (changed so that they refuse a proven mismatch, follow a value through
  functions that only forward it, and trust a declaration only where there is
  one), its `Err`-through-a-merge support, its refusal of operators on
  `Result`s, and its `len` fix (changed to keep what worked); and, earlier on
  the stack, its field-assignment fix and early-`?` write-back with their
  fixtures. It does not take the string identity emulation or the
  execution-owned aggregate snapshots. Known-issues §63, §70 and §72 are the
  record.
- **For the owner to do**: close #476 in favour of the revision, or keep it
  open until the revision is integrated. I have not closed it or commented on
  it.

## 9. Also delivered

- `report/tsg_requirements_reconciliation_2026_11_18.md` — `TSG-TeamComms` read
  in full, read-only: a 2025 team status board, not an engine specification,
  with no mention of Cx. The "older Cx master roadmap" is not in this
  repository; the three historical roadmaps that are contain no Vulkan,
  DirectX 12, math types, renderer, physics or NOD Protocol. Five questions
  only the owner can answer, five documents that would be needed. A 190-row
  table of every historical roadmap item against the code.
- `docment/ROADMAP.md` — the proposed version changes set beside the
  established sequence, which is not edited.
- `docs/backend/value_equality.md`, two new sections in
  `docs/backend/cx_eval_order.md`, `docs/backend/static_data_design.md`
  (design, as-built record, and what an object-file backend still needs).

## 10. Decisions requested

1. Confirm or reverse interpreter change 5 of §5 (a method runs on its receiver
   as the arguments left it).
2. Answer the operand/alias question of §7.2, or say it may wait.
3. The two analysis gaps of §7.3: should assigning a `Result` to a narrower or
   unrelated declared `Result` type be an error? Until then compiled code
   refuses what it cannot prove.
4. Whether the refusals of §7.4 are acceptable as they stand.
5. The interpreter bug of §72i: fix it now as its own small change with its own
   fixtures, or schedule it.
6. PR #476: close or keep open.
7. Whether to open pull requests for the four branches, and in what order. They
   are stacked, so one pull request per branch, lowest first, is the natural
   shape.
