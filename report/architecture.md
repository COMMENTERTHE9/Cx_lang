# Pillar 3 — Architecture: Where the Design Will Hurt Later

**Audit base:** `submain` @ `22015c420ad40483d0531d1109fa9837d44deca3`.
**Date:** 2026-05-24.
**Stance:** read as a senior reviewer who didn't write the code. Concerns are ranked by **how much they'll hurt as the language grows**, not by how they look today. Several findings from Pillars 1 and 2 are *architecture-shaped* and are cited here rather than re-investigated.
**Note on ownership:** the two largest files — `src/backend/cranelift/host_boundary.rs` (8,944 lines) and `src/ir/lower.rs` (8,019 lines) — are backend/IR territory. They are cited as read-only observations and flagged for the backend dev, not modified.

---

## Tier 1 — foundational; expensive once more code piles on top

### T1.1 — Two execution engines with no shared semantic core *(the central risk)*
The interpreter tree-walks `SemanticProgram` directly (`src/runtime/runtime.rs`), while the JIT lowers `SemanticProgram → IrModule → Cranelift` (`src/ir/lower.rs`, `src/backend/cranelift/host_boundary.rs`). **Below `SemanticProgram` they share nothing.** Every operation's semantics — overflow/wrapping, division guards, bounds checks, float→int casts, builtins — is implemented twice, independently.

This is the structural cause of the entire Pillar-2 disagreement catalog (13 findings): the interpreter guards `INT_MIN/-1` and div-by-zero and bounds; the JIT emits raw `sdiv`/loads that trap or read garbage. There is no single place that says "this is what `/` means in Cx," so the two engines drift, and the differential harness can only catch drift on the ~120 fixtures both can run.

- **Cost trajectory:** every new operation or type is a double implementation plus a parity fixture, forever. The drift surface grows monotonically.
- **Direction:** establish one semantic specification. Options, cheapest first: (a) a shared operation-semantics module (the guard/wrap/saturate rules) called by *both* the interpreter and the lowering; (b) make the interpreter consume `IrModule` too, so there is one lowering and the tree-walker disappears; (c) at minimum, a shared builtin/operation table (see T1.2). Anything is better than two hand-maintained copies.
- See also: `report/correctness.md` Category A.

### T1.2 — Builtins hardcoded as string literals in four modules
The set of builtins is duplicated, as bare string comparisons, with **no single source of truth**:
- `src/runtime/runtime.rs:1282-1417` — interpreter dispatch (`is_known`, `exit`, `print`, `println`, `printn`, `assert`, `assert_eq`, `read`, `input`).
- `src/frontend/semantic.rs:1615` — semantic recognition set.
- `src/ir/validate.rs:128` — validator set (`assert`/`assert_eq`/`is_known`/`cx_printn`).
- `src/ir/lower.rs:95` (`is_cx_builtin`) + interception sites — JIT lowering set.

Adding or renaming a builtin means editing four files; omitting one yields a silent cross-phase inconsistency. This is the mechanism behind Pillar-2's backend mismatches (e.g. `exit` works in the interpreter but not the JIT; `print` of `f64` diverges).
- **Direction:** a single builtin registry — `name → { signature, interp fn, lowering capability }` — consulted by the semantic phase, the interpreter, the validator, and the lowering. One row per builtin.

### T1.3 — Quadruple type representation + parallel AST hierarchy
There are four numeric type models — `ast::Type` (`T8…T128`, `src/frontend/ast.rs:36`), `SemanticType` (`I8…I128`, `src/frontend/semantic_types.rs:17`), `ir::IrType`, and Cranelift types — and the entire AST is mirrored: `ast::{Expr, Stmt, AstValue}` ↔ `Semantic{ExprKind, Stmt, Value}` (`semantic_types.rs:41-394`). Adding one construct touches **~8 files**: lexer → parser → `ast` → `semantic_types` → `semantic` (conversion) → `ir/instr` → `ir/lower` → backend.

The AST↔Semantic split *earns its keep* — `SemanticExpr` carries `ty` and resolved `BindingId`/`FunctionId`, which the raw AST can't. The waste is in the **numeric type duplication** and the conversion boilerplate (`impl From<SemanticType> for Type` at `runtime.rs:1919`, plus `Type→SemanticType` in `semantic.rs`, plus `SemanticType→IrType` in `lower.rs`), each a drift point.
- **Direction:** unify the numeric type representation to one enum shared across phases; keep the AST/Semantic structural split but consider generating or macro-ing the conversions. Don't merge the hierarchies — the annotations justify two — but stop spelling `t64` four ways.

### T1.4 — The interpreter discards the binding resolution the semantic phase computes *(perf = architecture)*
`SemanticExprKind::VarRef { binding, name }` (`semantic_types.rs:65`) carries a resolved `BindingId`. The interpreter **ignores it**: `eval_semantic_expr` destructures `VarRef { name, .. }` (`runtime.rs:677`) and calls `get_var(name)`, which walks scopes doing `frame.vars.get(name)` — a `HashMap<String, _>` lookup hashing the variable's name with SipHash on *every access* (`runtime.rs:426-428`, `ScopeFrame.vars` at `:24-25`). `BindingId` appears nowhere in the interpreter's variable path.

