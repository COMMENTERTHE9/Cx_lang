# Cx 0.1 Deep Dive — Synthesis & 0.2 Foundation

**Audit base:** `submain` @ `22015c420ad40483d0531d1109fa9837d44deca3` (the 0.1 line; one merge past the documented baseline `4d612df`).
**Date:** 2026-05-24. **Author:** deep-dive audit, four pillars.
**Detailed reports:** `report/performance.md` · `report/correctness.md` · `report/architecture.md` + `report/architecture_risks.md` · `report/dx.md`. Benchmarks in `bench/`; repros in `report/pillar2_repros/`; real programs in `report/pillar4_programs/`.

> This document is the foundation for the 0.2 roadmap. It does not decide the roadmap — it gives the evidence and the throughlines so the team can. One decision (R1) is explicitly left open for the team to make with all four pillars in view.

---

## 1. What Cx 0.1 actually is

Cx 0.1 is a small, coherent, statically-typed language with a **tree-walking interpreter as its shipped default** and an **experimental Cranelift JIT** behind `--features jit`. The frontend (lexer → parser → resolver → semantic) is fast, well-tested (189 fixtures, 243/418 unit tests green, differential parity 120 PASS / 69 SKIP / 0 FAIL), and produces a genuinely good intermediate form — `SemanticProgram`, with per-node types and resolved `BindingId`/`FunctionId`.

The defining structural fact, which nearly every finding traces back to: **there are two execution engines that re-implement the language's semantics independently** — the interpreter walks `SemanticProgram`; the JIT lowers `SemanticProgram → IR → Cranelift`. Below `SemanticProgram` they share nothing.

The honest one-line summary: **the frontend is in good shape; the value layer (how the interpreter represents and looks up values) and the two-engine split are where the cost, the bugs, and the future pain concentrate.**

## 2. The four pillars in one view

| | Finding in one line | Detail |
|---|---|---|
| **Fast** | The whole frontend: sub-millisecond lex+parse+semantic for every hand-written program. | perf §2 |
| **Slow** | Interpreter runtime is 100% of wall time. Two hot paths: per-call **64 KB arena memset (90%** of call-heavy instructions) and **SipHash variable lookup (~26%** of loop-code instructions). | perf §3–4 |
| **Right** | Div/mod-by-zero guards, integer wrapping, INT_MIN edge cases, `-0.0`, handle staleness, circular-import detection, array length checks — all correct in the interpreter. | corr §"found correct" |
| **Wrong** | Interpreter **does not type-check composite-literal contents** (struct missing/wrong/extra fields; array element types). 13 interpreter/JIT disagreements, all from the JIT omitting safety checks (OOB→garbage/segfault, div-by-zero→SIGILL, `INT_MIN/-1`→SIGFPE, `f64→t8` cast→codegen panic). | corr Cat A/B |
| **Will hurt** | No shared semantic core; builtins duplicated across 4 modules; interpreter discards computed `BindingId`; quadruple numeric type representation; `runtime.rs` 1,955-line monolith. | arch T1–T2 |
| **Fights user** | `let x = expr` is a parse error; enums can't be function params/returns; no string concat/`len`; parse errors leak raw token enum names; no `--help`. | dx |

## 3. The connective tissue (the real value of doing all four)

The pillars are not four independent lists. Four throughlines connect them, and each reframes a finding into a cheaper or more important action than any single pillar saw:

