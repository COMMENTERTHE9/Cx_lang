# Cx Language Development Roadmap v3.1 — Updated 2026-03-16

---

## Done ✅

- Phase 1 — Functions
- Phase 2 — Free checker
- Phase 3 — Copy system (.copy, .copy.free, copy_into)
- Phase 4 — Bump allocator
- Phase 4b — True arena string storage
- Phase 5 — Handle<T> registry + language surface
- Phase 6a — when blocks
- Phase 6b — Ranges .. and ..=
- Phase 6c — Basic enums + variant matching
- Phase 6d — Loops + compound assigns + comparison operators
- Phase 6e — Flat grouped enums
- Phase 6f — Super-group enums + {_} placeholder
- Fix nested function name leakage bug
- Forward function declarations
- Type::Str vs Type::StrRef split + boundary checker (Memory Boundary Rules v0.1)
- TBool + is_known(x) + Unknown state runtime
- Block comments /# ... #/

**Cleanup Sprint — Complete**
- u128 → i128 for Value::Num — negative numbers now work
- For-loop range — direct iteration, no Vec allocation
- Debug formatting gated behind debug_scope
- run_stmt takes &Stmt — eliminates loop body cloning

**Arrays (Phase 7a)**
- Declaration, init, partial init with unknown slots
- Index read arr:[0], index write arr:[1] = 99
- Function pass and return
- Copy semantics (value copy, not reference)

**Control Flow Extensions (Phase 7b–7c)**
- while in arr:[0], 0..N { *arr } — cursor iteration
- then in chaining across multiple arrays
- if / else if / else statements
- Unknown condition rejection on control-critical paths

**Generics v1 (Phase 8)**
- Single type parameter on functions: fnc identity<T>(x: T) -> T
- T consistency checking at call site
- [N]T array return types
- Full pipeline: parser → semantic → runtime

**Structs Phase 1+2 (mid dev — on submain)**
- Struct definition and instantiation
- Field read/write
- impl blocks and method dispatch
- Compound assign on dot-access (p.health -= amount)
- != operator, ! (not), exit codes

**Infrastructure**
- GitHub Actions CI — frontend matrix + backend tests + stale base gate
- CONTRIBUTING.md — four branch rules for humans and agents

---

## In Progress 🔄

**Semantic/Interpreter Parity (mid dev)**
- Bringing SemanticStmt to full parity with raw AST interpreter
- StructDef, ImplBlock, MethodCall wired in semantic layer
- Next: wire interpreter to run off SemanticProgram
- Then: remove dead raw AST interpretation code

**Generics v2 (frontend — active)**
- Multiple type parameters: fnc swap<A, B>(a: A, b: B)
- Self-contained, no struct dependency
- Parser extension minimal given v1 foundation

**Backend IR (backend dev)**
- Function lowering in progress
- if/else lowering next
- Cranelift backend: stubs, not yet functional

**Cleanup Sprint — Remaining**
- seen and order on RunTime never cleared — accumulate forever
- run_stmt free function vs eval_expr method — structural inconsistency

---

## Up Next 🔲

- NullPoint<T> — nullable pointer, maps into unknown/known model
- Generics v3 — type bounds (T: Numeric, T: Known) mapped onto existing semantic categories
- Generic structs — after structs reach parity and generics v2 lands
- Multi-struct impl blocks — impl (p: Player, w: World)
- gene + phen trait system — design pass needed before implementation
- := type inference — after generics

---

## Stdlib — Early, After Structs + Arrays 🔲

- Growable array
- Hash table
- Ring buffer
- Binary search + quicksort

---

## Memory System Completion 🔲

- Phase 5b — region_id bulk handle invalidation on arena reset
- Handle-backed containers — unlocks container boundary crossing
- rc<T> single-threaded shared ownership
- shared<T> multi-threaded shared ownership
- Reference cycle handling — design pass needed

---

## Strings — Full Model 🔲

- strref as arena view — cannot escape scope
- str copy-on-boundary fully implemented and tested
- UTF-8 decision locked
- Substring without copy (strref into existing str)
- String interop with handles

---

## I/O 🔲

All three are runtime name matches — no new lexer tokens. Same pattern as print.

- print — already in. Promote to function before backend locks.
- read(var) — reads from stdin, fills variable. Type inferred from declaration.
- input("prompt", var) — prints prompt, reads response, fills variable.
- String interpolation — {varname} inline syntax. print("name: {name} age: {age}")

---

## Tooling + Backend 🔲

- CLI visualizer
- print promoted from statement to function — before backend locks calling convention
- Cranelift JIT backend (Phase E)
- LLVM AOT backend
- Ricey registry server
- LSP

---

## GPU System 🔲

- VRAM registry
- GS types
- .drop(fence)
- GPU memory lifetime model

---

## Deferred / Design Pass Needed 🔲

- 2D/3D/4D arrays — use flat arrays + manual indexing (game engine pattern)
- Traits / interfaces / bounded polymorphism full design
- C interop — emitting C as compilation target makes this nearly free
- Async / continuations / lambdas — important for real-world use
- Labeled breaks for nested loops
- Ternary expressions / value-producing if

---

## Key Changes from v3.0

- Arrays, while-in, if/else, Generics v1 all landed — moved to Done
- Structs Phase 1+2 landed on submain (mid dev)
- Generics v2 (multi type params) now In Progress
- CI and CONTRIBUTING.md added to infrastructure
- v3.1 reflects actual current state as of 2026-03-16
