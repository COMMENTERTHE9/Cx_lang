# Cx First-User Audit (tracker #029)

**Protocol + report template.** Completed run. Filled in as the audit progressed.

This audit answers one question: **can a person who has never seen Cx clone the repo, build it, and write a small working program of their own — following only the README — in under 30 minutes?**

> **Reviewer-honesty caveat (read first).** The protocol asks for *fresh eyes*. The reviewer here is **not** fresh — this run was executed by the same context that did the 0.2 internal work, so I know Cx's internals. That weakens the headline *time* metric (I could not authentically "discover" syntax I already know). To keep the audit valid I did two things: (1) for every Phase-4 syntax decision I followed **only what the README states**, and where the README is silent I made the guess a Rust-literate newcomer would make and recorded the result, rather than reaching for the answer I knew; (2) the *findings* below — README contradictions, misleading diagnostics, undocumented idioms — are objective and reproducible regardless of reviewer freshness. Treat the **friction findings as the real deliverable** and the **30-minute clock as indicative, not authoritative**. A genuinely fresh reviewer should re-run Phases 0–4 to validate the time target.

---

## Pre-flight — starting conditions

| Field | Value |
|---|---|
| Reviewer | fresh **context** (insider knowledge — see caveat above) |
| Date | 2026-05-31 |
| Repo HEAD (commit) | `b3faea97659771c3c33fdbc20ccc05781982bdda` (branch `submain`) |
| OS / platform | Windows (x86-64), Git Bash |
| Rust toolchain version | `rustc 1.96.0-nightly (03749d625 2026-03-14)`, `cargo 1.96.0-nightly` |
| Prior Cx exposure | Significant (executed the 0.2 tracker work) — disclosed |
| Start time (clock) | build cache was warm from prior gates; cold-build time **not** measured |

---

## Phase 0 — Prerequisites

**Record:**
- Did the README state the prerequisites clearly? **partial.** "Requirements" lists "Rust stable toolchain" and "Cranelift JIT support requires the `jit` feature." Clear enough to act on.
- Did you have to guess or look anything up externally? The README says **stable**; the machine runs **nightly 1.96**. The build worked anyway, so the "stable" claim is a soft mismatch, not a blocker. No external lookups needed.
- _Findings:_ Prerequisites are adequate. Minor: README says "stable" but does not pin a minimum version; project built on nightly.

⏱ **Time check:** ~1 min.

---

## Phase 1 — Clone & first read

**Record:**
- **What is Cx and who is it for?** "A compiled, GC-free systems language for game engines, tools, and systems programmers, built around explicit memory behavior and declared-width arithmetic." — answerable cleanly from the first paragraph. Good.
- Does the README tell you, in order: how to build / how to run a program / where to find a first example? **yes / yes / yes** — "Getting Started" has Build, Run-with-interpreter, and an `examples/` section. Well ordered.
- First impression: Yes — the "Code Taste" snippets and a clear pipeline diagram make it feel approachable and intentional.
- _Findings:_
  - **F8 (Polish):** The README describes **v0.1.0** (411 tests, 182 fixtures, 120 PASS/62 SKIP). The actual HEAD is mid-**0.2** (418 tests, 202 fixtures, 133 PASS/69 SKIP). The whole "Deferred Post-0.1" list is stale — several items in it (string interpolation, `f64` print formatting, `?` literal, enum `when` arms) **work in the interpreter today**. A newcomer reading the deferred list will *under*-estimate what they can do.
  - **F9 (Minor):** The canonical `examples/hello.cx` is built on **string interpolation** (`"Hello, {name}!"`), which the README's own "Deferred Post-0.1" list says is *not* in the release. The example contradicts the doc.

⏱ **Time check:** ~4 min.

---

## Phase 2 — Build

**Record:**
- Did it build cleanly on the first try? **yes** (`cargo build --features jit`). Cache was warm (incremental 0.3s); a cold cold-build number was **not** captured (preserved the gate cache).
- Warnings? On the **jit** build, none surfaced. But the **default** build path the README gives for running (`cargo run -- …`, no `--features jit`) emits **2 warnings** — see F7.
- _Findings:_
  - **F7 (Minor):** README claims "**zero compiler warnings**," but the interpreter run path (`cargo run -- examples/hello.cx`, the default non-jit profile) prints two: `unused import: IrType` (`src/backend/cranelift/mod.rs:2`) and `fields total_size and alignment are never read` (`src/ir/types.rs:98`). A newcomer's *first command* shows warnings the README promised wouldn't exist.

