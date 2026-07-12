# Cx Trait System — gene / phen

**v1.1 — 2026-07-12 (updated from v1.0, 2026-05-19)**
**Status: design locked. Roadmap placement: 0.3.3 = design verification/finalization, 0.3.4 = implementation.**

---

## Why This Exists

Cx needs a way to express "any type that can do X" without inheritance, without vtables-by-default, and without the borrow-checker machinery that makes Rust traits heavy. The gene/phen system splits a trait into two halves that most languages fuse together:

- A **gene** is a contract. Pure signatures. It knows about no specific type.
- A **phen** is the expression of a gene for one concrete type.

The names come from biology: a gene is the abstract instruction; a phenotype is how that instruction expresses in a specific organism. The same gene expresses differently across organisms. The same Cx gene expresses differently across types.

This split is a language identity feature. It defines operator overloading, bounded polymorphism, and the entire stdlib. Nothing in the post-0.3 stdlib work can begin until this is implemented — every typed collection needs bounded generics to be safe and ergonomic, and bounded generics need gene/phen to exist.

---

## The Core Model

### gene — the contract

A gene is type-agnostic. It declares signatures only. No bodies, no fields, no type parameter. A gene never names a concrete type.

```cx
gene Damageable {
    fnc take_damage(amount: t32)
    fnc is_alive() -> bool
}
```

That is the entire contract. `Damageable` does not know what a `Player` is. It does not know what an `Enemy` is. It only knows that anything claiming to be `Damageable` must provide these two functions.

### phen — the expression

A phen binds exactly one gene to exactly one concrete type and supplies all bodies. The type appears here, never in the gene.

```cx
phen Damageable (p: Player) {
    fnc take_damage(amount: t32) {
        p.health -= amount
    }
    fnc is_alive() -> bool {
        p.health > 0
    }
}

phen Damageable (e: Enemy) {
    fnc take_damage(amount: t32) {
        e.shield -= amount
    }
    fnc is_alive() -> bool {
        e.hull > 0
    }
}
```

Same gene. Two unrelated types. Two completely different implementations. The gene never needed to know either type existed.

---

## Locked Rules

These are settled. Not open questions.

- A gene never names a concrete type.
- A phen binds exactly one gene to exactly one concrete type and supplies all bodies.
- A phen must implement every signature in the gene. Missing a signature is a compile error.
- A type may express many genes — many phens, one per gene, on the same type.
- A gene may be expressed by many types — many phens, one per type, for the same gene.
- `T: GeneName` is a generic bound, satisfied only if a phen exists binding `GeneName` to the concrete type substituted for `T`.
- Multi-bound generics are supported: `T: GeneA + GeneB` requires phens for both genes on the same type.
- `Self` inside a gene/phen resolves to the concrete type the phen is bound to. *(Exact resolution point in the semantic pass was the "A2" open item — see status below.)*
- The unknown arm in any `when` over a gene method result uses the canonical spelling `unknown` — not `0.0.0` or any other form. This matches the standing R6/R7-era conventions from the rest of the language.
- **Operator overloading is implemented as genes.** `+`, `-`, `*`, `/`, `%`, unary negation, equality, and ordering are each backed by a dedicated operator gene (Add, Sub, Mul, Div, Mod, Neg, Eq, Ord). Overloading a type's `+` operator means writing a phen that binds the Add gene to that type. There is no separate operator-overloading syntax — it is gene/phen end to end.

---

## What This Unblocks

Nothing in the stdlib work can begin until this lands. Specifically:

- **Operator overloading** — directly implemented via the operator gene set above.
- **Generics v3 type bounds** — `T: Numeric`, `T: Damageable`, and all bounded-generic syntax depend on the phen-lookup machinery.
- **The entire stdlib** — every typed collection (hashset, hashmap, hashweb, Vec\<T\>) needs bounded generics to be safe and ergonomic, and bounded generics need gene/phen.

This is why the roadmap sequences gene/phen as its own dedicated design-then-implementation pair (0.3.3 design, 0.3.4 implementation) rather than bundling it as a feature inside a larger version. It is the single highest-leverage item in the post-0.3 language work — everything downstream is blocked on it.

---

## Implementation Questions — Status

These are not design questions. The design above is locked. These are implementation decisions, originally scoped for what was called the "A1/A2 sprint" in earlier planning. 3 of 6 are now decided; 3 remain open: Self resolution and phen lookup are both pending a quick check of Cx's existing generics/declaration-registration patterns before deciding, and coherence is pending phen lookup's resolution, since cross-module duplicate detection likely falls out of that table's design.

