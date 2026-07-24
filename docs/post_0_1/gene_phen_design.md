# Cx Trait System — gene / phen

**v1.1 — 2026-07-12 (updated from v1.0, 2026-05-19)**
**Status: design locked. Design phase complete — all six implementation questions closed: each is either fully decided for 0.3.4, or explicitly deferred with a named extension point for infrastructure that doesn't exist in Cx yet (packages, re-exports, specialization). Roadmap placement: 0.3.3 = design verification/finalization, 0.3.4 = implementation.**

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
- `Self` inside a gene/phen resolves to the concrete type the phen is bound to, throughout the entire phen body (item 3 below).
- The unknown arm in any `when` over a gene method result uses the canonical spelling `unknown` — not `0.0.0` or any other form. This matches the standing R6/R7-era conventions from the rest of the language.
- **Operator overloading is implemented as genes.** `+`, `-`, `*`, `/`, `%`, unary negation, equality, and ordering are each backed by a dedicated operator gene (Add, Sub, Mul, Div, Mod, Neg, Eq, Ord). Overloading a type's `+` operator means writing a phen that binds the Add gene to that type. There is no separate operator-overloading syntax — it is gene/phen end to end.
- **Method ownership on a concrete type — locked principle.** For a given concrete type, a method name resolves to exactly one implementation. Uniqueness is enforced deterministically before execution. No silent precedence, no "closest" implementation, no declaration-order winner, no import-order winner. Future explicit qualified-call syntax may relax this rule; today, ambiguity is an error. Three applications: (1) a phen may not declare methods beyond its gene's contract — extras are rejected; extra methods belong in `impl` blocks; (2) an `impl` method and a phen method with the same name on one type is a collision, rejected at analysis time, neither side winning, diagnostic naming both definitions; (3) two phens (of different genes) supplying the same method name to one concrete type is a collision, rejected at registration/analysis time, diagnostic naming both phens and their source locations.

---

## What This Unblocks

Nothing in the stdlib work can begin until this lands. Specifically:

- **Operator overloading** — directly implemented via the operator gene set above.
- **Generics v3 type bounds** — `T: Numeric`, `T: Damageable`, and all bounded-generic syntax depend on the phen-lookup machinery.
- **The entire stdlib** — every typed collection (hashset, hashmap, hashweb, Vec\<T\>) needs bounded generics to be safe and ergonomic, and bounded generics need gene/phen.

This is why the roadmap sequences gene/phen as its own dedicated design-then-implementation pair (0.3.3 design, 0.3.4 implementation) rather than bundling it as a feature inside a larger version. It is the single highest-leverage item in the post-0.3 language work — everything downstream is blocked on it.

---

## Implementation Questions — Status

These are not design questions. The design above is locked. These are implementation decisions, originally scoped for what was called the "A1/A2 sprint" in earlier planning. All six are now closed: each is fully DECIDED for 0.3.4, or explicitly deferred with a named extension point for infrastructure that doesn't exist in Cx yet — never left ambiguous.

### 1. Dispatch strategy — DECIDED
Static monomorphization only for 0.3.4. No dynamic dispatch in this release — but not permanently rejected. Deferred until a concrete use case emerges that justifies designing trait-object semantics, object-safety rules, ABI implications, ownership interactions, and performance tradeoffs properly, not speculatively. Every concrete gene/type combination compiles to a specialized implementation with no runtime dispatch table or hidden indirection.

### 2. Phen lookup — DECIDED
Forward-reference capable, matching how functions already work — not declaration-before-use, matching how methods currently work. A phen must be discoverable regardless of where its declaration appears in the file or module; source order must never determine whether a valid gene implementation exists. Mechanism: a semantic pre-pass registers phen headers (signatures and gene/type-binding identity only, not bodies) before any method-call resolution, operator resolution, or generic-bound checking happens — mirroring the existing struct/enum/function pre-pass pattern, not `method_registry`'s in-main-pass, declaration-order-sensitive pattern. Bodies are still analyzed in the normal semantic pass, after the pre-pass has registered all headers. The pre-pass must detect duplicate or conflicting phen implementations deterministically. Rationale against declaration-before-use: it would create fragile, confusing behavior where moving an otherwise-unchanged phen block changes whether code compiles.