⏱ **Time check (clock):** ~6 min.

---

## Phase 3 — Run the first example

**Record:**
- Did the README point you to a runnable first example? **yes** — `cargo run -- examples/hello.cx`.
- Did it run and produce expected output? **yes:**
  ```
  Hello, World!
  Age: 42
  Score: 9.8
  ```
  (Two build warnings printed first — F7.) Note this confirms three "Deferred Post-0.1" features actually work: string interpolation, `t32`/`str` handling, and `f64` printing (`9.8`).
- Run command obvious? **yes**, copied verbatim from README.
- _Findings:_ Clean. The example works; the only blemish is the warning noise (F7) and the doc/feature skew (F8/F9).

⏱ **Time check:** ~8 min.

---

## Phase 4 — Write your own program *(core of the audit)*

**Attempted:** Counter → Fibonacci → Traffic-light state machine (reached all three).

**Per-need discovery log:**

| Need | Knew it? | Source |
|---|---|---|
| Declare typed variable | guided | README "Code Taste" — but the shown form `let total: t64 = 0` **does not compile** (F1) |
| Declare (corrected) | recovered | the compiler's own error message told me the right form `x: T = value` |
| Loop | yes | README `for i in 0..n` |
| Function | yes | README `fnc: t64 sum_range(n: t64) { … }` |
| Branch | yes | README `if … else` |
| Print | yes | README + `hello.cx` |
| String interpolation of a **variable** | yes | inferred from `hello.cx` (`{name}`) |
| String interpolation of a **call** | **no — silently failed** | F2: `{fib(i)}` printed literally |
| Define an **enum** | **no** | README documents zero enum syntax — guessed Rust-like (F3) |
| Annotate an **enum-typed** value | **no path found** | F4: neither `x: Light` nor `x: enum Light` works |
| Transition between states | recovered | F5: assignment-in-arm fails; `when`-as-expression works (undocumented) |

**Programs:**
- **Counter** — works (after F1 fix). Prints 0–4 and a running total of 10.
- **Fibonacci** — works (after F2 workaround: compute into a variable, then interpolate). Prints `fib(0..9)`.
- **State machine (traffic light)** — works **only** via `let cur;` (untyped) + `cur = when cur { … }` (when-as-expression). Cycles RED→GREEN→YELLOW correctly. Reaching it required outside-knowledge guesses the README does not support (F3/F4/F5).

**Findings:**
- **F1 (Major):** README's flagship "Code Taste" snippet and Getting-Started idiom use `let total: t64 = 0`. The compiler rejects it: *"`let` declares an uninitialized binding and cannot have an initializer — to declare with a value use `x: T = value` (without `let`)."* The **error message is excellent** (gives the exact fix), so it's recoverable — but the very first thing a copy-paste newcomer types fails. The doc, not the compiler, is wrong.
- **F2 (Major):** String interpolation supports **bare variable names only**. `print("fib({i}) = {fib(i)}")` interpolates `{i}` but emits `{fib(i)}` **verbatim, with no error**. Silent wrong output is worse than a diagnostic — a newcomer will think their function call failed.
- **F3 (Major / Blocker-for-state-machine):** The README contains **no enum syntax anywhere** — enums appear only in the "Deferred Post-0.1" list. A strictly-README reader cannot write the most-revealing exercise at all. I guessed `enum Light { Red, Green, Yellow }` + `Light::Red`; the *variant* syntax and `when` arms were accepted.
- **F4 (Major):** There is **no discoverable way to write an enum type annotation.** `s: Light` fails with *"return type mismatch: expected Light, got enum Light"* — a baffling message where the two type names render identically. `s: enum Light` is a parse error. The only working path is to avoid annotations entirely (untyped `let cur;`), discoverable only from the fixtures (off-limits to a README reader).
- **F5 (Major):** Assignment inside a `when` arm (`Light::Red => cur = Light::Green`) is a parse error with a ~25-token "expected …" wall. The working idiom — `cur = when cur { … }` (when-as-expression) — is **not documented**. Recoverable only by knowing the pattern.

