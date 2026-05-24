# Pillar 3 — Architecture Risks for the 0.2 Roadmap

**Audit base:** `submain` @ `22015c420ad40483d0531d1109fa9837d44deca3`. **Date:** 2026-05-24.

This is the subset of `report/architecture.md` that is **expensive to fix later and cheap(er) to fix now** — the items where every month of new code makes the eventual fix harder. These should shape the 0.2 roadmap. Ordered by the product of (blast radius) × (cost-of-delay).

---

## R1 — No shared semantic core between interpreter and JIT
**(architecture.md T1.1 · correctness.md Cat. A · the #1 risk)**

**Why it's expensive later:** today there are ~120 constructs implemented twice. Every feature added to 0.2–1.0 doubles its own implementation and adds a parity fixture. The drift surface — and the cost of any semantics change (you must edit two engines and reconcile them) — grows with the language. The 13 Pillar-2 disagreements are not 13 bugs; they are 13 symptoms of one missing abstraction.

**Cost of acting now vs. later:** the semantics that need unifying (arithmetic guards, casts, bounds, builtins) are still small enough to consolidate in one pass. After 0.2 adds generics/traits/more numerics, consolidation means reconciling two much larger, more divergent implementations.

**Decision for the roadmap:** pick the target shape now — shared operation-semantics module (incremental) vs. interpreter-on-IR (bigger, eliminates the tree-walker). Even committing to the *direction* lets every new 0.2 feature be built once.

## R2 — Builtin definitions scattered across four modules
**(architecture.md T1.2)**

**Why it's expensive later:** the builtin set will grow (math, string ops, collections). Each new builtin currently requires correct edits in `runtime.rs`, `semantic.rs`, `ir/validate.rs`, and `ir/lower.rs`; a miss is a silent backend disagreement that only a parity fixture catches. N builtins × 4 sites × every contributor = a steady stream of T1.1-style drift.

**Cost of acting now:** there are ~9 builtins. A registry refactor now is small and immediately pays for itself on the next builtin. Later it's a migration of dozens of call sites.

**Decision:** introduce a single builtin registry before 0.2 adds builtins. Low effort, high leverage; partially de-risks R1.

## R3 — The interpreter discards computed binding resolution
**(architecture.md T1.4 · performance.md §4)**

**Why it's expensive later:** this one is *cheap now and stays cheap* — but it's load-bearing for performance and for R1. The `BindingId` already exists in `SemanticProgram`; the interpreter ignores it and pays SipHash-per-access (~26% of loop-code instructions). Slot-indexed locals are the right runtime representation and the JIT will want the same indices. Doing it now means the runtime's variable model is correct before more of the interpreter is built on `get_var(name)`.

**Decision:** wire `BindingId` into a `Vec`-indexed local store in 0.2. Reclassifies Pillar-1 recommendation #2 from "new subsystem" to "use existing data." Pairs naturally with R5.

## R4 — Quadruple numeric type representation
**(architecture.md T1.3)**

**Why it's expensive later:** every new numeric type or conversion rule must be added to `ast::Type`, `SemanticType`, `IrType`, and the Cranelift mapping, plus the three conversion functions between them. 0.2 numeric work (more widths, unsigned, fixed-point for an engine language?) multiplies the edit fan-out and the drift points.

**Cost of acting now:** unifying to one numeric enum is a mechanical but wide change — easier across four representations than across four-plus-new-features. The AST/Semantic *structural* split should stay (it earns its keep); only the type spelling needs unifying.

**Decision:** unify the numeric type enum in 0.2, ideally before adding numeric types.

## R5 — `runtime.rs` monolith
**(architecture.md T2.1)**

**Why it's expensive later:** at 1,955 lines with 200–450-line methods, every runtime feature makes the hot functions longer and the file harder to test in isolation. The split seams (env / eval / exec / ops / builtins / format) are clean *now*; they blur as features cross-cut.

**Cost of acting now:** a structural split with no behaviour change is low-risk today and compounds with R2 (builtins module) and R3 (env module). Deferred, it becomes a split of a 3,000+ line file with more entanglement.

**Decision:** split in early 0.2, as the substrate for R2/R3.

---

## Lower-urgency (track, don't necessarily schedule)
- **Layering inversion** (`Value`/`RuntimeError` in `frontend/`) — architecture.md T2.2. Cheap to fix; do it alongside R5.
- **Eager per-scope 64 KB arena** — architecture.md T3.1 / performance.md §3. *Performance* fix is urgent (Pillar-1 #1) and independent of this risk list; the *architectural* point (arena coupled to scope) can ride along.
- **Coarse `RuntimeError` enum** forcing faked messages — architecture.md T2.4 / correctness.md D1. Fix with the error-message rewrites.
- **`Bool`/`TBool`/`Unknown` trichotomy** — architecture.md T3.2. Revisit if 0.2 expands three-valued logic.
- **`allow(dead_code)` / speculative API**, **34 panic sites**, **import dead-symbol elimination** — architecture.md T3.3–T3.5. Hygiene; address opportunistically.

## Suggested 0.2 sequencing
A coherent, low-regret order that lets each step lean on the previous:
1. **R5** (split `runtime.rs`) + move `Value`/`RuntimeError` to `runtime/` — creates the modules the rest live in.
2. **R2** (builtin registry) — into the new `builtins` module; de-risks R1.
3. **R3** (binding-indexed locals) — into the new `env` module; also the Pillar-1 perf win.
4. **R1 direction decision** (shared semantics vs. interpreter-on-IR) — the big one; everything above narrows its scope.
5. **R4** (unify numeric types) — mechanical, schedulable independently, ideally before new numeric features.

The Pillar-1 performance fixes (lazy arena, FxHash interim) are orthogonal and can land immediately, in parallel.