### 3. Self resolution — DECIDED
`Self` resolves throughout the entire `phen` body, not only during gene-contract checking. `Self` is a compile-time alias for the concrete implementing type, substituted during semantic analysis using the existing type-substitution machinery (the same mechanism already used for generic-struct type-parameter substitution). `Self` must resolve correctly in: method signatures; parameter and return types; local type annotations; associated function or constant access (e.g. `Self::new`); nested generic type expressions. No runtime lookup or dynamic behavior is involved — this is pure compile-time substitution. Rationale: forcing users to repeat a potentially long concrete type name throughout every phen body, just to minimize initial implementation scope, is the wrong tradeoff — `Self` has one clear, consistent meaning everywhere inside a phen.

### 4. Operator gene mapping — DECIDED (for `Ord` specifically; other operators still follow the same 1:1 token-to-method pattern)
Four separate methods for 0.3.4: `lt`, `gt`, `le`, `ge`, mapping directly to `<`, `>`, `<=`, `>=`. No `cmp`/`Ordering`-result-type convenience method in 0.3.4 — avoids introducing an unfinished ordering type for this release. The semantic and lowering path must be designed so a `cmp` method returning an `Ordering`-style type can be added later as a convenience without breaking the four operators or existing implementations. The required consistency laws between the four methods (standard total-order axioms) must be documented as an implementer requirement — no runtime enforcement of these laws in 0.3.4.