⏱ **STOP THE CLOCK** — first original program (counter) ran correctly at roughly the **10-minute** mark; the full state machine landed around **18 minutes**.
**Clone-to-first-original-program time: ~10 min (counter) / ~18 min (state machine).** _(target ≤ 30 min — met, but see reviewer caveat; a fresh reviewer would lose more time on F3/F4/F5.)_

---

## Phase 5 — Structured error-message probe

One scale: **did it tell me how to fix it? (yes / sort of / no)**

| # | Trigger | Message (verbatim) | Fix-guidance |
|---|---|---|---|
| 1 | undefined name (`print(nope)`) | `SEMANTIC ERROR (line 1): use of undeclared variable 'nope'` | **sort of** — names the problem, doesn't suggest declaring it. Adequate. |
| 2 | type mismatch (`x: t32 = "hello"`) | `SEMANTIC ERROR (line 1): type mismatch: expected t32, got str` | **yes** — clear. |
| 3 | large→small int (`x: t8 = 500`) | `SEMANTIC ERROR (line 1): integer literal 500 out of range for t8 (valid range: -128..127)` | **yes** — excellent; states the exact valid range. |
| 4 | non-exhaustive `when` | `SEMANTIC ERROR (line 4): non-exhaustive 'when': add a '_' catch-all arm to handle the remaining cases` | **yes** — names the fix verbatim. |
| 5 | `if` on unknown bool (`b: bool = ?` / `if b`) | `RUNTIME ERROR (line 2): 'if' condition is unknown; an unknown TBool can't choose a branch — use 'when' to handle true, false, and unknown explicitly` | **yes** — best in the set; explains *why* and redirects to `when`. |