### 1. Dispatch strategy — DECIDED
Static monomorphization only for 0.3.4. No dynamic dispatch in this release — but not permanently rejected. Deferred until a concrete use case emerges that justifies designing trait-object semantics, object-safety rules, ABI implications, ownership interactions, and performance tradeoffs properly, not speculatively. Every concrete gene/type combination compiles to a specialized implementation with no runtime dispatch table or hidden indirection.

### 2. Phen lookup — OPEN
How the semantic pass resolves "does type X have a phen for gene G." Proposed mechanism: a `(GeneId, TypeId) → PhenId` table populated during the collection pass, before the main semantic analysis walks the rest of the program. This needs to be locked before implementation starts since it's the core data structure the rest of gene/phen sits on.

### 3. Self resolution — OPEN (this was the specific "A2" blocker)
Where in the semantic pass `Self` gets substituted with the concrete bound type, and how that interacts with the existing generics machinery. This was flagged as deferred to the implementation sprint in the most recent verified status and, as of the last check, was still the single open item blocking a clean start on 0.3.4. **This should be the first thing resolved when the 0.3.3 design-verification pass begins** — it is small in scope but load-bearing, since every phen body relies on `Self` resolving correctly.

### 4. Operator gene mapping — DECIDED (for `Ord` specifically; other operators still follow the same 1:1 token-to-method pattern)
Four separate methods for 0.3.4: `lt`, `gt`, `le`, `ge`, mapping directly to `<`, `>`, `<=`, `>=`. No `cmp`/`Ordering`-result-type convenience method in 0.3.4 — avoids introducing an unfinished ordering type for this release. The semantic and lowering path must be designed so a `cmp` method returning an `Ordering`-style type can be added later as a convenience without breaking the four operators or existing implementations. The required consistency laws between the four methods (standard total-order axioms) must be documented as an implementer requirement — no runtime enforcement of these laws in 0.3.4.

### 5. Coherence — OPEN
Two phens binding the same gene to the same type must be a compile error. The open question is how cross-module phen collisions get detected by the resolver — a phen for `Damageable` on `Player` written in one module and another for the same pair written in a different module both need to be caught, not just same-file duplicates.

### 6. Built-in gene provenance — DECIDED
Prelude source is the canonical, permanent architecture for built-in operator genes (`Add`, `Sub`, `Mul`, `Div`, `Eq`, `Ord`, etc.) — never a permanent second hardcoded system. For 0.3.4 bootstrapping specifically, the compiler may recognize narrowly-scoped primitive intrinsics for built-in numeric implementations if the prelude cannot yet fully compile through the ordinary gene/phen path, under five hard constraints: (1) public contracts and method names must come from the prelude, never compiler-invented; (2) primitive intrinsics must conform exactly to those prelude-defined contracts; (3) user-defined types must always use normal gene/phen resolution, never the bootstrap shortcut; (4) no user-visible semantic behavior may exist only in compiler magic — everything an intrinsic does must be expressible as real gene/phen source; (5) the bootstrap path must be clearly marked, tested, and designed for removal once self-hosted prelude resolution works. Architecture is prelude-first with temporary primitive bootstrap support — not permanently hardcoded operator genes.

---

## Roadmap Placement (current, per merged master roadmap)

- **0.3.2** — Pattern matching (immediately prior; gene/phen's `when`-over-gene-method-result convention depends on pattern matching being settled first)
- **0.3.3** — Type System v3, design phase. Gene/phen design verification, generics v3 design, constraint system design, operator overloading design — all as one coherent type-system design pass, since they are deeply entangled (gene/phen literally is how operator overloading and bounded generics work).
- **0.3.4** — Type System v3, implementation phase. Gene/phen build, operator overloading build (mechanical once gene/phen core exists), generics v3 build.

The framing from the merged roadmap stands: gene/phen is "too weird and powerful to design in isolation" — it should not be finalized as a standalone item separate from generics v3 and the constraint system, because all three answer overlapping questions about how Cx's type system decides what a type can do.

---

## Immediate Next Step

Before 0.3.3 design work begins in earnest, the design doc itself needs a verification pass: confirm `docs/post_0_1/gene_phen_design.md` (or wherever it currently lives in the repo) still matches what's recorded here, and resolve the Self-resolution question specifically, since it was the one item flagged as still blocking a clean start every time this got checked. A short recon-and-report dispatch to an agent — read the current file, diff it against this document, flag any drift — is the natural next move before the full 0.3.3 design pass is scheduled.

---

*Cx Language Reference · Trait System · gene / phen · v1.1*