### 5. Coherence — DECIDED
Two phens sharing the same canonical key — `(gene_name, receiver_type)` — anywhere in the complete reachable module graph is a hard compile error, with no exceptions today (no specialization/blanket-phen system exists to justify an exception — see item 14's extension point below).

**Registration pass.** A dedicated, whole-graph gene/phen collection pass runs once, before the existing per-file enum/struct/function pre-passes and before any per-file analysis begins — see "Semantic-Pass Ordering" below for the exact sequence. This pass scans every file already present in the resolver's `ResolvedProgram.files` (the complete reachable module graph, already fully assembled by the existing `resolver.rs` before semantic analysis is ever invoked — no new graph-traversal machinery needed) for top-level `gene`/`phen` declarations, and populates one registry shared across the whole compilation, keyed on the canonical identity below. This registry is a new, distinct data structure — a per-file `Analyzer` instance's local tables (`structs`, `funcs`, etc.) are deliberately reset per file (each file gets its own fresh `Analyzer::new()`, `semantic.rs:2826`) and cannot hold something that must be visible and checked across the whole graph; the gene/phen registry instead lives at `analyze_resolved_program`'s own top level, alongside (but structurally separate from) the existing `alias_exports` accumulator (`semantic.rs:2808`).

**Canonical key.** `(gene_name: String, receiver_type: SemanticType)`. A phen's receiver is always a single concrete type (never left with an unresolved generic parameter, per the Locked Rules), so no generic-argument component is needed. Cx has no associated-types or where-constraints concept today (confirmed absent from the entire frontend), so the key needs no further refinement beyond gene name + concrete receiver type. One known, honest limitation: `SemanticType::Struct` (`semantic_types.rs:30`) carries only a bare name, not generic arguments — so two phens for two different concrete instantiations of the same generic struct (e.g. a future `Pair<t32>` vs. `Pair<f64>`) would collide under today's type representation, since both currently resolve to the identical `SemanticType::Struct("Pair")`. This is a real gap, but it belongs to generics v3 (the type representation itself needs to carry instantiation arguments before the phen key could distinguish them) — not something gene/phen's own design should solve.

**Exact duplicate vs. conflicting overlap.** With today's feature set these are the same thing: since a phen names one concrete type with no notion of partial or blanket coverage, two phens either share the identical key (rejected, regardless of whether their bodies are byte-identical or different — the check is on the key, never on body content) or they don't overlap at all. A genuine "conflicting-but-not-identical" category — the same key partially satisfied by two different, overlapping implementations, as opposed to a flat duplicate — only becomes a real, distinct question once specialization/blanket phens exist (item 14's extension point).

**Duplicates are rejected, never silently deduplicated.** This matches every existing precedent in Cx without exception — duplicate import alias (`semantic.rs:918`), duplicate loop label (`semantic.rs:271`), variable already declared (`semantic.rs:146`) are all hard compile errors, never a silent last-write-wins. Silent deduplication would also be actively dangerous here specifically, since two differently-buggy phen bodies sharing a key could silently pick the wrong one.

**Order-independence.** Because collision detection is a single flat scan over the whole reachable file set (not an incremental, order-sensitive walk), it is inherently independent of source-file order, import order, or topological position — a duplicate is caught the same way whether the two phens are in the same file, in a file and the one that imports it, or in two files with no import relationship to each other at all (as long as both are reachable from the entry point).

**Diagnostic shape.** Cx's existing `SemanticError { msg: String, pos: usize }` (`semantic.rs:8-11`) carries exactly one position — there is no existing two-location diagnostic anywhere in the codebase to reuse. The second location is therefore encoded as text inside the message itself (necessary for a cross-file collision, since a raw byte offset has no meaning across files), anchored at the position of whichever phen is encountered second in a stable, deterministic order (by `ModuleId` — already a stable integer assigned by the existing resolver — then by byte offset within that file). Exact drafted message, matching the project's established terse, single-quoted, name-first convention (`"duplicate import alias '{}'"`, `"variable already declared in this scope: {}"`):

```
gene '{gene}' already implemented for '{type}' — conflicting phen at {other_file}:{other_line}
```

**Re-exports — not applicable.** Confirmed via investigation: no `pub use`-equivalent or re-export concept exists anywhere in Cx's lexer, parser, AST, resolver, or semantic layer today (grep across all four returned zero matches). Deferred until Cx has a re-export mechanism. Extension point, when that lands: a re-export must expose an existing registration under a new path/alias — it must never create or duplicate one.

**Cyclic imports — a non-issue by construction.** Confirmed both by reading `resolver.rs:114-122` and by empirical reproduction: a cyclic import (file A imports file B imports file A) is hard-rejected by the existing resolver (`ResolveError::CircularImport`) at import-resolution time, before semantic analysis — and therefore before the gene/phen registration pass — ever begins. A cyclic-import scenario never reaches phen registration at all; the whole-graph registration pass can safely assume it only ever runs against an already-guaranteed-acyclic file set.

**Separate packages/dependencies — not applicable.** Confirmed via investigation (and empirical reproduction against `t70_macro_imports_registry_reject.cx`): Cx has no functional package or external-dependency concept today. `std/`-prefixed and non-relative "registry" import paths are recognized syntax but explicitly rejected (`ResolveError::RegistryNotSupported`, "stdlib not bundled in v0.1") — every reachable file today is part of one single compilation unit. Deferred until Cx has a real package system. Extension point: whether phen coherence checking should span package boundaries (a Rust-style orphan rule allows two independent crates to each safely implement the same trait for the same type without a cross-crate conflict, since they're never compiled together) is a genuinely new question for that future system — not decidable from Cx's current single-unit architecture.

**Orphan/coherence restrictions — explicitly deferred, recommend not implementing now.** An orphan rule (restricting who may implement a gene for a type — e.g. requiring the phen live in the same file/module as either the gene or the type) exists in languages like Rust specifically to prevent two independently-compiled, mutually-invisible crates from silently defining conflicting implementations. Cx has no packages, no separately-compiled units, no external dependencies today (see above) — every reachable file is checked together, in one pass, by the whole-graph registration mechanism this design specifies. That mechanism already catches every possible (gene, type) collision deterministically, regardless of which file each phen lives in — an orphan rule would add no additional safety in Cx's current single-compilation-unit world. Recommend deferring until Cx has genuinely independent, separately-versioned packages; revisit then.

**Specialization/blanket phens — extension point only, not designed here.** Out of scope per explicit instruction. The first thing that would need to change if either is added later: today's collision check is a flat key-equality test (`(gene, type)` matches or it doesn't); specialization requires a genuine overlap check instead (do two implementations' domains intersect for any concrete type, not just match identically) — a materially harder algorithm, well-known to be nontrivial in real trait systems (e.g. Rust's own coherence/specialization work). Not attempted speculatively now.

### 6. Built-in gene provenance — DECIDED
Prelude source is the canonical, permanent architecture for built-in operator genes (`Add`, `Sub`, `Mul`, `Div`, `Eq`, `Ord`, etc.) — never a permanent second hardcoded system. For 0.3.4 bootstrapping specifically, the compiler may recognize narrowly-scoped primitive intrinsics for built-in numeric implementations if the prelude cannot yet fully compile through the ordinary gene/phen path, under five hard constraints: (1) public contracts and method names must come from the prelude, never compiler-invented; (2) primitive intrinsics must conform exactly to those prelude-defined contracts; (3) user-defined types must always use normal gene/phen resolution, never the bootstrap shortcut; (4) no user-visible semantic behavior may exist only in compiler magic — everything an intrinsic does must be expressible as real gene/phen source; (5) the bootstrap path must be clearly marked, tested, and designed for removal once self-hosted prelude resolution works. Architecture is prelude-first with temporary primitive bootstrap support — not permanently hardcoded operator genes.

---

## Cross-Module Coherence — Reference

Supporting detail for item 5 above: the exact pass ordering, canonical key, worked examples, and the test matrix 0.3.4 implementation will need fixtures for.

### Semantic-Pass Ordering (complete, gene/phen integrated)

```
Once, before any per-file processing:
  0. Gene/phen collection pass — scans EVERY file in resolved.files (the complete
     reachable module graph, already assembled by the existing resolver.rs before
     semantic analysis begins), order-independent:
       0a. Register every gene's name + full signature list.
       0b. Register every phen's (gene_name, receiver_type) canonical key → its
           declaration location.
       0c. Reject any duplicate canonical key found anywhere in the whole graph,
           deterministically, regardless of file/import order.

Then, per file, in resolved.topo_order (dependency order, as today):
  1. Enum pre-pass       (existing, unchanged — semantic.rs:2829-2844)
  2. Struct pre-pass     (existing, unchanged — semantic.rs:2846-2852)
  3. Function pre-pass   (existing, unchanged — semantic.rs:2854-2868)
  4. Main analysis pass  (existing, extended — semantic.rs:2870-2882): walks every
     statement — including phen bodies — with the shared gene/phen registry from
     step 0 available (alongside the existing module_aliases) to:
       - verify a phen's body fully implements every signature its gene declares
         (full conformance; step 0 only checked identity/headers, not bodies)
       - resolve `T: GeneName` bounds at generic call/instantiation sites
       - resolve operator expressions (`a + b`) to the correct phen method
```

### Canonical Phen-Registration Key

```
(gene_name: String, receiver_type: SemanticType)
```

Mirrors `method_registry`'s existing `(String, String)` composite-key shape (`semantic.rs:62`), using the fully-resolved `SemanticType` for the receiver rather than a raw AST type name.

### Concrete Examples

The two invalid examples below both reduce to the same rule (see item 5: "exact duplicate" and "conflicting overlap" are the same category with today's feature set) — one shows byte-identical bodies, the other shows different (here, contradictory) bodies sharing the same key. Both are rejected identically, since the check is on the key, not the body.

**Valid — multi-gene, same type (single file):**
```cx
gene Add {
    fnc add(rhs: Self) -> Self
}

gene Eq {
    fnc eq(rhs: Self) -> bool
}

phen Add (v: Vector3) {
    fnc add(rhs: Vector3) -> Vector3 {
        Vector3 { x: v.x + rhs.x, y: v.y + rhs.y, z: v.z + rhs.z }
    }
}

phen Eq (v: Vector3) {
    fnc eq(rhs: Vector3) -> bool {
        v.x == rhs.x && v.y == rhs.y && v.z == rhs.z
    }
}
```
Valid: different genes (`Add`, `Eq`), same type. Keys `(Add, Vector3)` and `(Eq, Vector3)` are distinct.

**Valid — multi-type, same gene, across two files:**
```cx
// file: player.cx
struct Player { health: t32 }

gene Damageable {
    fnc take_damage(amount: t32)
    fnc is_alive() -> bool
}

phen Damageable (p: Player) {
    fnc take_damage(amount: t32) { p.health -= amount }
    fnc is_alive() -> bool { p.health > 0 }
}
```
```cx
// file: enemy.cx (also reachable from the same entry point)
struct Enemy { shield: t32 }

phen Damageable (e: Enemy) {
    fnc take_damage(amount: t32) { e.shield -= amount }
    fnc is_alive() -> bool { e.shield > 0 }
}
```
Valid: same gene (`Damageable`), different types, across two files with no import relationship to each other. Keys `(Damageable, Player)` and `(Damageable, Enemy)` are distinct.

**Invalid — exact duplicate (same file):**
```cx
struct Vector3 { x: t32, y: t32, z: t32 }

phen Add (v: Vector3) {
    fnc add(rhs: Vector3) -> Vector3 {
        Vector3 { x: v.x + rhs.x, y: v.y + rhs.y, z: v.z + rhs.z }
    }
}

phen Add (v: Vector3) {
    fnc add(rhs: Vector3) -> Vector3 {
        Vector3 { x: v.x + rhs.x, y: v.y + rhs.y, z: v.z + rhs.z }
    }
}
```
Rejected: two phens with the identical key `(Add, Vector3)` in the same file.

**Invalid — conflicting overlap (different bodies, same key, across two files):**
```cx
// file: physics_a.cx
phen Add (v: Vector3) {
    fnc add(rhs: Vector3) -> Vector3 {
        Vector3 { x: v.x + rhs.x, y: v.y + rhs.y, z: v.z + rhs.z }
    }
}
```
```cx
// file: physics_b.cx (also reachable from the same entry point)
phen Add (v: Vector3) {
    fnc add(rhs: Vector3) -> Vector3 {
        Vector3 { x: v.x - rhs.x, y: v.y - rhs.y, z: v.z - rhs.z }
    }
}
```
Rejected: same key `(Add, Vector3)` found across two files. The bodies differ, but that makes no difference to the check — Cx has no mechanism to decide which implementation should win, so a differing body is exactly as invalid as an identical one.

### Collision Diagnostic

```
gene '{gene}' already implemented for '{type}' — conflicting phen at {other_file}:{other_line}
```

### Required Test Matrix (for 0.3.4 implementation fixtures)

- Same-file, same-type/same-gene exact duplicate → rejected (invalid example 1 above).
- Cross-file duplicate (same key, two files, no import relationship) → rejected (invalid example 2 above).
- Conflicting overlap (different bodies, same key) → rejected identically to a duplicate — confirms the "check is on the key, not the body" rule.
- Valid multi-gene, same type → accepted (valid example 1 above).
- Valid multi-type, same gene → accepted (valid example 2 above).
- Cyclic-import scenario with phens in the cycle → the existing resolver's `CircularImport` rejection fires first, before phen registration ever runs — confirms phens don't change or bypass existing cyclic-import behavior.
- Re-export scenario → not applicable; no fixture needed until Cx has a re-export mechanism.
- Forward-reference ordering: a phen declared lexically after the code that uses it as a generic bound or via operator resolution (same file, and across files in either topo-order direction) → resolves correctly, confirming item 2's forward-reference-capable decision holds for coherence-checked code too.

---

## The Prelude (0.3.4 slice 6 — implemented)

- **Canonical source**: `src/prelude.cx` — a real Cx file in the repo, the
  single source of truth for the eight operator gene contracts
  (`Add`/`Sub`/`Mul`/`Div`/`Mod`/`Neg`/`Eq`/`Ord`, decision 4's four-method
  `Ord`, single-method `Eq` with `!=` derived as the logical NOT of `eq`).
- **Embedding**: compiled into `cx.exe` via `include_str!` — the embedded
  text is the sole runtime path; no filesystem probing, no fallback. Prelude
  changes require rebuilding `cx.exe` (accepted by ruling).
- **Injection**: exactly once, as the first source unit of every flattened
  program — before the root file and all imports — through the ordinary
  lexer → parser → semantic pipeline. Diagnostics into the prelude name it
  as `<prelude>` with real line numbers; user redeclaration of a prelude
  gene goes through the same duplicate-gene collision machinery as any
  other collision, naming both locations.
- **Primitives** satisfy the operator contracts through decision-6 bootstrap
  intrinsics: the built-in numeric semantic paths, marked at their sites,
  conforming to the prelude signatures, designed for removal once primitives
  can carry real phens. (Known bootstrap gap, tracked: primitives do not yet
  satisfy `T: Add`-style generic bounds, since bound checks consult the phen
  registry and intrinsics are not registry entries.)

### Interim restriction — operator dispatch requires a named variable

Operator-gene dispatch rewrites `a + b` to the same method-call node a
hand-written `a.add(b)` produces, and Cx's method machinery is name-based
end to end (runtime receiver lookup and mutation write-back both key on the
variable name). Until expression receivers exist, the LEFT operand of a
gene-dispatched operator must be a named variable: `v1 + v2` works;
`(v1 + v2) + v3` is a clean, explicit error. This is an interim restriction,
not a design position — it lifts when method receivers become expressions.

---

## Roadmap Placement (current, per merged master roadmap)

- **0.3.2** — Pattern matching (immediately prior; gene/phen's `when`-over-gene-method-result convention depends on pattern matching being settled first)
- **0.3.3** — Type System v3, design phase. Gene/phen design verification, generics v3 design, constraint system design, operator overloading design — all as one coherent type-system design pass, since they are deeply entangled (gene/phen literally is how operator overloading and bounded generics work).
- **0.3.4** — Type System v3, implementation phase. Gene/phen build, operator overloading build (mechanical once gene/phen core exists), generics v3 build.

The framing from the merged roadmap stands: gene/phen is "too weird and powerful to design in isolation" — it should not be finalized as a standalone item separate from generics v3 and the constraint system, because all three answer overlapping questions about how Cx's type system decides what a type can do.

---

## Immediate Next Step

Design phase complete: all six implementation questions are closed (fully decided, or explicitly deferred with a named extension point). The next move is the 0.3.4 implementation build itself — this document is the source of truth for that build, including the semantic-pass ordering, canonical registration key, collision/diagnostic rules, and required test matrix under "Cross-Module Coherence — Reference" above.

---

*Cx Language Reference · Trait System · gene / phen · v1.1*