This is the architecture root of Pillar-1 hot-path B (~26% of loop-code instructions in SipHash). The semantic phase already did the resolution; the interpreter throws it away and re-resolves by string, every time.
- **Direction:** store locals in a `Vec<VarEntry>` indexed by `BindingId` and read/write by index. This reframes Pillar-1's "slot-indexed locals" from a new subsystem into *wiring up data that already exists*. Compounds with the JIT, which will want the same indices.
- See also: `report/performance.md` §4.

---

## Tier 2 — friction today, worse with scale

### T2.1 — `runtime.rs` is a 1,955-line monolith
One file, one `impl RunTime`, with several oversized methods: `run_semantic_stmt` ≈ 447 lines (`:801`), `apply_op` ≈ 230 lines (`:443`), `call_semantic_func` ≈ 227 lines (`:1280`, which also inlines all builtin dispatch). It conflates scope/environment management, value storage, arena tracking, expression eval, statement execution, operator semantics, call/method machinery, builtin dispatch, `when`-matching, string interpolation, printing, and type conversions.
- **Direction:** split along the seams that already exist as method groups — `env`/scope, `eval`, `exec`, `ops`, `builtins`, `format`. Each becomes independently testable. Do this *before* the next big runtime feature, not after.

### T2.2 — Layering inversion: the interpreter's core types live in `frontend`
`Value` and `RuntimeError` — the interpreter's central runtime types — are defined in `src/frontend/types.rs`, which in turn imports `crate::runtime::handle::Handle`. So `frontend` depends on `runtime` and `runtime` depends on `frontend::types`: a muddy, mutually-entangled boundary. A new contributor will look for the interpreter's `Value` in `runtime/`, not `frontend/`.
- **Direction:** move `Value`/`RuntimeError` into `runtime/`. Keep `frontend` depending only on its own AST/semantic types.

### T2.3 — The interpreter isn't behind the `Backend` trait
`trait Backend { fn execute(&self, &IrModule) }` (`src/backend/mod.rs`) abstracts only the IR-consuming backends (Cranelift, LLVM). `BackendKind::Interpret` is dispatched ad hoc in `main.rs` against `SemanticProgram`. The asymmetry (one engine on `SemanticProgram`, others on `IrModule`) makes uniform testing and adding engines awkward, and is the surface form of T1.1.
- **Direction:** follows from T1.1 — if the interpreter consumes IR, it can implement `Backend` and the dispatch unifies.

### T2.4 — `RuntimeError` lacks granular variants, so call sites fake messages
Pillar-2 D1/D2: out-of-bounds and missing-field errors are constructed as `RuntimeError::UndefinedVar { name: format!("index 5 out of bounds…") }` (`runtime.rs:391, 739, 875`), rendering the nonsensical "declare it with 'index 5…: TYPE = value'". The architecture cause is a too-coarse error enum that forces call sites to smuggle context into the wrong variant.
- **Direction:** add precise variants (`IndexOutOfBounds`, `FieldNotInitialized`, …); see `report/correctness.md` §Error messages for proposed text.

---

## Tier 3 — watch items

- **T3.1 — Eager per-scope arena (perf = architecture).** `push_function_scope` always builds `Arena::new()` → a zeroed 64 KB `Chunk` (`arena.rs:3,13`; `runtime.rs:127`), even for functions that allocate nothing. The scope model conflates "function scope" with "needs an arena." Pillar-1 finding #1 (90% memset on call-heavy code). *Direction:* lazy/non-zeroing chunk; decouple arena from scope creation. See `report/performance.md` §3.
- **T3.2 — `Bool`/`TBool`/`Unknown` trichotomy in `Value`** (`frontend/types.rs:8-13`). Three boolean-ish representations force special cases in every operator (`apply_op:451-462`) and underlie Pillar-2 C1/C5. *Direction:* a single three-valued boolean representation.
- **T3.3 — `allow(dead_code)` across 14 files** plus speculative API (`ExportTable` "for upcoming importer expansion", fields "preserved for future diagnostics"). Dead code accumulating behind attributes rather than being wired up or removed. *Direction:* per item, wire up within one release or delete.
- **T3.4 — 34 `unwrap`/`expect`/`panic` in non-test `frontend`+`runtime`.** Each is a potential ungraceful abort (Pillar-2 A7 is one such path). *Direction:* audit for the ones reachable from user input; convert to `RuntimeError`.
- **T3.5 — Import system is early.** Whole-file resolution; **no dead-symbol elimination** (the task's "dead symbol elimination correctness" has nothing to audit yet — `ExportTable` is unused). Circular detection is sound (cycle chain + depth-100 cap, `resolver.rs:99-121`). *Direction:* fine for now; symbol-level pruning is a deliberate later feature.
- **T3.6 — AOT stubs.** The only two `TODO`s in `src/` are `cranelift/aot.rs` and `llvm/aot.rs`, both clearly-marked unimplemented future work. No action.

---

## What is *good* architecture (worth preserving)

So the rewrite instinct doesn't overreach: the **`SemanticProgram` IR with `BindingId`/`FunctionId` resolution and per-node `ty`** is a genuinely good design — the problem is that consumers under-use it (T1.4), not the design itself. The **`BackendError { message, exit_code }`** contract (`backend/mod.rs`) is clean and well-documented. **Circular-import detection** is correct. The **IR validator** (`ir/validate.rs`, 2,005 lines) is a strong investment in catching lowering bugs. The codebase is **TODO-clean** (2 total) and the test/fixture discipline (189 fixtures + differential parity) is excellent.
