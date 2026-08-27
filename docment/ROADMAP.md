# Cx Project Roadmap — Living Summary

Last updated: 2026-08-16

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

**0.3.3** — tagged `v0.3.3`, 2026-08-16. Detail for each item is in
`docs/known_issues.md` at the cited section.

- **Generic-struct instantiation lowering** (§18) and **generic-function
  monomorphization** (§19/§23) — both now lower. Transitive worklist,
  symbolic-map composition, per-template instantiation cap.
- **`exit()` and top-level `const`** lower (§11, §12).
- **Call-depth guards** (§24 interpreter, §25 JIT) — unbounded recursion is a
  diagnostic on both backends at the same frame, not a crash. The JIT's guard is
  emitted only for functions that can participate in a call cycle.
- **Enforcement-layer audit** (§14) — field, enum-variant and receiver-type
  facts moved from the access path's silence into semantic analysis, plus the
  locked principle in `docs/frontend/enforcement_layers.md`.
- **Const immutability** (§12) moved from runtime-only to analysis time,
  including writes *through* a const that had silently mutated on both backends.
- **Gene-bound soundness** (§22) — a bound promises methods, not fields.
- **Rejection-shape harness** (§15) — `.expected_fail` records and asserts how
  each backend refuses, and `.expected_exit` is honoured by the Rust harness
  rather than by `run_matrix.sh` alone.

Gate state at the tag: `cargo test` 250/0, `--features jit` 426/0, matrix
414/414, parity **374 PASS / 40 SKIP / 0 PARITY_FAIL**, clippy 110/110 on both
feature sets.

---

## Post-0.3.0 — landed on `submain`, not yet in a tagged release

**Scalar Handle core (D2.5a/b/c)** — landed `3ea986d`. `Handle<T>` for scalar
`T` (`{I8, I16, I32, I64, Bool}`): construct, read, drop, all checked against
the interpreter. Generational safety and double-drop non-aliasing empirically
proven on both backends (interpreter and Cranelift JIT).

---

The sequence below reflects the project's current stated direction. It was first
formally recorded on 2026-07-04 — no prior committed roadmap file contained a
0.2+ version sequence, so that was a first recording rather than a correction to
an existing plan — and last amended on 2026-08-16, when 0.3.3 shipped and its
remaining blockers folded into 0.4.

## Corrected Version Sequence

- **0.3.1** — Scalar Handle core (D2.5) + pattern matching (named binding
  `as v`, guard clauses). *(Shipped 2026-07-09 at `3430e4e`. Pattern matching
  landed inside this release rather than 0.3.2 as originally planned.)*
- **0.3.2** — gene + phen: design, implementation, generic bounds
  (`T: GeneName`), operator overloading via the embedded prelude. Plus the
  0.3.1-era audit fixes and the struct-return ABI fix. *(Shipped 2026-07-24.
  See `docs/post_0_1/gene_phen_design.md` for the full spec.)*
- **0.3.3** — Generic functions and structs lowered, call-depth guards on both
  backends, the access-path enforcement audit. *(Shipped 2026-08-16. Full item
  list under "Shipped" above.)*
- **0.4** — Stdlib v1, Cranelift AOT / Ricey v0, LLVM AOT, bootstrapping
  begins/completes, math layer. *(Unchanged from prior sequencing.)* **Plus:**
  finish the remaining lowering blockers, and open the multidimensional-array
  **design gate** in parallel — see below. The design gate touches no code, so
  it runs alongside 0.4's implementation work rather than queuing behind it.

  **Remaining lowering blockers** — array returns from methods, expression
  receivers for operator dispatch, `.copy` / `.copy.free` / `copy_into`
  parameter kinds, nested function definitions, `while-in`, function-body
  `const`, `t128` printing, `char`, and non-identifier string interpolation.
  Array returns and expression receivers were briefly carried as a prospective
  0.3.4; they belong here, and inventing a version slot to hold them would
  recreate the phantom-slot problem the roadmap reconciliation cleaned up. If a
  0.3.4 is ever needed, it gets created when something actually justifies it.
- **0.5** — **Multidimensional arrays landed, or actively completing.**
- **1.0** — First stable release.
- **1.0+** — Graphics begins: Vulkan/DX12 bindings. *(Not before 1.0 — the
  0.4 math layer is graphics PREP, not graphics itself.)*
- **1.1+** — Renderer.
- **1.2+** — Physics.
- **1.3+** — Audio.
- **1.4+** — Networking / NOD Protocol.
- **2029+** — TSG playable.

---

## Multidimensional Arrays — 0.4 design gate, 0.5 delivery

**Scheduled, not aspirational.** 2D and 3D are **first-class**, not a stepping
stone to something else, and multidimensional support is a **language-level
goal** — it is not contingent on any particular application's current needs, and
should not be re-scoped if a consumer of the language happens not to need it
this quarter.

### 0.4 — the design gate (no code)

The deliverables below are **ordered by dependency, not by preference**.
Deliverable 1 blocks 2, 3 and 4, because all three of them contain a `:`.

**1. Resolve the `:` collision.** Inside arrays, `:` already means two different
things:

- the **type**: `[3: t8]` — size, colon, element type
- the **index operator**: `a:[2]` — colon *before* the bracket

The colon-before-bracket indexing form was pinned deliberately: bare `a[2]`
silently mis-parses. That constraint stands, and any multidimensional syntax has
to live with a `:` that is already carrying two jobs. Nothing downstream can be
locked until this is settled.

**2. Lock type syntax.** *(blocked by 1)*

**3. Lock indexing syntax.** *(blocked by 1)*

**4. Lock literal syntax.** *(blocked by 1)*

**5. Dimension ordering and memory layout.** The recorded decision is
**row-major, last index varying fastest**, unless the design pass finds a
concrete reason to overturn it. Recording it now means a later change is a
deliberate reversal with a stated reason, not a drift.

**6. Whether 4D ships immediately, or falls out of a generalized N-dimensional
implementation.** The expectation is the latter: if offset computation, bounds
checking, and literal shape-checking are all written generically over the
dimension count, then **4D is not a separate feature** — it is the same code with
a different N. Deliverable 6 is to confirm or refute that expectation, not to
assume it.

### On the two existing array documents

`cx_arrays.md` and `cx_4d_arrays.md` are **design intent only**. They are also
**not in this repository** — they live outside it, so a reader looking for them
here will not find them.

Neither is written in Cx. One uses Rust syntax; the other uses a third invented
syntax that belongs to no language. **Their syntax must not be retrofitted into
Cx.** Read them for intent — what the feature is for, what shapes it needs to
express — and discard the notation entirely. Deliverables 2, 3 and 4 above exist
precisely because the syntax is genuinely undecided, not because it is written
down somewhere and merely needs transcribing.

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

**2026-05-18:** PR #268 merged `train/backend-determinism` → submain (host_boundary expansion, IR lowering fixes, 23 new parity fixtures including CX-228 t159–t177). CX-233 implements while-in loop source-to-IR lowering on `stokowski/CX-233` (branch-local, not yet merged) — WhileLoop parity moves to 8/0. Submain 171 commits ahead of main.

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