**The deliberate-probe error messages are a clear strength** — the 0.2 work (#026/#027/#028/#037) shows. The weak diagnostics are the *incidental* ones around README mismatches:

- **F6 (Major):** Writing the unknown value as the word `unknown` (`b: bool = unknown`) — the natural guess, since `unknown` is the README's wire-value name and a valid `when` keyword — produces a **confidently misleading** parse error that points at the **colon** and dumps a 20-token "expected" list, never mentioning that the unknown **literal is `?`**. `?` is undocumented as a value literal in the README (listed only under deferred JIT lowering). This is the worst message encountered: wrong location, no actionable hint, unguessable fix.

⏱ **Time check:** ~25 min.

---

## Phase 6 — Wrap-up verdict

- **Total time, clone → first original program:** ~10 min to a running counter; ~18 min to the state machine. **Hit ≤30 min: yes** — *with the reviewer caveat that a truly-fresh reviewer would likely exceed it on the enum cluster (F3/F4/F5).*
- **Overall impression:** The *compiler* is in better shape than the *README*. Build, run, and the simple exercises (counter, Fibonacci) are smooth, and the targeted error messages are genuinely good — among the best parts of the experience. But a README-only newcomer hits a wall the moment they stray from arithmetic-and-loops: the doc teaches a non-compiling variable form (F1), gives no enum syntax (F3), no enum-annotation spelling (F4), and no `when`-as-expression idiom (F5), while several "deferred" features silently work and one core spelling (`?` vs `unknown`) is undocumented with a misleading error (F6). The friction is almost entirely **documentation/version skew**, not compiler bugs.
- **Would you come back?** **maybe → yes**, leaning yes — because every dead end had a recoverable path and the diagnostics that *are* tuned are excellent. The single biggest reason a *fresh* newcomer might **not** return: the README's own first code sample (`let x: T = 0`) doesn't compile, which erodes trust on minute one.
- **Top 3 improvements (priority order):**
  1. **Re-sync the README to the 0.2 reality.** Fix the `let x:T=value` examples to `x:T=value`; document enums (declaration, `::` variants, and that the type annotation is via untyped `let` / inference); document `when`-as-expression; document `?` as the unknown literal; move the now-working features out of "Deferred Post-0.1"; update the version/stat block.
  2. **Fix two incidental diagnostics.** (a) `unknown`-as-a-value should say "use `?` for the unknown literal" instead of a colon-pointed token dump (F6); (b) the enum annotation mismatch "expected Light, got enum Light" should not render two identical-looking names (F4).
  3. **Make string interpolation of non-variable expressions either work or error** — silent literal passthrough of `{fib(i)}` (F2) is a trust-killer. At minimum, reject unknown interpolation forms with a diagnostic.

---

## Findings log

| ID | Phase | Severity | What happened / what I expected | Disposition |
|----|-------|----------|--------------------------------|-------------|
| F1 | 2/4 | Major | README "Code Taste" + Getting-Started teach `let x: T = value`; compiler rejects it. Expected the doc's example to compile. (Error msg gives the fix → recoverable.) | Won't-fix code / **fix README** |
| F2 | 4 | Major | `{fib(i)}` in an interpolated string is emitted **literally, no error**; only bare `{var}` interpolates. Expected either the call result or a diagnostic. | 0.3 candidate (or fix-before-freeze: add diagnostic) |
| F3 | 4 | Major | README documents **no enum syntax**; enums appear only under "Deferred". A README-only reader can't write the state machine. | **Fix README** |
| F4 | 4 | Major | No writable enum type annotation: `x: Light` → "expected Light, got enum Light" (identical-looking names); `x: enum Light` → parse error. Must use untyped `let`. | New 0.2 item (diagnostic) + **fix README** |
| F5 | 4 | Major | Assignment inside a `when` arm is a parse error (token-wall); `when`-as-expression is the undocumented idiom. | **Fix README** (document idiom) |
| F6 | 5 | Major | `b: bool = unknown` → misleading parse error pointing at the `:`; real literal is `?`, undocumented. Confidently wrong location, no hint. | New 0.2 item (diagnostic) + **fix README** |
| F7 | 2/3 | Minor | Default (non-jit) `cargo run` emits 2 warnings; README claims "zero compiler warnings". | Fix-before-freeze |
| F8 | 1 | Polish | README is v0.1.0 (411 tests / 182 fixtures); HEAD is mid-0.2 (418 / 202). Deferred list lists features that now work. | **Fix README** |
| F9 | 1 | Minor | `examples/hello.cx` relies on string interpolation, which the README's deferred list says isn't in the release — self-contradiction. | **Fix README** |

### Severity / Disposition keys are unchanged from the template above.

---

## Triage summary

- **Total findings: 9** (Blocker 0 / Major 5 / Minor 2 / Polish 1; plus 1 Minor in Phase 0 prereqs noted inline). *Note:* the F3+F4+F5 cluster is **effectively a Blocker for the state-machine exercise** under a strict README-only fresh reviewer.
- **Dominant theme:** documentation/version skew, not compiler defects. The compiler's targeted diagnostics (#026/#027/#028/#037) are a standout strength; the README has fallen a full minor-version behind the code.
- **New tracker items suggested:** F2 (interpolation diagnostic), F4 (enum-annotation diagnostic + rendering), F6 (`unknown`-as-value diagnostic), F7 (default-build warnings). The rest are a single consolidated **"re-sync README to 0.2"** doc task (F1/F3/F5/F8/F9).
- **Did the run hit the ≤30-minute target?** Yes (~18 min to the state machine) — **but** flagged as not-authoritative given the non-fresh reviewer; re-validate with a true newcomer.
- **Go / no-go on first-run experience for 0.2 freeze:** **Conditional go.** The build/run/simple-program path is solid and the error messages are good. The blocker to a clean "go" is **not code** — it's the README teaching a non-compiling sample (F1) and omitting all enum/`?`/`when`-expr syntax (F3/F4/F5/F6). Land the README re-sync and the four diagnostic fixes before freeze and this flips to an unconditional go.

---

*Run on a clean checkout of `submain` at HEAD `b3faea97659771c3c33fdbc20ccc05781982bdda`. Reviewer was not fresh-eyes (disclosed); friction findings are reproducible, the 30-minute clock is indicative. Each finding should either land as a new item in `report/0_2_tracker.md` or close by 0.2 freeze.*