**Throughline A — "the interpreter throws away work the semantic phase already did."**
Pillar 1 measured ~26% of loop-code instructions in SipHash hashing of variable-name strings. Pillar 3 found *why*: `SemanticExprKind::VarRef` carries a resolved `BindingId`, but the interpreter destructures `{ name, .. }` and looks up by string in a `HashMap<String,_>` (`runtime.rs:677,426`). **The fix is not "design slot-indexed locals" (Pillar 1's framing) — it's "use the `BindingId` that already exists and is currently discarded."** A significant-looking perf project collapses into wiring up existing data. *This single reframing is the clearest payoff of the four-pillar approach.*

**Throughline B — "the 13 correctness bugs are one architecture bug."**
Pillar 2 found 13 interpreter/JIT disagreements and catalogued them as Category A. Pillar 3 (T1.1/R1) showed they are not 13 independent defects — they are symptoms of **one missing abstraction**: there is no canonical definition of what each operation *means*, so the two engines drift. Fixing them one-by-one is endless; fixing the cause (a shared semantic core) closes the class.

**Throughline C — "the builtin mismatches and the error-message mess each have a single root."**
Backend builtin disagreements (Pillar 2) ↔ builtins hardcoded as strings in 4 modules (Pillar 3 R2). The misleading "declare it with 'index 5: TYPE = value'" message (Pillar 2 D1) and the user-hostile parse errors (Pillar 4 DX8) ↔ a too-coarse `RuntimeError` enum and raw `{:?}` formatting. In both cases a scattered symptom set reduces to one structural fix.

**Throughline D — "the worst perf finding and an architecture smell are the same line."**
The 64 KB-per-call memset (Pillar 1 #1) is `Arena::new()` eagerly zeroing a chunk in `push_function_scope` (Pillar 3 T3.1) — the arena is coupled to scope creation regardless of use. One change (lazy/non-zeroing arena) fixes both the measured 90% and the design smell.

## 4. The one big decision (for the team, not this document)

**R1 — commit to a shared semantic core for 0.2, or keep two engines in sync manually?**

- **Commit (expensive now):** define one canonical semantics — either a shared operation module both engines call, or make the interpreter consume IR so there is a single lowering. Closes the entire disagreement class (Throughline B), unifies the path for every future feature, lets the JIT eventually become default. Real architectural work.
- **Don't (cheaper now, costlier forever):** keep both engines, keep adding parity fixtures, keep fixing drift one bug at a time. Every 0.2 feature is implemented twice.

This is the largest leverage point the audit found. **Per the explicit instruction, it should be decided by the team with all four pillars in view — not from any single recommendation.** The sequencing in §5 is built so that the smaller Tier-1 fixes (R2, R3) make this decision *cheaper* whichever way it goes, so it can be deferred a little without cost.

## 5. Prioritized 0.2 recommendations (evidence-ranked)

Grouped by class. Each cites its evidence, rough effort, and owning area.

### Group 0 — ship-now quick wins (small, high daily impact, no dependencies)
| # | Action | Evidence | Effort | Owner |
|---|---|---|---|---|
| Q1 | **Lazy / non-zeroing arena** — stop the 64 KB-per-call memset | perf §3, arch T3.1 | small | runtime |
| Q2 | **Allow `let x = expr`** — kills the most pervasive friction | dx DX1 | small | frontend |
| Q3 | **Humanize parse errors** — map token kinds to friendly names, drop `{:?}` | dx DX8 | small | frontend |
| Q4 | **Array fill/zero-init shorthand** (`[0; N]`) | dx DX4, perf | small | frontend |
| Q5 | **`--help`/`--version`; document `--test` & `--backend=llvm`** | dx DX10–11 | trivial | CLI/docs |
| Q6 | **FxHash for the variable map** — interim, until R3 lands | perf §4 | trivial | runtime |

### Group 1 — Tier-1 foundational (do these regardless of the R1 decision; they de-risk it)
| # | Action | Evidence | Effort | Owner |
|---|---|---|---|---|
| F1 | **Builtin registry** — one source of truth, replacing the 4-module duplication. *Highest-leverage bounded fix.* | arch R2, corr Cat A | medium | cross |
| F2 | **Binding-indexed locals** — use `BindingId`, drop string lookup (Throughline A). Also the real Pillar-1 perf win. | perf §4, arch R3 | medium | runtime |
| F3 | **Composite-literal type/completeness checking** in the semantic phase — closes the shipped-interpreter soundness holes (struct missing/wrong/extra fields, array element types). 4 fixtures ready to promote. | corr Cat B | medium | frontend |
| F4 | **Split `runtime.rs`** into env/eval/exec/ops/builtins/format; move `Value`/`RuntimeError` into `runtime/`. Substrate for F1/F2. | arch R5, T2.2 | medium | runtime |

### Group 2 — correctness/safety gate (mostly backend-owned)
| # | Action | Evidence | Effort | Owner |
|---|---|---|---|---|
| S1 | **JIT bounds checks** (OOB read/write/neg) | corr A2–A4 | medium | backend |
| S2 | **Guard JIT `sdiv`/`srem`** (div-by-zero, `INT_MIN/-1`) | corr A5–A6 | small | backend |
| S3 | **Narrow float→int lowering** (`f64→t8/t16` codegen panic) | corr A1 | small | backend |
| S4 | **Enums in function signatures** — `Light` vs `enum Light` unification; add the missing fixtures | dx DX2, corr | medium | frontend |
| S5 | **Granular `RuntimeError` variants** + rewrite D1–D4 messages | corr D1–4 | small | frontend |
| S6 | **Catchable recursion-depth guard** (interpreter SIGABRT) | corr A7 | medium | runtime |

### Group 3 — the big decision
- **R1 — shared semantic core direction.** See §4. Decide with the team; sequence after F1–F4 have narrowed its scope.

### Group 4 — language/DX features (schedule against product goals)
- String `+`/`len` and basic string ops (dx DX3); a dynamic collection type (dx DX6); unify the numeric type representation (arch R4); rule on the design footguns — `if <unknown>`, `when` exhaustiveness, out-of-range-literal warnings (corr Cat C).

## 6. Suggested 0.2 sequencing

A low-regret order where each step leans on the previous:

1. **Group 0 quick wins** — land immediately, in parallel; visible improvement for users and contributors on day one.
2. **F4** (split `runtime.rs` + relocate `Value`) — creates the modules the rest live in.
3. **F1** (builtin registry) → **F2** (binding-indexed locals) → **F3** (composite-literal checking) — into the new modules; F1+F2 de-risk R1, F2 banks the perf win, F3 closes the shipped soundness holes.
4. **Group 2 safety gate** — in parallel on the backend side; gates any future "JIT as default".
5. **R1 decision** — now cheaper to scope; make the call, then execute.
6. **Group 4** — as product priorities dictate.

## 7. What *not* to do

The codebase has real strengths; the rewrite instinct should not overreach. Preserve: the **`SemanticProgram` design** (the problem is under-use, not the design — Throughline A); the **`BackendError { message, exit_code }`** contract; **circular-import detection**; the **2,005-line IR validator** (a strong investment); and the **test/fixture discipline** (189 fixtures + differential parity is the reason this audit could move fast and trust its baseline). The language's **core expression/control-flow surface, generics, `when`, Result/`?`, and string interpolation are pleasant and worked first-try** in Pillar 4. Keep the green matrix green: the Pillar-2 repros were deliberately kept out of `verification_matrix` and should be promoted to fixtures only as each bug is fixed.

## 8. Test-suite gaps surfaced

The 189-fixture matrix is strong but has blind spots the audit hit: **enum-typed function parameters/returns** (none exist — DX2/S4 slipped through), **composite-literal type validation** (F3 — wrong/missing/extra fields uncaught), and the JIT **safety-trap cases** (A1–A6 are SKIP-or-crash, not parity-covered). Promote the `report/pillar2_repros/` programs to fixtures as their bugs are fixed, and add enum-in-signature coverage with S4.

## 9. Coordination summary (for Zara → backend dev)

Backend/IR-owned items, all flagged across the daily notes:
- **S1–S3** (JIT bounds checks, guarded division, narrow float→int) — the Category-A safety gate; blocks JIT-as-default.
- **R1** (shared semantic core) — needs backend input on the IR-as-shared-path option; `host_boundary.rs` (8,944 L) and `ir/lower.rs` (8,019 L) are the two largest files and the relevant surface.
- **JIT performance baselines** — still need (a) phase timers around `prepare_ir`/`execute` in `main.rs`, (b) a short alignment on which JIT metrics matter (compile vs. execution vs. warmup). Not done this audit; the interpreter findings stand independently.

---

### Bottom line
Cx 0.1 has a solid frontend and a good IR, undercut by an interpreter value layer that discards its own resolution work and a two-engine split with no canonical semantics. The highest-value 0.2 work is mostly **bounded and cheaper than it first appeared** — a builtin registry, binding-indexed locals, composite-literal checking, a lazy arena, and a handful of quick wins — with **one genuinely large decision (R1)** to be made deliberately by the team. Fix the value layer and unify the semantics, and the bug class and the perf cliff both close.
