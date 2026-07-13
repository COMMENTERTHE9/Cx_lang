# Cx Project Roadmap — Living Summary

Last updated: 2026-07-13

This file is a concise synthesis of the project's roadmap state. Detailed
0.1-era phase logs live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0, frozen at the 0.1 RC — historical)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.4, frozen at the 0.1 RC — historical)

---

## Shipped

**0.1** — tagged `9fc0d24`, 2026-05-22. Language surface frozen: structs,
generics v1/v2, enums, arrays, control flow, memory boundary model (str/strref/
Handle<T>), Result<T> + `?`, test runner. Cranelift JIT backend for the
supported 0.1 construct subset.

**0.2** — tagged `7340116`, 2026-06-06.

**0.3** — tagged `1654f5b`, shipped as a GitHub Release. Independently verified:
merge commit has 2 parents, tag points to the merge commit, local == remote,
release notes match the approved changelog. Landed:
- D1 core convergence + the JIT memory-safety gate (zero unsound markers)
- `if` as expression and statement, enums, the unknown/three-state-bool arc
- Static strings (D2.3: length, repeat, concatenation, content equality,
  print-time interpolation) — complete
- Scalar `Result<T>` (D2.4a/b/c): construct/print, the `?` operator, equality
  — complete for scalar payloads
- Labeled breaks (`'outer: loop { break 'outer / continue 'outer }`), both
  parse+reject and execution commits

**Post-release hardening (on submain):**
- [x] Composite literal type-checking — struct field presence/type/unknown-field validation, array element type checking (8169d33)

**0.3.1** — tagged `3430e4e`, shipped. Scalar Handle core (D2.5a/b/c):
`Handle<T>` for scalar `T` (`{I8, I16, I32, I64, Bool}`): construct, read,
drop. Generational safety and double-drop non-aliasing verified on both
backends. Also includes post-0.3.0 correctness hardening merged from submain.

---

## Post-0.3.1 — landed on `submain`, not yet in a tagged release

Post-0.3.1 hardening on submain (12 commits ahead of main):
- [x] Parser trailing-builtin ret_expr fix (known-issues #2)
- [x] Scope boundary fix for if/while/loop/while-in (audit finding 3.1)
- [x] Width-range enforcement at method-call args, reassignment, array-index (audit finding 3.2)
- [x] Ordering comparison rejection on Bool/Enum (audit finding 3.3)
- [x] CI matrix consolidation (audit finding 3.4)
- [x] Enum ==/!= interpreter fix (known-issues #9)
- [x] Gene/phen design spec committed (`docs/post_0_1/gene_phen_design.md`)
- [x] Gene/phen implementation decisions: 5 of 6 resolved (dispatch, phen lookup, Self resolution, Ord mapping, built-in provenance). Cross-module coherence still open.

---

## Corrected Version Sequence

- **0.3.1** — Scalar Handle core (D2.5). *(Tagged and shipped.)*
- **0.3.2** — Pattern matching (named binding `as v`, guard clauses) — shifted
  from 0.3.1.
- **0.3.3** — gene + phen design pass — shifted from 0.3.2. Design spec at
  `docs/post_0_1/gene_phen_design.md` (v1.1, design locked). 5 of 6
  implementation questions resolved on submain; cross-module coherence
  remains open.
- **0.3.4** — gene + phen implementation, operator overloading, generics v3
  (type bounds) — shifted from 0.3.3.
- **0.4** — Stdlib v1, Cranelift AOT / Ricey v0, LLVM AOT, bootstrapping
  begins/completes, math layer. *(Unchanged from prior sequencing.)*
- **1.0** — First stable release.
- **1.0+** — Graphics begins: Vulkan/DX12 bindings. *(Not before 1.0 — the
  0.4 math layer is graphics PREP, not graphics itself.)*
- **1.1+** — Renderer.
- **1.2+** — Physics.
- **1.3+** — Audio.
- **1.4+** — Networking / NOD Protocol.
- **2029+** — TSG playable.

---

## Future Design Work (unscheduled)

Audited during the 0.3 cycle. Both found to have **no interpreter reference**
(no `Value` variant, no eval site, no fixtures) — not "not yet implemented,"
genuinely undesigned. Deferred until each gets its own design pass; only then
can either be scheduled into a version.

- **NullPoint<T>** — a nullable-pointer type. The only existing spec is one
  line of roadmap intent ("maps into the unknown/known model"); the audit
  found the intended design ties it to two other JIT-deferred subsystems
  (the unknown/TBool seam, `Handle<T>`), which needs resolving before any
  implementation starts.
- **random stdlib foundation** — audited and found to be intent-only-minus:
  no interpreter reference, no roadmap line recording it as a decision, no
  "open-decision #2" tracker entry anywhere in the repo. A future design pass
  will also need to resolve the RNG-determinism question (JIT parity requires
  matching the interpreter's algorithm + seed state exactly, not just
  "produces a random number").

**Also deferred, not yet placed on any version:** non-scalar `Handle<T>`
(`Handle<str>`, `Handle<struct>`) and `Handle<Handle<T>>` — the D2.5
investigation found the semantic layer's `Handle<T>` claim is hardcoded
regardless of the real payload type, which would need real type-flow work
before a non-scalar payload could lower safely. Untested, no claim either way
on nested Handles. Needs its own scoping audit before scheduling.

---

## Working Notes

**2026-07-13:** Gene/phen design decisions sprint: 2 commits on submain resolved 5 of 6 open implementation questions (dispatch strategy, phen lookup, Self resolution, Ord mapping, built-in gene provenance) from OPEN to DECIDED. Only cross-module coherence remains genuinely open. Submain 12 commits ahead of main. Matrix 321/0 on main (unchanged).

**2026-05-18:** PR #268 merged `train/backend-determinism` → submain (host_boundary expansion, IR lowering fixes, 23 new parity fixtures including CX-228 t159–t177). CX-233 implements while-in loop source-to-IR lowering on `stokowski/CX-233` (branch-local, not yet merged) — WhileLoop parity moves to 8/0. Submain 171 commits ahead of main.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
